use anyhow::{bail, Context, Result};
use flate2::read::GzDecoder;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::env;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use tar::Archive;
use zip::ZipArchive;

use crate::ui::Ui;

pub const DEFAULT_REPO: &str = "NewGuy012/srz";

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
        ("macos", "x86_64") => Ok(TargetInfo {
            target: "x86_64-apple-darwin",
            format: ArchiveFormat::TarGz,
            binary_name: "srz",
        }),
        ("linux", "x86_64") => Ok(TargetInfo {
            target: "x86_64-unknown-linux-gnu",
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

/// Formats primary versioned asset name and fallback alias.
pub fn format_asset_names(
    tag: &str,
    target: &str,
    format: ArchiveFormat,
) -> (String, String) {
    let ext = format.extension();
    let versioned = format!("srz-{}-{}.{}", tag, target, ext);
    let latest_alias = format!("srz-{}.{}", target, ext);
    (versioned, latest_alias)
}

/// Extracts the named binary from a gzipped tarball.
pub fn extract_binary_from_tar_gz(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let gz = GzDecoder::new(Cursor::new(bytes));
    let mut archive = Archive::new(gz);
    for entry in archive.entries().context("Failed to read tar entries")? {
        let mut entry = entry.context("Corrupt tar entry")?;
        let path = entry.path().context("Invalid tar entry path")?;
        if path.file_name().and_then(|n| n.to_str()) == Some(binary_name) {
            let mut buf = Vec::new();
            std::io::copy(&mut entry, &mut buf).context("Failed to extract binary from tar entry")?;
            return Ok(buf);
        }
    }
    bail!("Binary '{}' not found in release archive", binary_name)
}

/// Extracts the named binary from a zip archive.
pub fn extract_binary_from_zip(bytes: &[u8], binary_name: &str) -> Result<Vec<u8>> {
    let mut zip = ZipArchive::new(Cursor::new(bytes)).context("Failed to read zip archive")?;
    for i in 0..zip.len() {
        let mut file = zip.by_index(i).context("Corrupt zip entry")?;
        let name = file.name().to_string();
        if name == binary_name || name.ends_with(&format!("/{}", binary_name)) || name.ends_with(&format!("\\{}", binary_name)) {
            let mut buf = Vec::new();
            std::io::copy(&mut file, &mut buf).context("Failed to extract binary from zip entry")?;
            return Ok(buf);
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
    DryRun,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UpgradeResult {
    pub current_version: String,
    pub target_version: String,
    pub target_platform: String,
    pub status: UpgradeStatus,
    pub message: String,
}

/// Resolves the latest version tag from GitHub.
/// Uses a HEAD request to inspect the redirect URL to avoid GitHub's REST API rate limit (60 req/hr).
pub async fn resolve_latest_version(client: &reqwest::Client, repo: &str) -> Result<String> {
    let redirect_url = format!("https://github.com/{}/releases/latest", repo);
    let no_redirect = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build();

    if let Ok(no_redirect_client) = no_redirect {
        if let Ok(resp) = no_redirect_client.head(&redirect_url).send().await {
            let status = resp.status();
            if status.is_redirection() {
                if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
                    if let Ok(loc_str) = loc.to_str() {
                        if let Some(pos) = loc_str.rfind("/tag/") {
                            let tag = &loc_str[pos + 5..];
                            let clean_tag = tag.trim_matches('/').to_string();
                            if !clean_tag.is_empty() {
                                return Ok(clean_tag);
                            }
                        }
                    }
                }
            }
        }
    }

    // Fallback: GitHub REST API
    let api_url = format!("https://api.github.com/repos/{}/releases/latest", repo);
    let release_json: serde_json::Value = client
        .get(&api_url)
        .header(reqwest::header::ACCEPT, "application/vnd.github.v3+json")
        .send()
        .await
        .context("Failed to query GitHub releases API")?
        .error_for_status()
        .context("GitHub API error")?
        .json()
        .await
        .context("Failed to parse GitHub release JSON")?;

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

    let archive_pb = Ui::spinner(format!("Downloading {}...", archive_url));
    let archive_bytes = archive_res
        .error_for_status()
        .context("Failed to download release archive")?
        .bytes()
        .await
        .context("Failed to read archive bytes")?
        .to_vec();
    Ui::finish_spinner(&archive_pb, "Downloaded release archive.");

    let sha_pb = Ui::spinner("Verifying SHA256 checksum...");
    let sha_res = client.get(&sha_url).send().await;
    match sha_res {
        Ok(resp) if resp.status().is_success() => {
            let sha_text = resp.text().await.unwrap_or_default();
            if let Some(expected_hash) = parse_sha256_checksum(&sha_text) {
                let mut hasher = Sha256::new();
                hasher.update(&archive_bytes);
                let computed_hash = format!("{:x}", hasher.finalize());

                if !expected_hash.eq_ignore_ascii_case(&computed_hash) {
                    sha_pb.finish_and_clear();
                    bail!(
                        "SHA256 checksum verification failed!\nExpected: {}\nComputed: {}",
                        expected_hash,
                        computed_hash
                    );
                }
                Ui::finish_spinner(&sha_pb, &format!("Checksum verified ({:.8}...).", computed_hash));
            } else {
                Ui::finish_spinner(&sha_pb, "Checksum file empty; skipping strict hash check.");
            }
        }
        _ => {
            Ui::finish_spinner(&sha_pb, "Checksum file unavailable on release; proceeding.");
        }
    }

    Ok(archive_bytes)
}

/// Main entry point for executing `srz upgrade`.
pub async fn run_upgrade(
    check: bool,
    force: bool,
    custom_version: Option<&str>,
    dry_run: bool,
    json: bool,
) -> Result<()> {
    let current_version_str = env!("CARGO_PKG_VERSION");
    let current_version = parse_clean_version(current_version_str)
        .context("Failed to parse current CARGO_PKG_VERSION")?;

    let current_exe = env::current_exe().context("Failed to determine path to current executable")?;

    // 1. Check install origin
    let origin = detect_install_origin(&current_exe);
    if origin == InstallOrigin::Homebrew {
        let msg = "srz was installed via Homebrew. Please run `brew upgrade srz` instead.";
        if json {
            let res = UpgradeResult {
                current_version: current_version_str.to_string(),
                target_version: current_version_str.to_string(),
                target_platform: env::consts::OS.to_string(),
                status: UpgradeStatus::HomebrewManaged,
                message: msg.to_string(),
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
            return Ok(());
        }
        Ui::info(msg);
        return Ok(());
    }

    // 2. Detect platform target
    let target_info = match detect_target() {
        Ok(info) => info,
        Err(err) => {
            if json {
                let res = UpgradeResult {
                    current_version: current_version_str.to_string(),
                    target_version: current_version_str.to_string(),
                    target_platform: format!("{}-{}", env::consts::OS, env::consts::ARCH),
                    status: UpgradeStatus::AlreadyUpToDate,
                    message: err.clone(),
                };
                println!("{}", serde_json::to_string_pretty(&res)?);
                return Ok(());
            }
            bail!("{}", err);
        }
    };

    // 3. Setup HTTP client
    let client = reqwest::Client::builder()
        .user_agent(format!("srz/{}", current_version_str))
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

    let target_version = parse_clean_version(&target_tag)
        .with_context(|| format!("Failed to parse release tag '{}' as a semver version", target_tag))?;

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

        if json {
            let res = UpgradeResult {
                current_version: current_version_str.to_string(),
                target_version: target_version.to_string(),
                target_platform: target_info.target.to_string(),
                status,
                message: msg,
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
            return Ok(());
        }

        if is_newer {
            Ui::info(&msg);
        } else {
            Ui::success(&msg);
        }
        return Ok(());
    }

    // Up to date check
    if !is_newer && !force && custom_version.is_none() {
        let msg = format!("srz is already up to date (v{}).", current_version);
        if json {
            let res = UpgradeResult {
                current_version: current_version_str.to_string(),
                target_version: target_version.to_string(),
                target_platform: target_info.target.to_string(),
                status: UpgradeStatus::AlreadyUpToDate,
                message: msg,
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
            return Ok(());
        }
        Ui::success(&msg);
        return Ok(());
    }

    if origin == InstallOrigin::Cargo && !force {
        let msg = format!(
            "srz was installed via Cargo (~/.cargo/bin). You can update with `cargo install srz`, or use `srz upgrade --force` to overwrite directly."
        );
        if json {
            let res = UpgradeResult {
                current_version: current_version_str.to_string(),
                target_version: target_version.to_string(),
                target_platform: target_info.target.to_string(),
                status: UpgradeStatus::CargoManaged,
                message: msg,
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
            return Ok(());
        }
        Ui::warn(&msg);
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

    // 7. Write to temp file in current_exe directory or system temp
    let temp_dir = current_exe
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(env::temp_dir);

    let temp_name = format!(
        ".srz-upgrade-{}-{}",
        std::process::id(),
        target_info.binary_name
    );
    let temp_path = temp_dir.join(temp_name);
    std::fs::write(&temp_path, &binary_bytes)
        .with_context(|| format!("Failed to write temporary binary at {:?}", temp_path))?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = std::fs::metadata(&temp_path) {
            let mut perms = meta.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&temp_path, perms);
        }
    }

    // 8. Swap binary
    if dry_run {
        let _ = std::fs::remove_file(&temp_path);
        let msg = format!(
            "[dry-run] Would replace {} with srz v{} ({})",
            current_exe.display(),
            target_version,
            target_info.target
        );
        if json {
            let res = UpgradeResult {
                current_version: current_version_str.to_string(),
                target_version: target_version.to_string(),
                target_platform: target_info.target.to_string(),
                status: UpgradeStatus::DryRun,
                message: msg,
            };
            println!("{}", serde_json::to_string_pretty(&res)?);
            return Ok(());
        }
        Ui::info(&msg);
        return Ok(());
    }

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
    if let Ok(out) = Command::new(&current_exe).arg("--version").output() {
        if out.status.success() {
            let v_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !v_str.is_empty() {
                verified_version = v_str;
            }
        }
    }

    let success_msg = format!("Successfully upgraded srz to {}!", verified_version);
    if json {
        let res = UpgradeResult {
            current_version: current_version_str.to_string(),
            target_version: target_version.to_string(),
            target_platform: target_info.target.to_string(),
            status: UpgradeStatus::Upgraded,
            message: success_msg,
        };
        println!("{}", serde_json::to_string_pretty(&res)?);
        return Ok(());
    }

    Ui::success(&success_msg);
    Ok(())
}
