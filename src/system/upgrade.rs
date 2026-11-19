use crate::system::sha256::sha256_hex;
use crate::ui::Ui;
use anyhow::{Context, Result, bail};
use flate2::read::GzDecoder;
use serde::Serialize;
use std::cmp::Ordering;
use std::env;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const DEFAULT_REPO: &str = "NewGuy012/srz";
const MAX_ARCHIVE_BYTES: usize = 50 * 1024 * 1024;
const MAX_BINARY_BYTES: u64 = 100 * 1024 * 1024;
const MAX_TAR_STREAM_BYTES: u64 = MAX_BINARY_BYTES + 20 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ArchiveFormat {
    TarGz,
    Zip,
}

impl ArchiveFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::TarGz => "tar.gz",
            Self::Zip => "zip",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TargetInfo {
    target: &'static str,
    format: ArchiveFormat,
    binary_name: &'static str,
}

fn detect_target_from(os: &str, arch: &str) -> Result<TargetInfo, String> {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum InstallOrigin {
    Homebrew,
    Cargo,
    Direct,
}

fn detect_install_origin(exe_path: &Path) -> InstallOrigin {
    let lower = exe_path.to_string_lossy().to_ascii_lowercase();
    if lower.contains("/opt/homebrew/")
        || lower.contains("/usr/local/cellar/")
        || lower.contains("/cellar/srz/")
        || lower.contains("/.linuxbrew/")
    {
        InstallOrigin::Homebrew
    } else if lower.contains("/.cargo/bin/") || lower.contains("\\.cargo\\bin\\") {
        InstallOrigin::Cargo
    } else {
        InstallOrigin::Direct
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Version {
    major: u64,
    minor: u64,
    patch: u64,
    pre: Option<String>,
}

impl Version {
    fn parse(v: &str) -> Result<Self> {
        let s = v.trim();
        let s = s.strip_prefix(['v', 'V']).unwrap_or(s);
        let version_pre = match s.split_once('+') {
            Some((vp, b)) => {
                if b.is_empty()
                    || !b
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
                {
                    bail!("Invalid build metadata: {}", b);
                }
                vp
            }
            None => s,
        };
        let (core, pre) = match version_pre.split_once('-') {
            Some((c, p)) => {
                if p.is_empty()
                    || !p
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'.' || c == b'-')
                {
                    bail!("Invalid prerelease identifier: {}", p);
                }
                (c, Some(p.to_string()))
            }
            None => (version_pre, None),
        };

        let mut parts = core.split('.');
        let (Some(maj), Some(min), Some(pat), None) =
            (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            bail!("Expected major.minor.patch, got: {}", core);
        };
        let parse_part = |part: &str, name: &str| -> Result<u64> {
            part.parse::<u64>()
                .with_context(|| format!("Invalid {} version: {}", name, part))
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

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

fn cmp_prerelease(a: &str, b: &str) -> Ordering {
    for (pa, pb) in a.split('.').zip(b.split('.')) {
        let ord = match (pa.parse::<u64>(), pb.parse::<u64>()) {
            (Ok(na), Ok(nb)) => na.cmp(&nb),
            (Ok(_), Err(_)) => Ordering::Less,
            (Err(_), Ok(_)) => Ordering::Greater,
            (Err(_), Err(_)) => pa.cmp(pb),
        };
        if ord != Ordering::Equal {
            return ord;
        }
    }
    a.split('.').count().cmp(&b.split('.').count())
}

impl Ord for Version {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.major, self.minor, self.patch)
            .cmp(&(other.major, other.minor, other.patch))
            .then_with(|| match (&self.pre, &other.pre) {
                (None, None) => Ordering::Equal,
                (Some(_), None) => Ordering::Less,
                (None, Some(_)) => Ordering::Greater,
                (Some(a), Some(b)) => cmp_prerelease(a, b),
            })
    }
}

fn parse_sha256_checksum<'a>(content: &'a str, expected_asset: &str) -> Option<&'a str> {
    let line = content.lines().find(|l| !l.trim().is_empty())?;
    let mut parts = line.split_whitespace();
    let hex = parts
        .next()
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))?;
    if let Some(file_col) = parts.next() {
        let clean_name = file_col
            .trim_start_matches('*')
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or("");
        if clean_name != expected_asset {
            return None;
        }
    }
    Some(hex)
}

