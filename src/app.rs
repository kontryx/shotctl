use std::{
    path::{Path, PathBuf},
    process::Command,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Context, Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{DefaultTerminal, widgets::TableState};

use crate::{
    config::{Config, DateBasis, expand_path},
    directories::{self, Completion, DirectoryBrowser},
    import::{self, FolderSummary, Plan, ScanCache, SegmentInsert, Summary, WorkerEvent},
    media, ui,
};

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Sources,
    Settings,
    Scanning,
    Preview,
    Importing,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CustomFolderScope {
    SelectedFolder,
    AllNewFolders,
}

impl CustomFolderScope {
    fn next(self) -> Self {
        match self {
            Self::SelectedFolder => Self::AllNewFolders,
            Self::AllNewFolders => Self::SelectedFolder,
        }
    }

    fn prev(self) -> Self {
        self.next()
    }
}

pub struct App {
    pub config: Config,
    pub config_path: PathBuf,
    pub first_run: bool,
    pub draft: Config,
    pub archive_input: String,
    pub field: usize,
    pub screen: Screen,
    pub sources: Vec<PathBuf>,
    pub source_state: TableState,
    pub plan: Option<Plan>,
    pub preview_state: TableState,
    pub show_files: bool,
    pub segment_insert: Option<SegmentInsert>,
    pub custom_folder_input: Option<String>,
    pub custom_folder_scope: CustomFolderScope,
    /// Layout-relative new folders shown in the custom-folder picker.
    pub custom_folder_choices: Vec<PathBuf>,
    /// Which `custom_folder_choices` entries are checked.
    pub custom_folder_checked: Vec<bool>,
    /// Cursor index inside the custom-folder picker list.
    pub custom_folder_cursor: usize,
    pub summary: Option<Summary>,
    pub import_archive: Option<PathBuf>,
    /// Absolute paths of the new folders the last import was set up to create.
    import_new_folders: Vec<PathBuf>,
    pub message: String,
    pub error: bool,
    pub help: bool,
    pub confirming: bool,
    pub source_input: Option<String>,
    pub directory_browser: Option<DirectoryBrowser>,
    pub current: String,
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub total_bytes: u64,
    pub tick: usize,
    pub detail_scroll: u16,
    /// Hashes and EXIF dates learned so far; lets a repeat scan skip unchanged files.
    scan_cache: Arc<ScanCache>,
    cache_path: PathBuf,
    receiver: Option<Receiver<WorkerEvent>>,
    worker: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    quit: bool,
}

impl App {
    pub fn new(config_path: PathBuf, source: Option<PathBuf>) -> Result<Self> {
        let cache_path = config_path.with_file_name("hash-cache.tsv");
        let saved = Config::load(&config_path)?;
        let first_run = saved.is_none();
        let config = saved.unwrap_or_default();
        let mut sources = media::discover_sources();
        if let Some(source) = source {
            let source = source.canonicalize()?;
            if !source.is_dir() {
                bail!("--source must point to a directory");
            }
            sources.retain(|p| p != &source);
            sources.insert(0, source);
        }
        Ok(Self {
            archive_input: config.archive.display().to_string(),
            draft: config.clone(),
            config,
            config_path,
            first_run,
            field: 0,
            screen: if first_run {
                Screen::Settings
            } else {
                Screen::Sources
            },
            source_state: TableState::default().with_selected(if sources.is_empty() {
                None
            } else {
                Some(0)
            }),
            sources,
            plan: None,
            preview_state: TableState::default().with_selected(Some(0)),
            show_files: false,
            segment_insert: None,
            custom_folder_input: None,
            custom_folder_scope: CustomFolderScope::AllNewFolders,
            custom_folder_choices: Vec::new(),
            custom_folder_checked: Vec::new(),
            custom_folder_cursor: 0,
            summary: None,
            import_archive: None,
            import_new_folders: Vec::new(),
            message: String::new(),
            error: false,
            help: false,
            confirming: false,
            source_input: None,
            directory_browser: None,
            current: String::new(),
            done: 0,
            total: 0,
            bytes: 0,
            total_bytes: 0,
            tick: 0,
            detail_scroll: 0,
            scan_cache: Arc::new(ScanCache::load(&cache_path)),
            cache_path,
            receiver: None,
            worker: None,
            cancel: Arc::new(AtomicBool::new(false)),
            quit: false,
        })
    }

    pub fn run(&mut self, terminal: &mut DefaultTerminal) -> Result<()> {
        while !self.quit {
            self.poll_worker();
            terminal.draw(|frame| ui::draw(frame, self))?;
            if event::poll(Duration::from_millis(100))?
                && let Event::Key(key) = event::read()?
                && key.kind == KeyEventKind::Press
            {
                self.key(key);
            }
            self.tick = self.tick.wrapping_add(1);
        }
        Ok(())
    }

    fn poll_worker(&mut self) {
        let events: Vec<_> = self
            .receiver
            .as_ref()
            .map(|rx| rx.try_iter().collect())
            .unwrap_or_default();
        for event in events {
            match event {
                WorkerEvent::Scanning { files, current } => {
                    self.done = files;
                    self.current = current;
                }
                WorkerEvent::Planned(plan) => {
                    let empty = plan.photos.is_empty();
                    // Store the plan first: the message depends on its new folders.
                    self.plan = Some(plan);
                    self.scan_cache.save(&self.cache_path);
                    self.message = if empty {
                        "No supported photos found. Press Esc to choose another source.".into()
                    } else if self.segment_insert.is_some() {
                        "Custom folder active. Press n to change or clear, Enter to confirm.".into()
                    } else {
                        self.review_message()
                    };
                    self.error = false;
                    self.screen = Screen::Preview;
                    self.show_files = false;
                    self.preview_state = TableState::default()
                        .with_selected(Some(self.first_selectable_preview_row().unwrap_or(0)));
                    self.finish_worker();
                }
                WorkerEvent::Progress {
                    done,
                    total,
                    bytes,
                    total_bytes,
                    current,
                } => {
                    self.done = done;
                    self.total = total;
                    self.bytes = bytes;
                    self.total_bytes = total_bytes;
                    self.current = current;
                }
                WorkerEvent::Finished(summary) => {
                    self.scan_cache.save(&self.cache_path);
                    self.screen = Screen::Done;
                    self.summary = Some(summary);
                    self.detail_scroll = 0;
                    self.message.clear();
                    self.finish_worker();
                }
                WorkerEvent::Failed(error) => {
                    self.screen = Screen::Sources;
                    self.set_error(error);
                    self.finish_worker();
                }
            }
        }
        if self
            .worker
            .as_ref()
            .is_some_and(|worker| worker.is_finished())
            && self.receiver.is_some()
        {
            // A panic is reported instead of leaving the UI stuck on a busy screen.
            let worker = self.worker.take().unwrap();
            if worker.join().is_err() {
                self.receiver = None;
                self.screen = Screen::Sources;
                self.set_error(
                    "The background operation failed unexpectedly. No source files were removed."
                        .into(),
                );
            } else {
                self.worker = None;
            }
        }
    }

    fn finish_worker(&mut self) {
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        self.receiver = None;
    }

    fn set_error(&mut self, message: String) {
        self.message = message;
        self.error = true;
    }

    fn clear_message(&mut self) {
        self.message.clear();
        self.error = false;
    }

    pub fn selected_source(&self) -> Option<&PathBuf> {
        self.source_state
            .selected()
            .and_then(|index| self.sources.get(index))
    }

    fn scan(&mut self) {
        let Some(source) = self.selected_source().cloned() else {
            self.set_error("Choose a source, or press s to enter a directory.".into());
            return;
        };
        if let Err(e) = import::validate_roots(&source, &self.config.archive) {
            self.set_error(format!("{e:#}"));
            return;
        }
        self.clear_message();
        self.screen = Screen::Scanning;
        self.done = 0;
        self.current.clear();
        self.plan = None;
        self.import_archive = None;
        self.import_new_folders.clear();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let config = self.config.clone();
        let segment_insert = self.segment_insert.clone();
        let cache = self.scan_cache.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.worker = Some(thread::spawn(move || {
            let event = match import::build_plan_cached(
                &source,
                &config,
                segment_insert.as_ref(),
                &tx,
                &cancel,
                &cache,
            ) {
                Ok(plan) => WorkerEvent::Planned(plan),
                Err(e) => WorkerEvent::Failed(format!("{e:#}")),
            };
            let _ = tx.send(event);
        }));
    }

    fn start_import(&mut self) {
        let Some(plan) = self.plan.take() else { return };
        self.import_archive = Some(plan.archive.clone());
        self.import_new_folders = plan
            .folders()
            .into_iter()
            .filter(|folder| !folder.exists && folder.included && folder.copy > 0)
            .map(|folder| plan.archive.join(folder.path))
            .collect();
        self.confirming = false;
        self.screen = Screen::Importing;
        self.clear_message();
        self.total = plan.copy_count();
        self.total_bytes = plan.bytes();
        self.done = 0;
        self.bytes = 0;
        self.current.clear();
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.worker = Some(thread::spawn(move || {
            let summary = import::execute_plan(plan, &tx, cancel);
            let _ = tx.send(WorkerEvent::Finished(summary));
        }));
    }

    fn settings(&mut self) {
        self.draft = self.config.clone();
        self.archive_input = self.config.archive.display().to_string();
        self.field = 0;
        self.screen = Screen::Settings;
        self.plan = None;
        self.clear_custom_folder();
        self.clear_message();
    }

    fn clear_custom_folder(&mut self) {
        self.segment_insert = None;
        self.custom_folder_input = None;
        self.custom_folder_scope = CustomFolderScope::AllNewFolders;
        self.custom_folder_choices.clear();
        self.custom_folder_checked.clear();
        self.custom_folder_cursor = 0;
    }

    pub fn completed_archive(&self) -> Option<&Path> {
        if self.screen == Screen::Done
            && self
                .summary
                .as_ref()
                .is_some_and(|summary| !summary.cancelled && summary.failed == 0)
        {
            self.import_archive.as_deref()
        } else {
            None
        }
    }

    /// New folders this import created, once it has finished cleanly.
    pub fn created_folders(&self) -> Vec<&Path> {
        if self.completed_archive().is_none() {
            return Vec::new();
        }
        self.import_new_folders
            .iter()
            .map(PathBuf::as_path)
            .filter(|path| path.is_dir())
            .collect()
    }

    /// The one folder that was created, when exactly one was.
    pub fn created_folder(&self) -> Option<&Path> {
        match self.created_folders().as_slice() {
            [only] => Some(only),
            _ => None,
        }
    }

    /// The folder that contains everything that was created: the parent of a single
    /// new folder, or the deepest folder shared by several. Never the archive itself,
    /// which has its own shortcut.
    pub fn created_parent(&self) -> Option<PathBuf> {
        let archive = self.completed_archive()?;
        let folders = self.created_folders();
        let parent = match folders.as_slice() {
            [] => return None,
            [only] => only.parent()?.to_path_buf(),
            [first, rest @ ..] => {
                let mut shared: PathBuf = first.to_path_buf();
                for other in rest {
                    while !other.starts_with(&shared) {
                        if !shared.pop() {
                            return None;
                        }
                    }
                }
                shared
            }
        };
        (parent.starts_with(archive) && parent != archive).then_some(parent)
    }

    /// `o` in the folder preview: open the highlighted folder if it is already archived.
    fn open_selected_folder(&mut self) {
        let Some(folder) = self.selected_preview_folder() else {
            return;
        };
        if !folder.exists {
            self.message = "That folder doesn't exist yet; it is created when you import.".into();
            self.error = false;
            return;
        }
        let Some(archive) = self.plan.as_ref().map(|plan| plan.archive.clone()) else {
            return;
        };
        self.open_in_file_manager(&archive.join(&folder.path), "the folder");
    }

    fn open_created_folder(&mut self) {
        let Some(folder) = self.created_folder().map(Path::to_owned) else {
            return;
        };
        self.open_in_file_manager(&folder, "the new folder");
    }

    fn open_created_parent(&mut self) {
        let Some(parent) = self.created_parent() else {
            return;
        };
        self.open_in_file_manager(&parent, "the parent folder");
    }

    fn open_in_file_manager(&mut self, path: &Path, what: &str) {
        match open_folder(path) {
            Ok(()) => {
                self.clear_message();
                self.message = format!("Opened {what}.");
            }
            Err(error) => self.set_error(format!("Could not open {what}: {error:#}")),
        }
    }

    fn open_archive(&mut self) {
        let Some(archive) = self.completed_archive() else {
            return;
        };
        match open_folder(archive) {
            Ok(()) => {
                self.clear_message();
                self.message = "Opened the archive folder.".into();
            }
            Err(error) => self.set_error(format!("Could not open the archive folder: {error:#}")),
        }
    }

    fn save_settings(&mut self) {
        self.draft.archive = expand_path(&self.archive_input);
        if let Some(source) = self.selected_source()
            && let Err(e) = import::validate_roots(source, &self.draft.archive)
        {
            self.set_error(format!("{e:#}"));
            return;
        }
        match self.draft.save(&self.config_path) {
            Ok(()) => {
                self.config = self.draft.clone();
                self.first_run = false;
                self.screen = Screen::Sources;
                self.clear_message();
                self.message =
                    "Configuration saved. Choose a source and press Enter to preview.".into();
            }
            Err(e) => self.set_error(format!("{e:#}")),
        }
    }

    fn refresh(&mut self) {
        self.sources = media::discover_sources();
        self.source_state = TableState::default().with_selected(if self.sources.is_empty() {
            None
        } else {
            Some(0)
        });
        self.plan = None;
        self.clear_message();
    }

    fn key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('c') {
            if matches!(self.screen, Screen::Scanning | Screen::Importing) {
                self.cancel_operation();
            } else {
                self.quit = true;
            }
            return;
        }
        if self.help {
            if matches!(
                key.code,
                KeyCode::Esc | KeyCode::Char('?') | KeyCode::Char('q') | KeyCode::Enter
            ) {
                self.help = false;
            }
            return;
        }
        if self.directory_browser.is_some() {
            self.directory_key(key);
            return;
        }
        if self.source_input.is_some() {
            self.source_key(key);
            return;
        }
        if self.confirming {
            match key.code {
                KeyCode::Enter | KeyCode::Char('y') => self.start_import(),
                KeyCode::Esc | KeyCode::Char('n') => self.confirming = false,
                _ => {}
            }
            return;
        }
        if self.custom_folder_input.is_some() {
            self.custom_folder_key(key);
            return;
        }
        if self.screen == Screen::Settings {
            self.settings_key(key);
            return;
        }
        if key.code == KeyCode::Char('?') {
            self.help = true;
            return;
        }
        if matches!(self.screen, Screen::Scanning | Screen::Importing) {
            if matches!(key.code, KeyCode::Esc | KeyCode::Char('q')) {
                self.cancel_operation();
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.quit = true,
            KeyCode::Char('c') => self.settings(),
            KeyCode::Char('s') => {
                self.source_input = Some(String::new());
                self.clear_message();
            }
            KeyCode::Esc if self.screen == Screen::Preview && self.show_files => {
                self.show_files = false;
                self.preview_state = TableState::default()
                    .with_selected(Some(self.first_selectable_preview_row().unwrap_or(0)));
            }
            KeyCode::Esc => {
                self.screen = Screen::Sources;
                self.plan = None;
                // A custom folder is a one-time choice for this preview.
                self.clear_custom_folder();
                self.clear_message();
            }
            _ => match self.screen {
                Screen::Sources => match key.code {
                    KeyCode::Down | KeyCode::Char('j') => {
                        move_selection(&mut self.source_state, self.sources.len(), 1)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        move_selection(&mut self.source_state, self.sources.len(), -1)
                    }
                    KeyCode::Char('r') => self.refresh(),
                    KeyCode::Enter | KeyCode::Char('i') => self.scan(),
                    _ => {}
                },
                Screen::Preview => match key.code {
                    KeyCode::Char('f') | KeyCode::Tab => {
                        self.show_files = !self.show_files;
                        self.preview_state =
                            TableState::default().with_selected(Some(if self.show_files {
                                0
                            } else {
                                self.first_selectable_preview_row().unwrap_or(0)
                            }));
                    }
                    KeyCode::Char('n') if !self.show_files => {
                        self.open_custom_folder_dialog();
                    }
                    KeyCode::Char('x') if !self.show_files && self.segment_insert.is_some() => {
                        self.clear_applied_custom_folder();
                    }
                    KeyCode::Char(' ') if !self.show_files => self.toggle_selected_new_folder(),
                    KeyCode::Char('a') if !self.show_files => self.toggle_all_new_folders(),
                    KeyCode::Char('o') if !self.show_files => self.open_selected_folder(),
                    KeyCode::Enter | KeyCode::Char('i') => self.request_confirmation(),
                    KeyCode::Char('r') => self.scan(),
                    KeyCode::Down | KeyCode::Char('j') => self.move_preview(1),
                    KeyCode::Up | KeyCode::Char('k') => self.move_preview(-1),
                    KeyCode::PageDown => self.move_preview(10),
                    KeyCode::PageUp => self.move_preview(-10),
                    _ => {}
                },
                Screen::Done => match key.code {
                    KeyCode::Char('o') => self.open_archive(),
                    KeyCode::Char('f') => self.open_created_folder(),
                    KeyCode::Char('p') => self.open_created_parent(),
                    KeyCode::Enter | KeyCode::Char('r') => {
                        self.screen = Screen::Sources;
                        self.clear_custom_folder();
                        self.clear_message();
                    }
                    KeyCode::Down | KeyCode::Char('j') => {
                        self.detail_scroll = self.detail_scroll.saturating_add(1)
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        self.detail_scroll = self.detail_scroll.saturating_sub(1)
                    }
                    _ => {}
                },
                _ => {}
            },
        }
    }

    fn review_message(&self) -> String {
        let has_new = self
            .plan
            .as_ref()
            .is_some_and(|plan| !plan.new_folder_layouts().is_empty());
        if has_new {
            "Review folders. Space selects a new folder, a selects all, n adds a custom folder, Enter confirms."
                .into()
        } else {
            "Review folders. Press n for a custom folder, or Enter to confirm.".into()
        }
    }

    fn request_confirmation(&mut self) {
        let Some(plan) = &self.plan else { return };
        if plan.photos.is_empty() {
            return;
        }
        if plan.copy_count() == 0 {
            let message = if plan.excluded_count() > 0 {
                "No new folders are selected, so nothing would be imported. Press a to select them, or Esc to go back."
            } else {
                "Nothing to import: every photo is already in the archive. Press Esc to choose another source."
            };
            self.set_error(message.into());
            return;
        }
        self.clear_message();
        self.confirming = true;
    }

    fn new_folder_selection_message(&self) -> String {
        let Some(plan) = &self.plan else {
            return String::new();
        };
        let total = plan.new_folder_layouts().len();
        let included = total - plan.excluded.len().min(total);
        match (included, total) {
            (_, 0) => "No new folders will be created.".into(),
            (n, t) if n == t => format!("All {t} new folders will be created."),
            (0, t) => format!("None of the {t} new folders will be created."),
            (n, t) => format!("{n} of {t} new folders will be created."),
        }
    }

    /// Space: include or exclude the highlighted new folder.
    fn toggle_selected_new_folder(&mut self) {
        let Some(folder) = self.selected_preview_folder() else {
            return;
        };
        if folder.exists {
            self.message =
                "Existing folders are always used; only new folders can be selected.".into();
            self.error = false;
            return;
        }
        if let Some(plan) = self.plan.as_mut() {
            plan.toggle_new_folder(&folder.layout);
        }
        self.message = self.new_folder_selection_message();
        self.error = false;
    }

    /// `a`: create every new folder, or none of them if all are already selected.
    fn toggle_all_new_folders(&mut self) {
        let Some(plan) = self.plan.as_mut() else {
            return;
        };
        let total = plan.new_folder_layouts().len();
        if total == 0 {
            return;
        }
        let all_included = plan.excluded.is_empty();
        plan.set_all_new_folders(!all_included);
        self.message = self.new_folder_selection_message();
        self.error = false;
    }

    fn preview_len(&self) -> usize {
        self.plan
            .as_ref()
            .map(|p| {
                if self.show_files {
                    p.photos.len()
                } else {
                    ui::preview_folder_rows(p).len()
                }
            })
            .unwrap_or(0)
    }

    fn first_selectable_preview_row(&self) -> Option<usize> {
        let plan = self.plan.as_ref()?;
        if self.show_files {
            return (!plan.photos.is_empty()).then_some(0);
        }
        ui::preview_folder_rows(plan)
            .iter()
            .position(|row| row.is_folder())
    }

    fn move_preview(&mut self, delta: isize) {
        let len = self.preview_len();
        if len == 0 {
            return;
        }
        if self.show_files {
            move_selection(&mut self.preview_state, len, delta);
            return;
        }
        let Some(plan) = &self.plan else { return };
        let rows = ui::preview_folder_rows(plan);
        let current = self.preview_state.selected().unwrap_or(0);
        let mut index = current as isize;
        let step = if delta == 0 {
            0
        } else if delta > 0 {
            1
        } else {
            -1
        };
        let mut remaining = delta.unsigned_abs();
        while remaining > 0 && step != 0 {
            let next = index + step;
            if next < 0 || next as usize >= rows.len() {
                break;
            }
            index = next;
            if rows[index as usize].is_folder() {
                remaining -= 1;
            }
        }
        if !rows[index as usize].is_folder()
            && let Some(fallback) = rows.iter().position(|row| row.is_folder())
        {
            index = fallback as isize;
        }
        self.preview_state.select(Some(index as usize));
    }

    fn open_custom_folder_dialog(&mut self) {
        self.custom_folder_scope = match &self.segment_insert {
            Some(SegmentInsert::Selected { .. }) => CustomFolderScope::SelectedFolder,
            Some(SegmentInsert::AllNew { .. }) | None => CustomFolderScope::AllNewFolders,
        };
        self.custom_folder_input = Some(
            self.segment_insert
                .as_ref()
                .map(|insert| insert.name().to_owned())
                .unwrap_or_default(),
        );
        self.refresh_custom_folder_choices();
        self.clear_message();
    }

    fn refresh_custom_folder_choices(&mut self) {
        let choices = self.new_folder_layout_paths();
        let previously_checked: std::collections::BTreeSet<PathBuf> = match &self.segment_insert {
            Some(SegmentInsert::Selected { folders, .. }) => folders.iter().cloned().collect(),
            _ => self.selected_new_folder_layout_path().into_iter().collect(),
        };
        let mut checked: Vec<bool> = choices
            .iter()
            .map(|path| previously_checked.contains(path))
            .collect();
        if !checked.iter().any(|c| *c) {
            if let Some(selected) = self.selected_new_folder_layout_path() {
                if let Some(index) = choices.iter().position(|path| path == &selected) {
                    checked[index] = true;
                }
            } else if let Some(first) = checked.first_mut() {
                *first = true;
            }
        }
        self.custom_folder_cursor = checked
            .iter()
            .position(|value| *value)
            .unwrap_or(0)
            .min(choices.len().saturating_sub(1));
        self.custom_folder_choices = choices;
        self.custom_folder_checked = checked;
    }

    pub fn new_folder_layout_paths(&self) -> Vec<PathBuf> {
        let Some(plan) = &self.plan else {
            return Vec::new();
        };
        let mut paths = Vec::new();
        for folder in plan.folders() {
            let natural = self.natural_layout_path(&folder.path);
            let is_new = !folder.exists && !plan.archive.join(&natural).exists();
            if is_new && !paths.iter().any(|path| path == &natural) {
                paths.push(natural);
            }
        }
        paths
    }

    pub fn checked_custom_folders(&self) -> Vec<PathBuf> {
        self.custom_folder_choices
            .iter()
            .zip(self.custom_folder_checked.iter())
            .filter(|(_, checked)| **checked)
            .map(|(path, _)| path.clone())
            .collect()
    }

    pub fn selected_preview_folder(&self) -> Option<FolderSummary> {
        let plan = self.plan.as_ref()?;
        if self.show_files {
            return None;
        }
        let index = self.preview_state.selected()?;
        match ui::preview_folder_rows(plan).get(index)? {
            ui::PreviewFolderRow::Folder(folder) => Some(folder.clone()),
            ui::PreviewFolderRow::Header { .. } => None,
        }
    }

    /// Natural layout path for the selected new folder (strips parent / insert segment).
    pub fn selected_new_folder_layout_path(&self) -> Option<PathBuf> {
        let folder = self.selected_preview_folder()?;
        if folder.exists {
            return None;
        }
        Some(self.natural_layout_path(&folder.path))
    }

    fn natural_layout_path(&self, path: &Path) -> PathBuf {
        let mut path = path.to_owned();
        if let Some(name) = self.segment_insert.as_ref().map(SegmentInsert::name)
            && let (Some(leaf), Some(parent)) = (path.file_name(), path.parent())
            && parent.file_name().is_some_and(|n| n == name)
        {
            let grand = parent.parent().unwrap_or_else(|| Path::new(""));
            path = grand.join(leaf);
        }
        path
    }

    fn custom_folder_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.custom_folder_input = None;
                self.clear_message();
            }
            // Every printable key, including `x` and Space, belongs to the name; the
            // dialog's commands use keys that can't be part of a folder name.
            KeyCode::Char('x') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.clear_applied_custom_folder();
            }
            KeyCode::Left => {
                self.custom_folder_scope = self.custom_folder_scope.prev();
                self.clear_message();
            }
            KeyCode::Right => {
                self.custom_folder_scope = self.custom_folder_scope.next();
                self.clear_message();
            }
            KeyCode::Up
                if self.custom_folder_scope == CustomFolderScope::SelectedFolder
                    && !self.custom_folder_choices.is_empty() =>
            {
                self.custom_folder_cursor = self.custom_folder_cursor.saturating_sub(1);
                self.clear_message();
            }
            KeyCode::Down
                if self.custom_folder_scope == CustomFolderScope::SelectedFolder
                    && !self.custom_folder_choices.is_empty() =>
            {
                let max = self.custom_folder_choices.len().saturating_sub(1);
                self.custom_folder_cursor = (self.custom_folder_cursor + 1).min(max);
                self.clear_message();
            }
            KeyCode::Tab
                if self.custom_folder_scope == CustomFolderScope::SelectedFolder
                    && !self.custom_folder_checked.is_empty() =>
            {
                let index = self.custom_folder_cursor;
                if let Some(checked) = self.custom_folder_checked.get_mut(index) {
                    *checked = !*checked;
                }
                self.clear_message();
            }
            KeyCode::Enter => {
                let input = self.custom_folder_input.as_ref().unwrap().clone();
                let trimmed = input.trim();
                if trimmed.is_empty() {
                    self.segment_insert = None;
                    self.custom_folder_input = None;
                    self.apply_custom_folder_remap();
                    return;
                }
                match import::validate_one_time_folder(trimmed) {
                    Ok(name) => match self.custom_folder_scope {
                        CustomFolderScope::SelectedFolder => {
                            let folders = self.checked_custom_folders();
                            if folders.is_empty() {
                                self.set_error(
                                    "Select at least one new folder with Tab, then Apply.".into(),
                                );
                                return;
                            }
                            self.segment_insert = Some(SegmentInsert::Selected {
                                folders,
                                name: name.to_owned(),
                            });
                            self.custom_folder_input = None;
                            self.apply_custom_folder_remap();
                        }
                        CustomFolderScope::AllNewFolders => {
                            self.segment_insert = Some(SegmentInsert::AllNew {
                                name: name.to_owned(),
                            });
                            self.custom_folder_input = None;
                            self.apply_custom_folder_remap();
                        }
                    },
                    Err(error) => self.set_error(format!("{error:#}")),
                }
            }
            KeyCode::Backspace => {
                self.custom_folder_input.as_mut().unwrap().pop();
                self.clear_message();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.custom_folder_input.as_mut().unwrap().clear();
                self.clear_message();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.custom_folder_input.as_mut().unwrap().push(c);
                self.clear_message();
            }
            _ => {}
        }
    }

    fn clear_applied_custom_folder(&mut self) {
        self.segment_insert = None;
        self.custom_folder_input = None;
        self.custom_folder_choices.clear();
        self.custom_folder_checked.clear();
        self.custom_folder_cursor = 0;
        self.apply_custom_folder_remap();
        if !self.error {
            self.message =
                "Custom folder removed. Press n to add one again, or Enter to confirm.".into();
        }
    }

    /// Remap the current preview in place — no card rescan or source rehash.
    fn apply_custom_folder_remap(&mut self) {
        if self.plan.is_none() {
            self.scan();
            return;
        }
        let segment = self.segment_insert.clone();
        let result = import::remap_plan_destinations(
            self.plan.as_mut().unwrap(),
            segment.as_ref(),
            &AtomicBool::new(false),
        );
        match result {
            Ok(()) => {
                self.show_files = false;
                let selected = self.first_selectable_preview_row().unwrap_or(0);
                self.preview_state = TableState::default().with_selected(Some(selected));
                self.message = if self.segment_insert.is_some() {
                    "Custom folder active. Press n to change or clear, Enter to confirm.".into()
                } else {
                    self.review_message()
                };
                self.error = false;
            }
            Err(error) => {
                self.set_error(format!("{error:#}"));
                self.custom_folder_input = Some(
                    self.segment_insert
                        .as_ref()
                        .map(|insert| insert.name().to_owned())
                        .unwrap_or_default(),
                );
            }
        }
    }

    fn cancel_operation(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.message = "Cancelling safely; completed copies stay in the archive.".into();
    }

    fn source_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('b') {
            self.browse_directory();
            return;
        }
        match key.code {
            KeyCode::Tab => self.complete_directory(),
            KeyCode::Esc => {
                self.source_input = None;
                self.clear_message();
            }
            KeyCode::Enter => {
                let path = expand_path(self.source_input.as_ref().unwrap());
                match path.canonicalize() {
                    Ok(path) if path.is_dir() => {
                        if let Err(e) = import::validate_roots(&path, &self.config.archive) {
                            self.set_error(format!("{e:#}"));
                            return;
                        }
                        self.sources.retain(|p| p != &path);
                        self.sources.insert(0, path);
                        self.source_state = TableState::default().with_selected(Some(0));
                        self.source_input = None;
                        self.screen = Screen::Sources;
                        self.plan = None;
                        self.clear_custom_folder();
                        self.clear_message();
                    }
                    _ => self.set_error(
                        "Enter a readable directory. You can use ~/ for your home directory."
                            .into(),
                    ),
                }
            }
            KeyCode::Backspace => {
                self.source_input.as_mut().unwrap().pop();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.source_input.as_mut().unwrap().clear()
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.source_input.as_mut().unwrap().push(c)
            }
            _ => {}
        }
    }

    fn settings_key(&mut self, key: KeyEvent) {
        if key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char('s') {
            self.save_settings();
            return;
        }
        if self.field == 0
            && key.modifiers.contains(KeyModifiers::CONTROL)
            && key.code == KeyCode::Char('b')
        {
            self.browse_directory();
            return;
        }
        match key.code {
            KeyCode::Esc => {
                if self.first_run {
                    self.quit = true;
                } else {
                    self.screen = Screen::Sources;
                    self.clear_message();
                }
            }
            KeyCode::Tab if self.field == 0 => self.complete_directory(),
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % 5,
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + 4) % 5,
            KeyCode::Enter if self.field == 4 => self.save_settings(),
            KeyCode::Enter if self.field == 0 => self.browse_directory(),
            KeyCode::Left | KeyCode::Right | KeyCode::Enter | KeyCode::Char(' ')
                if self.field > 0 =>
            {
                match self.field {
                    1 => {
                        self.draft.layout = if key.code == KeyCode::Left {
                            self.draft.layout.next().next()
                        } else {
                            self.draft.layout.next()
                        }
                    }
                    2 => {
                        self.draft.grouping = if key.code == KeyCode::Left {
                            self.draft.grouping.next().next()
                        } else {
                            self.draft.grouping.next()
                        }
                    }
                    3 => {
                        self.draft.date_basis = match self.draft.date_basis {
                            DateBasis::Capture => DateBasis::Import,
                            DateBasis::Import => DateBasis::Capture,
                        }
                    }
                    _ => {}
                }
            }
            KeyCode::Backspace if self.field == 0 => {
                self.archive_input.pop();
            }
            KeyCode::Char('u')
                if self.field == 0 && key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.archive_input.clear()
            }
            KeyCode::Char(c)
                if self.field == 0 && !key.modifiers.contains(KeyModifiers::CONTROL) =>
            {
                self.archive_input.push(c)
            }
            KeyCode::Char('?') => self.help = true,
            _ => {}
        }
    }

    fn directory_input(&self) -> &str {
        self.source_input.as_deref().unwrap_or(&self.archive_input)
    }

    fn use_directory(&mut self, input: String) {
        if let Some(source) = &mut self.source_input {
            *source = input;
        } else {
            self.archive_input = input;
        }
        self.directory_browser = None;
        self.clear_message();
    }

    fn complete_directory(&mut self) {
        match directories::complete(self.directory_input()) {
            Ok(Completion::Completed(input)) => self.use_directory(input),
            Ok(Completion::Choices(browser)) => {
                self.directory_browser = Some(browser);
                self.clear_message();
            }
            Err(error) => self.set_error(format!("{error:#}")),
        }
    }

    fn browse_directory(&mut self) {
        match DirectoryBrowser::open(self.directory_input()) {
            Ok(browser) => {
                self.directory_browser = Some(browser);
                self.clear_message();
            }
            Err(error) => self.set_error(format!("{error:#}")),
        }
    }

    fn directory_key(&mut self, key: KeyEvent) {
        if self
            .directory_browser
            .as_ref()
            .unwrap()
            .new_folder_input
            .is_some()
        {
            self.new_folder_key(key);
            return;
        }
        let browser = self.directory_browser.as_mut().unwrap();
        match key.code {
            KeyCode::Char('n') => {
                browser.new_folder_input = Some(String::new());
                browser.error.clear();
            }
            KeyCode::Esc => {
                self.directory_browser = None;
                self.clear_message();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                move_selection(&mut browser.state, browser.entries.len(), 1);
            }
            KeyCode::Up | KeyCode::Char('k') => {
                move_selection(&mut browser.state, browser.entries.len(), -1);
            }
            KeyCode::PageDown => move_selection(&mut browser.state, browser.entries.len(), 10),
            KeyCode::PageUp => move_selection(&mut browser.state, browser.entries.len(), -10),
            KeyCode::Enter | KeyCode::Right => {
                if let Some(path) = browser.selected().cloned() {
                    browser.navigate(path);
                }
            }
            KeyCode::Left | KeyCode::Backspace => {
                if let Some(parent) = browser.current.parent() {
                    browser.navigate(parent.to_owned());
                }
            }
            KeyCode::Char('h') => {
                if let Some(home) = dirs::home_dir() {
                    browser.navigate(home);
                }
            }
            KeyCode::Char('/') => {
                if let Some(root) = browser.current.ancestors().last() {
                    browser.navigate(root.to_owned());
                }
            }
            KeyCode::Char('.') => browser.toggle_hidden(),
            KeyCode::Tab => {
                if let Some(input) = browser.selected_input() {
                    self.use_directory(input);
                }
            }
            KeyCode::Char(' ') => {
                let path = browser.current.to_str().map(str::to_owned);
                if let Some(path) = path {
                    self.use_directory(path);
                } else {
                    browser.error = "This folder's name cannot be represented as text.".into();
                }
            }
            _ => {}
        }
    }

    fn new_folder_key(&mut self, key: KeyEvent) {
        let browser = self.directory_browser.as_mut().unwrap();
        match key.code {
            KeyCode::Esc => {
                browser.new_folder_input = None;
                browser.error.clear();
            }
            KeyCode::Enter => {
                let name = browser.new_folder_input.as_ref().unwrap().clone();
                match browser.create_folder(&name) {
                    Ok(()) => browser.new_folder_input = None,
                    Err(error) => browser.error = format!("{error:#}"),
                }
            }
            KeyCode::Backspace => {
                browser.new_folder_input.as_mut().unwrap().pop();
                browser.error.clear();
            }
            KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                browser.new_folder_input.as_mut().unwrap().clear();
                browser.error.clear();
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                browser.new_folder_input.as_mut().unwrap().push(c);
                browser.error.clear();
            }
            _ => {}
        }
    }
}

