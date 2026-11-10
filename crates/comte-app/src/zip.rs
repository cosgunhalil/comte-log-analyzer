//! A small zip writer for support packs: deflated files, UTF-8 names, no
//! zip64 (files and archives stay well under 4 GB; bigger files are refused).
//!
//! Built on `flate2` and `crc32fast`, which the app already depends on
//! through `png`.

use std::io::{self, Write};

use flate2::Compression;
use flate2::write::DeflateEncoder;

const LOCAL_HEADER: u32 = 0x0403_4b50;
const CENTRAL_HEADER: u32 = 0x0201_4b50;
const END_OF_CENTRAL: u32 = 0x0605_4b50;
/// Names are UTF-8 (general purpose flag bit 11).
const UTF8_NAMES: u16 = 1 << 11;
const DEFLATE: u16 = 8;
const VERSION: u16 = 20;

struct Entry {
    name: String,
    crc: u32,
    compressed: u32,
    size: u32,
    offset: u32,
}

pub struct ZipWriter<W: Write> {
    out: W,
    offset: u64,
    entries: Vec<Entry>,
    /// MS-DOS time and date of every entry.
    dos_time: u16,
    dos_date: u16,
}

impl<W: Write> std::fmt::Debug for ZipWriter<W> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipWriter")
            .field("entries", &self.entries.len())
            .finish()
    }
}

fn too_big() -> io::Error {
    io::Error::other("file too large for a support pack (over 4 GB)")
}

impl<W: Write> ZipWriter<W> {
    pub fn new(out: W, now: chrono::NaiveDateTime) -> Self {
        use chrono::{Datelike, Timelike};
        let year = u16::try_from(now.year().clamp(1980, 2107) - 1980).unwrap_or(0);
        Self {
            out,
            offset: 0,
            entries: Vec::new(),
            dos_time: (now.hour() as u16) << 11
                | (now.minute() as u16) << 5
                | (now.second() as u16 / 2),
            dos_date: year << 9 | (now.month() as u16) << 5 | now.day() as u16,
        }
    }

    fn put(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.out.write_all(bytes)?;
        self.offset += bytes.len() as u64;
        Ok(())
    }

    /// Adds a file. `name` uses `/` between folders.
    pub fn add(&mut self, name: &str, data: &[u8]) -> io::Result<()> {
        let size = u32::try_from(data.len()).map_err(|_| too_big())?;
        let crc = crc32fast::hash(data);
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data)?;
        let compressed = encoder.finish()?;
        let compressed_len = u32::try_from(compressed.len()).map_err(|_| too_big())?;
        let offset = u32::try_from(self.offset).map_err(|_| too_big())?;
        let name_len = u16::try_from(name.len()).map_err(|_| io::Error::other("name too long"))?;