fn verify_tar_header_checksum(header: &[u8; 512]) -> bool {
    let cksum_str = std::str::from_utf8(&header[148..156])
        .unwrap_or("")
        .trim_matches(['\0', ' ']);
    let Ok(expected) = u32::from_str_radix(cksum_str, 8) else {
        return false;
    };
    let unsigned: u32 = header
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                b as u32
            }
        })
        .sum();
    let signed: i32 = header
        .iter()
        .enumerate()
        .map(|(i, &b)| {
            if (148..156).contains(&i) {
                32
            } else {
                (b as i8) as i32
            }
        })
        .sum();
    unsigned == expected || signed == expected as i32
}

fn extract_binary_from_tar_gz(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    extract_binary_from_tar_gz_limited(bytes, binary_name, MAX_TAR_STREAM_BYTES)
}

fn extract_binary_from_tar_gz_limited(
    bytes: &[u8],
    binary_name: &str,
    max_stream_bytes: u64,
) -> Result<Vec<u8>> {
    let mut gz = GzDecoder::new(Cursor::new(bytes)).take(max_stream_bytes.saturating_add(1));
    let mut header = [0u8; 512];

    loop {
        match gz.read_exact(&mut header) {
            Ok(()) => {}
            Err(_) if gz.limit() == 0 => bail!("Decompressed tar archive exceeds security limit"),
            Err(err) if err.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(err) => return Err(err).context("Failed to read tar header from release archive"),
        }

        if header.iter().all(|&b| b == 0) {
            break;
        }
        if !verify_tar_header_checksum(&header) {
            bail!("Corrupt tar archive: invalid USTAR header checksum");
        }

        let name_bytes = &header[0..100];
        let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(100);
        let name = std::str::from_utf8(&name_bytes[..name_len]).unwrap_or("");

        let size_str = std::str::from_utf8(&header[124..136])
            .unwrap_or("")
            .trim_matches(['\0', ' ']);
        let size = if size_str.is_empty() {
            0
        } else {
            u64::from_str_radix(size_str, 8)
                .context("Corrupt tar archive: invalid octal entry size")?
        };

        let is_regular_file = matches!(header[156], 0 | b'0' | b'7');
        let filename = name.rsplit('/').next().unwrap_or(name);

        if is_regular_file && filename == binary_name {
            if size == 0 {
                bail!(
                    "Corrupt tar archive: release binary '{}' is empty",
                    binary_name
                );
            }
            if size > MAX_BINARY_BYTES || size > max_stream_bytes {
                bail!("Decompressed binary exceeds security limit");
            }
            let initial_cap = (size as usize).min(64 * 1024);
            let mut binary_buf = Vec::with_capacity(initial_cap);
            let bytes_read = gz
                .by_ref()
                .take(size)
                .read_to_end(&mut binary_buf)
                .context("Failed to read binary data from tar")?;
            if gz.limit() == 0 && (bytes_read as u64) < size {
                bail!("Decompressed tar archive exceeds security limit");
            }
            if bytes_read as u64 != size {
                bail!("Corrupt tar archive: unexpected EOF while reading binary payload");
            }
            return Ok(binary_buf);
        }

        let rem = size % 512;
        let pad = if rem == 0 { 0 } else { 512 - rem };
        let padded_size = size
            .checked_add(pad)
            .context("Corrupt tar archive: entry size overflow")?;
        if padded_size > gz.limit() {
            bail!("Decompressed tar archive exceeds security limit");
        }
        if padded_size > 0 {
            let mut discard = gz.by_ref().take(padded_size);
            let skipped = std::io::copy(&mut discard, &mut std::io::sink())
                .context("Failed to skip entry in tar archive")?;
            if gz.limit() == 0 {
                bail!("Decompressed tar archive exceeds security limit");
            }
            if skipped != padded_size {
                bail!("Corrupt tar archive: unexpected EOF while skipping tar entry");
            }
        }
    }

    bail!("Binary '{}' not found in release archive", binary_name)
}

