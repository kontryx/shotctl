use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::SystemTime,
};

use anyhow::{Context, Result, bail};
use chrono::{Local, NaiveDate};
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

/// Remembers expensive per-file work (full-file hashes and EXIF dates) for the
/// whole session, so rescanning a source — for example to start another import —
/// does not read unchanged files again. Entries are keyed by path and only reused
/// while the file's size and modification time are unchanged.
#[derive(Debug, Default)]
pub struct ScanCache {
    inner: Mutex<CacheInner>,
}

#[derive(Debug, Default)]
struct CacheInner {
    hashes: HashMap<PathBuf, (u64, SystemTime, blake3::Hash)>,
    captures: HashMap<PathBuf, (u64, SystemTime, Option<NaiveDate>)>,
    /// Hashes learned since the last save.
    dirty: bool,
}

const CACHE_HEADER: &str = "shotctl-hash-cache 1";
/// Bounds the file; this is only a cache, so dropping entries just means re-reading.
const CACHE_MAX_ENTRIES: usize = 300_000;

impl ScanCache {
    fn lock(&self) -> std::sync::MutexGuard<'_, CacheInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn stamp(path: &Path) -> Result<(u64, SystemTime)> {
        let metadata =
            fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
        let modified = metadata
            .modified()
            .with_context(|| format!("Cannot read file date: {}", path.display()))?;
        Ok((metadata.len(), modified))
    }

    /// Full-file hash, read from disk only if the file is new or has changed.
    fn hash(&self, path: &Path, cancel: &AtomicBool) -> Result<blake3::Hash> {
        let (len, modified) = Self::stamp(path)?;
        if let Some(hash) = self.peek(path, len, modified) {
            return Ok(hash);
        }
        let hash = hash_file(path, cancel)?;
        self.remember(path, len, modified, hash);
        Ok(hash)
    }

    fn peek(&self, path: &Path, len: u64, modified: SystemTime) -> Option<blake3::Hash> {
        self.lock()
            .hashes
            .get(path)
            .filter(|(l, m, _)| *l == len && *m == modified)
            .map(|(_, _, hash)| *hash)
    }

    fn remember(&self, path: &Path, len: u64, modified: SystemTime, hash: blake3::Hash) {
        let mut inner = self.lock();
        inner.hashes.insert(path.to_owned(), (len, modified, hash));
        inner.dirty = true;
    }

    /// Load hashes remembered by earlier runs. A missing or damaged file is simply
    /// an empty cache; entries are still only used while size and date still match.
    pub fn load(path: &Path) -> Self {
        let cache = Self::default();
        let Ok(text) = fs::read_to_string(path) else {
            return cache;
        };
        let mut lines = text.lines();
        if lines.next() != Some(CACHE_HEADER) {
            return cache;
        }
        let mut inner = cache.lock();
        for line in lines {
            let mut parts = line.splitn(5, '\t');
            let (Some(hash), Some(len), Some(secs), Some(nanos), Some(file)) = (
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
                parts.next(),
            ) else {
                continue;
            };
            let (Ok(hash), Ok(len), Ok(secs), Ok(nanos)) = (
                blake3::Hash::from_hex(hash),
                len.parse::<u64>(),
                secs.parse::<u64>(),
                nanos.parse::<u32>(),
            ) else {
                continue;
            };
            let modified = SystemTime::UNIX_EPOCH + std::time::Duration::new(secs, nanos);
            inner
                .hashes
                .insert(PathBuf::from(file), (len, modified, hash));
        }
        drop(inner);
        cache
    }

    /// Write the cache if anything was learned. Best effort: failing to save only
    /// costs speed next time.
    pub fn save(&self, path: &Path) {
        let text = {
            let mut inner = self.lock();
            if !inner.dirty {
                return;
            }
            inner.dirty = false;
            let mut text = format!("{CACHE_HEADER}\n");
            for (file, (len, modified, hash)) in inner.hashes.iter().take(CACHE_MAX_ENTRIES) {
                let (Some(file), Ok(since)) = (
                    file.to_str(),
                    modified.duration_since(SystemTime::UNIX_EPOCH),
                ) else {
                    continue;
                };
                if file.contains(['\t', '\n', '\r']) {
                    continue;
                }
                text.push_str(&format!(
                    "{}\t{len}\t{}\t{}\t{file}\n",
                    hash.to_hex(),
                    since.as_secs(),
                    since.subsec_nanos()
                ));
            }
            text
        };
        let Some(parent) = path.parent() else { return };
        let _ = fs::create_dir_all(parent);
        if let Ok(mut file) = tempfile::NamedTempFile::new_in(parent)
            && file.write_all(text.as_bytes()).is_ok()
        {
            let _ = file.persist(path);
        }
    }

    fn capture_date(&self, path: &Path, len: u64, modified: SystemTime) -> Option<NaiveDate> {
        if let Some((l, m, date)) = self.lock().captures.get(path)
            && *l == len
            && *m == modified
        {
            return *date;
        }
        let date = media::capture_date(path);
        self.lock()
            .captures
            .insert(path.to_owned(), (len, modified, date));
        date
    }
}

