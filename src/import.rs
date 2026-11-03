use std::{
    collections::{BTreeMap, HashMap},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
};

use anyhow::{Context, Result, bail};
use chrono::Local;
use walkdir::WalkDir;

use crate::{
    config::{Config, DateBasis},
    media,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Copy,
    Skip,
    Rename,
}

#[derive(Debug)]
pub struct Photo {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// Natural destination folder relative to the archive, before custom-folder overrides.
    pub layout_relative: PathBuf,
    pub bytes: u64,
    pub hash: blake3::Hash,
    pub action: Action,
    pub fallback: bool,
}

#[derive(Debug)]
pub struct Plan {
    pub source: PathBuf,
    pub archive: PathBuf,
    pub photos: Vec<Photo>,
    pub skipped_entries: usize,
}

#[derive(Debug, Clone)]
pub struct FolderSummary {
    pub path: PathBuf,
    pub copy: usize,
    pub skip: usize,
    pub bytes: u64,
    pub exists: bool,
}

impl Plan {
    pub fn copy_count(&self) -> usize {
        self.photos
            .iter()
            .filter(|p| p.action != Action::Skip)
            .count()
    }
    pub fn skip_count(&self) -> usize {
        self.photos.len() - self.copy_count()
    }
    pub fn rename_count(&self) -> usize {
        self.photos
            .iter()
            .filter(|p| p.action == Action::Rename)
            .count()
    }
    pub fn fallback_count(&self) -> usize {
        self.photos.iter().filter(|p| p.fallback).count()
    }
    pub fn bytes(&self) -> u64 {
        self.photos
            .iter()
            .filter(|p| p.action != Action::Skip)
            .map(|p| p.bytes)
            .sum()
    }
    pub fn folders(&self) -> Vec<FolderSummary> {
        let mut folders = BTreeMap::new();
        for photo in &self.photos {
            let relative = photo
                .destination
                .parent()
                .unwrap()
                .strip_prefix(&self.archive)
                .unwrap();
            let entry = folders.entry(relative.to_owned()).or_insert((0, 0, 0));
            if photo.action == Action::Skip {
                entry.1 += 1;
            } else {
                entry.0 += 1;
                entry.2 += photo.bytes;
            }
        }
        folders
            .into_iter()
            .map(|(path, (copy, skip, bytes))| {
                let exists = self.archive.join(&path).exists();
                FolderSummary {
                    path,
                    copy,
                    skip,
                    bytes,
                    exists,
                }
            })
            .collect()
    }
}

/// Single path component for a one-time parent under the archive.
pub fn validate_one_time_folder(name: &str) -> Result<&str> {
    let name = name.trim();
    anyhow::ensure!(!name.is_empty(), "Enter a folder name.");
    let mut components = Path::new(name).components();
    anyhow::ensure!(
        matches!(components.next(), Some(std::path::Component::Normal(_)))
            && components.next().is_none()
            && !name.contains(['/', '\\'])
            && !name.chars().any(char::is_control),
        "Enter a single folder name, without path separators."
    );
    Ok(name)
}

/// How a custom segment is applied under the normal layout.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SegmentInsert {
    /// Insert before the leaf for the listed layout-relative new folders.
    Selected { folders: Vec<PathBuf>, name: String },
    /// Insert before the leaf of every destination folder that does not exist yet.
    AllNew { name: String },
}

impl SegmentInsert {
    pub fn name(&self) -> &str {
        match self {
            Self::Selected { name, .. } | Self::AllNew { name } => name,
        }
    }
}

/// `year/month/leaf` → `year/month/<name>/leaf`
pub fn insert_segment_before_leaf(relative: &Path, name: &str) -> PathBuf {
    match relative.file_name() {
        Some(leaf) => relative
            .parent()
            .unwrap_or_else(|| Path::new(""))
            .join(name)
            .join(leaf),
        None => relative.to_owned(),
    }
}

fn apply_segment_insert(
    relative: &Path,
    archive: &Path,
    insert: Option<&SegmentInsert>,
) -> PathBuf {
    match insert {
        Some(SegmentInsert::Selected { folders, name })
            if folders.iter().any(|folder| folder.as_path() == relative) =>
        {
            insert_segment_before_leaf(relative, name)
        }
        Some(SegmentInsert::AllNew { name }) if !archive.join(relative).exists() => {
            insert_segment_before_leaf(relative, name)
        }
        _ => relative.to_owned(),
    }
}

#[derive(Default, Debug)]
pub struct Summary {
    pub copied: usize,
    pub skipped: usize,
    pub failed: usize,
    pub cancelled: bool,
    pub errors: Vec<String>,
}

