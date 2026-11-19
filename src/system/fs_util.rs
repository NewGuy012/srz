use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

fn non_empty_env_path(var: &str) -> Option<PathBuf> {
    env::var(var)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|s| !s.is_empty())
        .map(PathBuf::from)
}

fn home_dir() -> Option<PathBuf> {
    non_empty_env_path("HOME").or_else(|| non_empty_env_path("USERPROFILE"))
}

#[cfg(target_os = "windows")]
pub(crate) fn system_root_bin(rel: &str) -> PathBuf {
    let base = env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    base.join("System32").join(rel)
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

/// Returns the default persistent data directory (`$SRZ_DATA_DIR`, `$XDG_DATA_HOME/srz`, `~/.local/share/srz`, or `%LOCALAPPDATA%/srz/data`).
pub(crate) fn default_data_dir() -> PathBuf {
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
pub(crate) fn default_cache_dir() -> PathBuf {
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
pub(crate) fn default_config_path() -> PathBuf {
    resolve_app_path(
        &["SRZ_CONFIG", "SRZ_CONFIG_PATH"],
        "XDG_CONFIG_HOME",
        &["srz", "config.toml"],
        ("APPDATA", &["srz", "config.toml"]),
        &[".config", "srz", "config.toml"],
        &["srz", "config.toml"],
    )
}

pub(crate) fn create_temp_file_path(dir: &Path, prefix: &str, suffix: &str) -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let count = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    dir.join(format!(
        "{}-{}-{:x}-{}{}",
        prefix,
        std::process::id(),
        nonce,
        count,
        suffix
    ))
}

pub(crate) fn set_unix_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
        Ok(())
    }
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

/// Creates a directory (and parents) with owner-only `0700` permissions on Unix, rejecting symlinks.
pub(crate) fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    if path.as_os_str().is_empty() {
        return Ok(());
    }
    reject_symlink(path, "create directory at symlink path")?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(path)?;
    }
    #[cfg(not(unix))]
    fs::create_dir_all(path)?;
    set_unix_mode(path, 0o700)
}

pub(crate) fn write_staged_temp_file(
    dir: &Path,
    prefix: &str,
    suffix: &str,
    contents: &[u8],
    unix_mode: u32,
) -> std::io::Result<PathBuf> {
    let mut open_opts = fs::OpenOptions::new();
    open_opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        open_opts.mode(unix_mode);
    }
    #[cfg(not(unix))]
    let _ = unix_mode;

    let mut attempt = 0;
    let (temp_path, mut temp_file) = loop {
        let candidate = create_temp_file_path(dir, prefix, suffix);
        match open_opts.open(&candidate) {
            Ok(file) => break (candidate, file),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists && attempt < 7 => {
                attempt += 1;
            }
            Err(err) => return Err(err),
        }
    };

    let write_res = temp_file
        .write_all(contents)
        .and_then(|_| temp_file.flush())
        .and_then(|_| temp_file.sync_all());
    #[cfg(unix)]
    let write_res = write_res.and_then(|_| {
        use std::os::unix::fs::PermissionsExt;
        temp_file.set_permissions(fs::Permissions::from_mode(unix_mode))
    });

    if let Err(err) = write_res {
        drop(temp_file);
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    drop(temp_file);
    Ok(temp_path)
}

/// Writes `contents` atomically with `0600` permissions on Unix, rejecting symlinks.
pub(crate) fn write_private_file(path: &Path, contents: impl AsRef<[u8]>) -> std::io::Result<()> {
    reject_symlink(path, "write to symlink")?;

    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    if !parent.as_os_str().is_empty() && !parent.exists() {
        create_private_dir_all(parent)?;
    }

    let temp_path = write_staged_temp_file(parent, ".srz-tmp", ".tmp", contents.as_ref(), 0o600)?;
    if let Err(err) = fs::rename(&temp_path, path) {
        let _ = fs::remove_file(&temp_path);
        return Err(err);
    }
    Ok(())
}