#[derive(Debug)]
pub struct Photo {
    pub source: PathBuf,
    pub destination: PathBuf,
    /// Natural destination folder relative to the archive, before custom-folder overrides.
    pub layout_relative: PathBuf,
    pub bytes: u64,
    /// Source modification time at scan; a change before copying means the file moved on.
    pub modified: SystemTime,
    /// Known only when a duplicate or name-conflict check needed it. New photos are
    /// not hashed until they are copied.
    pub hash: Option<blake3::Hash>,
    pub action: Action,
    pub fallback: bool,
}

#[derive(Debug)]
pub struct Plan {
    pub source: PathBuf,
    pub archive: PathBuf,
    pub photos: Vec<Photo>,
    pub skipped_entries: usize,
    pub cache: Arc<ScanCache>,
    /// Layout-relative new folders the user chose not to create. Photos that
    /// belong to them are left out of the import entirely.
    pub excluded: BTreeSet<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct FolderSummary {
    pub path: PathBuf,
    /// Natural (layout-relative) folder; the key used to include or exclude it.
    pub layout: PathBuf,
    /// False when the user opted not to create this new folder.
    pub included: bool,
    pub copy: usize,
    pub skip: usize,
    pub bytes: u64,
    pub exists: bool,
    /// When an existing folder was created, i.e. when photos first arrived in it.
    /// `None` for new folders and where the filesystem does not record creation dates.
    pub imported: Option<NaiveDate>,
}

impl Plan {
    pub fn is_excluded(&self, photo: &Photo) -> bool {
        self.excluded.contains(&photo.layout_relative)
    }
    fn active(&self) -> impl Iterator<Item = &Photo> {
        self.photos.iter().filter(|p| !self.is_excluded(p))
    }
    /// Number of photos left out because their new folder is not selected.
    pub fn excluded_count(&self) -> usize {
        self.photos.len() - self.active().count()
    }
    pub fn copy_count(&self) -> usize {
        self.active().filter(|p| p.action != Action::Skip).count()
    }
    pub fn skip_count(&self) -> usize {
        self.active().filter(|p| p.action == Action::Skip).count()
    }
    pub fn rename_count(&self) -> usize {
        self.active().filter(|p| p.action == Action::Rename).count()
    }
    pub fn fallback_count(&self) -> usize {
        self.photos.iter().filter(|p| p.fallback).count()
    }
    pub fn bytes(&self) -> u64 {
        self.active()
            .filter(|p| p.action != Action::Skip)
            .map(|p| p.bytes)
            .sum()
    }
    pub fn folders(&self) -> Vec<FolderSummary> {
        let mut folders: BTreeMap<PathBuf, (PathBuf, usize, usize, u64)> = BTreeMap::new();
        for photo in &self.photos {
            let relative = photo
                .destination
                .parent()
                .unwrap()
                .strip_prefix(&self.archive)
                .unwrap();
            let entry = folders
                .entry(relative.to_owned())
                .or_insert_with(|| (photo.layout_relative.clone(), 0, 0, 0));
            if photo.action == Action::Skip {
                entry.2 += 1;
            } else {
                entry.1 += 1;
                entry.3 += photo.bytes;
            }
        }
        folders
            .into_iter()
            .map(|(path, (layout, copy, skip, bytes))| {
                let metadata = fs::metadata(self.archive.join(&path)).ok();
                let exists = metadata.is_some();
                let imported = metadata
                    .and_then(|metadata| metadata.created().ok())
                    .map(|created| chrono::DateTime::<Local>::from(created).date_naive());
                FolderSummary {
                    imported,
                    included: exists || !self.excluded.contains(&layout),
                    path,
                    layout,
                    copy,
                    skip,
                    bytes,
                    exists,
                }
            })
            .collect()
    }