pub enum WorkerEvent {
    Scanning {
        files: usize,
        current: String,
    },
    Planned(Plan),
    Progress {
        done: usize,
        total: usize,
        bytes: u64,
        total_bytes: u64,
        current: String,
    },
    Finished(Summary),
    Failed(String),
}

fn cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(Ordering::Relaxed) {
        bail!("Operation cancelled");
    }
    Ok(())
}

fn hash_file(path: &Path, cancel: &AtomicBool) -> Result<blake3::Hash> {
    let mut file = File::open(path).with_context(|| format!("Cannot open {}", path.display()))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    loop {
        cancelled(cancel)?;
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
    }
    Ok(hasher.finalize())
}

// Resolve existing ancestors too, so symlinks cannot conceal overlapping roots.
pub fn resolved_path(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        return resolved_path(&std::env::current_dir()?.join(path));
    }
    if path.exists() {
        return Ok(path.canonicalize()?);
    }
    let parent = path.parent().context("Path has no existing ancestor")?;
    let base = resolved_path(parent)?;
    let name = path.file_name().context("Invalid directory path")?;
    if name == ".." {
        return Ok(base
            .parent()
            .context("Invalid parent directory")?
            .to_owned());
    }
    Ok(base.join(name))
}

pub fn validate_roots(source: &Path, archive: &Path) -> Result<(PathBuf, PathBuf)> {
    let source = source
        .canonicalize()
        .context("Source directory is unavailable")?;
    if !source.is_dir() {
        bail!("Source must be a directory");
    }
    let archive = resolved_path(archive)?;
    if source.starts_with(&archive) || archive.starts_with(&source) {
        bail!("The source and archive must be separate directories; neither may contain the other");
    }
    Ok((source, archive))
}

