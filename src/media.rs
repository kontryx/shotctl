use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::BufReader,
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
    let mut reader = BufReader::new(File::open(path).ok()?);
    let exif = exif::Reader::new().read_from_container(&mut reader).ok()?;
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
