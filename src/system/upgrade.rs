use crate::system::sha256::Sha256;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use std::env;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::ui::Ui;

pub const DEFAULT_REPO: &str = "NewGuy012/srz";
pub const MAX_ARCHIVE_BYTES: usize = 50 * 1024 * 1024; // 50 MB limit for release archives
pub const MAX_BINARY_BYTES: u64 = 100 * 1024 * 1024; // 100 MB limit for decompressed binary
pub const MAX_TAR_STREAM_BYTES: u64 = MAX_BINARY_BYTES + 20 * 1024 * 1024; // 120 MB limit for total decompressed tar stream

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    pub fn extension(&self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetInfo {
    pub target: &'static str,
    pub format: ArchiveFormat,
    pub binary_name: &'static str,
}

/// Detects the target triplet and package format for the current operating system and architecture.
pub fn detect_target() -> Result<TargetInfo, String> {
    detect_target_from(env::consts::OS, env::consts::ARCH)
}

/// Core mapping logic separated for deterministic unit testing.
pub fn detect_target_from(os: &str, arch: &str) -> Result<TargetInfo, String> {
    match (os, arch) {
        ("macos", "aarch64") => Ok(TargetInfo {
            target: "aarch64-apple-darwin",
            format: ArchiveFormat::TarGz,
            binary_name: "srz",
        }),
        ("macos", "x86_64") => Err(
            "Intel Macs (x86_64) are not supported with prebuilt binaries. Please install from source using 'cargo install srz'.".to_string(),
        ),
        ("linux", "x86_64") => Ok(TargetInfo {
            target: "x86_64-unknown-linux-gnu",
            format: ArchiveFormat::TarGz,
            binary_name: "srz",
        }),
        ("linux", "aarch64") => Ok(TargetInfo {
            target: "aarch64-unknown-linux-gnu",
            format: ArchiveFormat::TarGz,
            binary_name: "srz",
        }),
        ("windows", "x86_64") => Ok(TargetInfo {
            target: "x86_64-pc-windows-msvc",
            format: ArchiveFormat::Zip,
            binary_name: "srz.exe",
        }),
        (unsupported_os, unsupported_arch) => Err(format!(
            "Unsupported platform: {}-{}. Please build from source using 'cargo install srz'.",
            unsupported_os, unsupported_arch
        )),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallOrigin {
    Homebrew,
    Cargo,
    Direct,
}

/// Detects if srz is installed via a package manager to avoid breaking package tracking.
pub fn detect_install_origin(exe_path: &Path) -> InstallOrigin {
    let path_str = exe_path.to_string_lossy();
    if path_str.contains("/opt/homebrew/")
        || path_str.contains("/usr/local/Cellar/")
        || path_str.contains("/Cellar/")
        || path_str.contains("/homebrew/")
        || path_str.contains(".linuxbrew/")
    {
        InstallOrigin::Homebrew
    } else if path_str.contains("/.cargo/bin/") || path_str.contains("\\.cargo\\bin\\") {
        InstallOrigin::Cargo
    } else {
        InstallOrigin::Direct
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Version {
    pub major: u64,
    pub minor: u64,
    pub patch: u64,
    pub pre: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionError(pub String);

impl std::fmt::Display for VersionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Invalid semver: {}", self.0)
    }
}

impl std::error::Error for VersionError {}

impl Version {
    pub const fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self {
            major,
            minor,
            patch,
            pre: None,
        }
    }

    pub fn parse(v: &str) -> Result<Self, VersionError> {
        let trimmed = v.trim();
        let s = trimmed
            .strip_prefix('v')
            .or_else(|| trimmed.strip_prefix('V'))
            .unwrap_or(trimmed);

        // Disallow path traversal, shell characters, whitespace
        if s.contains('/') || s.contains('\\') || s.contains(';') || s.contains(' ') {
            return Err(VersionError(format!(
                "Illegal characters in version: {}",
                s
            )));
        }

        // Strip build metadata (+...)
        let (version_pre, _build) = match s.split_once('+') {
            Some((vp, b)) => {
                if b.is_empty()
                    || !b
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                {
                    return Err(VersionError(format!("Invalid build metadata: {}", b)));
                }
                (vp, Some(b))
            }
            None => (s, None),
        };

        // Split core version from pre-release
        let (core, pre) = match version_pre.split_once('-') {
            Some((c, p)) => {
                if p.is_empty()
                    || !p
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-')
                {
                    return Err(VersionError(format!(
                        "Invalid prerelease identifier: {}",
                        p
                    )));
                }
                (c, Some(p.to_string()))
            }
            None => (version_pre, None),
        };

        let parts: Vec<&str> = core.split('.').collect();
        let [maj, min, pat] = parts.as_slice() else {
            return Err(VersionError(format!(
                "Expected major.minor.patch, got: {}",
                core
            )));
        };
        let parse_part = |s: &str, name: &str| {
            s.parse::<u64>()
                .map_err(|_| VersionError(format!("Invalid {} version: {}", name, s)))
        };

        Ok(Self {
            major: parse_part(maj, "major")?,
            minor: parse_part(min, "minor")?,
            patch: parse_part(pat, "patch")?,
            pre,
        })
    }
}

impl std::fmt::Display for Version {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)?;
        if let Some(ref pre) = self.pre {
            write!(f, "-{}", pre)?;
        }
        Ok(())
    }
}

impl std::str::FromStr for Version {
    type Err = VersionError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::parse(s)
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => std::cmp::Ordering::Equal,
                (Some(_), None) => std::cmp::Ordering::Less,
                (None, Some(_)) => std::cmp::Ordering::Greater,
                (Some(a), Some(b)) => a.cmp(b),
            })
    }
}

