use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::{BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
};

use anyhow::{Context, Result};
use chrono::{DateTime, Local, NaiveDate};

pub fn photo_kind(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" => Some("JPEG"),
        "arw" | "cr2" | "cr3" | "crw" | "dng" | "erf" | "fff" | "iiq" | "kdc" | "mos" | "mrw"
        | "nef" | "nrw" | "orf" | "pef" | "raf" | "raw" | "rw2" | "rwl" | "sr2" | "srf" | "srw"
        | "x3f" => Some("RAW"),
        "heic" | "heif" => Some("HEIF"),
        "tif" | "tiff" => Some("TIFF"),
        "png" => Some("PNG"),
        _ => None,
    }
}

pub fn capture_date(path: &Path) -> Option<NaiveDate> {
    let mut file = File::open(path).ok()?;
    let mut head = [0u8; 4];
    file.read_exact(&mut head).ok()?;
    file.seek(SeekFrom::Start(0)).ok()?;
    if matches!(&head, b"II*\0" | b"MM\0*") {
        // TIFF-based files (DNG, ARW, NEF, CR2, …): the EXIF library would load the
        // whole file, tens of megabytes of card reads per photo for one date. Their
        // directories are tiny, so read just those few bytes.
        match tiff_capture_date(&mut file) {
            Ok(date) => return date,
            Err(_) => file.seek(SeekFrom::Start(0)).ok()?, // Unusual layout: use the library.
        };
    }
    // JPEG, PNG, HEIF and WebP are read selectively by the library already.
    let exif = exif::Reader::new()
        .read_from_container(&mut BufReader::new(file))
        .ok()?;
    date_from_exif(&exif)
}

struct TiffEntry {
    tag: u16,
    kind: u16,
    count: u32,
    value: [u8; 4],
}

struct TiffReader<'a> {
    file: &'a mut File,
    len: u64,
    big_endian: bool,
}

impl TiffReader<'_> {
    fn u16(&self, bytes: [u8; 2]) -> u16 {
        if self.big_endian {
            u16::from_be_bytes(bytes)
        } else {
            u16::from_le_bytes(bytes)
        }
    }

    fn u32(&self, bytes: [u8; 4]) -> u32 {
        if self.big_endian {
            u32::from_be_bytes(bytes)
        } else {
            u32::from_le_bytes(bytes)
        }
    }

    fn read_at(&mut self, offset: u64, size: usize) -> Result<Vec<u8>, ()> {
        if offset
            .checked_add(size as u64)
            .is_none_or(|end| end > self.len)
        {
            return Err(());
        }
        let mut buf = vec![0; size];
        self.file.seek(SeekFrom::Start(offset)).map_err(|_| ())?;
        self.file.read_exact(&mut buf).map_err(|_| ())?;
        Ok(buf)
    }

    fn ifd(&mut self, offset: u64) -> Result<Vec<TiffEntry>, ()> {
        let count = self.read_at(offset, 2)?;
        let count = self.u16([count[0], count[1]]) as usize;
        let raw = self.read_at(offset + 2, count * 12)?;
        Ok(raw
            .as_chunks::<12>()
            .0
            .iter()
            .map(|entry| TiffEntry {
                tag: self.u16([entry[0], entry[1]]),
                kind: self.u16([entry[2], entry[3]]),
                count: self.u32([entry[4], entry[5], entry[6], entry[7]]),
                value: [entry[8], entry[9], entry[10], entry[11]],
            })
            .collect())
    }

    fn date(&mut self, entry: &TiffEntry) -> Option<NaiveDate> {
        // ASCII "YYYY:MM:DD HH:MM:SS\0": longer than 4 bytes, so `value` is an offset.
        if entry.kind != 2 || entry.count < 10 || entry.count > 64 {
            return None;
        }
        let offset = u64::from(self.u32(entry.value));
        let text = self.read_at(offset, entry.count as usize).ok()?;
        NaiveDate::parse_from_str(std::str::from_utf8(text.get(..10)?).ok()?, "%Y:%m:%d").ok()
    }
}

/// `Err` means the file is not laid out as expected, `Ok(None)` that it has no date.
fn tiff_capture_date(file: &mut File) -> Result<Option<NaiveDate>, ()> {
    let len = file.metadata().map_err(|_| ())?.len();
    let mut reader = TiffReader {
        file,
        len,
        big_endian: false,
    };
    let header = reader.read_at(0, 8)?;
    reader.big_endian = &header[..2] == b"MM";
    if reader.u16([header[2], header[3]]) != 42 {
        return Err(());
    }
    let ifd0 = reader.ifd(u64::from(
        reader.u32([header[4], header[5], header[6], header[7]]),
    ))?;
    let exif_ifd = match ifd0
        .iter()
        .find(|entry| entry.tag == 0x8769 && entry.kind == 4)
    {
        Some(pointer) => reader.ifd(u64::from(reader.u32(pointer.value)))?,
        None => Vec::new(),
    };
    // Same preference order as before: original, digitized, then the file's own date.
    for (entries, tag) in [(&exif_ifd, 0x9003), (&exif_ifd, 0x9004), (&ifd0, 0x0132)] {
        if let Some(entry) = entries.iter().find(|entry| entry.tag == tag)
            && let Some(date) = reader.date(entry)
        {
            return Ok(Some(date));
        }
    }
    Ok(None)
}

fn date_from_exif(exif: &exif::Exif) -> Option<NaiveDate> {
    [
        exif::Tag::DateTimeOriginal,
        exif::Tag::DateTimeDigitized,
        exif::Tag::DateTime,
    ]
    .into_iter()
    .find_map(|tag| {
        let field = exif.get_field(tag, exif::In::PRIMARY)?;
        if let exif::Value::Ascii(values) = &field.value {
            let text = std::str::from_utf8(values.first()?).ok()?;
            NaiveDate::parse_from_str(text.get(..10)?, "%Y:%m:%d").ok()
        } else {
            None
        }
    })
}

pub fn modified_date(path: &Path) -> Result<NaiveDate> {
    let modified: DateTime<Local> = fs::metadata(path)?
        .modified()
        .with_context(|| format!("Cannot read file date: {}", path.display()))?
        .into();
    Ok(modified.date_naive())
}

// Mounted volumes are candidates only when a DCIM directory is visible.
// Manual selection supports any readable directory, including an unmounted-card copy.
pub fn discover_sources() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from("/Volumes"),
        PathBuf::from("/media"),
        PathBuf::from("/run/media"),
        PathBuf::from("/mnt"),
    ];
    if cfg!(windows) {
        roots.extend(('D'..='Z').map(|letter| PathBuf::from(format!("{letter}:\\"))));
    }
    let mut found = BTreeSet::new();
    for root in roots {
        if has_dcim(&root) {
            found.insert(root.clone());
        }
        let Ok(children) = fs::read_dir(&root) else {
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            if !path.is_dir() {
                continue;
            }
            if has_dcim(&path) {
                found.insert(path);
                continue;
            }
            if let Ok(volumes) = fs::read_dir(path) {
                for volume in volumes.flatten() {
                    if has_dcim(&volume.path()) {
                        found.insert(volume.path());
                    }
                }
            }
        }
    }
    found.into_iter().collect()
}

fn has_dcim(path: &Path) -> bool {
    path.join("DCIM").is_dir() || path.join("dcim").is_dir()
}
