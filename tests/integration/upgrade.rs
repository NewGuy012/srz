use srz::upgrade::{
    ArchiveFormat, InstallOrigin, Version, compute_sha256_hex, detect_install_origin,
    detect_target_from, extract_binary_from_tar_gz, extract_binary_from_zip, format_asset_names,
    parse_clean_version, parse_sha256_checksum,
};
use std::io::Write;
use std::path::Path;

#[test]
fn test_detect_target_mappings() {
    // macOS Apple Silicon
    let mac_arm = detect_target_from("macos", "aarch64").expect("Valid target");
    assert_eq!(mac_arm.target, "aarch64-apple-darwin");
    assert_eq!(mac_arm.format, ArchiveFormat::TarGz);
    assert_eq!(mac_arm.binary_name, "srz");

    // macOS Intel (unsupported prebuilt binaries)
    let mac_intel_err = detect_target_from("macos", "x86_64").unwrap_err();
    assert!(mac_intel_err.contains("Intel Macs (x86_64) are not supported"));

    // Linux x86_64
    let linux = detect_target_from("linux", "x86_64").expect("Valid target");
    assert_eq!(linux.target, "x86_64-unknown-linux-gnu");
    assert_eq!(linux.format, ArchiveFormat::TarGz);
    assert_eq!(linux.binary_name, "srz");

    // Linux aarch64
    let linux_arm = detect_target_from("linux", "aarch64").expect("Valid target");
    assert_eq!(linux_arm.target, "aarch64-unknown-linux-gnu");
    assert_eq!(linux_arm.format, ArchiveFormat::TarGz);
    assert_eq!(linux_arm.binary_name, "srz");

    // Windows x86_64
    let win = detect_target_from("windows", "x86_64").expect("Valid target");
    assert_eq!(win.target, "x86_64-pc-windows-msvc");
    assert_eq!(win.format, ArchiveFormat::Zip);
    assert_eq!(win.binary_name, "srz.exe");

    // Unsupported
    assert!(detect_target_from("solaris", "sparc").is_err());
    assert!(detect_target_from("linux", "arm").is_err());
}

#[test]
fn test_detect_install_origin() {
    // Homebrew on Apple Silicon
    assert_eq!(
        detect_install_origin(Path::new("/opt/homebrew/bin/srz")),
        InstallOrigin::Homebrew
    );
    assert_eq!(
        detect_install_origin(Path::new("/opt/homebrew/Cellar/srz/0.1.0/bin/srz")),
        InstallOrigin::Homebrew
    );

    // Homebrew on Intel macOS
    assert_eq!(
        detect_install_origin(Path::new("/usr/local/Cellar/srz/0.1.0/bin/srz")),
        InstallOrigin::Homebrew
    );

    // Linuxbrew
    assert_eq!(
        detect_install_origin(Path::new("/home/linuxbrew/.linuxbrew/bin/srz")),
        InstallOrigin::Homebrew
    );
    assert_eq!(
        detect_install_origin(Path::new(
            "/home/linuxbrew/.linuxbrew/Cellar/srz/0.1.0/bin/srz"
        )),
        InstallOrigin::Homebrew
    );

    // Cargo installation
    assert_eq!(
        detect_install_origin(Path::new("/home/user/.cargo/bin/srz")),
        InstallOrigin::Cargo
    );
    assert_eq!(
        detect_install_origin(Path::new("C:\\Users\\User\\.cargo\\bin\\srz.exe")),
        InstallOrigin::Cargo
    );

    // Direct / curl installation
    assert_eq!(
        detect_install_origin(Path::new("/home/user/.local/bin/srz")),
        InstallOrigin::Direct
    );
    assert_eq!(
        detect_install_origin(Path::new("/usr/local/bin/srz")),
        InstallOrigin::Direct
    );
}

#[test]
fn test_parse_clean_version() {
    let v1 = parse_clean_version("v0.1.0").unwrap();
    assert_eq!(v1, Version::new(0, 1, 0));

    let v2 = parse_clean_version("0.2.5").unwrap();
    assert_eq!(v2, Version::new(0, 2, 5));

    let v3 = parse_clean_version("  v1.3.4-alpha.1 \n").unwrap();
    assert_eq!(v3.major, 1);
    assert_eq!(v3.minor, 3);
    assert_eq!(v3.patch, 4);

    assert!(parse_clean_version("invalid-version").is_err());
}

#[test]
fn test_parse_sha256_checksum() {
    let test_hash = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    // Standard output from sha256sum
    let standard = format!("{}  srz-v0.1.0.tar.gz\n", test_hash);
    assert_eq!(parse_sha256_checksum(&standard), Some(test_hash));

    // Binary mode output
    let binary_mode = format!("{} *srz-v0.1.0.tar.gz", test_hash);
    assert_eq!(parse_sha256_checksum(&binary_mode), Some(test_hash));

    // Raw hash only
    assert_eq!(parse_sha256_checksum(test_hash), Some(test_hash));

    // Empty content
    assert_eq!(parse_sha256_checksum("   \n\n  "), None);
}