pub fn build_plan(
    source: &Path,
    config: &Config,
    segment_insert: Option<&SegmentInsert>,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<Plan> {
    if let Some(insert) = segment_insert {
        validate_one_time_folder(insert.name())?;
    }
    let (source, archive) = validate_roots(source, &config.archive)?;
    let mut config = config.clone();
    config.archive = archive.clone();
    let today = Local::now().date_naive();
    let mut files = Vec::new();
    let mut skipped_entries = 0;
    let mut walker = WalkDir::new(&source).follow_links(false).into_iter();
    while let Some(entry) = walker.next() {
        cancelled(cancel)?;
        let entry = entry.with_context(|| format!("Cannot fully read {}", source.display()))?;
        // Prune volume housekeeping before WalkDir tries to read protected contents.
        // Keep scanning ordinary hidden folders, and honor an explicitly selected root.
        if entry.depth() > 0
            && entry.file_type().is_dir()
            && matches!(
                entry.file_name().to_str(),
                Some(
                    ".Trashes"
                        | ".Spotlight-V100"
                        | ".fseventsd"
                        | ".TemporaryItems"
                        | ".DocumentRevisions-V100"
                        | "$RECYCLE.BIN"
                        | "System Volume Information"
                )
            )
        {
            skipped_entries += 1;
            walker.skip_current_dir();
            continue;
        }
        if entry.file_type().is_symlink() {
            skipped_entries += 1;
            continue;
        }
        if !entry.file_type().is_file() {
            continue;
        }
        if media::photo_kind(entry.path()).is_some() {
            files.push(entry.into_path());
        } else {
            skipped_entries += 1;
        }
    }
    files.sort();
    let mut photos = Vec::new();
    let mut reserved: HashMap<PathBuf, blake3::Hash> = HashMap::new();
    for (index, path) in files.into_iter().enumerate() {
        cancelled(cancel)?;
        let _ = tx.send(WorkerEvent::Scanning {
            files: index + 1,
            current: path.file_name().unwrap().to_string_lossy().into_owned(),
        });
        let bytes = fs::metadata(&path)?.len();
        let hash = hash_file(&path, cancel)?;
        let (date, fallback) = match config.date_basis {
            DateBasis::Import => (today, false),
            DateBasis::Capture => match media::capture_date(&path) {
                Some(date) => (date, false),
                None => (media::modified_date(&path)?, true),
            },
        };
        let base = config.directory(date, media::photo_kind(&path).unwrap());
        let layout_relative = base
            .strip_prefix(&config.archive)
            .with_context(|| {
                format!(
                    "Destination folder is outside the archive: {}",
                    base.display()
                )
            })?
            .to_owned();
        let directory =
            destination_directory(&config.archive, &layout_relative, segment_insert)?;
        let file_name = path.file_name().unwrap();
        let (destination, action) = resolve_destination(
            &directory,
            file_name,
            bytes,
            hash,
            &mut reserved,
            cancel,
        )?;
        photos.push(Photo {
            source: path,
            destination,
            layout_relative,
            bytes,
            hash,
            action,
            fallback,
        });
    }
    Ok(Plan {
        source,
        archive,
        photos,
        skipped_entries,
    })
}

fn destination_directory(
    archive: &Path,
    layout_relative: &Path,
    segment_insert: Option<&SegmentInsert>,
) -> Result<PathBuf> {
    let relative = apply_segment_insert(layout_relative, archive, segment_insert);
    let directory = archive.join(relative);
    if !resolved_path(&directory)?.starts_with(archive) {
        bail!(
            "Destination folder resolves outside the archive: {}",
            directory.display()
        );
    }
    Ok(directory)
}

fn resolve_destination(
    directory: &Path,
    file_name: &std::ffi::OsStr,
    bytes: u64,
    hash: blake3::Hash,
    reserved: &mut HashMap<PathBuf, blake3::Hash>,
    cancel: &AtomicBool,
) -> Result<(PathBuf, Action)> {
    let original = directory.join(file_name);
    let mut destination = original.clone();
    let mut action = Action::Copy;
    let mut suffix = 0usize;
    loop {
        let matches = if let Some(existing) = reserved.get(&destination) {
            Some(*existing == hash)
        } else if fs::symlink_metadata(&destination).is_ok() {
            // A symlink or directory is a conflict, even if its target matches.
            let metadata = fs::symlink_metadata(&destination)?;
            Some(
                metadata.is_file()
                    && metadata.len() == bytes
                    && hash_file(&destination, cancel)? == hash,
            )
        } else {
            None
        };
        match matches {
            None => break,
            Some(true) => {
                action = Action::Skip;
                break;
            }
            Some(false) => {
                suffix += 1;
                let stem = original.file_stem().unwrap().to_string_lossy();
                let ext = original.extension().unwrap().to_string_lossy();
                let digest = hash.to_hex();
                let extra = if suffix == 1 {
                    String::new()
                } else {
                    format!("-{suffix}")
                };
                destination = directory.join(format!("{stem}-{}{extra}.{ext}", &digest[..12]));
                action = Action::Rename;
            }
        }
    }
    reserved.insert(destination.clone(), hash);
    Ok((destination, action))
}

/// Recompute destinations from an existing plan after custom-folder overrides change.
///
/// Reuses the initial scan (source paths, hashes, layout folders). Photos whose
/// destination folder is unchanged keep their action as-is. Only photos that move
/// into a new folder are re-checked against the archive.
pub fn remap_plan_destinations(
    plan: &mut Plan,
    segment_insert: Option<&SegmentInsert>,
    cancel: &AtomicBool,
) -> Result<()> {
    if let Some(insert) = segment_insert {
        validate_one_time_folder(insert.name())?;
    }
    let mut reserved = HashMap::new();
    // Keep stable destinations first so remapped photos see them as reserved.
    for photo in &plan.photos {
        let directory =
            destination_directory(&plan.archive, &photo.layout_relative, segment_insert)?;
        if photo.destination.parent() == Some(directory.as_path()) {
            reserved.insert(photo.destination.clone(), photo.hash);
        }
    }
    for photo in &mut plan.photos {
        cancelled(cancel)?;
        let directory =
            destination_directory(&plan.archive, &photo.layout_relative, segment_insert)?;
        if photo.destination.parent() == Some(directory.as_path()) {
            continue;
        }
        let file_name = photo.source.file_name().context("Photo has no file name")?;
        let (destination, action) = resolve_destination(
            &directory,
            file_name,
            photo.bytes,
            photo.hash,
            &mut reserved,
            cancel,
        )?;
        photo.destination = destination;
        photo.action = action;
    }
    Ok(())
}

fn copy_photo(
    photo: &Photo,
    archive: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(u64),
) -> Result<bool> {
    cancelled(cancel)?;
    let parent = photo.destination.parent().unwrap();
    let resolved_archive = archive.canonicalize()?;
    if resolved_archive != archive {
        bail!("Archive changed since preview; refresh the preview");
    }
    if !resolved_path(parent)?.starts_with(archive) {
        bail!("Destination resolves outside the archive");
    }
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    if !parent.starts_with(archive) {
        bail!("Destination resolves outside the archive");
    }
    if fs::symlink_metadata(&photo.destination).is_ok() {
        let metadata = fs::symlink_metadata(&photo.destination)?;
        if metadata.is_file()
            && metadata.len() == photo.bytes
            && hash_file(&photo.destination, cancel)? == photo.hash
        {
            return Ok(false);
        }
        bail!(
            "Destination changed after preview: {}. Refresh the preview.",
            photo.destination.display()
        );
    }
    let mut source = File::open(&photo.source)?;
    if source.metadata()?.len() != photo.bytes {
        bail!("Source changed since preview");
    }
    let mut output = tempfile::Builder::new()
        .prefix(".shotctl-")
        .suffix(".partial")
        .tempfile_in(parent)?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; 1024 * 1024];
    let mut bytes = 0;
    loop {
        cancelled(cancel)?;
        let n = source.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        output.write_all(&buffer[..n])?;
        hasher.update(&buffer[..n]);
        bytes += n as u64;
        progress(bytes);
    }
    if bytes != photo.bytes || hasher.finalize() != photo.hash {
        bail!("Source changed since preview; incomplete copy discarded");
    }
    filetime::set_file_mtime(
        output.path(),
        filetime::FileTime::from_last_modification_time(&source.metadata()?),
    )?;
    output.as_file().sync_all()?;
    if hash_file(output.path(), cancel)? != photo.hash {
        bail!("Copy verification failed");
    }
    cancelled(cancel)?;
    // Uses an exclusive rename where supported, with a no-overwrite hard-link fallback.
    output.persist_noclobber(&photo.destination).map_err(|error| error.error)
        .context("Cannot publish photo without overwriting; check the archive filesystem and refresh the preview")?;
    Ok(true)
}

