use srz::upgrade::{
    compute_sha256_hex, detect_install_origin, detect_target_from, extract_binary_from_tar_gz,
    extract_binary_from_zip, format_asset_names, parse_clean_version, parse_sha256_checksum,
    ArchiveFormat, InstallOrigin,
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
        detect_install_origin(Path::new("/home/linuxbrew/.linuxbrew/Cellar/srz/0.1.0/bin/srz")),
        InstallOrigin::Homebrew
    );

    // Cargo installation
    assert_eq!(
        detect_install_origin(Path::new("/Users/myoo/.cargo/bin/srz")),
        InstallOrigin::Cargo
    );
    assert_eq!(
        detect_install_origin(Path::new("C:\\Users\\User\\.cargo\\bin\\srz.exe")),
        InstallOrigin::Cargo
    );

    // Direct / curl installation
    assert_eq!(
        detect_install_origin(Path::new("/Users/myoo/.local/bin/srz")),
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
    assert_eq!(v1, semver::Version::new(0, 1, 0));

    let v2 = parse_clean_version("0.2.5").unwrap();
    assert_eq!(v2, semver::Version::new(0, 2, 5));

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
    let (v_tar, l_tar) =
        format_asset_names("v0.2.0", "aarch64-apple-darwin", ArchiveFormat::TarGz);
    assert_eq!(v_tar, "srz-v0.2.0-aarch64-apple-darwin.tar.gz");
    assert_eq!(l_tar, "srz-aarch64-apple-darwin.tar.gz");

    let (v_zip, l_zip) =
        format_asset_names("v0.2.0", "x86_64-pc-windows-msvc", ArchiveFormat::Zip);
    assert_eq!(v_zip, "srz-v0.2.0-x86_64-pc-windows-msvc.zip");
    assert_eq!(l_zip, "srz-x86_64-pc-windows-msvc.zip");
}

#[test]
fn test_extract_binary_from_tar_gz() {
    let binary_content = b"echo 'srz v0.2.0'";

    // Create an in-memory tar.gz containing 'srz'
    let mut gz_encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    {
        let mut tar_builder = tar::Builder::new(&mut gz_encoder);
        let mut header = tar::Header::new_gnu();
        header.set_size(binary_content.len() as u64);
        header.set_mode(0o755);
        header.set_cksum();
        tar_builder
            .append_data(&mut header, "srz", &binary_content[..])
            .expect("Failed to append file to tar");
        tar_builder.finish().expect("Failed to finish tar archive");
    }
    let archive_bytes = gz_encoder.finish().expect("Failed to finish gzip encoding");

    // Extract 'srz'
    let extracted = extract_binary_from_tar_gz(&archive_bytes, "srz")
        .expect("Extraction should succeed");
    assert_eq!(extracted, binary_content);

    // Extraction fails if requesting non-existent binary name
    assert!(extract_binary_from_tar_gz(&archive_bytes, "nonexistent").is_err());
}

#[test]
fn test_extract_binary_from_zip() {
    let binary_content = b"MZ-mock-binary-srz-exe";

    // Create an in-memory zip containing 'srz.exe'
    let mut zip_buf = std::io::Cursor::new(Vec::new());
    {
        let mut zip_writer = zip::ZipWriter::new(&mut zip_buf);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        zip_writer
            .start_file("srz.exe", options)
            .expect("Failed to start zip file");
        zip_writer
            .write_all(binary_content)
            .expect("Failed to write binary to zip");
        zip_writer.finish().expect("Failed to finish zip archive");
    }
    let zip_bytes = zip_buf.into_inner();

    // Extract 'srz.exe'
    let extracted = extract_binary_from_zip(&zip_bytes, "srz.exe")
        .expect("Extraction should succeed");
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
fn test_upgrade_temp_file_creation_prevents_overwrite() {
    let tmp_dir = std::env::temp_dir().join("srz_test_upgrade_overwrite");
    let _ = std::fs::remove_dir_all(&tmp_dir);
    std::fs::create_dir_all(&tmp_dir).unwrap();

    let target_file = tmp_dir.join(".srz-upgrade-existing");
    std::fs::write(&target_file, b"existing-content").unwrap();

    // Verify create_new(true) fails if file already exists (preventing symlink overwrites)
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    assert!(opts.open(&target_file).is_err());

    let _ = std::fs::remove_dir_all(&tmp_dir);
}
