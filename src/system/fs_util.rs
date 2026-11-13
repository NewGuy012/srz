use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

/// Reads a trimmed, non-empty path from the environment variable `var`.
pub fn non_empty_env_path(var: &str) -> Option<PathBuf> {
    env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

/// Returns the user's home directory (`$HOME` or `%USERPROFILE%`).
pub fn home_dir() -> Option<PathBuf> {
    non_empty_env_path("HOME").or_else(|| non_empty_env_path("USERPROFILE"))
}

fn resolve_app_path(
    overrides: &[&str],
    xdg_env: &str,
    xdg_sub: &[&str],
    #[cfg_attr(not(target_os = "windows"), allow(unused_variables))] win: (&str, &[&str]),
    home_sub: &[&str],
    temp_sub: &[&str],
) -> PathBuf {
    let join = |base: PathBuf, segs: &[&str]| segs.iter().fold(base, |p, s| p.join(s));
    if let Some(p) = overrides.iter().find_map(|k| non_empty_env_path(k)) {
        return p;
    }
    if let Some(xdg) = non_empty_env_path(xdg_env) {
        return join(xdg, xdg_sub);
    }
    #[cfg(target_os = "windows")]
    if let Some(w) = non_empty_env_path(win.0) {
        return join(w, win.1);
    }
    home_dir().map_or_else(
        || join(std::env::temp_dir(), temp_sub),
        |h| join(h, home_sub),
    )
}

/// Returns the default persistent data directory for srz (`$SRZ_DATA_DIR`, `$XDG_DATA_HOME/srz`, `~/.local/share/srz`, or `%LOCALAPPDATA%/srz/data`).
pub fn default_data_dir() -> PathBuf {
    resolve_app_path(
        &["SRZ_DATA_DIR"],
        "XDG_DATA_HOME",
        &["srz"],
        ("LOCALAPPDATA", &["srz", "data"]),
        &[".local", "share", "srz"],
        &["srz-data"],
    )
}

/// Returns the default cache directory (`$SRZ_CACHE_DIR`, `$XDG_CACHE_HOME/srz`, `~/.cache/srz`, `%LOCALAPPDATA%/srz/cache`).
pub fn default_cache_dir() -> PathBuf {
    resolve_app_path(
        &["SRZ_CACHE_DIR"],
        "XDG_CACHE_HOME",
        &["srz"],
        ("LOCALAPPDATA", &["srz", "cache"]),
        &[".cache", "srz"],
        &["srz-cache"],
    )
}

/// Returns the default config path (`$SRZ_CONFIG`, `$SRZ_CONFIG_PATH`, `$XDG_CONFIG_HOME/srz/config.toml`, `~/.config/srz/config.toml`, or `%APPDATA%/srz/config.toml`).
pub fn default_config_path() -> PathBuf {
    resolve_app_path(
        &["SRZ_CONFIG", "SRZ_CONFIG_PATH"],
        "XDG_CONFIG_HOME",
        &["srz", "config.toml"],
        ("APPDATA", &["srz", "config.toml"]),
        &[".config", "srz", "config.toml"],
        &["srz", "config.toml"],
    )
}

/// Generates a unique temporary file path within `dir` using PID, nanos timestamp, and atomic counter.
pub fn create_temp_file_path(dir: &Path, prefix: &str, suffix: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_name = format!(
        "{}-{}-{:x}-{}{}",
        prefix,
        std::process::id(),
        nonce,
        count,
        suffix
    );
    dir.join(temp_name)
}

/// Sets directory permissions to `0700` on Unix systems (no-op on other platforms).
pub fn set_private_dir_perms(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = path;
}

/// Sets file permissions to `0600` on Unix systems (no-op on other platforms).
pub fn set_private_file_perms(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    #[cfg(not(unix))]
    let _ = path;
}

pub(crate) fn reject_symlink(path: &Path, action: &str) -> std::io::Result<()> {
    if fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Refusing to {action}: {}", path.display()),
        ));
    }
    Ok(())
}

/// Creates a directory (and parents) and restricts permissions to `0700` on Unix.
/// Rejects symlink paths to prevent symlink traversal/privilege elevation (CWE-59).
pub fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    reject_symlink(path, "create directory at symlink path")?;
    fs::create_dir_all(path)?;
    set_private_dir_perms(path);
    Ok(())
}

/// Atomically opens or creates a file with owner-only permissions (`0600` on Unix).
/// Rejects symlink targets to prevent arbitrary file truncation/privilege escalation (CWE-59).
pub fn open_private_file_for_write(path: &Path) -> std::io::Result<File> {
    reject_symlink(path, "open symlink for writing")?;
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = file.set_permissions(fs::Permissions::from_mode(0o600));
    }
    Ok(file)
}

/// Writes `contents` to `path` with crash-safe atomic rename and owner-only permissions (`0600` on Unix).
/// Rejects symlink targets to prevent arbitrary file truncation/privilege escalation (CWE-59).
pub fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    reject_symlink(path, "write to symlink")?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        create_private_dir_all(parent)?;
    }

    let temp_path = create_temp_file_path(parent, ".srz-tmp", ".tmp");

    let mut open_opts = fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(0o600);
    }

    let mut temp_file = open_opts.open(&temp_path)?;
    if let Err(err) = temp_file
        .write_all(contents.as_ref())
        .and_then(|_| temp_file.flush())
    {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    drop(temp_file);

    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }

    set_private_file_perms(path);
    Ok(())
}

// Re-export test utilities from dedicated testing module for backward compatibility
pub use crate::testing::{TempDir, tempdir};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_non_empty_env_path() {
        assert_eq!(non_empty_env_path("SRZ_NON_EXISTENT_VAR_XYZ"), None);
    }

    #[test]
    fn test_default_data_dir() {
        let dir = default_data_dir();
        assert!(!dir.as_os_str().is_empty());
    }

    #[test]
    fn test_create_and_write_private_file() {
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("subdir");
        create_private_dir_all(&dir).unwrap();
        assert!(dir.is_dir());

        let file_path = dir.join("test_write.txt");
        write_private_file(&file_path, b"test private content").unwrap();

        let read_back = fs::read(&file_path).unwrap();
        assert_eq!(read_back, b"test private content");
    }

    #[cfg(unix)]
    #[test]
    fn test_write_private_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempdir().unwrap();
        let file_path = tmp.path().join("test.txt");

        write_private_file(&file_path, b"test private content").unwrap();

        let meta = fs::metadata(&file_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_fs_util_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempdir().unwrap();
        let dir = tmp.path();

        let target = dir.join("real_target.txt");
        fs::write(&target, b"important data").unwrap();

        let link = dir.join("symlink_file.txt");
        symlink(&target, &link).unwrap();

        // Opening symlink for private write must be rejected with InvalidInput
        let write_res = open_private_file_for_write(&link);
        assert!(write_res.is_err());
        assert_eq!(
            write_res.err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );

        // Atomic write to symlink must also be rejected
        let atomic_res = write_private_file(&link, b"hijack attempt");
        assert!(atomic_res.is_err());
        assert_eq!(
            atomic_res.err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );

        // Creating private dir at symlink must be rejected
        let link_dir = dir.join("symlink_dir");
        symlink(&target, &link_dir).unwrap();
        let dir_res = create_private_dir_all(&link_dir);
        assert!(dir_res.is_err());
        assert_eq!(
            dir_res.err().unwrap().kind(),
            std::io::ErrorKind::InvalidInput
        );
    }
}
