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

impl<'a> ZipReader<'a> {
    /// Opens and parses the central directory of a ZIP archive from in-memory bytes.
    pub fn new(data: &'a [u8]) -> Result<Self> {
        if data.len() < 22 {
            bail!("ZIP archive too short");
        }

        // Search for End of Central Directory (EOCD) signature 0x06054b50 from the end
        let min_offset = data.len().saturating_sub(22 + 65535);
        let eocd_sig = [0x50, 0x4b, 0x05, 0x06];
        let mut eocd_pos = None;
        for i in (min_offset..=data.len() - 22).rev() {
            if data[i..i + 4] == eocd_sig {
                eocd_pos = Some(i);
                break;
            }
        }

        let eocd_offset = eocd_pos.context("End of Central Directory (EOCD) not found")?;
        let total_entries =
            u16::from_le_bytes(data[eocd_offset + 10..eocd_offset + 12].try_into()?) as usize;
        let cd_size =
            u32::from_le_bytes(data[eocd_offset + 12..eocd_offset + 16].try_into()?) as usize;
        let cd_offset =
            u32::from_le_bytes(data[eocd_offset + 16..eocd_offset + 20].try_into()?) as usize;

        if cd_offset.saturating_add(cd_size) > data.len() {
            bail!("Corrupt ZIP: central directory out of bounds");
        }

        let mut pos = cd_offset;
        let cd_end = cd_offset + cd_size;
        let mut entries = Vec::with_capacity(total_entries.min(1024));

        while pos + 46 <= cd_end {
            if data[pos..pos + 4] != [0x50, 0x4b, 0x01, 0x02] {
                break;
            }
            let compression_method = u16::from_le_bytes(data[pos + 10..pos + 12].try_into()?);
            let compressed_size = u32::from_le_bytes(data[pos + 20..pos + 24].try_into()?) as usize;
            let uncompressed_size =
                u32::from_le_bytes(data[pos + 24..pos + 28].try_into()?) as usize;
            let name_len = u16::from_le_bytes(data[pos + 28..pos + 30].try_into()?) as usize;
            let extra_len = u16::from_le_bytes(data[pos + 30..pos + 32].try_into()?) as usize;
            let comment_len = u16::from_le_bytes(data[pos + 32..pos + 34].try_into()?) as usize;
            let local_header_offset =
                u32::from_le_bytes(data[pos + 42..pos + 46].try_into()?) as usize;

            let name_start = pos + 46;
            if name_start + name_len > cd_end {
                bail!("Corrupt ZIP: filename out of bounds");
            }
            let name = std::str::from_utf8(&data[name_start..name_start + name_len]).unwrap_or("");

            entries.push(ZipEntry {
                name,
                compression_method,
                compressed_size,
                uncompressed_size,
                local_header_offset,
            });

            pos = name_start + name_len + extra_len + comment_len;
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
        if lh_offset + 30 > self.data.len() {
            bail!("Corrupt ZIP: local file header out of bounds");
        }
        if self.data[lh_offset..lh_offset + 4] != [0x50, 0x4b, 0x03, 0x04] {
            bail!("Corrupt ZIP: invalid local file header signature");
        }
        let fn_len =
            u16::from_le_bytes(self.data[lh_offset + 26..lh_offset + 28].try_into()?) as usize;
        let extra_len =
            u16::from_le_bytes(self.data[lh_offset + 28..lh_offset + 30].try_into()?) as usize;
        let data_start = lh_offset + 30 + fn_len + extra_len;
        let data_end = data_start + entry.compressed_size;
        if data_end > self.data.len() {
            bail!("Corrupt ZIP: file payload out of bounds");
        }
        let compressed = &self.data[data_start..data_end];

        let limit = (max_bytes as u64) + 1;
        match entry.compression_method {
            0 => {
                // Stored (uncompressed)
                if entry.uncompressed_size > max_bytes {
                    bail!("Decompressed entry exceeds limit of {} bytes", max_bytes);
                }
                Ok(compressed.to_vec())
            }
            8 => {
                // Deflated
                let mut decoder = DeflateDecoder::new(compressed).take(limit);
                let mut out = Vec::with_capacity(entry.uncompressed_size.min(max_bytes));
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
#[doc(hidden)]
pub fn create_test_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
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