pub fn execute_plan(plan: Plan, tx: &Sender<WorkerEvent>, cancel: Arc<AtomicBool>) -> Summary {
    let mut summary = Summary::default();
    let total = plan.copy_count();
    let total_bytes = plan.bytes();
    let mut completed_bytes = 0;
    let mut done = 0;
    for photo in &plan.photos {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let current = photo
            .source
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        let result = if photo.action == Action::Skip {
            // Recheck duplicates at import time, including those copied earlier in this plan.
            (|| -> Result<bool> {
                if fs::metadata(&photo.source)?.len() != photo.bytes
                    || hash_file(&photo.source, &cancel)? != photo.hash
                {
                    bail!("Source changed since preview; refresh the preview");
                }
                if plan.archive.canonicalize()? != plan.archive
                    || !resolved_path(photo.destination.parent().unwrap())?
                        .starts_with(&plan.archive)
                {
                    bail!("Archive changed since preview; refresh the preview");
                }
                let metadata = fs::symlink_metadata(&photo.destination)?;
                if metadata.is_file()
                    && metadata.len() == photo.bytes
                    && hash_file(&photo.destination, &cancel)? == photo.hash
                {
                    Ok(false)
                } else {
                    bail!("Duplicate destination changed since preview; refresh the preview");
                }
            })()
        } else {
            copy_photo(photo, &plan.archive, &cancel, |bytes| {
                let _ = tx.send(WorkerEvent::Progress {
                    done,
                    total,
                    bytes: completed_bytes + bytes,
                    total_bytes,
                    current: current.clone(),
                });
            })
        };
        match result {
            Ok(true) => summary.copied += 1,
            Ok(false) => summary.skipped += 1,
            Err(_) if cancel.load(Ordering::Relaxed) => {
                summary.cancelled = true;
                break;
            }
            Err(e) => {
                summary.failed += 1;
                summary.errors.push(format!("{current}: {e:#}"));
            }
        }
        if photo.action != Action::Skip {
            done += 1;
            completed_bytes += photo.bytes;
        }
        let _ = tx.send(WorkerEvent::Progress {
            done,
            total,
            bytes: completed_bytes,
            total_bytes,
            current,
        });
    }
    summary
}

pub fn human_bytes(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64;
    let mut unit = "KiB";
    for next in ["KiB", "MiB", "GiB", "TiB"] {
        value /= 1024.0;
        unit = next;
        if value < 1024.0 {
            break;
        }
    }
    format!("{value:.1} {unit}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_mid_copy_discards_partial_output() {
        let temp = tempfile::TempDir::new().unwrap();
        let archive = temp.path().join("archive");
        fs::create_dir(&archive).unwrap();
        let archive = archive.canonicalize().unwrap();
        let source = temp.path().join("photo.JPG");
        let contents = vec![42; 3 * 1024 * 1024];
        fs::write(&source, &contents).unwrap();
        let photo = Photo {
            source: source.clone(),
            destination: archive.join("photo.JPG"),
            layout_relative: PathBuf::new(),
            bytes: contents.len() as u64,
            hash: blake3::hash(&contents),
            action: Action::Copy,
            fallback: false,
        };
        let cancel = AtomicBool::new(false);
        let result = copy_photo(&photo, &archive, &cancel, |_| {
            cancel.store(true, Ordering::Relaxed)
        });
        assert!(result.is_err());
        assert!(!photo.destination.exists());
        assert_eq!(fs::read_dir(archive).unwrap().count(), 0);
        assert_eq!(fs::read(source).unwrap(), contents);
    }
}