/// Reads at most `max_bytes` from a regular file, validating type and size before and after open.
pub(crate) fn read_file_bytes_limited(
    path: &Path,
    max_bytes: u64,
    reject_symlinks: bool,
    label: &str,
) -> std::io::Result<Vec<u8>> {
    use std::io::{Error, ErrorKind, Read};

    if reject_symlinks {
        reject_symlink(path, &format!("read {label} from symlink"))?;
    }
    if fs::metadata(path).is_ok_and(|m| !m.is_file()) {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("{label} path is not a regular file: {path:?}"),
        ));
    }

    let file = fs::File::open(path).map_err(|err| {
        Error::new(
            err.kind(),
            format!("Failed to open {label} file {path:?}: {err}"),
        )
    })?;

    let meta = file.metadata()?;
    if !meta.is_file() {
        return Err(Error::new(
            ErrorKind::InvalidInput,
            format!("{label} path is not a regular file: {path:?}"),
        ));
    }
    if meta.len() > max_bytes {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("{label} file is too large (exceeds {max_bytes} bytes): {path:?}"),
        ));
    }

    let mut bytes = Vec::new();
    file.take(max_bytes.saturating_add(1))
        .read_to_end(&mut bytes)?;
    if bytes.len() as u64 > max_bytes {
        return Err(Error::new(
            ErrorKind::InvalidData,
            format!("{label} file is too large (exceeds {max_bytes} bytes): {path:?}"),
        ));
    }
    Ok(bytes)
}

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct TempDir {
    path: PathBuf,
}

#[cfg(test)]
impl TempDir {
    pub(crate) fn new() -> std::io::Result<Self> {
        let path = create_temp_file_path(&std::env::temp_dir(), "srz-test", "");
        create_private_dir_all(&path)?;
        Ok(Self { path })
    }

    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

#[cfg(test)]
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
pub(crate) fn tempdir() -> std::io::Result<TempDir> {
    TempDir::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_write_private_file() {
        create_private_dir_all(Path::new("")).unwrap();
        let tmp = tempdir().unwrap();
        let dir = tmp.path().join("subdir");
        create_private_dir_all(&dir).unwrap();
        assert!(dir.is_dir());

        let file_path = dir.join("test_write.txt");
        write_private_file(&file_path, b"test private content").unwrap();
        assert_eq!(fs::read(&file_path).unwrap(), b"test private content");

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&file_path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
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

        // Atomic write to symlink must be rejected
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

    #[test]
    fn test_read_file_bytes_limited_enforces_size_and_type() {
        let tmp = tempdir().unwrap();
        let file = tmp.path().join("sample.bin");
        fs::write(&file, b"0123456789").unwrap();

        assert_eq!(
            read_file_bytes_limited(&file, 10, false, "Test").unwrap(),
            b"0123456789"
        );
        assert!(
            read_file_bytes_limited(&file, 9, false, "Test")
                .unwrap_err()
                .to_string()
                .contains("too large")
        );
        assert!(
            read_file_bytes_limited(tmp.path(), 1024, false, "Test")
                .unwrap_err()
                .to_string()
                .contains("not a regular file")
        );
        assert!(
            read_file_bytes_limited(&tmp.path().join("missing.bin"), 1024, false, "Test")
                .unwrap_err()
                .to_string()
                .contains("Failed to open")
        );

        #[cfg(unix)]
        {
            let link = tmp.path().join("link.bin");
            std::os::unix::fs::symlink(&file, &link).unwrap();
            assert_eq!(
                read_file_bytes_limited(&link, 10, false, "Test").unwrap(),
                b"0123456789"
            );
            assert!(
                read_file_bytes_limited(&link, 10, true, "Test")
                    .unwrap_err()
                    .to_string()
                    .contains("symlink")
            );
        }
    }
}