fn extract_binary_from_zip(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let reader = crate::system::zip::ZipReader::new(bytes).context("Failed to read zip archive")?;
    for entry in reader.entries() {
        if entry.is_regular_file && entry.name.rsplit(['/', '\\']).next() == Some(binary_name) {
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

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum UpgradeStatus {
    AlreadyUpToDate,
    UpdateAvailable,
    Upgraded,
    HomebrewManaged,
    CargoManaged,
}

#[derive(Debug, Serialize)]
struct UpgradeResult {
    current_version: String,
    target_version: String,
    target_platform: String,
    status: UpgradeStatus,
    message: String,
}

impl UpgradeResult {
    fn emit(
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

fn is_allowed_upgrade_host(host: &str) -> bool {
    let h = host.trim_end_matches('.').to_ascii_lowercase();
    h == "github.com"
        || h.ends_with(".github.com")
        || h == "githubusercontent.com"
        || h.ends_with(".githubusercontent.com")
}

/// Validates that an upgrade redirect target remains HTTPS on a public GitHub release host.
fn validate_upgrade_redirect_url(url: &reqwest::Url) -> Result<()> {
    if url.scheme() != "https" {
        bail!("Refusing non-HTTPS upgrade redirect: {}", url.scheme());
    }
    crate::scraper::validate_parsed_public_http_url(url)?;
    let host = url
        .host_str()
        .context("Upgrade redirect URL missing host")?;
    if !is_allowed_upgrade_host(host) {
        bail!("Refusing upgrade redirect to untrusted host '{}'", host);
    }
    Ok(())
}

fn build_upgrade_client(
    user_agent: &str,
    timeout: Duration,
    redirect: reqwest::redirect::Policy,
) -> std::result::Result<reqwest::Client, reqwest::Error> {
    reqwest::Client::builder()
        .user_agent(user_agent)
        .dns_resolver(Arc::new(crate::scraper::PublicDnsResolver))
        .redirect(redirect)
        .connect_timeout(Duration::from_secs(5))
        .timeout(timeout)
        .build()
}

async fn resolve_latest_version(
    client: &reqwest::Client,
    user_agent: &str,
    repo: &str,
) -> Result<String> {
    let redirect_url = format!("https://github.com/{}/releases/latest", repo);
    if let Ok(no_redirect) = build_upgrade_client(
        user_agent,
        Duration::from_secs(15),
        reqwest::redirect::Policy::none(),
    ) && let Ok(resp) = no_redirect.head(&redirect_url).send().await
        && resp.status().is_redirection()
        && let Some(loc) = resp.headers().get(reqwest::header::LOCATION)
        && let Ok(loc_str) = loc.to_str()
        && let Ok(parsed) = reqwest::Url::parse(loc_str)
        && validate_upgrade_redirect_url(&parsed).is_ok()
        && parsed
            .host_str()
            .is_some_and(|h| h.eq_ignore_ascii_case("github.com"))
        && let Some(pos) = parsed.path().rfind("/releases/tag/")
    {
        let clean_tag = parsed.path()[pos + "/releases/tag/".len()..].trim_matches('/');
        if let Ok(ver) = Version::parse(clean_tag) {
            return Ok(format!("v{}", ver));
        }
    }

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
    let ver = Version::parse(tag).context("Failed to parse GitHub release tag_name as semver")?;
    Ok(format!("v{}", ver))
}

async fn download_and_verify_archive(
    client: &reqwest::Client,
    repo: &str,
    tag: &str,
    versioned_name: &str,
    latest_alias: &str,
) -> Result<Vec<u8>> {
    let base_dl = format!("https://github.com/{}/releases/download/{}", repo, tag);
    let v_url = format!("{}/{}", base_dl, versioned_name);
    let alias_url = format!("{}/{}", base_dl, latest_alias);

    let first_resp = client
        .get(&v_url)
        .send()
        .await
        .context("Failed to initiate archive download")?;
    let (asset_name, archive_url, archive_res) =
        if first_resp.status() == reqwest::StatusCode::NOT_FOUND {
            let alias_resp = client
                .get(&alias_url)
                .send()
                .await
                .context("Failed to initiate fallback archive download")?;
            (latest_alias, alias_url, alias_resp)
        } else {
            (versioned_name, v_url, first_resp)
        };

    if !archive_res.status().is_success() {
        bail!(
            "Failed to download release archive (HTTP status: {})",
            archive_res.status()
        );
    }

    let archive_bytes = Ui::with_spinner(
        "Downloading release archive...",
        "Downloaded release archive.",
        crate::scraper::read_response_bytes_limited(archive_res, MAX_ARCHIVE_BYTES),
    )
    .await?;

    let sha_url = format!("{}.sha256", archive_url);
    let sha_pb = Ui::spinner("Verifying SHA256 checksum...");
    let sha_resp = match client.get(&sha_url).send().await {
        Ok(r) if r.status().is_success() => r,
        Ok(r) => {
            sha_pb.finish_and_clear();
            bail!(
                "Failed to retrieve SHA256 checksum file from {} (HTTP status: {})",
                sha_url,
                r.status()
            );
        }
        Err(err) => {
            sha_pb.finish_and_clear();
            return Err(err).with_context(|| {
                format!("Failed to retrieve SHA256 checksum file from {}", sha_url)
            });
        }
    };

    let sha_text = match crate::scraper::read_response_text_limited(sha_resp, 64 * 1024).await {
        Ok(t) => t,
        Err(err) => {
            sha_pb.finish_and_clear();
            return Err(err).context("Failed to read SHA256 checksum file");
        }
    };
    let Some(expected_hash) = parse_sha256_checksum(&sha_text, asset_name) else {
        sha_pb.finish_and_clear();
        bail!(
            "SHA256 checksum file from {} was empty or malformed. Aborting upgrade.",
            sha_url
        );
    };

    let computed_hash = sha256_hex(&archive_bytes);
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
    Ok(archive_bytes)
}

fn replace_binary(new_binary: &Path, target_exe: &Path) -> std::io::Result<()> {
    crate::system::fs_util::reject_symlink(target_exe, "replace binary at symlink")?;
    if std::fs::rename(new_binary, target_exe).is_ok() {
        return Ok(());
    }

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

pub(crate) async fn run_upgrade(
    check: bool,
    force: bool,
    custom_version: Option<&str>,
    json: bool,
) -> Result<()> {
    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_version =
        Version::parse(current_version_str).context("Failed to parse current CARGO_PKG_VERSION")?;
    let current_exe =
        env::current_exe().context("Failed to determine path to current executable")?;
    let canonical_exe = std::fs::canonicalize(&current_exe).unwrap_or_else(|_| current_exe.clone());
    let origin = detect_install_origin(&canonical_exe);

    if origin == InstallOrigin::Homebrew {
        return UpgradeResult::emit(
            UpgradeStatus::HomebrewManaged,
            current_version_str,
            env::consts::OS,
            "srz was installed via Homebrew. Please run `brew upgrade srz` instead.",
            json,
        );
    }

    let target_info = detect_target_from(env::consts::OS, env::consts::ARCH)
        .map_err(|err| anyhow::anyhow!("{}", err))?;
    let user_agent = format!("srz/{}", current_version_str);
    let redirect_policy = reqwest::redirect::Policy::custom(|attempt| {
        if attempt.previous().len() >= 10 {
            return attempt.error("Too many redirects during upgrade");
        }
        if let Err(err) = validate_upgrade_redirect_url(attempt.url()) {
            return attempt.error(err.to_string());
        }
        attempt.follow()
    });
    let client = build_upgrade_client(&user_agent, Duration::from_secs(60), redirect_policy)
        .context("Failed to build HTTP client")?;

    let spinner = Ui::spinner("Checking for updates...");
    let raw_target = match custom_version {
        Some(v) => v.to_string(),
        None => match resolve_latest_version(&client, &user_agent, DEFAULT_REPO).await {
            Ok(tag) => tag,
            Err(err) => {
                spinner.finish_and_clear();
                return Err(err).context(
                    "Failed to resolve latest version tag from GitHub. Ensure you have internet connectivity.",
                );
            }
        },
    };
    spinner.finish_and_clear();

    let target_version = Version::parse(&raw_target).with_context(|| {
        format!(
            "Failed to parse release tag '{}' as a semver version",
            raw_target
        )
    })?;
    let target_tag = format!("v{}", target_version);
    let is_newer = target_version > current_version;

    if check {
        let (status, msg) = if is_newer {
            (
                UpgradeStatus::UpdateAvailable,
                format!(
                    "A newer version of srz is available: v{} (current: v{}). Run `srz upgrade` to install.",
                    target_version, current_version
                ),
            )
        } else if target_version == current_version {
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
        return UpgradeResult::emit(
            status,
            target_version.to_string(),
            target_info.target,
            msg,
            json,
        );
    }

    if !is_newer && !force && custom_version.is_none() {
        return UpgradeResult::emit(
            UpgradeStatus::AlreadyUpToDate,
            target_version.to_string(),
            target_info.target,
            format!("srz is already up to date (v{}).", current_version),
            json,
        );
    }

    if origin == InstallOrigin::Cargo && !force {
        return UpgradeResult::emit(
            UpgradeStatus::CargoManaged,
            target_version.to_string(),
            target_info.target,
            "srz was installed via Cargo (~/.cargo/bin). You can update with `cargo install srz`, or use `srz upgrade --force` to overwrite directly.",
            json,
        );
    }

    let ext = target_info.format.extension();
    let versioned_name = format!("srz-{}-{}.{}", target_tag, target_info.target, ext);
    let latest_alias = format!("srz-{}.{}", target_info.target, ext);

    let archive_bytes = download_and_verify_archive(
        &client,
        DEFAULT_REPO,
        &target_tag,
        &versioned_name,
        &latest_alias,
    )
    .await?;

    let extract_pb = Ui::spinner("Extracting new binary...");
    let binary_bytes = match target_info.format {
        ArchiveFormat::TarGz => extract_binary_from_tar_gz(&archive_bytes, target_info.binary_name),
        ArchiveFormat::Zip => extract_binary_from_zip(&archive_bytes, target_info.binary_name),
    };
    drop(archive_bytes);
    let binary_bytes = match binary_bytes {
        Ok(b) => {
            Ui::finish_spinner(&extract_pb, "Extracted new executable.");
            b
        }
        Err(err) => {
            extract_pb.finish_and_clear();
            return Err(err);
        }
    };

    let temp_dir = current_exe
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);

    if let Ok(entries) = std::fs::read_dir(&temp_dir) {
        for entry in entries.flatten() {
            if let Some(name) = entry.file_name().to_str()
                && name.starts_with(".srz-old-")
                && name.ends_with(".tmp")
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }

    let temp_path = crate::system::fs_util::write_staged_temp_file(
        &temp_dir,
        ".srz-upgrade",
        &format!("-{}", target_info.binary_name),
        &binary_bytes,
        0o755,
    )
    .with_context(|| format!("Failed to stage temporary binary in {:?}", temp_dir))?;
    drop(binary_bytes);

    let verified_version = match Command::new(&temp_path).arg("--version").output() {
        Ok(out) if out.status.success() => {
            let v_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if v_str.is_empty() {
                target_tag.clone()
            } else {
                v_str
            }
        }
        Ok(out) => {
            let _ = std::fs::remove_file(&temp_path);
            bail!(
                "Staged binary failed verification (`--version` exited with {})",
                out.status
            );
        }
        Err(err) => {
            let _ = std::fs::remove_file(&temp_path);
            return Err(err).context("Failed to execute staged binary for verification");
        }
    };

    let swap_pb = Ui::spinner(format!("Installing to {}...", current_exe.display()));
    if let Err(err) = replace_binary(&temp_path, &current_exe) {
        let _ = std::fs::remove_file(&temp_path);
        swap_pb.finish_and_clear();
        bail!("Failed to replace binary at {:?}: {}", current_exe, err);
    }
    Ui::finish_spinner(&swap_pb, "Binary replaced successfully.");

    UpgradeResult::emit(
        UpgradeStatus::Upgraded,
        target_version.to_string(),
        target_info.target,
        format!("Successfully upgraded srz to {}!", verified_version),
        json,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_validate_upgrade_redirect_url() {
        for ok in [
            "https://github.com/NewGuy012/srz/releases/download/v1.0.0/srz.tar.gz",
            "https://objects.githubusercontent.com/release/srz.tar.gz",
        ] {
            let parsed = reqwest::Url::parse(ok).unwrap();
            assert!(validate_upgrade_redirect_url(&parsed).is_ok(), "{ok}");
        }
        for bad in [
            "http://github.com/NewGuy012/srz/releases/download/v1.0.0/srz.tar.gz",
            "https://evil.example.com/srz.tar.gz",
            "https://127.0.0.1/srz.tar.gz",
            "https://169.254.169.254/latest/meta-data",
            "https://localhost/srz.tar.gz",
        ] {
            let parsed = reqwest::Url::parse(bad).unwrap();
            assert!(validate_upgrade_redirect_url(&parsed).is_err(), "{bad}");
        }
    }

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
        header[148..156].copy_from_slice(format!("{:06o}\0 ", cksum).as_bytes());
        header
    }

    fn gzip_tar_entries(entries: &[([u8; 512], &[u8])], trailing_zeros: usize) -> Vec<u8> {
        let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
        for (header, body) in entries {
            gz.write_all(header).unwrap();
            gz.write_all(body).unwrap();
            let pad = (512 - (body.len() % 512)) % 512;
            if pad > 0 {
                gz.write_all(&vec![0u8; pad]).unwrap();
            }
        }
        if trailing_zeros > 0 {
            gz.write_all(&vec![0u8; trailing_zeros]).unwrap();
        }
        gz.finish().unwrap()
    }

    #[test]
    fn test_detect_target_and_origin() {
        assert_eq!(
            detect_target_from("windows", "x86_64").unwrap().binary_name,
            "srz.exe"
        );
        assert!(
            detect_target_from("macos", "x86_64")
                .unwrap_err()
                .contains("Intel Macs")
        );
        assert!(detect_target_from("solaris", "sparc").is_err());

        for (path, expected) in [
            (
                "/opt/homebrew/Cellar/srz/0.1.0/bin/srz",
                InstallOrigin::Homebrew,
            ),
            (
                "/home/linuxbrew/.linuxbrew/bin/srz",
                InstallOrigin::Homebrew,
            ),
            (
                "C:\\Users\\User\\.CARGO\\BIN\\srz.exe",
                InstallOrigin::Cargo,
            ),
            ("/Users/alice/homebrew/srz", InstallOrigin::Direct),
        ] {
            assert_eq!(detect_install_origin(Path::new(path)), expected, "{path}");
        }
    }

    #[test]
    fn test_parse_sha256_checksum() {
        let hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert_eq!(
            parse_sha256_checksum(&format!("{hash}  srz.tar.gz\n"), "srz.tar.gz"),
            Some(hash)
        );
        assert_eq!(parse_sha256_checksum(hash, "srz.tar.gz"), Some(hash));
        assert_eq!(
            parse_sha256_checksum(&format!("{hash}  other.tar.gz\n"), "srz.tar.gz"),
            None
        );
        assert_eq!(
            parse_sha256_checksum("   \n<html>bad</html>", "srz.tar.gz"),
            None
        );
    }

    #[test]
    fn test_version_parse_and_semver_prerelease_order() {
        assert!(
            Version::parse("v1.0.0-alpha.2").unwrap() < Version::parse("v1.0.0-alpha.10").unwrap()
        );
        assert!(Version::parse("1.0.0-rc.1").unwrap() < Version::parse("1.0.0").unwrap());
        for invalid in ["invalid", "../../../evil", "v0.7.1/payload", "; rm -rf /"] {
            assert!(Version::parse(invalid).is_err(), "{invalid}");
        }
    }

    #[cfg(unix)]
    #[test]
    fn test_replace_binary_refuses_symlink() {
        let tmp = crate::fs_util::tempdir().unwrap();
        let new_binary = tmp.path().join("new_bin");
        let real_target = tmp.path().join("real_target");
        let symlink_target = tmp.path().join("symlink_target");
        std::fs::write(&new_binary, b"new").unwrap();
        std::fs::write(&real_target, b"old").unwrap();
        std::os::unix::fs::symlink(&real_target, &symlink_target).unwrap();
        assert!(
            replace_binary(&new_binary, &symlink_target)
                .unwrap_err()
                .to_string()
                .contains("symlink")
        );
    }

    #[test]
    fn test_extract_binary_from_tar_gz_and_zip() {
        let symlink_hdr = build_ustar_header("srz", b'2', "00000000000 ");
        let dir_hdr = build_ustar_header("srz", b'5', "00000000000 ");
        let payload = b"real-srz-binary";
        let reg_hdr = build_ustar_header("srz", b'0', &format!("{:011o} ", payload.len()));
        let mixed_tar = gzip_tar_entries(
            &[(symlink_hdr, b""), (dir_hdr, b""), (reg_hdr, payload)],
            1024,
        );
        assert_eq!(
            extract_binary_from_tar_gz(&mixed_tar, "srz").unwrap(),
            payload
        );

        let filler_hdr = build_ustar_header("filler.bin", b'0', &format!("{:011o} ", 20_000));
        let bomb_tar = gzip_tar_entries(&[(filler_hdr, &vec![0u8; 20_000])], 1024);
        assert!(extract_binary_from_tar_gz_limited(&bomb_tar, "srz", 8 * 1024).is_err());

        let bad_cksum_hdr = [b'x'; 512];
        let bad_cksum_tar = gzip_tar_entries(&[(bad_cksum_hdr, b"")], 512);
        assert!(extract_binary_from_tar_gz(&bad_cksum_tar, "srz").is_err());

        let zip_bytes = crate::system::zip::create_test_zip(&[("srz.exe", b"MZ-mock")]);
        assert_eq!(
            extract_binary_from_zip(&zip_bytes, "srz.exe").unwrap(),
            b"MZ-mock"
        );
        let empty_zip = crate::system::zip::create_test_zip(&[("srz.exe", b"")]);
        assert!(extract_binary_from_zip(&empty_zip, "srz.exe").is_err());
    }
}
