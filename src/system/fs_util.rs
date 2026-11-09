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

/// Returns the default persistent data directory for srz (`$SRZ_DATA_DIR`, `$XDG_DATA_HOME/srz`, `~/.local/share/srz`, or `%LOCALAPPDATA%/srz/data`).
pub fn default_data_dir() -> PathBuf {
    if let Some(srz_data) = non_empty_env_path("SRZ_DATA_DIR") {
        return srz_data;
    }
    if let Some(xdg_data) = non_empty_env_path("XDG_DATA_HOME") {
        return xdg_data.join("srz");
    }
    #[cfg(target_os = "windows")]
    {
        if let Some(local_app_data) = non_empty_env_path("LOCALAPPDATA") {
            return local_app_data.join("srz").join("data");
        }
    }
    if let Some(home) = non_empty_env_path("HOME") {
        return home.join(".local").join("share").join("srz");
    }
    if let Some(user_profile) = non_empty_env_path("USERPROFILE") {
        return user_profile.join(".local").join("share").join("srz");
    }
    std::env::temp_dir().join("srz-data")
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

/// Creates a directory (and parents) and restricts permissions to `0700` on Unix.
/// Rejects symlink paths to prevent symlink traversal/privilege elevation (CWE-59).
pub fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!(
                "Refusing to create directory at symlink path: {}",
                path.display()
            ),
        ));
    }
    fs::create_dir_all(path)?;
    set_private_dir_perms(path);
    Ok(())
}

/// Atomically opens or creates a file with owner-only permissions (`0600` on Unix).
/// Rejects symlink targets to prevent arbitrary file truncation/privilege escalation (CWE-59).
pub fn open_private_file_for_write(path: &Path) -> std::io::Result<File> {
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Refusing to open symlink for writing: {}", path.display()),
        ));
    }
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
    if let Ok(meta) = fs::symlink_metadata(path)
        && meta.file_type().is_symlink()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("Refusing to write to symlink: {}", path.display()),
        ));
    }

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        create_private_dir_all(parent)?;
    }

    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp_name = format!(".srz-tmp-{}-{:x}-{}.tmp", std::process::id(), nonce, count);
    let temp_path = parent.join(temp_name);

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