fn open_folder(path: &Path) -> Result<()> {
    if !path.is_dir() {
        bail!("Folder is unavailable: {}", path.display());
    }
    #[cfg(target_os = "macos")]
    let mut command = Command::new("open");
    #[cfg(target_os = "windows")]
    let mut command = Command::new("explorer.exe");
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let mut command = Command::new("xdg-open");
    let output = command
        .arg(path)
        .output()
        .context("Could not launch the file manager")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        bail!(
            "File manager exited with {}: {}",
            output.status,
            detail.trim()
        );
    }
    Ok(())
}

impl Drop for App {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn move_selection(state: &mut TableState, len: usize, delta: isize) {
    if len == 0 {
        state.select(None);
        return;
    }
    let current = state.selected().unwrap_or(0);
    state.select(Some(current.saturating_add_signed(delta).min(len - 1)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Instant};

    fn press(app: &mut App, code: KeyCode) {
        app.key(KeyEvent::new(code, KeyModifiers::NONE));
    }

    fn wait_for(app: &mut App, target: Screen) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while app.screen != target && Instant::now() < deadline {
            app.poll_worker();
            thread::sleep(Duration::from_millis(5));
        }
        assert!(
            app.screen == target,
            "Operation did not reach its expected screen: {}",
            app.message
        );
    }

