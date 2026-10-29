use std::env;
use std::fs;
use std::path::{Path, PathBuf};

/// Reads a trimmed, non-empty path from the environment variable `var`.
pub fn non_empty_env_path(var: &str) -> Option<PathBuf> {
    env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
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
pub fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    fs::create_dir_all(path)?;
    set_private_dir_perms(path);
    Ok(())
}

/// Writes `contents` to `path` and restricts file permissions to `0600` on Unix.
pub fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    fs::write(path, contents)?;
    set_private_file_perms(path);
    Ok(())
}