/// Strips optional leading 'v' and parses a semantic version.
pub fn parse_clean_version(v: &str) -> Result<Version, VersionError> {
    let clean = v.trim().trim_start_matches('v').trim_matches('/');
    Version::parse(clean)
}

/// Extracts the hex digest from a standard sha256sum line (e.g. "a1b2...  srz.tar.gz").
pub fn parse_sha256_checksum(content: &str) -> Option<&str> {
    content
        .lines()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| line.split_whitespace().next())
}

/// Formats primary versioned asset name and fallback alias.
pub fn format_asset_names(tag: &str, target: &str, format: ArchiveFormat) -> (String, String) {
    let ext = format.extension();
    let versioned = format!("srz-{}-{}.{}", tag, target, ext);
    let latest_alias = format!("srz-{}.{}", target, ext);
    (versioned, latest_alias)
}

/// Extracts the named binary from a gzipped tarball with bounded decompression.
pub fn extract_binary_from_tar_gz(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let mut gz = GzDecoder::new(Cursor::new(bytes)).take(MAX_TAR_STREAM_BYTES + 1);
    let mut header = [0u8; 512];

    loop {
        match gz.read_exact(&mut header) {
            Ok(()) => {}
            Err(_) if gz.limit() == 0 => {
                bail!(
                    "Decompressed tar archive exceeds security limit of {} MB",
                    MAX_TAR_STREAM_BYTES / (1024 * 1024)
                );
            }
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err).context("Failed to read tar header from release archive"),
        }

        if header.iter().all(|&b| b == 0) {
            break;
        }

        // File name is at 0..100
        let name_bytes = &header[0..100];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(100);
        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap_or("");

        // File size is octal ASCII at 124..136
        let size_bytes = &header[124..136];
        let size_str = std::str::from_utf8(size_bytes)
            .unwrap_or("")
            .trim_matches(['\0', ' ']);
        let size = if size_str.is_empty() {
            0
        } else {
            u64::from_str_radix(size_str, 8)
                .context("Corrupt tar archive: invalid octal entry size")?
        };

        // Only extract regular files (V7 NUL, USTAR '0', or contiguous '7').
        let typeflag = header[156];
        let is_regular_file = matches!(typeflag, 0 | b'0' | b'7');
        let filename = name.rsplit('/').next().unwrap_or(name);

        if is_regular_file && filename == binary_name {
            if size == 0 {
                bail!(
                    "Corrupt tar archive: release binary '{}' is empty",
                    binary_name
                );
            }
            if size > MAX_BINARY_BYTES {
                bail!(
                    "Decompressed binary exceeds security limit of {} MB",
                    MAX_BINARY_BYTES / (1024 * 1024)
                );
            }
            // Cap initial buffer capacity to avoid pre-allocating untrusted header sizes.
            let initial_cap = (size as usize).min(64 * 1024);
            let mut binary_buf = Vec::with_capacity(initial_cap);
            let bytes_read = gz
                .by_ref()
                .take(size)
                .read_to_end(&mut binary_buf)
                .context("Failed to read binary data from tar")?;
            if bytes_read as u64 != size {
                bail!("Corrupt tar archive: unexpected EOF while reading binary payload");
            }
            return Ok(binary_buf);
        }

        // Skip to next 512-byte block boundary with checked arithmetic and stream caps.
        let rem = size % 512;
        let pad = if rem == 0 { 0 } else { 512 - rem };
        let padded_size = size
            .checked_add(pad)
            .context("Corrupt tar archive: entry size overflow")?;
        if padded_size > MAX_TAR_STREAM_BYTES {
            bail!(
                "Tar archive entry exceeds security limit of {} MB",
                MAX_TAR_STREAM_BYTES / (1024 * 1024)
            );
        }
        if padded_size > 0 {
            let mut discard = gz.by_ref().take(padded_size);
            let skipped = std::io::copy(&mut discard, &mut std::io::sink())
                .context("Failed to skip entry in tar archive")?;
            if gz.limit() == 0 {
                bail!(
                    "Decompressed tar archive exceeds security limit of {} MB",
                    MAX_TAR_STREAM_BYTES / (1024 * 1024)
                );
            }
            if skipped != padded_size {
                bail!("Corrupt tar archive: unexpected EOF while skipping tar entry");
            }
        }
    }

    bail!("Binary '{}' not found in release archive", binary_name)
}

