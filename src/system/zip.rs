use anyhow::{Context, Result, bail};
use flate2::read::DeflateDecoder;
use std::io::Read;

/// Representation of an entry in a ZIP central directory.
#[derive(Debug, Clone)]
pub struct ZipEntry<'a> {
    pub name: &'a str,
    pub compression_method: u16,
    pub compressed_size: usize,
    pub uncompressed_size: usize,
    pub local_header_offset: usize,
}

/// Lightweight zero-copy ZIP reader.
pub struct ZipReader<'a> {
    data: &'a [u8],
    entries: Vec<ZipEntry<'a>>,
}

fn le16(b: &[u8]) -> usize {
    u16::from_le_bytes([b[0], b[1]]) as usize
}

fn le32(b: &[u8]) -> usize {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize
}

impl<'a> ZipReader<'a> {
    /// Opens and parses the central directory of a ZIP archive from in-memory bytes.
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 22 {
            bail!("ZIP archive too short");
        }

        // Search for End of Central Directory (EOCD) signature 0x06054b50 from the end
        let min_offset = data.len().saturating_sub(22 + 65535);
        let eocd_offset = (min_offset..=data.len() - 22)
            .rev()
            .find(|&i| data[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
            .context("End of Central Directory (EOCD) not found")?;
        let total_entries = le16(&data[eocd_offset + 10..]);
        let cd_size = le32(&data[eocd_offset + 12..]);
        let cd_offset = le32(&data[eocd_offset + 16..]);

        let cd_end = cd_offset
            .checked_add(cd_size)
            .context("Corrupt ZIP: central directory offset overflow")?;
        if cd_end > data.len() {
            bail!("Corrupt ZIP: central directory out of bounds");
        }

        let mut pos = cd_offset;
        let mut entries = Vec::with_capacity(total_entries.min(1024));

        while pos.checked_add(46).is_some_and(|end| end <= cd_end)
            && data[pos..pos + 4] == [0x50, 0x4b, 0x01, 0x02]
        {
            let compression_method = le16(&data[pos + 10..]) as u16;
            let compressed_size = le32(&data[pos + 20..]);
            let uncompressed_size = le32(&data[pos + 24..]);
            let name_len = le16(&data[pos + 28..]);
            let extra_len = le16(&data[pos + 30..]);
            let comment_len = le16(&data[pos + 32..]);
            let local_header_offset = le32(&data[pos + 42..]);

            let name_start = pos + 46;
            let name_end = name_start
                .checked_add(name_len)
                .context("Corrupt ZIP: filename offset overflow")?;
            if name_end > cd_end {
                bail!("Corrupt ZIP: filename out of bounds");
            }
            let name = std::str::from_utf8(&data[name_start..name_end]).unwrap_or("");

            let next_pos = name_end
                .checked_add(extra_len)
                .and_then(|v| v.checked_add(comment_len))
                .context("Corrupt ZIP: central directory entry offset overflow")?;
            if next_pos > cd_end {
                bail!("Corrupt ZIP: central directory entry out of bounds");
            }

            entries.push(ZipEntry {
                name,
                compression_method,
                compressed_size,
                uncompressed_size,
                local_header_offset,
            });

            pos = next_pos;
        }

        Ok(Self { data, entries })
    }

    /// Returns the parsed central directory entries.
    pub fn entries(&self) -> &[ZipEntry<'a>] {
        &self.entries
    }

    /// Iterates over all file names in the ZIP archive.
    pub fn file_names(&self) -> impl Iterator<Item = &str> {
        self.entries.iter().map(|e| e.name)
    }

    /// Reads and decompresses a file by its exact name, subject to `max_bytes` ceiling.
    pub fn read_file(&self, name: &str, max_bytes: usize) -> Result<Vec<u8>> {
        let entry = self
            .entries
            .iter()
            .find(|e| e.name == name)
            .with_context(|| format!("File '{}' not found in ZIP archive", name))?;
        self.decompress_entry(entry, max_bytes)
    }

    /// Decompresses an entry with strict bounding to prevent zip bombs.
    pub fn decompress_entry(&self, entry: &ZipEntry<'a>, max_bytes: usize) -> Result<Vec<u8>> {
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

        let limit = (max_bytes as u64).saturating_add(1);
        match entry.compression_method {
            0 => {
                // Stored (uncompressed)
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
                Ok(compressed.to_vec())
            }
            8 => {
                // Deflated: cap initial pre-allocation to avoid header-spoofed heap spikes
                let mut decoder = DeflateDecoder::new(compressed).take(limit);
                let initial_cap = entry.uncompressed_size.min(max_bytes).min(64 * 1024);
                let mut out = Vec::with_capacity(initial_cap);
                decoder.read_to_end(&mut out)?;
                if out.len() > max_bytes {
                    bail!("Decompressed entry exceeds limit of {} bytes", max_bytes);
                }
                Ok(out)
            }
            m => bail!("Unsupported ZIP compression method: {}", m),
        }
    }
}

/// Helper function to create valid ZIP archives in tests without external dependencies.
#[cfg(test)]
pub(crate) fn create_test_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cd = Vec::new();