    /// Layout keys of every folder this import would create.
    pub fn new_folder_layouts(&self) -> BTreeSet<PathBuf> {
        self.folders()
            .into_iter()
            .filter(|folder| !folder.exists)
            .map(|folder| folder.layout)
            .collect()
    }

    /// Flip whether one new folder is created. Returns the new `included` state.
    pub fn toggle_new_folder(&mut self, layout: &Path) -> bool {
        if !self.new_folder_layouts().contains(layout) {
            return true;
        }
        if self.excluded.remove(layout) {
            true
        } else {
            self.excluded.insert(layout.to_owned());
            false
        }
    }

    /// Include or exclude every new folder at once.
    pub fn set_all_new_folders(&mut self, included: bool) {
        self.excluded = if included {
            BTreeSet::new()
        } else {
            self.new_folder_layouts()
        };
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

#[cfg(test)]
pub fn build_plan(
    source: &Path,
    config: &Config,
    segment_insert: Option<&SegmentInsert>,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<Plan> {
    build_plan_cached(
        source,
        config,
        segment_insert,
        tx,
        cancel,
        &Arc::new(ScanCache::default()),
    )
}

/// Scan `source` and decide a destination for every photo. Photos are only
/// hashed when a duplicate or name-conflict check needs it, and anything already
/// known to `cache` is not read again.
pub fn build_plan_cached(
    source: &Path,
    config: &Config,
    segment_insert: Option<&SegmentInsert>,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
    cache: &Arc<ScanCache>,
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

    // Stage 1: cheap facts for every file — size, date, and the folder it belongs in.
    struct Scanned {
        path: PathBuf,
        bytes: u64,
        modified: SystemTime,
        fallback: bool,
        layout_relative: PathBuf,
        directory: PathBuf,
    }
    let mut scanned = Vec::with_capacity(files.len());
    for (index, path) in files.into_iter().enumerate() {
        cancelled(cancel)?;
        let _ = tx.send(WorkerEvent::Scanning {
            files: index + 1,
            current: path.file_name().unwrap().to_string_lossy().into_owned(),
        });
        let metadata = fs::metadata(&path)?;
        let bytes = metadata.len();
        let modified = metadata
            .modified()
            .with_context(|| format!("Cannot read file date: {}", path.display()))?;
        let (date, fallback) = match config.date_basis {
            DateBasis::Import => (today, false),
            DateBasis::Capture => match cache.capture_date(&path, bytes, modified) {
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
        let directory = destination_directory(&config.archive, &layout_relative, segment_insert)?;
        scanned.push(Scanned {
            path,
            bytes,
            modified,
            fallback,
            layout_relative,
            directory,
        });
    }

    // Stage 2: a photo can only be a duplicate if a file of the same name and size is
    // already at its destination. Only those pairs need comparing, so read just them —
    // together, with several files in flight, instead of one after another.
    let mut variants = VariantIndex::new(&archive);
    let mut to_hash: Vec<PathBuf> = Vec::new();
    for item in &scanned {
        let name = item.path.file_name().unwrap();
        let natural = archive.join(&item.layout_relative);
        for existing in
            duplicate_candidates(&item.directory, &natural, name, item.bytes, &mut variants)
        {
            to_hash.push(item.path.clone());
            to_hash.push(existing);
        }
    }
    prehash(&to_hash, scanned.len(), cache, tx, cancel)?;

    // Stage 3: settle every destination. Comparisons now come from the cache.
    let mut photos = Vec::with_capacity(scanned.len());
    let mut reserved: HashMap<PathBuf, Reserved> = HashMap::new();
    for item in scanned {
        cancelled(cancel)?;
        let file_name = item.path.file_name().unwrap();
        let (destination, action) = resolve_destination(
            &item.directory,
            &archive.join(&item.layout_relative),
            file_name,
            &item.path,
            item.bytes,
            &mut reserved,
            &mut variants,
            cache,
            cancel,
        )?;
        let hash = cache.peek(&item.path, item.bytes, item.modified);
        photos.push(Photo {
            source: item.path,
            destination,
            layout_relative: item.layout_relative,
            bytes: item.bytes,
            modified: item.modified,
            hash,
            action,
            fallback: item.fallback,
        });
    }
    Ok(Plan {
        source,
        archive,
        photos,
        skipped_entries,
        cache: cache.clone(),
        excluded: BTreeSet::new(),
    })
}

/// Hash `paths` on a few threads, filling the cache. Failures are left for the
/// sequential pass to report with proper context; cancellation is honoured.
fn prehash(
    paths: &[PathBuf],
    photos_inspected: usize,
    cache: &ScanCache,
    tx: &Sender<WorkerEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    const WORKERS: usize = 4;
    if paths.len() < 2 {
        return Ok(());
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..WORKERS.min(paths.len()) {
            let tx = tx.clone();
            let next = &next;
            scope.spawn(move || {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    let Some(path) = paths.get(index) else { break };
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let _ = tx.send(WorkerEvent::Scanning {
                        files: photos_inspected,
                        current: format!(
                            "Comparing {}",
                            path.file_name().unwrap_or_default().to_string_lossy()
                        ),
                    });
                    let _ = cache.hash(path, cancel);
                }
            });
        }
    });
    cancelled(cancel)
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

/// Finds folders that hold the same photos as a destination folder under a custom
/// name. A one-time custom folder turns `year/month/leaf` into `year/month/<name>/leaf`,
/// so the alternatives are `<parent>/*/<leaf>` for the natural folder `<parent>/<leaf>`.
struct VariantIndex {
    archive: PathBuf,
    found: HashMap<PathBuf, Vec<PathBuf>>,
}

impl VariantIndex {
    fn new(archive: &Path) -> Self {
        Self {
            archive: archive.to_owned(),
            found: HashMap::new(),
        }
    }

    fn dirs(&mut self, directory: &Path) -> &[PathBuf] {
        if !self.found.contains_key(directory) {
            let list = Self::scan(&self.archive, directory);
            self.found.insert(directory.to_owned(), list);
        }
        &self.found[directory]
    }

    fn scan(archive: &Path, directory: &Path) -> Vec<PathBuf> {
        let (Some(parent), Some(leaf)) = (directory.parent(), directory.file_name()) else {
            return Vec::new();
        };
        if !parent.starts_with(archive) {
            return Vec::new();
        }
        let Ok(children) = fs::read_dir(parent) else {
            return Vec::new();
        };
        let mut dirs: Vec<PathBuf> = children
            .flatten()
            // Real folders only: never follow a symlink out of the archive.
            .filter(|child| child.file_type().is_ok_and(|kind| kind.is_dir()))
            .map(|child| child.path().join(leaf))
            .filter(|candidate| {
                candidate != directory
                    && fs::symlink_metadata(candidate).is_ok_and(|meta| meta.is_dir())
            })
            .collect();
        dirs.sort();
        dirs
    }
}

/// Existing files that could be the same photo: same name and size at the
/// destination, or in one of its custom-named variants when it has no such file.
fn duplicate_candidates(
    directory: &Path,
    natural: &Path,
    name: &std::ffi::OsStr,
    bytes: u64,
    variants: &mut VariantIndex,
) -> Vec<PathBuf> {
    let same_size = |path: &Path| {
        fs::symlink_metadata(path).is_ok_and(|meta| meta.is_file() && meta.len() == bytes)
    };
    let primary = directory.join(name);
    if fs::symlink_metadata(&primary).is_ok() {
        return if same_size(&primary) {
            vec![primary]
        } else {
            Vec::new()
        };
    }
    variants
        .dirs(natural)
        .iter()
        .map(|dir| dir.join(name))
        .filter(|path| same_size(path))
        .collect()
}

fn find_in_variants(
    directory: &Path,
    natural: &Path,
    name: &std::ffi::OsStr,
    source: &Path,
    bytes: u64,
    variants: &mut VariantIndex,
    cache: &ScanCache,
    cancel: &AtomicBool,
) -> Result<Option<PathBuf>> {
    for candidate in duplicate_candidates(directory, natural, name, bytes, variants) {
        if cache.hash(&candidate, cancel)? == cache.hash(source, cancel)? {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

/// A destination already claimed by an earlier photo in the same plan.
struct Reserved {
    source: PathBuf,
    bytes: u64,
}

fn resolve_destination(
    directory: &Path,
    natural: &Path,
    file_name: &std::ffi::OsStr,
    source: &Path,
    bytes: u64,
    reserved: &mut HashMap<PathBuf, Reserved>,
    variants: &mut VariantIndex,
    cache: &ScanCache,
    cancel: &AtomicBool,
) -> Result<(PathBuf, Action)> {
    let original = directory.join(file_name);
    let mut destination = original.clone();
    let mut action = Action::Copy;
    let mut suffix = 0usize;
    loop {
        // Differing sizes can never be identical, so hashing is only needed when
        // something with the same name and size is already there.
        let matches = if let Some(other) = reserved.get(&destination) {
            Some(
                other.bytes == bytes
                    && cache.hash(&other.source, cancel)? == cache.hash(source, cancel)?,
            )
        } else if fs::symlink_metadata(&destination).is_ok() {
            // A symlink or directory is a conflict, even if its target matches.
            let metadata = fs::symlink_metadata(&destination)?;
            Some(
                metadata.is_file()
                    && metadata.len() == bytes
                    && cache.hash(&destination, cancel)? == cache.hash(source, cancel)?,
            )
        } else {
            None
        };
        match matches {
            None => {
                // Nothing at the natural destination. The photo may still have been
                // imported earlier into a custom-named folder next to it.
                if suffix == 0
                    && let Some(found) = find_in_variants(
                        directory, natural, file_name, source, bytes, variants, cache, cancel,
                    )?
                {
                    reserved.insert(
                        found.clone(),
                        Reserved {
                            source: source.to_owned(),
                            bytes,
                        },
                    );
                    return Ok((found, Action::Skip));
                }
                break;
            }
            Some(true) => {
                action = Action::Skip;
                break;
            }
            Some(false) => {
                suffix += 1;
                let stem = original.file_stem().unwrap().to_string_lossy();
                let ext = original.extension().unwrap().to_string_lossy();
                let digest = cache.hash(source, cancel)?.to_hex();
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
    reserved.insert(
        destination.clone(),
        Reserved {
            source: source.to_owned(),
            bytes,
        },
    );
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
    let mut reserved: HashMap<PathBuf, Reserved> = HashMap::new();
    let mut variants = VariantIndex::new(&plan.archive);
    // Keep stable destinations first so remapped photos see them as reserved.
    for photo in &plan.photos {
        let directory =
            destination_directory(&plan.archive, &photo.layout_relative, segment_insert)?;
        if photo.destination.parent() == Some(directory.as_path()) {
            reserved.insert(
                photo.destination.clone(),
                Reserved {
                    source: photo.source.clone(),
                    bytes: photo.bytes,
                },
            );
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
            &plan.archive.join(&photo.layout_relative),
            file_name,
            &photo.source,
            photo.bytes,
            &mut reserved,
            &mut variants,
            &plan.cache,
            cancel,
        )?;
        photo.hash = plan.cache.peek(&photo.source, photo.bytes, photo.modified);
        photo.destination = destination;
        photo.action = action;
    }
    // A custom folder can land on a folder that already exists; existing folders
    // are always used, so they can no longer be deselected.
    let new_folders = plan.new_folder_layouts();
    plan.excluded.retain(|layout| new_folders.contains(layout));
    Ok(())
}

fn copy_photo(
    photo: &Photo,
    archive: &Path,
    cache: &ScanCache,
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
        if metadata.is_file() && metadata.len() == photo.bytes {
            let expected = match photo.hash {
                Some(hash) => hash,
                None => cache.hash(&photo.source, cancel)?,
            };
            if cache.hash(&photo.destination, cancel)? == expected {
                return Ok(false);
            }
        }
        bail!(
            "Destination changed after preview: {}. Refresh the preview.",
            photo.destination.display()
        );
    }
    let mut source = File::open(&photo.source)?;
    let source_metadata = source.metadata()?;
    if source_metadata.len() != photo.bytes || source_metadata.modified()? != photo.modified {
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
    let digest = hasher.finalize();
    let source_metadata = source.metadata()?;
    if bytes != photo.bytes
        || source_metadata.modified()? != photo.modified
        || photo.hash.is_some_and(|expected| expected != digest)
    {
        bail!("Source changed since preview; incomplete copy discarded");
    }
    filetime::set_file_mtime(
        output.path(),
        filetime::FileTime::from_last_modification_time(&source.metadata()?),
    )?;
    output.as_file().sync_all()?;
    if hash_file(output.path(), cancel)? != digest {
        bail!("Copy verification failed");
    }
    cancelled(cancel)?;
    // Uses an exclusive rename where supported, with a no-overwrite hard-link fallback.
    output.persist_noclobber(&photo.destination).map_err(|error| error.error)
        .context("Cannot publish photo without overwriting; check the archive filesystem and refresh the preview")?;
    // Both ends are now known, so a later scan can recognise this photo as a
    // duplicate without reading either file again.
    cache.remember(&photo.source, photo.bytes, photo.modified, digest);
    if let Ok((len, modified)) = ScanCache::stamp(&photo.destination) {
        cache.remember(&photo.destination, len, modified, digest);
    }
    Ok(true)
}

pub fn execute_plan(plan: Plan, tx: &Sender<WorkerEvent>, cancel: Arc<AtomicBool>) -> Summary {
    let mut summary = Summary::default();
    let total = plan.copy_count();
    let total_bytes = plan.bytes();
    let mut completed_bytes = 0;
    let mut done = 0;
    for photo in &plan.photos {
        if plan.is_excluded(photo) {
            continue;
        }
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
            // Duplicates were proven identical during the scan and cost nothing here:
            // unchanged files are confirmed from the cache instead of being re-read.
            (|| -> Result<bool> {
                let expected = photo
                    .hash
                    .context("Duplicate has no recorded checksum; refresh the preview")?;
                if plan.cache.hash(&photo.source, &cancel)? != expected {
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
                    && plan.cache.hash(&photo.destination, &cancel)? == expected
                {
                    Ok(false)
                } else {
                    bail!("Duplicate destination changed since preview; refresh the preview");
                }
            })()
        } else {
            copy_photo(photo, &plan.archive, &plan.cache, &cancel, |bytes| {
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
        if photo.action == Action::Skip {
            continue;
        }
        done += 1;
        completed_bytes += photo.bytes;
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
            modified: fs::metadata(&source).unwrap().modified().unwrap(),
            hash: Some(blake3::hash(&contents)),
            action: Action::Copy,
            fallback: false,
        };
        let cancel = AtomicBool::new(false);
        let result = copy_photo(&photo, &archive, &ScanCache::default(), &cancel, |_| {
            cancel.store(true, Ordering::Relaxed)
        });
        assert!(result.is_err());
        assert!(!photo.destination.exists());
        assert_eq!(fs::read_dir(archive).unwrap().count(), 0);
        assert_eq!(fs::read(source).unwrap(), contents);
    }
}