    #[test]
    fn keyboard_flow_requires_confirmation_before_copying() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        fs::write(card.join("photo.ARW"), b"raw data").unwrap();
        let archive = temp.path().join("archive");
        let config_path = temp.path().join("config.toml");
        let mut app = App::new(config_path.clone(), Some(card.clone())).unwrap();
        assert!(app.first_run && app.screen == Screen::Settings);
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for ch in archive.to_string_lossy().chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.draft.layout, crate::config::Layout::YearMonthDate);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.draft.layout, crate::config::Layout::MonthDate);
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.screen == Screen::Sources && config_path.exists());
        assert!(!app.first_run);
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 2);
        assert_eq!(fs::read_dir(&archive).unwrap().count(), 0);
        press(&mut app, KeyCode::Char('f'));
        assert!(app.show_files);
        press(&mut app, KeyCode::Enter);
        assert!(app.confirming);
        assert_eq!(fs::read_dir(&archive).unwrap().count(), 0);
        press(&mut app, KeyCode::Esc);
        assert!(!app.confirming && app.screen == Screen::Preview);
        assert!(app.show_files);
        let preview_message = app.message.clone();
        press(&mut app, KeyCode::Esc);
        assert!(app.screen == Screen::Preview && !app.show_files);
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 2);
        assert_eq!(app.message, preview_message);
        assert_eq!(
            app.preview_state.selected(),
            Some(app.first_selectable_preview_row().unwrap())
        );
        press(&mut app, KeyCode::Tab);
        assert!(app.show_files);
        press(&mut app, KeyCode::Esc);
        assert!(app.screen == Screen::Preview && !app.show_files);
        press(&mut app, KeyCode::Esc);
        assert!(app.screen == Screen::Sources && app.plan.is_none());
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        wait_for(&mut app, Screen::Done);
        let summary = app.summary.as_ref().unwrap();
        assert_eq!((summary.copied, summary.failed), (2, 0));
        assert_eq!(fs::read_dir(&card).unwrap().count(), 2);
        let imported_archive = archive.canonicalize().unwrap();
        assert_eq!(app.completed_archive(), Some(imported_archive.as_path()));
        app.config.archive = temp.path().join("another archive");
        assert_eq!(app.completed_archive(), Some(imported_archive.as_path()));
        press(&mut app, KeyCode::Enter);
        assert!(app.completed_archive().is_none());
    }

    #[test]
    fn opening_archive_requires_success_and_reports_unavailable_folders() {
        let temp = tempfile::TempDir::new().unwrap();
        let mut app = App::new(temp.path().join("config.toml"), None).unwrap();
        app.import_archive = Some(temp.path().join("missing archive"));
        app.screen = Screen::Done;
        for summary in [
            None,
            Some(Summary {
                cancelled: true,
                ..Summary::default()
            }),
            Some(Summary {
                failed: 1,
                ..Summary::default()
            }),
        ] {
            app.summary = summary;
            assert!(app.completed_archive().is_none());
            press(&mut app, KeyCode::Char('o'));
            assert!(app.message.is_empty());
        }
        app.summary = Some(Summary {
            copied: 1,
            ..Summary::default()
        });
        press(&mut app, KeyCode::Char('o'));
        assert!(app.error && app.message.contains("Folder is unavailable"));
        assert!(app.screen == Screen::Done);
    }

    #[test]
    fn changing_configuration_discards_the_old_preview() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        let config_path = temp.path().join("config.toml");
        Config {
            archive: temp.path().join("archive"),
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        assert!(app.plan.is_some());
        press(&mut app, KeyCode::Char('c'));
        assert!(app.plan.is_none() && app.screen == Screen::Settings);
        press(&mut app, KeyCode::Esc);
        assert!(app.screen == Screen::Sources && app.plan.is_none());
    }

    #[test]
    fn new_folders_can_be_toggled_individually_or_all_at_once_before_importing() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        fs::write(card.join("photo.ARW"), b"raw data").unwrap();
        let archive = temp.path().join("archive");
        let config_path = temp.path().join("config.toml");
        Config {
            archive: archive.clone(),
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 2);

        // Space excludes the highlighted new folder; pressing it again restores it.
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 1);
        assert!(app.message.contains("1 of 2"), "{}", app.message);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 2);

        // `a` excludes every new folder, and a second press includes them all again.
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 0);
        press(&mut app, KeyCode::Enter);
        assert!(!app.confirming && app.error);
        press(&mut app, KeyCode::Char('a'));
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 2);

        // Only the chosen folder is created on import.
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        assert!(app.confirming);
        press(&mut app, KeyCode::Char('y'));
        wait_for(&mut app, Screen::Done);
        let summary = app.summary.as_ref().unwrap();
        assert_eq!((summary.copied, summary.failed), (1, 0));
        let files = walkdir::WalkDir::new(&archive)
            .into_iter()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.file_type().is_file())
            .count();
        assert_eq!(files, 1);
    }

    fn screen_text(app: &mut App) -> String {
        use ratatui::{Terminal, backend::TestBackend};
        let mut terminal = Terminal::new(TestBackend::new(120, 40)).unwrap();
        terminal.draw(|frame| ui::draw(frame, app)).unwrap();
        let buffer = terminal.backend().buffer();
        (0..40)
            .map(|y| {
                (0..120)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn example_and_dialog_always_reflect_the_applied_custom_folder() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        let config_path = temp.path().join("config.toml");
        Config {
            archive: temp.path().join("archive"),
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        let example_line = |text: &str| {
            text.lines()
                .find(|line| line.contains("Example"))
                .unwrap()
                .to_owned()
        };
        assert!(!example_line(&screen_text(&mut app)).contains("demo-trip"));

        press(&mut app, KeyCode::Char('n'));
        assert!(screen_text(&mut app).contains("Applied now: no custom folder"));
        for ch in "demo-trip".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        // The header example follows the applied custom folder once the dialog closes.
        let example = example_line(&screen_text(&mut app));
        assert!(
            example.contains("/demo-trip/2026-10-22-RAW"),
            "stale example: {example}"
        );

        // Reopening shows what is applied, and flags edits that are not applied yet.
        press(&mut app, KeyCode::Char('n'));
        let dialog = screen_text(&mut app);
        assert!(dialog.contains("Applied now: demo-trip/"));
        assert!(!dialog.contains("isn't applied"));
        press(&mut app, KeyCode::Backspace);
        assert!(screen_text(&mut app).contains("isn't applied"));
        press(&mut app, KeyCode::Esc);
        assert!(example_line(&screen_text(&mut app)).contains("/demo-trip/"));

        // Removing it puts the plain example back.
        press(&mut app, KeyCode::Char('x'));
        assert!(!example_line(&screen_text(&mut app)).contains("demo-trip"));
    }

    fn preview_app(files: &[(&str, &[u8])]) -> (tempfile::TempDir, App) {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        for (name, contents) in files {
            fs::write(card.join(name), contents).unwrap();
        }
        let config_path = temp.path().join("config.toml");
        Config {
            archive: temp.path().join("archive"),
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        (temp, app)
    }

    #[test]
    fn folder_names_can_contain_x_and_spaces_in_every_scope() {
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        for scope_right_presses in [0, 1] {
            press(&mut app, KeyCode::Char('n'));
            for _ in 0..scope_right_presses {
                press(&mut app, KeyCode::Right);
            }
            app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
            for ch in "xmas trip x".chars() {
                press(&mut app, KeyCode::Char(ch));
            }
            assert_eq!(app.custom_folder_input.as_deref(), Some("xmas trip x"));
            press(&mut app, KeyCode::Enter);
            assert_eq!(
                app.segment_insert.as_ref().map(SegmentInsert::name),
                Some("xmas trip x")
            );
        }
    }

    #[test]
    fn ctrl_x_removes_the_applied_custom_folder_from_the_dialog() {
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        press(&mut app, KeyCode::Char('n'));
        for ch in "Trip".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.segment_insert.is_some());
        press(&mut app, KeyCode::Char('n'));
        app.key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
        assert!(app.segment_insert.is_none() && app.custom_folder_input.is_none());
    }

    #[test]
    fn tab_selects_folders_in_the_choose_folders_scope() {
        let (_temp, mut app) = preview_app(&[("a.JPG", b"jpeg"), ("b.ARW", b"raw")]);
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Left); // Choose folders
        assert_eq!(app.custom_folder_scope, CustomFolderScope::SelectedFolder);
        let before = app.custom_folder_checked.clone();
        press(&mut app, KeyCode::Tab);
        assert_ne!(app.custom_folder_checked, before);
        assert_eq!(app.custom_folder_input.as_deref(), Some(""));
    }

    #[test]
    fn first_scan_message_explains_how_to_select_new_folders() {
        let (_temp, app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        assert!(app.message.contains("Space selects"), "{}", app.message);
    }

    #[test]
    fn importing_when_everything_is_already_archived_explains_instead_of_confirming() {
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        wait_for(&mut app, Screen::Done);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        assert_eq!(app.plan.as_ref().unwrap().copy_count(), 0);
        press(&mut app, KeyCode::Enter);
        assert!(!app.confirming && app.error);
        assert!(
            app.message.contains("already in the archive"),
            "{}",
            app.message
        );
    }

    #[test]
    fn leaving_a_preview_forgets_its_one_time_custom_folder() {
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        press(&mut app, KeyCode::Char('n'));
        for ch in "Trip".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.segment_insert.is_some());
        press(&mut app, KeyCode::Esc);
        assert!(app.screen == Screen::Sources && app.segment_insert.is_none());
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        assert!(app.segment_insert.is_none());
        let destination = &app.plan.as_ref().unwrap().photos[0].destination;
        assert!(!destination.to_string_lossy().contains("Trip"));
    }

    fn import_everything(app: &mut App) {
        press(app, KeyCode::Enter);
        press(app, KeyCode::Char('y'));
        wait_for(app, Screen::Done);
    }

    #[test]
    fn finished_import_offers_the_single_new_folder_and_its_parent() {
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        import_everything(&mut app);
        let archive = app.completed_archive().unwrap().to_owned();
        let folder = app
            .created_folder()
            .expect("one folder was created")
            .to_owned();
        assert!(folder.is_dir() && folder.starts_with(&archive));
        let parent = app.created_parent().expect("parent below the archive");
        assert_eq!(Some(parent.as_path()), folder.parent());
        assert_ne!(parent, archive);
        let text = screen_text(&mut app);
        assert!(text.contains("Press f to open the new folder"), "{text}");
        assert!(text.contains("Press p to open its parent"), "{text}");
        assert!(text.contains("[f] Open new folder") && text.contains("[p] Open parent"));
    }

    #[test]
    fn several_new_folders_offer_only_their_shared_parent() {
        let (_temp, mut app) = preview_app(&[("a.JPG", b"jpeg"), ("b.ARW", b"raw")]);
        import_everything(&mut app);
        assert_eq!(app.created_folders().len(), 2);
        assert!(app.created_folder().is_none());
        let parent = app.created_parent().expect("shared parent");
        assert!(
            app.created_folders()
                .iter()
                .all(|f| f.parent() == Some(parent.as_path()))
        );
        let text = screen_text(&mut app);
        assert!(
            text.contains("Press p to open the folder containing the new folders"),
            "{text}"
        );
        assert!(!text.contains("Press f"), "{text}");
    }

    #[test]
    fn only_folders_that_were_actually_created_are_offered() {
        let (_temp, mut app) = preview_app(&[("a.JPG", b"jpeg"), ("b.ARW", b"raw")]);
        press(&mut app, KeyCode::Char(' ')); // Deselect the highlighted new folder.
        import_everything(&mut app);
        assert_eq!(app.created_folders().len(), 1);
        assert!(app.created_folder().is_some());
    }

    #[test]
    fn nothing_is_offered_when_the_new_folder_is_the_archive_root_or_the_import_failed() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        let config_path = temp.path().join("config.toml");
        Config {
            archive: temp.path().join("archive"),
            layout: crate::config::Layout::Date,
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        import_everything(&mut app);
        // The date folder sits directly in the archive, so a parent shortcut would
        // only duplicate "open archive".
        assert!(app.created_folder().is_some());
        assert!(app.created_parent().is_none());

        app.summary.as_mut().unwrap().failed = 1;
        assert!(app.created_folders().is_empty() && app.created_folder().is_none());
    }

    #[test]
    fn hashes_learned_during_an_import_are_saved_next_to_the_config() {
        let (temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        let cache_file = temp.path().join("hash-cache.tsv");
        assert!(!cache_file.exists());
        import_everything(&mut app);
        let saved = fs::read_to_string(&cache_file).unwrap();
        assert!(saved.starts_with("shotctl-hash-cache 1\n"));
        assert!(saved.lines().count() >= 2);
    }

    #[test]
    fn archived_folders_show_their_import_date_when_there_is_room() {
        use ratatui::{Terminal, backend::TestBackend};
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        import_everything(&mut app);
        press(&mut app, KeyCode::Enter); // New import
        press(&mut app, KeyCode::Enter); // Scan again: the folder now exists
        wait_for(&mut app, Screen::Preview);
        assert!(
            app.plan
                .as_ref()
                .unwrap()
                .folders()
                .iter()
                .all(|f| f.exists)
        );
        let today = chrono::Local::now().format("%Y-%m-%d").to_string();
        let supported = fs::metadata(&app.config.archive)
            .and_then(|metadata| metadata.created())
            .is_ok();
        let draw = |app: &mut App, width: u16| {
            let mut terminal = Terminal::new(TestBackend::new(width, 40)).unwrap();
            terminal.draw(|frame| ui::draw(frame, app)).unwrap();
            let buffer = terminal.backend().buffer();
            (0..40)
                .map(|y| {
                    (0..width)
                        .map(|x| buffer[(x, y)].symbol())
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n")
        };
        let wide = draw(&mut app, 120);
        assert!(wide.contains("IMPORTED"), "{wide}");
        // The detail line wraps on long temp paths, so look for the label only.
        assert_eq!(wide.contains("· imported"), supported, "{wide}");
        let table_row = wide.lines().find(|line| line.contains("JPEG/")).unwrap();
        assert_eq!(table_row.contains(&today), supported, "{table_row}");
        if !supported {
            assert!(table_row.contains('—'));
        }
        // Narrow terminals drop the column rather than squeezing the folder names.
        let narrow = draw(&mut app, 66);
        assert!(!narrow.contains("IMPORTED"), "{narrow}");
        assert!(narrow.contains("JPEG/"));
    }

    #[test]
    fn o_opens_only_folders_that_are_already_archived() {
        // A new folder doesn't exist yet: explain instead of trying to open it.
        let (_temp, mut app) = preview_app(&[("photo.JPG", b"jpeg data")]);
        assert!(!app.selected_preview_folder().unwrap().exists);
        assert!(!screen_text(&mut app).contains("[o] Open folder"));
        press(&mut app, KeyCode::Char('o'));
        assert!(
            !app.error && app.message.contains("doesn't exist yet"),
            "{}",
            app.message
        );

        // After importing, the folder is archived and `o` is offered for it.
        import_everything(&mut app);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        let folder = app.selected_preview_folder().unwrap();
        assert!(folder.exists);
        assert!(screen_text(&mut app).contains("[o] Open folder"));
        // Existence is checked live: if it vanishes, `o` explains instead of opening
        // anything (and never launches a file manager from the test).
        let path = app.plan.as_ref().unwrap().archive.join(&folder.path);
        fs::remove_dir_all(&path).unwrap();
        assert!(!screen_text(&mut app).contains("[o] Open folder"));
        press(&mut app, KeyCode::Char('o'));
        assert!(app.message.contains("doesn't exist yet"), "{}", app.message);
        // The file view has no folder to open.
        press(&mut app, KeyCode::Char('f'));
        assert!(!screen_text(&mut app).contains("[o] Open folder"));
    }

    #[test]
    fn custom_folder_all_new_can_be_set_cleared_and_is_reset_from_settings() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        let archive = temp.path().join("archive");
        let config_path = temp.path().join("config.toml");
        Config {
            archive: archive.clone(),
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        let layout = app.plan.as_ref().unwrap().photos[0].layout_relative.clone();
        press(&mut app, KeyCode::Char('n'));
        assert!(app.custom_folder_input.is_some());
        assert_eq!(app.custom_folder_scope, CustomFolderScope::AllNewFolders);
        for ch in "MyTrip".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.segment_insert,
            Some(SegmentInsert::AllNew {
                name: "MyTrip".into()
            })
        );
        let destination = app.plan.as_ref().unwrap().photos[0]
            .destination
            .parent()
            .unwrap()
            .to_owned();
        assert_eq!(
            destination,
            app.plan
                .as_ref()
                .unwrap()
                .archive
                .join(import::insert_segment_before_leaf(&layout, "MyTrip"))
        );
        press(&mut app, KeyCode::Char('n'));
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        press(&mut app, KeyCode::Enter);
        assert!(app.segment_insert.is_none());
        press(&mut app, KeyCode::Char('n'));
        for ch in "Keep".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.segment_insert.as_ref().map(SegmentInsert::name),
            Some("Keep")
        );
        press(&mut app, KeyCode::Char('c'));
        assert!(app.segment_insert.is_none() && app.custom_folder_input.is_none());
    }

    #[test]
    fn custom_folder_can_insert_segment_for_selected_new_folder() {
        let temp = tempfile::TempDir::new().unwrap();
        let card = temp.path().join("card");
        fs::create_dir(&card).unwrap();
        fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
        let archive = temp.path().join("archive");
        let config_path = temp.path().join("config.toml");
        Config {
            archive: archive.clone(),
            ..Config::default()
        }
        .save(&config_path)
        .unwrap();
        let mut app = App::new(config_path, Some(card)).unwrap();
        press(&mut app, KeyCode::Enter);
        wait_for(&mut app, Screen::Preview);
        let from = app.selected_new_folder_layout_path().unwrap();
        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Left);
        assert_eq!(app.custom_folder_scope, CustomFolderScope::SelectedFolder);
        assert!(app.custom_folder_checked.iter().any(|checked| *checked));
        for ch in "CustomShoot".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(
            app.segment_insert,
            Some(SegmentInsert::Selected {
                folders: vec![from.clone()],
                name: "CustomShoot".into(),
            })
        );
        let destination = app.plan.as_ref().unwrap().photos[0]
            .destination
            .parent()
            .unwrap();
        let expected = app
            .plan
            .as_ref()
            .unwrap()
            .archive
            .join(import::insert_segment_before_leaf(&from, "CustomShoot"));
        assert_eq!(destination, expected);
    }

    #[test]
    fn archive_completion_and_browser_update_only_the_draft_until_saved() {
        let temp = tempfile::TempDir::new().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let albums = base.join("Photo Albums");
        let archive = base.join("Photo Archive");
        fs::create_dir(&albums).unwrap();
        fs::create_dir(&archive).unwrap();
        fs::create_dir(archive.join("Shoots")).unwrap();
        let config_path = base.join("config.toml");
        let mut app = App::new(config_path.clone(), None).unwrap();
        app.archive_input = format!("{}/Pho", base.display());
        let original_config = app.config.archive.clone();
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.archive_input, format!("{}/Photo A", base.display()));
        assert_eq!(app.field, 0);
        press(&mut app, KeyCode::Tab);
        assert!(app.directory_browser.is_some());
        press(&mut app, KeyCode::Down);
        press(&mut app, KeyCode::Tab);
        assert_eq!(app.archive_input, format!("{}/", archive.display()));
        assert!(app.directory_browser.is_none());
        assert_eq!(app.config.archive, original_config);
        assert!(!config_path.exists());

        let original_input = app.archive_input.clone();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Right);
        assert_eq!(
            app.directory_browser.as_ref().unwrap().current,
            archive.join("Shoots")
        );
        press(&mut app, KeyCode::Esc);
        assert_eq!(app.archive_input, original_input);
        assert!(app.screen == Screen::Settings && !app.quit);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Left);
        assert_eq!(
            app.directory_browser.as_ref().unwrap().selected(),
            Some(&archive.join("Shoots"))
        );
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(
            app.archive_input,
            archive.join("Shoots").display().to_string()
        );
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.screen == Screen::Sources);
        assert_eq!(
            Config::load(&config_path).unwrap().unwrap().archive,
            archive.join("Shoots")
        );
    }

    #[test]
    fn source_picker_supports_completion_and_retains_overlap_validation() {
        let temp = tempfile::TempDir::new().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let card = base.join("Camera Card");
        fs::create_dir(&card).unwrap();
        let config_path = base.join("config.toml");
        let config = Config {
            archive: base.join("archive"),
            ..Config::default()
        };
        config.save(&config_path).unwrap();
        let mut app = App::new(config_path, None).unwrap();
        press(&mut app, KeyCode::Char('s'));
        app.source_input = Some(format!("{}/Cam", base.display()));
        press(&mut app, KeyCode::Tab);
        assert_eq!(
            app.source_input.as_deref(),
            Some(format!("{}/", card.display()).as_str())
        );
        app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.source_input.as_deref(), card.to_str());
        press(&mut app, KeyCode::Enter);
        assert!(app.source_input.is_none());
        assert_eq!(app.selected_source(), Some(&card));

        press(&mut app, KeyCode::Char('s'));
        app.source_input = Some(config.archive.display().to_string());
        app.key(KeyEvent::new(KeyCode::Char('b'), KeyModifiers::CONTROL));
        press(&mut app, KeyCode::Char(' '));
        press(&mut app, KeyCode::Enter);
        assert!(app.error && app.source_input.is_some());
        assert_eq!(app.selected_source(), Some(&card));
        press(&mut app, KeyCode::Esc);
        press(&mut app, KeyCode::Char('c'));
        app.archive_input = card.display().to_string();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char(' '));
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert!(app.error && app.screen == Screen::Settings);
        assert_eq!(app.config.archive, config.archive);
    }

    #[test]
    fn new_folder_prompt_can_cancel_retry_and_choose_the_created_archive() {
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let config_path = root.join("config.toml");
        let mut app = App::new(config_path.clone(), None).unwrap();
        app.archive_input = root.display().to_string();
        let original_archive = app.config.archive.clone();
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('n'));
        for ch in "Cancel Me".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Esc);
        assert!(app.directory_browser.is_some());
        assert!(!root.join("Cancel Me").exists());
        assert!(
            app.directory_browser
                .as_ref()
                .unwrap()
                .new_folder_input
                .is_none()
        );

        press(&mut app, KeyCode::Char('n'));
        press(&mut app, KeyCode::Enter);
        assert!(!app.directory_browser.as_ref().unwrap().error.is_empty());
        fs::create_dir(root.join("Existing")).unwrap();
        for ch in "Existing".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        assert!(
            app.directory_browser
                .as_ref()
                .unwrap()
                .error
                .contains("already exists")
        );
        app.key(KeyEvent::new(KeyCode::Char('u'), KeyModifiers::CONTROL));
        for ch in "New Photo Archive".chars() {
            press(&mut app, KeyCode::Char(ch));
        }
        press(&mut app, KeyCode::Enter);
        let created = root.join("New Photo Archive");
        assert!(created.is_dir());
        let browser = app.directory_browser.as_ref().unwrap();
        assert_eq!(browser.current, created);
        assert!(browser.error.is_empty() && browser.new_folder_input.is_none());
        assert_eq!(app.config.archive, original_archive);
        assert!(!config_path.exists());
        press(&mut app, KeyCode::Char(' '));
        assert_eq!(app.archive_input, created.display().to_string());
        app.key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
        assert_eq!(
            Config::load(&config_path).unwrap().unwrap().archive,
            created
        );
    }
}
