use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::Archive;
use zip::ZipArchive;

use crate::ui::Ui;

pub const DEFAULT_REPO: &str = "NewGuy012/srz";
pub const MAX_ARCHIVE_BYTES: usize = 50 * 1024 * 1024; // 50 MB limit for release archives
pub const MAX_BINARY_BYTES: u64 = 100 * 1024 * 1024; // 100 MB limit for decompressed binary

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

/// Strips optional leading 'v' and parses a semantic version.
pub fn parse_clean_version(v: &str) -> Result<semver::Version, semver::Error> {
    let clean = v.trim().strip_prefix('v').unwrap_or(v.trim());
    semver::Version::parse(clean)
}

/// Extracts the hex digest from a standard sha256sum line (e.g. "a1b2...  srz.tar.gz").
pub fn parse_sha256_checksum(content: &str) -> Option<&str> {
    content
        .lines()
        .find(|line| !line.trim().is_empty())
        .and_then(|line| line.split_whitespace().next())
}

/// Computes the lowercase hex-encoded SHA-256 checksum of `bytes` in a single allocation.
pub fn compute_sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    crate::system::text_util::format_hex(&digest)
}

/// Formats primary versioned asset name and fallback alias.
pub fn format_asset_names(tag: &str, target: &str, format: ArchiveFormat) -> (String, String) {
    let ext = format.extension();
    let versioned = format!("srz-{}-{}.{}", tag, target, ext);
    let latest_alias = format!("srz-{}.{}", target, ext);
    (versioned, latest_alias)
}

fn read_bounded_binary(mut reader: impl Read, source_label: &str) -> Result<Vec<u8>> {
    let mut buf = Vec::new();
    let mut limited = (&mut reader).take(MAX_BINARY_BYTES + 1);
    let bytes_copied = std::io::copy(&mut limited, &mut buf)
        .with_context(|| format!("Failed to extract binary from {}", source_label))?;
    if bytes_copied > MAX_BINARY_BYTES {
        bail!(
            "Decompressed binary exceeds security limit of {} MB",
            MAX_BINARY_BYTES / (1024 * 1024)
        );
    }
    Ok(buf)
}

/// Extracts the named binary from a gzipped tarball with bounded decompression.
pub fn extract_binary_from_tar_gz(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let gz = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(gz);
    for entry in archive.entries().context("Failed to read tar entries")? {
        let entry = entry.context("Corrupt tar entry")?;
        let path = entry.path().context("Invalid tar entry path")?;
        if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            return read_bounded_binary(entry, "tar entry");
        }
    }
    bail!("Binary '{}' not found in release archive", binary_name)
}

/// Extracts the named binary from a zip archive with bounded decompression.
pub fn extract_binary_from_zip(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("Failed to read zip archive")?;
    for i in 0..zip.len() {
        let file = zip.by_index(i).context("Corrupt zip entry")?;
        let name = file
            .name()
            .context("Failed to read zip entry name")?
            .to_string();
        if name == binary_name
            || name.ends_with(&format!("/{}", binary_name))
            || name.ends_with(&format!("\\{}", binary_name))
        {
            return read_bounded_binary(file, "zip entry");
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
        if !clean_tag.is_empty() {
            return Ok(clean_tag.to_string());
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

    Ok(tag.to_string())
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
                let computed_hash = compute_sha256_hex(&archive_bytes);

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

    let nonce: u64 = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);

    let temp_name = format!(
        ".srz-upgrade-{}-{:x}-{}",
        std::process::id(),
        nonce,
        target_info.binary_name
    );
    let temp_path = temp_dir.join(temp_name);

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
    std::io::Write::write_all(&mut temp_file, &binary_bytes)
        .with_context(|| format!("Failed to write temporary binary at {:?}", temp_path))?;
    std::io::Write::flush(&mut temp_file)
        .with_context(|| format!("Failed to flush temporary binary at {:?}", temp_path))?;

    // 8. Swap binary
    let swap_pb = Ui::spinner(format!("Installing to {}...", current_exe.display()));
    if let Err(err) = self_replace::self_replace(&temp_path) {
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
