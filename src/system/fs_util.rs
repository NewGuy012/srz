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

/// Writes `contents` to `path` with atomic owner-only permissions (`0600` on Unix).
pub fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    let mut file = open_private_file_for_write(path)?;
    file.write_all(contents.as_ref())?;
    file.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn test_write_private_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = tempfile::tempdir().unwrap();
        let file_path = tmp.path().join("test.txt");

        write_private_file(&file_path, b"test private content").unwrap();

        let meta = fs::metadata(&file_path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn test_fs_util_rejects_symlinks() {
        use std::os::unix::fs::symlink;

        let tmp = tempfile::tempdir().unwrap();
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