/// Extracts the named binary from a zip archive with bounded decompression.
pub fn extract_binary_from_zip(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let reader = crate::system::zip::ZipReader::new(bytes).context("Failed to read zip archive")?;
    for entry in reader.entries() {
        if entry.name.rsplit(['/', '\\']).next() == Some(binary_name) {
            let data = reader.decompress_entry(entry, MAX_BINARY_BYTES as usize)?;
            if data.is_empty() {
                bail!(
                    "Corrupt zip archive: release binary '{}' is empty",
                    binary_name
                );
            }
            return Ok(data);
        }
    }
    bail!("Binary '{}' not found in release zip archive", binary_name)
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum UpgradeStatus {
    AlreadyUpToDate,
    UpdateAvailable,
    Upgraded,
    HomebrewManaged,
    CargoManaged,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpgradeResult {
    pub current_version: String,
    pub target_version: String,
    pub target_platform: String,
    pub status: UpgradeStatus,
    pub message: String,
}

impl UpgradeResult {
    /// Emits a structured result as formatted JSON to stdout or as a user-friendly UI message to stderr.
    pub fn emit(
        status: UpgradeStatus,
        target_version: impl Into<String>,
        target_platform: impl Into<String>,
        message: impl Into<String>,
        json: bool,
    ) -> Result<()> {
        let message = message.into();
        if json {
            let res = Self {
                current_version: env!("CARGO_PKG_VERSION").to_string(),
                target_version: target_version.into(),
                target_platform: target_platform.into(),
                status,
                message,
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
        } else {
            match status {
                UpgradeStatus::Upgraded | UpgradeStatus::AlreadyUpToDate => Ui::success(&message),
                UpgradeStatus::UpdateAvailable | UpgradeStatus::HomebrewManaged => {
                    Ui::info(&message)
                }
                UpgradeStatus::CargoManaged => Ui::warn(&message),
            }
        }
        Ok(())
    }
}

/// Resolves the latest version tag from GitHub.
/// Uses a HEAD request to inspect the redirect URL to avoid GitHub's REST API rate limit (60 req/hr).
pub async fn resolve_latest_version(client: &reqwest::Client, repo: &str) -> Result<String> {
    let redirect_url = format!("https://github.com/{}/releases/latest", repo);
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(15))
        .build();

    if let Ok(no_redirect_client) = no_redirect
        && let Ok(resp) = no_redirect_client.head(&redirect_url).send().await
        && resp.status().is_redirection()
        && let Some(loc) = resp.headers().get(reqwest::header::LOCATION)
        && let Ok(loc_str) = loc.to_str()
        && let Some(pos) = loc_str.rfind("/tag/")
    {
        let clean_tag = loc_str[pos + 5..].trim_matches('/');
        if let Ok(ver) = parse_clean_version(clean_tag) {
            return Ok(format!("v{}", ver));
        }
    }

    // Fallback: GitHub REST API
    let api_url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let resp = client
        .get(&api_url)
        .header(reqwest::header::ACCEPT, "application/vnd.github.v3+json")
        .send()
        .await
        .context("Failed to query GitHub releases API")?
        .error_for_status()
        .context("GitHub API error")?;

    let bytes = crate::scraper::read_response_bytes_limited(resp, 1024 * 1024)
        .await
        .context("Failed to read GitHub release response")?;
    let release_json: serde_json::Value =
        serde_json::from_slice(&bytes).context("Failed to parse GitHub release JSON")?;

    let tag = release_json["tag_name"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing 'tag_name' in GitHub release response"))?;
    let ver =
        parse_clean_version(tag).context("Failed to parse GitHub release tag_name as semver")?;

    Ok(format!("v{}", ver))
}

/// Downloads and verifies an asset bytes matching either versioned or latest name.
async fn download_and_verify_archive(
    client: &reqwest::Client,
    repo: &str,
    tag: &str,
    versioned_name: &str,
    latest_alias: &str,
    force: bool,
) -> Result<Vec<u8>> {
    let base_dl = format!("https://github.com/{}/releases/download/{}", repo, tag);
    let v_url = format!("{}/{}", base_dl, versioned_name);
    let alias_url = format!("{}/{}", base_dl, latest_alias);

    let (archive_url, sha_url, archive_res) = {
        let resp = client.get(&v_url).send().await;
        match resp {
            Ok(r) if r.status().is_success() => (v_url.clone(), format!("{}.sha256", v_url), r),
            _ => {
                let r2 = client
                    .get(&alias_url)
                    .send()
                    .await
                    .context("Failed to initiate archive download")?;
                (alias_url.clone(), format!("{}.sha256", alias_url), r2)
            }
        }
    };

    if !archive_res.status().is_success() {
        bail!(
            "Failed to download release archive (HTTP status: {})",
            archive_res.status()
        );
    }

    let archive_pb = Ui::spinner(format!("Downloading {}...", archive_url));
    let archive_bytes =
        match crate::scraper::read_response_bytes_limited(archive_res, MAX_ARCHIVE_BYTES).await {
            Ok(b) => b,
            Err(err) => {
                archive_pb.finish_and_clear();
                return Err(err);
            }
        };
    Ui::finish_spinner(&archive_pb, "Downloaded release archive.");

    let sha_pb = Ui::spinner("Verifying SHA256 checksum...");
    let sha_res = client.get(&sha_url).send().await;
    match sha_res {
        Ok(resp) if resp.status().is_success() => {
            let sha_text = crate::scraper::read_response_text_limited(resp, 64 * 1024)
                .await
                .unwrap_or_default();
            if let Some(expected_hash) = parse_sha256_checksum(&sha_text) {
                let computed_hash = Sha256::digest_hex(&archive_bytes);

                if !expected_hash.eq_ignore_ascii_case(&computed_hash) {
                    sha_pb.finish_and_clear();
                    bail!(
                        "SHA256 checksum verification failed!\nExpected: {}\nComputed: {}",
                        expected_hash,
                        computed_hash
                    );
                }
                Ui::finish_spinner(
                    &sha_pb,
                    &format!("Checksum verified ({:.8}...).", computed_hash),
                );
            } else if force {
                Ui::finish_spinner(
                    &sha_pb,
                    "Checksum file empty; bypassing check due to --force.",
                );
            } else {
                sha_pb.finish_and_clear();
                bail!(
                    "SHA256 checksum file was empty. Aborting upgrade for security. Use `srz upgrade --force` to bypass."
                );
            }
        }
        _ => {
            if force {
                Ui::finish_spinner(
                    &sha_pb,
                    "Checksum file unavailable; bypassing check due to --force.",
                );
            } else {
                sha_pb.finish_and_clear();
                bail!(
                    "Failed to retrieve SHA256 checksum file from {}. Aborting upgrade for security. Use `srz upgrade --force` to bypass.",
                    sha_url
                );
            }
        }
    }

    Ok(archive_bytes)
}

/// Safely replaces an executable binary with a staged new binary file.
pub fn replace_binary(new_binary: &Path, target_exe: &Path) -> std::io::Result<()> {
    crate::system::fs_util::reject_symlink(target_exe, "replace binary at symlink")?;

    if std::fs::rename(new_binary, target_exe).is_ok() {
        return Ok(());
    }

    // Target may be locked or on a filesystem requiring unlink.
    // Stage a backup so target_exe is preserved if swap fails.
    let parent = target_exe.parent().unwrap_or_else(|| Path::new("."));
    let backup_path = crate::system::fs_util::create_temp_file_path(parent, ".srz-old", ".tmp");

    std::fs::rename(target_exe, &backup_path)?;

    if let Err(err) = std::fs::rename(new_binary, target_exe) {
        let _ = std::fs::rename(&backup_path, target_exe);
        return Err(err);
    }

    let _ = std::fs::remove_file(&backup_path);
    Ok(())
}

/// Main entry point for executing `srz upgrade`.
pub async fn run_upgrade(
    check: bool,
    force: bool,
    custom_version: Option<&str>,
    json: bool,
) -> Result<()> {
    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_version = parse_clean_version(current_version_str)
        .context("Failed to parse current CARGO_PKG_VERSION")?;

    let current_exe =
        env::current_exe().context("Failed to determine path to current executable")?;

    // 1. Check install origin
    let origin = detect_install_origin(&current_exe);
    if origin == InstallOrigin::Homebrew {
        let msg = "srz was installed via Homebrew. Please run `brew upgrade srz` instead.";
        UpgradeResult::emit(
            UpgradeStatus::HomebrewManaged,
            current_version_str,
            env::consts::OS,
            msg,
            json,
        )?;
        return Ok(());
    }

    // 2. Detect platform target
    let target_info = match detect_target() {
        Ok(info) => info,
        Err(err) => {
            if json {
                UpgradeResult::emit(
                    UpgradeStatus::AlreadyUpToDate,
                    current_version_str,
                    format!("{}-{}", env::consts::OS, env::consts::ARCH),
                    &err,
                    true,
                )?;
                return Ok(());
            }
            bail!("{}", err);
        }
    };

    // 3. Setup HTTP client
    let client = reqwest::Client::builder()
        .user_agent(format!("srz/{}", current_version_str))
        .connect_timeout(std::time::Duration::from_secs(5))
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .context("Failed to build HTTP client")?;

    // 4. Resolve target version
    let spinner = Ui::spinner("Checking for updates...");
    let target_tag = match custom_version {
        Some(v) => {
            if v.starts_with('v') {
                v.to_string()
            } else {
                format!("v{}", v)
            }
        }
        None => resolve_latest_version(&client, DEFAULT_REPO).await.context(
            "Failed to resolve latest version tag from GitHub. Ensure you have internet connectivity.",
        )?,
    };
    spinner.finish_and_clear();

    let target_version = parse_clean_version(&target_tag).with_context(|| {
        format!(
            "Failed to parse release tag '{}' as a semver version",
            target_tag
        )
    })?;
    let target_tag = format!("v{}", target_version);

    let is_newer = target_version > current_version;
    let is_target_equal = target_version == current_version;

    // Handle --check
    if check {
        let (status, msg) = if is_newer {
            (
                UpgradeStatus::UpdateAvailable,
                format!(
                    "A newer version of srz is available: v{} (current: v{}). Run `srz upgrade` to install.",
                    target_version, current_version
                ),
            )
        } else if is_target_equal {
            (
                UpgradeStatus::AlreadyUpToDate,
                format!("srz is already up to date (v{}).", current_version),
            )
        } else {
            (
                UpgradeStatus::AlreadyUpToDate,
                format!(
                    "srz is running a newer build (v{}) than the requested release (v{}).",
                    current_version, target_version
                ),
            )
        };

        UpgradeResult::emit(
            status,
            target_version.to_string(),
            target_info.target,
            msg,
            json,
        )?;
        return Ok(());
    }

    // Up to date check
    if !is_newer && !force && custom_version.is_none() {
        let msg = format!("srz is already up to date (v{}).", current_version);
        UpgradeResult::emit(
            UpgradeStatus::AlreadyUpToDate,
            target_version.to_string(),
            target_info.target,
            msg,
            json,
        )?;
        return Ok(());
    }

    if origin == InstallOrigin::Cargo && !force {
        let msg = "srz was installed via Cargo (~/.cargo/bin). You can update with `cargo install srz`, or use `srz upgrade --force` to overwrite directly.";
        UpgradeResult::emit(
            UpgradeStatus::CargoManaged,
            target_version.to_string(),
            target_info.target,
            msg,
            json,
        )?;
        return Ok(());
    }

    let (versioned_name, latest_alias) =
        format_asset_names(&target_tag, target_info.target, target_info.format);

    // 5. Download & verify
    let archive_bytes = download_and_verify_archive(
        &client,
        DEFAULT_REPO,
        &target_tag,
        &versioned_name,
        &latest_alias,
        force,
    )
    .await?;

    // 6. Decompress binary
    let extract_pb = Ui::spinner("Extracting new binary...");
    let binary_bytes = match target_info.format {
        ArchiveFormat::TarGz => {
            extract_binary_from_tar_gz(&archive_bytes, target_info.binary_name)?
        }
        ArchiveFormat::Zip => extract_binary_from_zip(&archive_bytes, target_info.binary_name)?,
    };
    Ui::finish_spinner(&extract_pb, "Extracted new executable.");

    // 7. Write to temp file with atomic creation and private executable permissions
    let temp_dir = current_exe
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);

    let temp_path = crate::system::fs_util::create_temp_file_path(
        &temp_dir,
        ".srz-upgrade",
        &format!("-{}", target_info.binary_name),
    );

    let mut open_opts = std::fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o755);
    }

    let mut temp_file = open_opts
        .open(&temp_path)
        .with_context(|| format!("Failed to create temporary binary at {:?}", temp_path))?;
    if let Err(err) = std::io::Write::write_all(&mut temp_file, &binary_bytes)
        .and_then(|_| std::io::Write::flush(&mut temp_file))
    {
        let _ = std::fs::remove_file(&temp_path);
        return Err(err)
            .with_context(|| format!("Failed to write temporary binary at {:?}", temp_path));
    }

    // 8. Swap binary
    let swap_pb = Ui::spinner(format!("Installing to {}...", current_exe.display()));
    if let Err(err) = replace_binary(&temp_path, &current_exe) {
        let _ = std::fs::remove_file(&temp_path);
        swap_pb.finish_and_clear();
        bail!("Failed to replace binary at {:?}: {}", current_exe, err);
    }
    let _ = std::fs::remove_file(&temp_path);
    Ui::finish_spinner(&swap_pb, "Binary replaced successfully.");

    // 9. Post-upgrade verification
    let mut verified_version = target_tag.clone();
    if let Ok(out) = Command::new(&current_exe).arg("--version").output()
        && out.status.success()
    {
        let v_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !v_str.is_empty() {
            verified_version = v_str;
        }
    }

    let success_msg = format!("Successfully upgraded srz to {}!", verified_version);
    UpgradeResult::emit(
        UpgradeStatus::Upgraded,
        target_version.to_string(),
        target_info.target,
        success_msg,
        json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn build_ustar_header(name: &str, typeflag: u8, size_octal: &str) -> [u8; 512] {
        let mut header = [0u8; 512];
        let name_bytes = name.as_bytes();
        header[..name_bytes.len()].copy_from_slice(name_bytes);
        header[100..108].copy_from_slice(b"0000755\0");
        let size_bytes = size_octal.as_bytes();
        header[124..124 + size_bytes.len()].copy_from_slice(size_bytes);
        header[156] = typeflag;
        header[257..263].copy_from_slice(b"ustar\0");
        header[148..156].copy_from_slice(b"        ");
        let cksum: u32 = header.iter().map(|&b| b as u32).sum();
        let cksum_octal = format!("{:06o}\0 ", cksum);
        header[148..156].copy_from_slice(cksum_octal.as_bytes());
        header
    }

    fn gzip_tar_entries(
        entries: &[([u8; 512], &[u8])],
        pad_and_eof: bool,
        trailing_zeros: usize,
    ) -> Vec<u8> {
        let mut gz_encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        for (header, body) in entries {
            gz_encoder.write_all(header).unwrap();
            gz_encoder.write_all(body).unwrap();
            if pad_and_eof {
                let pad = (512 - (body.len() % 512)) % 512;
                if pad > 0 {
                    gz_encoder.write_all(&vec![0u8; pad]).unwrap();
                }
            }
        }
        if pad_and_eof && trailing_zeros > 0 {
            let chunk = [0u8; 64 * 1024];
            let mut remaining = trailing_zeros;
            while remaining > 0 {
                let n = remaining.min(chunk.len());
                gz_encoder.write_all(&chunk[..n]).unwrap();
                remaining -= n;
            }
        }
        gz_encoder.finish().expect("Failed to finish gzip encoding")
    }

    #[test]
    fn test_detect_target_mappings() {
        for (os, arch, target, format, binary_name) in [
            (
                "macos",
                "aarch64",
                "aarch64-apple-darwin",
                ArchiveFormat::TarGz,
                "srz",
            ),
            (
                "linux",
                "x86_64",
                "x86_64-unknown-linux-gnu",
                ArchiveFormat::TarGz,
                "srz",
            ),
            (
                "linux",
                "aarch64",
                "aarch64-unknown-linux-gnu",
                ArchiveFormat::TarGz,
                "srz",
            ),
            (
                "windows",
                "x86_64",
                "x86_64-pc-windows-msvc",
                ArchiveFormat::Zip,
                "srz.exe",
            ),
        ] {
            let info = detect_target_from(os, arch).expect("Valid target");
            assert_eq!(info.target, target);
            assert_eq!(info.format, format);
            assert_eq!(info.binary_name, binary_name);
        }

        let mac_intel_err = detect_target_from("macos", "x86_64").unwrap_err();
        assert!(mac_intel_err.contains("Intel Macs (x86_64) are not supported"));

        for (os, arch) in [("solaris", "sparc"), ("linux", "arm")] {
            assert!(detect_target_from(os, arch).is_err());
        }
    }

    #[test]
    fn test_detect_install_origin() {
        for (path, expected) in [
            ("/opt/homebrew/bin/srz", InstallOrigin::Homebrew),
            (
                "/opt/homebrew/Cellar/srz/0.1.0/bin/srz",
                InstallOrigin::Homebrew,
            ),
            (
                "/usr/local/Cellar/srz/0.1.0/bin/srz",
                InstallOrigin::Homebrew,
            ),
            (
                "/home/linuxbrew/.linuxbrew/bin/srz",
                InstallOrigin::Homebrew,
            ),
            (
                "/home/linuxbrew/.linuxbrew/Cellar/srz/0.1.0/bin/srz",
                InstallOrigin::Homebrew,
            ),
            ("/home/user/.cargo/bin/srz", InstallOrigin::Cargo),
            (
                "C:\\Users\\User\\.cargo\\bin\\srz.exe",
                InstallOrigin::Cargo,
            ),
            ("/home/user/.local/bin/srz", InstallOrigin::Direct),
            ("/usr/local/bin/srz", InstallOrigin::Direct),
        ] {
            assert_eq!(detect_install_origin(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn test_parse_sha256_checksum_and_sha256_computation() {
        let test_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        for (input, expected) in [
            (format!("{test_hash}  srz-v0.1.0.tar.gz\n"), Some(test_hash)),
            (format!("{test_hash} *srz-v0.1.0.tar.gz"), Some(test_hash)),
            (test_hash.to_string(), Some(test_hash)),
            ("   \n\n  ".to_string(), None),
        ] {
            assert_eq!(parse_sha256_checksum(&input), expected);
        }

        let computed =
            Sha256::digest_hex(b"Arbitrary release archive test data for SHA256 hashing.");
        let sha256_file = format!("{computed}  srz.tar.gz");
        let parsed = parse_sha256_checksum(&sha256_file).expect("Parsed hash");
        assert_eq!(parsed, computed);
        assert!(parsed.eq_ignore_ascii_case(&computed.to_uppercase()));
    }

    #[test]
    fn test_format_asset_names() {
        for (target, fmt, expected_v, expected_l) in [
            (
                "aarch64-apple-darwin",
                ArchiveFormat::TarGz,
                "srz-v0.2.0-aarch64-apple-darwin.tar.gz",
                "srz-aarch64-apple-darwin.tar.gz",
            ),
            (
                "x86_64-pc-windows-msvc",
                ArchiveFormat::Zip,
                "srz-v0.2.0-x86_64-pc-windows-msvc.zip",
                "srz-x86_64-pc-windows-msvc.zip",
            ),
        ] {
            let (v, l) = format_asset_names("v0.2.0", target, fmt);
            assert_eq!(v, expected_v);
            assert_eq!(l, expected_l);
        }
    }

    #[test]
    fn test_extract_binary_from_tar_gz() {
        let binary_content = b"echo 'srz v0.2.0'";
        let hdr = build_ustar_header("srz", b'0', &format!("{:011o} ", binary_content.len()));
        let archive_bytes = gzip_tar_entries(&[(hdr, binary_content)], true, 1024);

        let extracted =
            extract_binary_from_tar_gz(&archive_bytes, "srz").expect("Extraction should succeed");
        assert_eq!(extracted, binary_content);
        assert!(extract_binary_from_tar_gz(&archive_bytes, "nonexistent").is_err());
    }

    #[test]
    fn test_extract_binary_from_zip() {
        let binary_content = b"MZ-mock-binary-srz-exe";
        let zip_bytes = crate::system::zip::create_test_zip(&[("srz.exe", binary_content)]);

        let extracted =
            extract_binary_from_zip(&zip_bytes, "srz.exe").expect("Extraction should succeed");
        assert_eq!(extracted, binary_content);
        assert!(extract_binary_from_zip(&zip_bytes, "srz-other.exe").is_err());

        let empty_zip = crate::system::zip::create_test_zip(&[("srz.exe", b"")]);
        assert!(extract_binary_from_zip(&empty_zip, "srz.exe").is_err());
    }

    #[test]
    fn test_parse_clean_version_canonicalization_and_rejection() {
        for (input, major, minor, patch) in [
            ("v0.1.0", 0, 1, 0),
            ("0.2.5", 0, 2, 5),
            ("  v1.3.4-alpha.1 \n", 1, 3, 4),
            ("v0.7.1", 0, 7, 1),
            (" 0.7.1 ", 0, 7, 1),
        ] {
            let v = parse_clean_version(input).unwrap();
            assert_eq!((v.major, v.minor, v.patch), (major, minor, patch));
        }

        for invalid in [
            "invalid-version",
            "../../../evil",
            "latest",
            "v0.7.1/payload",
            "; rm -rf /",
        ] {
            assert!(parse_clean_version(invalid).is_err());
        }
    }

    #[test]
    fn test_upgrade_temp_file_creation_and_symlink_safety() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let target_file = tmp.path().join(".srz-upgrade-existing");
        std::fs::write(&target_file, b"existing-content").unwrap();

        // create_new(true) fails if file already exists (preventing symlink overwrites)
        assert!(
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target_file)
                .is_err()
        );

        #[cfg(unix)]
        {
            let new_binary = tmp.path().join("new_bin");
            let real_target = tmp.path().join("real_target");
            let symlink_target = tmp.path().join("symlink_target");
            std::fs::write(&new_binary, b"new binary content").unwrap();
            std::fs::write(&real_target, b"old target content").unwrap();
            std::os::unix::fs::symlink(&real_target, &symlink_target).unwrap();

            let res = replace_binary(&new_binary, &symlink_target);
            assert!(res.unwrap_err().to_string().contains("symlink"));
        }
    }

    #[test]
    fn test_extract_binary_from_tar_gz_rejects_bomb_and_oversized_entries() {
        // 1. Decompressed tar stream exceeding MAX_TAR_STREAM_BYTES (gzip bomb / huge stream)
        let filler_len = (MAX_TAR_STREAM_BYTES as usize) + 1024;
        let filler_header =
            build_ustar_header("filler.bin", b'0', &format!("{:011o} ", filler_len));
        let bomb_bytes = gzip_tar_entries(&[(filler_header, b"")], true, filler_len);
        assert!(extract_binary_from_tar_gz(&bomb_bytes, "srz").is_err());

        // 2. Target binary claiming size > MAX_BINARY_BYTES without backing payload
        let target_header =
            build_ustar_header("srz", b'0', &format!("{:011o} ", MAX_BINARY_BYTES + 1));
        let oversized_tar = gzip_tar_entries(&[(target_header, b"short")], true, 1024);
        assert!(extract_binary_from_tar_gz(&oversized_tar, "srz").is_err());

        // 3. Non-matching entry with huge 12-digit octal size (no panic / clean error)
        let skip_header = build_ustar_header("other.bin", b'0', "777777777777");
        let huge_skip_tar = gzip_tar_entries(&[(skip_header, b"")], true, 1024);
        assert!(extract_binary_from_tar_gz(&huge_skip_tar, "srz").is_err());
    }

    #[test]
    fn test_extract_binary_from_tar_gz_skips_symlinks_and_directories_and_rejects_empty() {
        // 1. Symlink and directory entries named "srz" are ignored; regular file "srz" succeeds
        let symlink_hdr = build_ustar_header("srz", b'2', "00000000000 ");
        let dir_hdr = build_ustar_header("srz", b'5', "00000000000 ");
        let payload = b"real-srz-binary";
        let reg_hdr = build_ustar_header("srz", b'0', &format!("{:011o} ", payload.len()));
        let mixed_tar = gzip_tar_entries(
            &[(symlink_hdr, b""), (dir_hdr, b""), (reg_hdr, payload)],
            true,
            1024,
        );
        assert_eq!(
            extract_binary_from_tar_gz(&mixed_tar, "srz").unwrap(),
            payload
        );

        // 2-4. Symlink-only, 0-byte regular file, and truncated payload must fail
        let empty_hdr = build_ustar_header("srz", b'0', "00000000000 ");
        let truncated_hdr = build_ustar_header("srz", b'0', &format!("{:011o} ", 100usize));
        for archive in [
            gzip_tar_entries(&[(symlink_hdr, b"")], true, 1024),
            gzip_tar_entries(&[(empty_hdr, b"")], true, 1024),
            gzip_tar_entries(&[(truncated_hdr, b"only10byte")], false, 0),
        ] {
            assert!(extract_binary_from_tar_gz(&archive, "srz").is_err());
        }
    }
}