#[test]
fn test_format_asset_names() {
    let (v_tar, l_tar) = format_asset_names("v0.2.0", "aarch64-apple-darwin", ArchiveFormat::TarGz);
    assert_eq!(v_tar, "srz-v0.2.0-aarch64-apple-darwin.tar.gz");
    assert_eq!(l_tar, "srz-aarch64-apple-darwin.tar.gz");

    let (v_zip, l_zip) = format_asset_names("v0.2.0", "x86_64-pc-windows-msvc", ArchiveFormat::Zip);
    assert_eq!(v_zip, "srz-v0.2.0-x86_64-pc-windows-msvc.zip");
    assert_eq!(l_zip, "srz-x86_64-pc-windows-msvc.zip");
}

#[test]
fn test_extract_binary_from_tar_gz() {
    let binary_content = b"echo 'srz v0.2.0'";

    // Create an in-memory tar.gz containing 'srz' using standard USTAR format
    let mut header = [0u8; 512];
    header[..3].copy_from_slice(b"srz");
    header[100..108].copy_from_slice(b"0000755\0");
    let size_octal = format!("{:011o} ", binary_content.len());
    header[124..136].copy_from_slice(size_octal.as_bytes());
    header[156] = b'0';
    header[257..263].copy_from_slice(b"ustar\0");
    header[148..156].copy_from_slice(b"        ");
    let cksum: u32 = header.iter().map(|&b| b as u32).sum();
    let cksum_octal = format!("{:06o}\0 ", cksum);
    header[148..156].copy_from_slice(cksum_octal.as_bytes());

    let mut gz_encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz_encoder.write_all(&header).unwrap();
    gz_encoder.write_all(binary_content).unwrap();
    let pad = (512 - (binary_content.len() % 512)) % 512;
    if pad > 0 {
        gz_encoder.write_all(&vec![0u8; pad]).unwrap();
    }
    gz_encoder.write_all(&[0u8; 1024]).unwrap();
    let archive_bytes = gz_encoder.finish().expect("Failed to finish gzip encoding");

    // Extract 'srz'
    let extracted =
        extract_binary_from_tar_gz(&archive_bytes, "srz").expect("Extraction should succeed");
    assert_eq!(extracted, binary_content);

    // Extraction fails if requesting non-existent binary name
    assert!(extract_binary_from_tar_gz(&archive_bytes, "nonexistent").is_err());
}

#[test]
fn test_extract_binary_from_zip() {
    let binary_content = b"MZ-mock-binary-srz-exe";
    let zip_bytes = srz::system::zip::create_test_zip(&[("srz.exe", binary_content)]);

    // Extract 'srz.exe'
    let extracted =
        extract_binary_from_zip(&zip_bytes, "srz.exe").expect("Extraction should succeed");
    assert_eq!(extracted, binary_content);

    // Extraction fails if requesting non-existent binary name
    assert!(extract_binary_from_zip(&zip_bytes, "srz-other.exe").is_err());
}

#[test]
fn test_sha256_verification_computation() {
    let test_data = b"Arbitrary release archive test data for SHA256 hashing.";
    let computed_hash = compute_sha256_hex(test_data);

    let sha256_file = format!("{}  srz.tar.gz", computed_hash);
    let parsed_hash = parse_sha256_checksum(&sha256_file).expect("Parsed hash");

    assert_eq!(parsed_hash, computed_hash);
    assert!(parsed_hash.eq_ignore_ascii_case(&computed_hash.to_uppercase()));
}

#[test]
fn test_parse_clean_version_canonicalization_and_rejection() {
    assert_eq!(
        parse_clean_version("v0.7.1").unwrap(),
        Version::new(0, 7, 1)
    );
    assert_eq!(
        parse_clean_version(" 0.7.1 ").unwrap(),
        Version::new(0, 7, 1)
    );
    // Malicious or invalid tags must fail to parse as semver
    assert!(parse_clean_version("../../../evil").is_err());
    assert!(parse_clean_version("latest").is_err());
    assert!(parse_clean_version("v0.7.1/payload").is_err());
    assert!(parse_clean_version("; rm -rf /").is_err());
}

#[test]
fn test_upgrade_temp_file_creation_prevents_overwrite() {
    let tmp = srz::fs_util::tempdir().unwrap();
    let target_file = tmp.path().join(".srz-upgrade-existing");
    std::fs::write(&target_file, b"existing-content").unwrap();

    // Verify create_new(true) fails if file already exists (preventing symlink overwrites)
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    assert!(opts.open(&target_file).is_err());
}

#[cfg(unix)]
#[test]
fn test_replace_binary_refuses_symlink() {
    let tmp = srz::fs_util::tempdir().unwrap();
    let new_binary = tmp.path().join("new_bin");
    let real_target = tmp.path().join("real_target");
    let symlink_target = tmp.path().join("symlink_target");

    std::fs::write(&new_binary, b"new binary content").unwrap();
    std::fs::write(&real_target, b"old target content").unwrap();
    std::os::unix::fs::symlink(&real_target, &symlink_target).unwrap();

    let res = srz::upgrade::replace_binary(&new_binary, &symlink_target);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("symlink"));
}
