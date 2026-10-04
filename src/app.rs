use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

use anyhow::{Result, bail};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::{DefaultTerminal, widgets::TableState};

use crate::{
    config::{Config, DateBasis, expand_path},
    import::{self, Plan, Summary, WorkerEvent},
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
    pub summary: Option<Summary>,
    pub message: String,
    pub error: bool,
    pub help: bool,
    pub confirming: bool,
    pub source_input: Option<String>,
    pub current: String,
    pub done: usize,
    pub total: usize,
    pub bytes: u64,
    pub total_bytes: u64,
    pub tick: usize,
    pub detail_scroll: u16,
    receiver: Option<Receiver<WorkerEvent>>,
    worker: Option<JoinHandle<()>>,
    cancel: Arc<AtomicBool>,
    quit: bool,
}

impl App {
    pub fn new(config_path: PathBuf, source: Option<PathBuf>) -> Result<Self> {
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
            summary: None,
            message: String::new(),
            error: false,
            help: false,
            confirming: false,
            source_input: None,
            current: String::new(),
            done: 0,
            total: 0,
            bytes: 0,
            total_bytes: 0,
            tick: 0,
            detail_scroll: 0,
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
                    self.message = if plan.photos.is_empty() {
                        "No supported photos found. Press Esc to choose another source.".into()
                    } else {
                        "Review the folders below. Press f for individual files, then Enter to confirm.".into()
                    };
                    self.plan = Some(plan);
                    self.screen = Screen::Preview;
                    self.preview_state = TableState::default().with_selected(Some(0));
                    self.show_files = false;
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
        self.cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.cancel.clone();
        let config = self.config.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.worker = Some(thread::spawn(move || {
            let event = match import::build_plan(&source, &config, &tx, &cancel) {
                Ok(plan) => WorkerEvent::Planned(plan),
                Err(e) => WorkerEvent::Failed(format!("{e:#}")),
            };
            let _ = tx.send(event);
        }));
    }

    fn start_import(&mut self) {
        let Some(plan) = self.plan.take() else { return };
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
        self.clear_message();
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
            KeyCode::Esc => {
                self.screen = Screen::Sources;
                self.plan = None;
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
                        self.preview_state = TableState::default().with_selected(Some(0));
                    }
                    KeyCode::Enter | KeyCode::Char('i') => {
                        if self.plan.as_ref().is_some_and(|p| !p.photos.is_empty()) {
                            self.confirming = true;
                        }
                    }
                    KeyCode::Char('r') => self.scan(),
                    KeyCode::Down | KeyCode::Char('j') => {
                        let len = self.preview_len();
                        move_selection(&mut self.preview_state, len, 1);
                    }
                    KeyCode::Up | KeyCode::Char('k') => {
                        let len = self.preview_len();
                        move_selection(&mut self.preview_state, len, -1);
                    }
                    KeyCode::PageDown => {
                        let len = self.preview_len();
                        move_selection(&mut self.preview_state, len, 10);
                    }
                    KeyCode::PageUp => {
                        let len = self.preview_len();
                        move_selection(&mut self.preview_state, len, -10);
                    }
                    _ => {}
                },
                Screen::Done => match key.code {
                    KeyCode::Enter | KeyCode::Char('r') => {
                        self.screen = Screen::Sources;
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

    fn preview_len(&self) -> usize {
        self.plan
            .as_ref()
            .map(|p| {
                if self.show_files {
                    p.photos.len()
                } else {
                    p.folders().len()
                }
            })
            .unwrap_or(0)
    }

    fn cancel_operation(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.message = "Cancelling safely; completed copies stay in the archive.".into();
    }

    fn source_key(&mut self, key: KeyEvent) {
        match key.code {
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
        match key.code {
            KeyCode::Esc => {
                if self.first_run {
                    self.quit = true;
                } else {
                    self.screen = Screen::Sources;
                    self.clear_message();
                }
            }
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % 5,
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + 4) % 5,
            KeyCode::Enter if self.field == 4 => self.save_settings(),
            KeyCode::Enter if self.field == 0 => self.field = 1,
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
        press(&mut app, KeyCode::Tab);
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
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('y'));
        wait_for(&mut app, Screen::Done);
        let summary = app.summary.as_ref().unwrap();
        assert_eq!((summary.copied, summary.failed), (2, 0));
        assert_eq!(fs::read_dir(&card).unwrap().count(), 2);
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
}
