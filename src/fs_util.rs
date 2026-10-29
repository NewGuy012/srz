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

/// Atomically opens or creates a file with owner-only permissions (`0600` on Unix).
pub fn open_private_file_for_write(path: &Path) -> std::io::Result<File> {
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let file = opts.open(path)?;
    set_private_file_perms(path);
    Ok(file)
}

/// Writes `contents` to `path` with atomic owner-only permissions (`0600` on Unix).
pub fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut file = open_private_file_for_write(path)?;
    file.write_all(contents.as_ref())?;
    file.flush()?;
    Ok(())
}