    for (name, content) in files {
        let lh_offset = out.len() as u32;
        let name_bytes = name.as_bytes();
        let content_len = content.len() as u32;

        // Local file header: PK\x03\x04
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        out.extend_from_slice(&[20, 0]); // version needed (2.0)
        out.extend_from_slice(&[0, 0]); // flags
        out.extend_from_slice(&[0, 0]); // compression = 0 (stored)
        out.extend_from_slice(&[0, 0, 0, 0]); // mtime/mdate
        out.extend_from_slice(&[0, 0, 0, 0]); // crc-32
        out.extend_from_slice(&content_len.to_le_bytes()); // compressed size
        out.extend_from_slice(&content_len.to_le_bytes()); // uncompressed size
        out.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]); // extra field length
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(content);

        // Central directory file header: PK\x01\x02
        cd.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        cd.extend_from_slice(&[20, 0]); // version made by
        cd.extend_from_slice(&[20, 0]); // version needed
        cd.extend_from_slice(&[0, 0]); // flags
        cd.extend_from_slice(&[0, 0]); // compression = 0
        cd.extend_from_slice(&[0, 0, 0, 0]); // mtime/mdate
        cd.extend_from_slice(&[0, 0, 0, 0]); // crc-32
        cd.extend_from_slice(&content_len.to_le_bytes());
        cd.extend_from_slice(&content_len.to_le_bytes());
        cd.extend_from_slice(&(name_bytes.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0, 0]); // extra field length
        cd.extend_from_slice(&[0, 0]); // comment length
        cd.extend_from_slice(&[0, 0]); // disk number start
        cd.extend_from_slice(&[0, 0]); // internal attributes
        cd.extend_from_slice(&[0, 0, 0, 0]); // external attributes
        cd.extend_from_slice(&lh_offset.to_le_bytes()); // local header offset
        cd.extend_from_slice(name_bytes);
    }

    let cd_offset = out.len() as u32;
    let cd_size = cd.len() as u32;
    let count = files.len() as u16;
    out.extend_from_slice(&cd);

    // End of central directory record: PK\x05\x06
    out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    out.extend_from_slice(&[0, 0]); // disk number
    out.extend_from_slice(&[0, 0]); // disk with start of cd
    out.extend_from_slice(&count.to_le_bytes()); // entries on this disk
    out.extend_from_slice(&count.to_le_bytes()); // total entries
    out.extend_from_slice(&cd_size.to_le_bytes()); // cd size
    out.extend_from_slice(&cd_offset.to_le_bytes()); // cd offset
    out.extend_from_slice(&[0, 0]); // comment length

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::Compression;
    use flate2::write::DeflateEncoder;
    use std::io::Write;

    #[test]
    fn test_zip_stored_spoofed_uncompressed_size_rejected() {
        // Construct a valid stored ZIP with a 256-byte payload, then overwrite
        // Central Directory uncompressed_size (offset cd_offset + 24..28) to 0.
        let payload = vec![b'A'; 256];
        let mut zip_bytes = create_test_zip(&[("payload.txt", &payload)]);

        // Locate central directory offset from EOCD
        let eocd_pos = zip_bytes
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .unwrap();
        let cd_offset =
            u32::from_le_bytes(zip_bytes[eocd_pos + 16..eocd_pos + 20].try_into().unwrap())
                as usize;
        // Spoof uncompressed_size = 0 in CD while compressed_size remains 256
        zip_bytes[cd_offset + 24..cd_offset + 28].copy_from_slice(&0u32.to_le_bytes());

        let reader = ZipReader::new(&zip_bytes).unwrap();
        // With max_bytes = 128, spoofed stored entry MUST be rejected
        let err = reader.read_file("payload.txt", 128).unwrap_err();
        assert!(err.to_string().contains("exceeds limit"));

        // Even with larger max_bytes = 1024, stored mismatch (256 vs 0) MUST be rejected
        let err2 = reader.read_file("payload.txt", 1024).unwrap_err();
        assert!(err2.to_string().contains("mismatch"));
    }

    #[test]
    fn test_zip_deflate_bomb_bounded() {
        let big_payload = vec![b'Z'; 4096];
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
        encoder.write_all(&big_payload).unwrap();
        let compressed = encoder.finish().unwrap();
        assert!(compressed.len() < 128);

        // Build ZIP entry with method=8 (Deflated) and spoofed small uncompressed_size
        let mut out = Vec::new();
        let name = b"bomb.txt";
        out.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        out.extend_from_slice(&[20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        out.extend_from_slice(&10u32.to_le_bytes()); // spoofed small uncompressed size
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out.extend_from_slice(name);
        out.extend_from_slice(&compressed);

        let cd_offset = out.len() as u32;
        let mut cd = Vec::new();
        cd.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        cd.extend_from_slice(&[20, 0, 20, 0, 0, 0, 8, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        cd.extend_from_slice(&(compressed.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(100_000_000u32).to_le_bytes()); // also ensure spoofed huge pre-alloc is safe
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        cd.extend_from_slice(name);
        let cd_size = cd.len() as u32;
        out.extend_from_slice(&cd);

        out.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06, 0, 0, 0, 0, 1, 0, 1, 0]);
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&[0, 0]);

        let reader = ZipReader::new(&out).unwrap();
        let err = reader.read_file("bomb.txt", 1024).unwrap_err();
        assert!(err.to_string().contains("exceeds limit"));

        // Within limit succeeds without allocating 100 MB
        let ok = reader.read_file("bomb.txt", 8192).unwrap();
        assert_eq!(ok.len(), 4096);
    }

    #[test]
    fn test_zip_corrupt_offsets_no_panic() {
        let mut zip_bytes = create_test_zip(&[("a.txt", b"hello")]);
        let eocd_pos = zip_bytes
            .windows(4)
            .rposition(|w| w == [0x50, 0x4b, 0x05, 0x06])
            .unwrap();
        let cd_offset =
            u32::from_le_bytes(zip_bytes[eocd_pos + 16..eocd_pos + 20].try_into().unwrap())
                as usize;
        // Corrupt local_header_offset to u32::MAX - 5 (would panic without checked_add on 32-bit, or OOB on 64-bit)
        zip_bytes[cd_offset + 42..cd_offset + 46].copy_from_slice(&(u32::MAX - 5).to_le_bytes());

        let reader = ZipReader::new(&zip_bytes).unwrap();
        assert!(reader.read_file("a.txt", 1024).is_err());

        // Corrupt compressed_size to u32::MAX
        let mut zip_bytes2 = create_test_zip(&[("a.txt", b"hello")]);
        zip_bytes2[cd_offset + 20..cd_offset + 24].copy_from_slice(&u32::MAX.to_le_bytes());
        let reader2 = ZipReader::new(&zip_bytes2).unwrap();
        assert!(reader2.read_file("a.txt", 1024).is_err());
    }
}
