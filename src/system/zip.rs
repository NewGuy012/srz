use anyhow::{Context, Result, bail};
use flate2::Crc;
use flate2::read::DeflateDecoder;
use std::collections::HashSet;
use std::io::Read;

const MAX_ZIP_ENTRIES: usize = 16_384;

#[derive(Debug, Clone)]
pub(crate) struct ZipEntry<'a> {
    pub(crate) name: &'a str,
    compression_method: u16,
    crc32: u32,
    compressed_size: usize,
    uncompressed_size: usize,
    local_header_offset: usize,
    pub(crate) is_regular_file: bool,
}

/// Lightweight zero-copy ZIP reader.
pub(crate) struct ZipReader<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry<'a>>,
}

fn le16(b: &[u8]) -> usize {
    u16::from_le_bytes([b[0], b[1]]) as usize
}

fn le32(b: &[u8]) -> usize {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = Crc::new();
    crc.update(data);
    crc.sum()
}

impl<'a> ZipReader<'a> {
    /// Opens and parses the central directory of a ZIP archive from in-memory bytes.
    pub(crate) fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 22 {
            bail!("ZIP archive too short");
        }

        // Verify EOCD comment length reaches EOF to defeat trailing comment polyglots.
        let min_offset = data.len().saturating_sub(22 + 65535);
        let eocd_offset = (min_offset..=data.len() - 22)
            .rev()
            .find(|&i| {
                data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06]
                    && i + 22 + le16(&data[i + 20..]) == data.len()
            })
            .context("End of Central Directory (EOCD) not found")?;

        let disk_num = le16(&data[eocd_offset + 4..]);
        let cd_disk = le16(&data[eocd_offset + 6..]);
        let disk_entries = le16(&data[eocd_offset + 8..]);
        let total_entries = le16(&data[eocd_offset + 10..]);
        if disk_num != 0 || cd_disk != 0 || disk_entries != total_entries {
            bail!("Multi-disk ZIP archives are not supported");
        }
        let cd_size = le32(&data[eocd_offset + 12..]);
        let cd_offset = le32(&data[eocd_offset + 16..]);
        if total_entries == 0xFFFF || cd_size == 0xFFFF_FFFF || cd_offset == 0xFFFF_FFFF {
            bail!("ZIP64 archives are not supported");
        }

        let cd_end = cd_offset
            .checked_add(cd_size)
            .context("Corrupt ZIP: central directory offset overflow")?;
        if cd_end > eocd_offset {
            bail!("Corrupt ZIP: central directory out of bounds");
        }

        let mut pos = cd_offset;
        let mut entries = Vec::with_capacity(total_entries.min(1024));
        let mut seen_regular = HashSet::with_capacity(total_entries.min(1024));

        while pos.checked_add(46).is_some_and(|end| end <= cd_end)
            && data[pos..pos + 4] == [0x50, 0x4b, 0x01, 0x02]
        {
            if entries.len() >= MAX_ZIP_ENTRIES {
                bail!("ZIP archive exceeds entry limit of {}", MAX_ZIP_ENTRIES);
            }
            let creator_os = data[pos + 5];
            let flags = le16(&data[pos + 8..]);
            if (flags & 0x0001) != 0 {
                bail!("Encrypted ZIP entries are not supported");
            }
            let compression_method = le16(&data[pos + 10..]) as u16;
            let entry_crc32 = le32(&data[pos + 16..]) as u32;
            let compressed_size = le32(&data[pos + 20..]);
            let uncompressed_size = le32(&data[pos + 24..]);
            let name_len = le16(&data[pos + 28..]);
            let extra_len = le16(&data[pos + 30..]);
            let comment_len = le16(&data[pos + 32..]);
            let ext_attr = le32(&data[pos + 38..]) as u32;
            let local_header_offset = le32(&data[pos + 42..]);
            if compressed_size == 0xFFFF_FFFF
                || uncompressed_size == 0xFFFF_FFFF
                || local_header_offset == 0xFFFF_FFFF
            {
                bail!("ZIP64 entries are not supported");
            }

            let name_start = pos + 46;
            let name_end = name_start
                .checked_add(name_len)
                .context("Corrupt ZIP: filename offset overflow")?;
            if name_end > cd_end {
                bail!("Corrupt ZIP: filename out of bounds");
            }
            let name = std::str::from_utf8(&data[name_start..name_end])
                .context("Corrupt ZIP: non-UTF-8 entry name")?;

            let next_pos = name_end
                .checked_add(extra_len)
                .and_then(|v| v.checked_add(comment_len))
                .context("Corrupt ZIP: central directory entry offset overflow")?;
            if next_pos > cd_end {
                bail!("Corrupt ZIP: central directory entry out of bounds");
            }

            let is_dir = name.ends_with(['/', '\\'])
                || (matches!(creator_os, 0 | 10 | 14) && (ext_attr & 0x10) != 0);
            let unix_type = matches!(creator_os, 3 | 19)
                .then_some((ext_attr >> 16) & 0o170000)
                .filter(|&m| m != 0);
            let is_regular_file = !is_dir && unix_type.is_none_or(|m| m == 0o100000);
            if is_regular_file && !seen_regular.insert(name) {
                bail!("Corrupt ZIP: duplicate entry '{}'", name);
            }

            entries.push(ZipEntry {
                name,
                compression_method,
                crc32: entry_crc32,
                compressed_size,
                uncompressed_size,
                local_header_offset,
                is_regular_file,
            });

            pos = next_pos;
        }

        Ok(Self { data, entries })
    }

    pub(crate) fn entries(&self) -> &[ZipEntry<'a>] {
        &self.entries
    }

    pub(crate) fn file_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name)
    }

    /// Reads and decompresses a file by its exact name, subject to `max_bytes` ceiling.
    pub(crate) fn read_file(&self, name: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.name == name)
            .with_context(|| format!("File '{}' not found in ZIP archive", name))?;
        self.decompress_entry(entry, max_bytes)
    }

    /// Decompresses a regular-file entry with size bounds and CRC-32 verification.
    pub(crate) fn decompress_entry(
        &self,
        entry: &ZipEntry<'a>,
        max_bytes: usize,
    ) -> Result<Vec<u8>> {
        if !entry.is_regular_file {
            bail!("Refusing to extract non-regular ZIP entry '{}'", entry.name);
        }
        let lh_offset = entry.local_header_offset;
        let lh_fixed_end = lh_offset
            .checked_add(30)
            .context("Corrupt ZIP: local file header offset overflow")?;
        if lh_fixed_end > self.data.len() {
            bail!("Corrupt ZIP: local file header out of bounds");
        }
        if self.data[lh_offset..lh_offset + 4] != [0x50, 0x4b, 0x03, 0x04] {
            bail!("Corrupt ZIP: invalid local file header signature");
        }
        let fn_len = le16(&self.data[lh_offset + 26..]);
        let extra_len = le16(&self.data[lh_offset + 28..]);
        let data_start = lh_fixed_end
            .checked_add(fn_len)
            .and_then(|v| v.checked_add(extra_len))
            .context("Corrupt ZIP: local file data offset overflow")?;
        let data_end = data_start
            .checked_add(entry.compressed_size)
            .context("Corrupt ZIP: file payload offset overflow")?;
        if data_end > self.data.len() {
            bail!("Corrupt ZIP: file payload out of bounds");
        }
        let compressed = &self.data[data_start..data_end];

        let out = match entry.compression_method {
            0 => {
                if compressed.len() > max_bytes || entry.uncompressed_size > max_bytes {
                    bail!("Decompressed entry exceeds limit of {} bytes", max_bytes);
                }
                if compressed.len() != entry.uncompressed_size {
                    bail!(
                        "Corrupt ZIP: stored entry size mismatch (compressed={}, uncompressed={})",
                        compressed.len(),
                        entry.uncompressed_size
                    );
                }
                compressed.to_vec()
            }
            8 => {
                let limit = (max_bytes as u64).saturating_add(1);
                let mut decoder = DeflateDecoder::new(compressed).take(limit);
                let initial_cap = entry.uncompressed_size.min(max_bytes).min(64 * 1024);
                let mut out = Vec::with_capacity(initial_cap);
                decoder.read_to_end(&mut out)?;
                if out.len() > max_bytes {
                    bail!("Decompressed entry exceeds limit of {} bytes", max_bytes);
                }
                out
            }
            m => bail!("Unsupported ZIP compression method: {}", m),
        };

        let actual_crc = crc32(&out);
        if actual_crc != entry.crc32 {
            bail!(
                "Corrupt ZIP: CRC-32 mismatch for '{}' (expected {:08x}, got {:08x})",
                entry.name,
                entry.crc32,
                actual_crc
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
struct TestZipEntry<'a> {
    name: &'a str,
    method: u16,
    payload: &'a [u8],
    raw_uncompressed: &'a [u8],
    spoofed_uncomp_len: usize,
}

#[cfg(test)]
pub(crate) fn create_test_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let entries: Vec<_> = files
        .iter()
        .map(|&(name, content)| TestZipEntry {
            name,
            method: 0,
            payload: content,
            raw_uncompressed: content,
            spoofed_uncomp_len: content.len(),
        })
        .collect();
    build_test_zip(&entries)
}

#[cfg(test)]
fn build_test_zip(files: &[TestZipEntry<'_>]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();

    for entry in files {
        let lh_offset = out.len() as u32;
        let name_bytes = entry.name.as_bytes();
        let comp_len = entry.payload.len() as u32;
        let uncomp_len = entry.spoofed_uncomp_len as u32;
        let sum = crc32(entry.raw_uncompressed).to_le_bytes();

        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0]);
        out.extend_from_slice(&entry.method.to_le_bytes());
        out.extend_from_slice(&[0, 0, 0, 0]);
        out.extend_from_slice(&sum);
        out.extend_from_slice(&comp_len.to_le_bytes());
        out.extend_from_slice(&uncomp_len.to_le_bytes());
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(entry.payload);

        cd.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02, 20, 0, 20, 0, 0, 0]);
        cd.extend_from_slice(&entry.method.to_le_bytes());
        cd.extend_from_slice(&[0, 0, 0, 0]);
        cd.extend_from_slice(&sum);
        cd.extend_from_slice(&comp_len.to_le_bytes());
        cd.extend_from_slice(&uncomp_len.to_le_bytes());
        cd.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0; 12]);
        cd.extend_from_slice(&lh_offset.to_le_bytes());
        cd.extend_from_slice(name_bytes);
    }

    let cd_offset = out.len() as u32;
    let cd_size = cd.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(&cd);
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0]);
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&count.to_le_bytes());
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use std::io::Write;

    fn first_cd_offset(zip_bytes: &[u8]) -> usize {
        let eocd_pos = zip_bytes
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .unwrap();
        u32::from_le_bytes(zip_bytes[eocd_pos + 16..eocd_pos + 20].try_into().unwrap()) as usize
    }

    #[test]
    fn test_zip_stored_spoofed_uncompressed_size_and_crc_rejected() {
        let payload = vec![b'A'; 256];
        let mut zip_bytes = create_test_zip(&[("payload.txt", &payload)]);
        let cd_offset = first_cd_offset(&zip_bytes);
        zip_bytes[cd_offset + 24..cd_offset + 28].copy_from_slice(&0u32.to_le_bytes());

        let reader = ZipReader::new(&zip_bytes).unwrap();
        assert!(
            reader
                .read_file("payload.txt", 128)
                .unwrap_err()
                .to_string()
                .contains("exceeds limit")
        );
        assert!(
            reader
                .read_file("payload.txt", 1024)
                .unwrap_err()
                .to_string()
                .contains("mismatch")
        );

        let mut bad_crc = create_test_zip(&[("payload.txt", b"hello")]);
        let crc_cd = first_cd_offset(&bad_crc);
        bad_crc[crc_cd + 16..crc_cd + 20].copy_from_slice(&0xDEADBEEFu32.to_le_bytes());
        assert!(
            ZipReader::new(&bad_crc)
                .unwrap()
                .read_file("payload.txt", 1024)
                .unwrap_err()
                .to_string()
                .contains("CRC-32 mismatch")
        );
    }

    #[test]
    fn test_zip_deflate_bomb_bounded() {
        let big_payload = vec![b'Z'; 4096];
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&big_payload).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 128);

        let out = build_test_zip(&[TestZipEntry {
            name: "bomb.txt",
            method: 8,
            payload: &compressed,
            raw_uncompressed: &big_payload,
            spoofed_uncomp_len: 100_000_000,
        }]);
        let reader = ZipReader::new(&out).unwrap();
        assert!(
            reader
                .read_file("bomb.txt", 1024)
                .unwrap_err()
                .to_string()
                .contains("exceeds limit")
        );
        assert_eq!(reader.read_file("bomb.txt", 8192).unwrap().len(), 4096);
    }

    #[test]
    fn test_zip_corrupt_offsets_and_security_rejections() {
        let mut zip_bytes = create_test_zip(&[("a.txt", b"hello")]);
        let cd_offset = first_cd_offset(&zip_bytes);
        zip_bytes[cd_offset + 42..cd_offset + 46].copy_from_slice(&(u32::MAX - 5).to_le_bytes());
        assert!(
            ZipReader::new(&zip_bytes)
                .unwrap()
                .read_file("a.txt", 1024)
                .is_err()
        );

        let mut zip_bytes2 = create_test_zip(&[("a.txt", b"hello")]);
        zip_bytes2[cd_offset + 20..cd_offset + 24].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(ZipReader::new(&zip_bytes2).is_err());

        // Unix symlink external_attributes (creator OS 3, mode 0o120777) must be rejected.
        let mut sym_zip = create_test_zip(&[("srz.exe", b"/etc/passwd")]);
        let sym_cd = first_cd_offset(&sym_zip);
        sym_zip[sym_cd + 5] = 3;
        sym_zip[sym_cd + 38..sym_cd + 42].copy_from_slice(&((0o120777u32) << 16).to_le_bytes());
        let sym_reader = ZipReader::new(&sym_zip).unwrap();
        assert!(!sym_reader.entries()[0].is_regular_file);
        assert!(sym_reader.read_file("srz.exe", 1024).is_err());

        // Encrypted flag and duplicate regular entries must be rejected.
        let mut enc_zip = create_test_zip(&[("a.txt", b"hello")]);
        enc_zip[cd_offset + 8] |= 1;
        assert!(ZipReader::new(&enc_zip).is_err());

        let dup_zip = create_test_zip(&[("a.txt", b"1"), ("a.txt", b"2")]);
        assert!(ZipReader::new(&dup_zip).is_err());
    }
}