        let mut header = Vec::with_capacity(30 + name.len());
        header.extend_from_slice(&LOCAL_HEADER.to_le_bytes());
        header.extend_from_slice(&VERSION.to_le_bytes());
        header.extend_from_slice(&UTF8_NAMES.to_le_bytes());
        header.extend_from_slice(&DEFLATE.to_le_bytes());
        header.extend_from_slice(&self.dos_time.to_le_bytes());
        header.extend_from_slice(&self.dos_date.to_le_bytes());
        header.extend_from_slice(&crc.to_le_bytes());
        header.extend_from_slice(&compressed_len.to_le_bytes());
        header.extend_from_slice(&size.to_le_bytes());
        header.extend_from_slice(&name_len.to_le_bytes());
        header.extend_from_slice(&0u16.to_le_bytes()); // extra field length
        header.extend_from_slice(name.as_bytes());
        self.put(&header)?;
        self.put(&compressed)?;
        self.entries.push(Entry {
            name: name.to_owned(),
            crc,
            compressed: compressed_len,
            size,
            offset,
        });
        Ok(())
    }

    /// Writes the central directory. Returns the writer.
    pub fn finish(mut self) -> io::Result<W> {
        let start = u32::try_from(self.offset).map_err(|_| too_big())?;
        let entries = std::mem::take(&mut self.entries);
        for e in &entries {
            let mut h = Vec::with_capacity(46 + e.name.len());
            h.extend_from_slice(&CENTRAL_HEADER.to_le_bytes());
            h.extend_from_slice(&VERSION.to_le_bytes()); // made by
            h.extend_from_slice(&VERSION.to_le_bytes()); // needed
            h.extend_from_slice(&UTF8_NAMES.to_le_bytes());
            h.extend_from_slice(&DEFLATE.to_le_bytes());
            h.extend_from_slice(&self.dos_time.to_le_bytes());
            h.extend_from_slice(&self.dos_date.to_le_bytes());
            h.extend_from_slice(&e.crc.to_le_bytes());
            h.extend_from_slice(&e.compressed.to_le_bytes());
            h.extend_from_slice(&e.size.to_le_bytes());
            h.extend_from_slice(&(e.name.len() as u16).to_le_bytes());
            h.extend_from_slice(&[0; 2]); // extra field length
            h.extend_from_slice(&[0; 2]); // comment length
            h.extend_from_slice(&[0; 2]); // disk number
            h.extend_from_slice(&[0; 2]); // internal attributes
            h.extend_from_slice(&[0; 4]); // external attributes
            h.extend_from_slice(&e.offset.to_le_bytes());
            h.extend_from_slice(e.name.as_bytes());
            self.put(&h)?;
        }
        let size = u32::try_from(self.offset).map_err(|_| too_big())? - start;
        let count = u16::try_from(entries.len()).map_err(|_| io::Error::other("too many files"))?;
        let mut end = Vec::with_capacity(22);
        end.extend_from_slice(&END_OF_CENTRAL.to_le_bytes());
        end.extend_from_slice(&[0; 4]); // disk numbers
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&count.to_le_bytes());
        end.extend_from_slice(&size.to_le_bytes());
        end.extend_from_slice(&start.to_le_bytes());
        end.extend_from_slice(&[0; 2]); // comment length
        self.put(&end)?;
        self.out.flush()?;
        Ok(self.out)
    }
}

/// Reads back the files of a zip written by [`ZipWriter`] (for tests).
#[cfg(test)]
pub fn read_back(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
    use std::io::Read;
    let u16_at = |i: usize| u16::from_le_bytes([zip[i], zip[i + 1]]) as usize;
    let u32_at = |i: usize| u32::from_le_bytes([zip[i], zip[i + 1], zip[i + 2], zip[i + 3]]);
    let end = zip.len() - 22;
    assert_eq!(u32_at(end), END_OF_CENTRAL);
    let count = u16_at(end + 10);
    let mut at = u32_at(end + 16) as usize;
    let mut files = Vec::new();
    for _ in 0..count {
        assert_eq!(u32_at(at), CENTRAL_HEADER);
        let crc = u32_at(at + 16);
        let compressed = u32_at(at + 20) as usize;
        let name_len = u16_at(at + 28);
        let offset = u32_at(at + 42) as usize;
        let name = String::from_utf8(zip[at + 46..at + 46 + name_len].to_vec()).unwrap();
        assert_eq!(u32_at(offset), LOCAL_HEADER);
        let data_at = offset + 30 + u16_at(offset + 26) + u16_at(offset + 28);
        let mut data = Vec::new();
        flate2::read::DeflateDecoder::new(&zip[data_at..data_at + compressed])
            .read_to_end(&mut data)
            .unwrap();
        assert_eq!(crc32fast::hash(&data), crc, "{name}");
        files.push((name, data));
        at += 46 + name_len;
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_readable_zip() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 11, 10)
            .unwrap()
            .and_hms_opt(14, 5, 30)
            .unwrap();
        let mut zip = ZipWriter::new(Vec::new(), now);
        zip.add("summary.txt", b"Crashed in Scene #3\n").unwrap();
        zip.add("crash/error.log", &b"line\n".repeat(1000)).unwrap();
        zip.add("Ünïcode.log", b"").unwrap();
        let bytes = zip.finish().unwrap();
        let files = read_back(&bytes);
        assert_eq!(files.len(), 3);
        assert_eq!(
            files[0],
            ("summary.txt".to_owned(), b"Crashed in Scene #3\n".to_vec())
        );
        assert_eq!(files[1].1.len(), 5000);
        assert_eq!(files[2].0, "Ünïcode.log");
        assert!(bytes.len() < 1000, "compressed: {}", bytes.len());
    }
}
