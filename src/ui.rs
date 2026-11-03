use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{App, CustomFolderScope, Screen},
    directories::DirectoryBrowser,
    import::{Action, FolderSummary, Plan, SegmentInsert, human_bytes},
};

#[derive(Debug, Clone)]
pub enum PreviewFolderRow {
    Header { exists: bool },
    Folder(FolderSummary),
}

impl PreviewFolderRow {
    pub fn is_folder(&self) -> bool {
        matches!(self, Self::Folder(_))
    }
}

/// New folders first, then existing, each preceded by a section header when non-empty.
pub fn preview_folder_rows(plan: &Plan) -> Vec<PreviewFolderRow> {
    let folders = plan.folders();
    let mut new_folders = Vec::new();
    let mut existing = Vec::new();
    for folder in folders {
        if folder.exists {
            existing.push(folder);
        } else {
            new_folders.push(folder);
        }
    }
    let mut rows = Vec::new();
    if !new_folders.is_empty() {
        rows.push(PreviewFolderRow::Header { exists: false });
        rows.extend(new_folders.into_iter().map(PreviewFolderRow::Folder));
    }
    if !existing.is_empty() {
        rows.push(PreviewFolderRow::Header { exists: true });
        rows.extend(existing.into_iter().map(PreviewFolderRow::Folder));
    }
    rows
}

const BG: Color = Color::Rgb(15, 20, 29);
const PANEL: Color = Color::Rgb(20, 28, 39);
const TEXT: Color = Color::Rgb(214, 225, 238);
const MUTED: Color = Color::Rgb(135, 154, 176);
const CYAN: Color = Color::Rgb(91, 214, 231);
const GREEN: Color = Color::Rgb(134, 218, 165);
const YELLOW: Color = Color::Rgb(241, 203, 117);
const RED: Color = Color::Rgb(248, 132, 132);
const SELECTED: Color = Color::Rgb(34, 57, 74);

fn accent() -> Style {
    Style::default().fg(CYAN).add_modifier(Modifier::BOLD)
}
fn muted() -> Style {
    Style::default().fg(MUTED)
}
fn border(title: &str) -> Block<'_> {
    Block::default()
        .title(format!(" {title} "))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Rgb(57, 80, 101)))
        .title_style(accent())
        .style(Style::default().bg(PANEL).fg(TEXT))
}

fn shortcut_lines(app: &App, width: u16) -> Vec<Line<'static>> {
    let shortcuts: &[(&str, &str)] = if app.custom_folder_input.is_some() {
        if app.custom_folder_scope == CustomFolderScope::SelectedFolder {
            &[
                ("←→", "Scope"),
                ("↑↓", "List"),
                ("Space", "Check"),
                ("Enter", "Apply"),
                ("x", "Remove"),
                ("Esc", "Back"),
            ]
        } else {
            &[
                ("←→", "Scope"),
                ("Enter", "Apply"),
                ("x", "Remove"),
                ("Esc", "Back"),
            ]
        }
    } else if let Some(browser) = &app.directory_browser {
        if browser.new_folder_input.is_some() {
            &[("Enter", "Create folder"), ("Esc", "Cancel")]
        } else {
            &[
                ("↑↓", "Select"),
                ("Enter/→", "Open"),
                ("←/Backspace", "Parent"),
                ("Space", "Use current"),
                ("Tab", "Use selected"),
                ("Esc", "Cancel"),
                ("n", "New folder here"),
            ]
        }
    } else {
        match app.screen {
            Screen::Settings if app.field == 0 => &[
                ("Tab", "Complete path"),
                ("Enter", "Browse folders"),
                ("↑↓", "Change field"),
                ("Ctrl+S", "Save"),
                ("Esc", "Cancel"),
            ],
            Screen::Settings => &[
                ("Tab/↑↓", "Change field"),
                ("←→", "Change option"),
                ("Ctrl+S", "Save"),
                ("Esc", "Cancel"),
            ],
            Screen::Sources => &[
                ("↑↓/jk", "Select"),
                ("Enter", "Preview"),
                ("s", "Source"),
                ("r", "Refresh"),
                ("c", "Config"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
            Screen::Preview if app.segment_insert.is_some() => &[
                ("↑↓/jk", "Scroll"),
                ("f", "Folders/files"),
                ("n", "Edit custom"),
                ("x", "Remove custom"),
                ("Enter", "Confirm"),
                ("r", "Rescan"),
                ("Esc", if app.show_files { "Folders" } else { "Sources" }),
            ],
            Screen::Preview => &[
                ("↑↓/jk", "Scroll"),
                ("f", "Folders/files"),
                ("n", "Custom folder"),
                ("Enter", "Confirm"),
                ("r", "Rescan"),
                ("c", "Config"),
                ("Esc", if app.show_files { "Folders" } else { "Sources" }),
            ],
            Screen::Scanning | Screen::Importing => {
                &[("Esc/Ctrl+C", "Cancel safely"), ("?", "Help")]
            }
            Screen::Done if app.completed_archive().is_some() => &[
                ("Enter", "New import"),
                ("o", "Open archive"),
                ("↑↓", "Scroll"),
                ("c", "Config"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
            Screen::Done => &[
                ("Enter", "New import"),
                ("↑↓", "Scroll"),
                ("c", "Config"),
                ("?", "Help"),
                ("q", "Quit"),
            ],
        }
    };
    let mut lines = Vec::new();
    let mut line = Line::default();
    for &(key, action) in shortcuts {
        let group = Line::from(vec![
            Span::styled(format!("[{key}]"), accent().bg(SELECTED)),
            Span::styled(format!(" {action}"), Style::default().fg(TEXT)),
        ]);
        if !line.spans.is_empty() && line.width() + 5 + group.width() > usize::from(width) {
            lines.push(line);
            line = Line::default();
        }
        if !line.spans.is_empty() {
            line.spans.push(Span::styled("  │  ", muted()));
        }
        line.spans.extend(group.spans);
    }
    lines.push(line);
    lines
}

pub fn draw(frame: &mut Frame, app: &mut App) {
    let area = frame.area();
    frame.render_widget(
        Block::default().style(Style::default().bg(BG).fg(TEXT)),
        area,
    );
    if area.width < 66 || area.height < 22 {
        frame.render_widget(Paragraph::new("shotctl\n\nEnlarge your terminal to at least 66 columns × 22 rows.\n\nCtrl+C exits or safely cancels an active import.")
            .style(Style::default().fg(CYAN)).wrap(Wrap { trim: false }), area);
        return;
    }
    let hints = shortcut_lines(app, area.width.saturating_sub(2));
    let regions = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if area.height < 32 { 5 } else { 7 }),
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(2),
        Constraint::Length(hints.len() as u16),
    ])
    .margin(1)
    .split(area);
    let banner = Line::from(vec![
        Span::styled(
            " shotctl ",
            Style::default()
                .bg(CYAN)
                .fg(BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("  PHOTO IMPORT & ARCHIVE", muted()),
        Span::styled(format!("  v{}", env!("CARGO_PKG_VERSION")), muted()),
    ]);
    frame.render_widget(Paragraph::new(banner), regions[0]);
    header(frame, app, regions[1]);
    steps(frame, app, regions[2]);
    match app.screen {
        Screen::Settings => settings(frame, app, regions[3]),
        Screen::Sources => sources(frame, app, regions[3]),
        Screen::Preview => preview(frame, app, regions[3]),
        Screen::Scanning | Screen::Importing => progress(frame, app, regions[3]),
        Screen::Done => done(frame, app, regions[3]),
    }
    let message = if app.error { RED } else { YELLOW };
    frame.render_widget(
        Paragraph::new(app.message.as_str())
            .style(Style::default().fg(message))
            .wrap(Wrap { trim: false }),
        regions[4],
    );
    frame.render_widget(Paragraph::new(hints), regions[5]);
    if app.help {
        help(frame);
    }
    if let Some(input) = &app.source_input {
        source_dialog(frame, input, app);
    }
    if app.confirming {
        confirmation(frame, app);
    }
    if app.custom_folder_input.is_some() {
        custom_folder_dialog(frame, app);
    }
    if app.directory_browser.is_some() {
        directory_dialog(frame, app);
    }
}

fn header(frame: &mut Frame, app: &App, area: Rect) {
    let mut config = if app.screen == Screen::Settings {
        app.draft.clone()
    } else {
        app.config.clone()
    };
    if app.screen == Screen::Settings {
        config.archive = crate::config::expand_path(&app.archive_input);
    }
    let mut lines = vec![
        Line::from(vec![
            Span::styled(" Archive  ", muted()),
            Span::raw(config.archive.display().to_string()),
        ]),
        Line::from(vec![
            Span::styled(" Layout   ", muted()),
            Span::styled(config.layout.to_string(), Style::default().fg(CYAN)),
            Span::styled("    Grouping  ", muted()),
            Span::raw(config.grouping.to_string()),
        ]),
        Line::from(vec![
            Span::styled(" Dates    ", muted()),
            Span::raw(config.date_basis.to_string()),
            Span::styled("    Mode  ", muted()),
            Span::styled("Copy + verify", Style::default().fg(GREEN)),
        ]),
        Line::from(vec![
            Span::styled(" Example  ", muted()),
            Span::raw(config.example()),
        ]),
        Line::from(vec![
            Span::styled(" Source   ", muted()),
            Span::raw(
                app.selected_source()
                    .map(|p| p.display().to_string())
                    .unwrap_or_else(|| "No card selected · press s to choose a directory".into()),
            ),
        ]),
    ];
    if area.height < 7 {
        lines.truncate(3);
    }
    frame.render_widget(
        Paragraph::new(lines).block(border(if app.first_run {
            "Archive configuration · first run"
        } else if app.screen == Screen::Settings {
            "Archive configuration · unsaved changes"
        } else {
            "Archive configuration · c to edit"
        })),
        area,
    );
}

fn steps(frame: &mut Frame, app: &App, area: Rect) {
    let current = match app.screen {
        Screen::Settings => 0,
        Screen::Sources => 1,
        Screen::Scanning | Screen::Preview => 2,
        Screen::Importing | Screen::Done => 3,
    };
    let mut spans = Vec::new();
    for (i, title) in ["CONFIGURE", "SELECT SOURCE", "REVIEW FOLDERS", "IMPORT"]
        .iter()
        .enumerate()
    {
        if i > 0 {
            spans.push(Span::styled("  ›  ", muted()));
        }
        spans.push(Span::styled(
            format!(" {} {title} ", i + 1),
            if i == current {
                Style::default()
                    .bg(SELECTED)
                    .fg(CYAN)
                    .add_modifier(Modifier::BOLD)
            } else {
                muted()
            },
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn settings(frame: &mut Frame, app: &App, area: Rect) {
    let block = border(if app.first_run {
        "Welcome · set up your photo archive"
    } else {
        "Edit configuration"
    });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let compact = inner.height < 15;
    let chunks = Layout::vertical([
        Constraint::Length(if compact { 0 } else { 2 }),
        Constraint::Length(5),
        Constraint::Min(1),
    ])
    .horizontal_margin(1)
    .vertical_margin(u16::from(inner.height >= 8))
    .split(inner);
    frame.render_widget(
        Paragraph::new("Archive: Tab completes folders · Enter opens the folder browser.")
            .style(muted()),
        chunks[0],
    );
    let input = if app.field == 0 {
        let available = chunks[1].width.saturating_sub(24).max(1) as usize;
        let chars: Vec<_> = app.archive_input.chars().collect();
        format!(
            "{}▏",
            chars[chars.len().saturating_sub(available.saturating_sub(1))..]
                .iter()
                .collect::<String>()
        )
    } else {
        app.archive_input.clone()
    };
    let values = [
        ("Archive directory", input),
        ("Folder structure", format!("‹ {} ›", app.draft.layout)),
        ("Photo grouping", format!("‹ {} ›", app.draft.grouping)),
        ("Organize by", format!("‹ {} ›", app.draft.date_basis)),
        ("", "Save configuration & continue".into()),
    ];
    let rows: Vec<_> = values
        .iter()
        .enumerate()
        .map(|(i, (label, value))| {
            let style = if app.field == i {
                Style::default()
                    .bg(SELECTED)
                    .fg(CYAN)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(TEXT)
            };
            Row::new(vec![
                format!("{} {label}", if app.field == i { "›" } else { " " }),
                value.clone(),
            ])
            .style(style)
        })
        .collect();
    frame.render_widget(
        Table::new(rows, [Constraint::Length(21), Constraint::Min(20)]).column_spacing(2),
        chunks[1],
    );
    let mut draft = app.draft.clone();
    draft.archive = crate::config::expand_path(&app.archive_input);
    let example = chrono::NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    let raw = draft.directory(example, "RAW");
    let jpeg = draft.directory(example, "JPEG");
    let mut lines = vec![
        Line::from(Span::styled("Example import · October 22, 2026", accent())),
        Line::from(format!("  {}", raw.join("DSC_0001.ARW").display())),
        Line::from(format!("  {}", jpeg.join("DSC_0001.JPG").display())),
        Line::from(""),
        Line::from(Span::styled(
            "Capture date uses EXIF; unavailable dates use file modification time.",
            muted(),
        )),
        Line::from(Span::styled(
            "The preview identifies those fallbacks before you confirm.",
            muted(),
        )),
    ];
    if !app.first_run {
        lines.push(Line::from(Span::styled(
            format!("Config: {}", app.config_path.display()),
            muted(),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[2]);
}

fn sources(frame: &mut Frame, app: &mut App, area: Rect) {
    if app.sources.is_empty() {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled("  Ready for your next shoot", accent())),
            Line::from(""),
            Line::from("  Plug in a camera card, then press r to refresh."),
            Line::from("  Cards with a DCIM folder appear automatically."),
            Line::from(""),
            Line::from(vec![
                Span::styled("  s", accent()),
                Span::raw("  Choose a card or any photo directory manually"),
            ]),
            Line::from(vec![
                Span::styled("  c", accent()),
                Span::raw("  Change your archive preferences"),
            ]),
            Line::from(""),
            Line::from(Span::styled(
                "  Every import starts with a destination preview.",
                muted(),
            )),
        ];
        frame.render_widget(
            Paragraph::new(lines).block(border("Sources · no camera cards detected")),
            area,
        );
        return;
    }
    let chunks = Layout::vertical([Constraint::Min(5), Constraint::Length(4)]).split(area);
    let rows: Vec<_> = app
        .sources
        .iter()
        .map(|path| {
            Row::new(vec![
                path.file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
                path.display().to_string(),
                "Ready".into(),
            ])
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(20),
            Constraint::Min(25),
            Constraint::Length(8),
        ],
    )
    .header(
        Row::new(["SOURCE", "DIRECTORY", "STATUS"])
            .style(muted())
            .bottom_margin(1),
    )
    .block(border("Sources · select a card or directory"))
    .row_highlight_style(Style::default().bg(SELECTED).fg(CYAN))
    .highlight_symbol("› ");
    frame.render_stateful_widget(table, chunks[0], &mut app.source_state);
    frame.render_widget(Paragraph::new(vec![
        Line::from("Enter scans photos and builds an exact folder preview."),
        Line::from(Span::styled("Identical files are skipped. Name conflicts receive a stable suffix. Originals stay on the card.", muted())),
    ]).block(border("Next import")).wrap(Wrap { trim: false }), chunks[1]);
}

fn preview(frame: &mut Frame, app: &mut App, area: Rect) {
    let Some(plan) = &app.plan else { return };
    let has_warning = plan.fallback_count() > 0 || plan.skipped_entries > 0;
    let has_custom = app.segment_insert.is_some();
    let chunks = Layout::vertical([
        Constraint::Length(if has_custom { 4 } else { 3 }),
        Constraint::Length(if has_warning { 2 } else { 0 }),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(area);
    let mut metrics = vec![Line::from(vec![
        Span::styled(format!(" {} photos", plan.photos.len()), accent()),
        Span::raw(format!(
            "    {} to copy · {}",
            plan.copy_count(),
            human_bytes(plan.bytes())
        )),
        Span::styled(
            format!("    {} duplicate skips", plan.skip_count()),
            Style::default().fg(GREEN),
        ),
        Span::styled(
            format!("    {} renamed", plan.rename_count()),
            Style::default().fg(YELLOW),
        ),
    ])];
    if let Some(insert) = &app.segment_insert {
        let scope_label = match insert {
            SegmentInsert::Selected { folders, .. } if folders.len() != 1 => {
                format!("{} chosen folders", folders.len())
            }
            SegmentInsert::Selected { .. } => "1 chosen folder".into(),
            SegmentInsert::AllNew { .. } => "all new folders".into(),
        };
        metrics.push(Line::from(vec![
            Span::styled(" 📂 ", Style::default().fg(CYAN)),
            Span::styled("Custom segment: ", Style::default().fg(CYAN)),
            Span::styled(
                format!("{}/", insert.name()),
                Style::default().fg(YELLOW).add_modifier(Modifier::BOLD),
            ),
            Span::styled(format!("   ({scope_label})"), muted()),
        ]));
    }
    frame.render_widget(
        Paragraph::new(metrics).block(border("Import preview · no files copied yet")),
        chunks[0],
    );
    if has_warning {
        let text = format!(
            "{} photos use file modification dates (capture EXIF unavailable). {} unsupported files / symlinks / system folders ignored. Press f to inspect each photo.",
            plan.fallback_count(),
            plan.skipped_entries
        );
        frame.render_widget(
            Paragraph::new(text)
                .style(Style::default().fg(YELLOW))
                .wrap(Wrap { trim: false }),
            chunks[1],
        );
    }
    if app.show_files {
        let rows: Vec<_> = plan
            .photos
            .iter()
            .map(|photo| {
                let relative = photo
                    .destination
                    .strip_prefix(&plan.archive)
                    .unwrap_or(&photo.destination);
                let action = match photo.action {
                    Action::Copy => "COPY",
                    Action::Skip => "SKIP",
                    Action::Rename => "RENAME",
                };
                Row::new(vec![
                    relative.display().to_string(),
                    action.into(),
                    if photo.fallback { "file date" } else { "" }.into(),
                    human_bytes(photo.bytes),
                ])
                .style(Style::default().fg(if photo.action == Action::Skip {
                    MUTED
                } else {
                    TEXT
                }))
            })
            .collect();
        frame.render_stateful_widget(
            Table::new(
                rows,
                [
                    Constraint::Min(30),
                    Constraint::Length(7),
                    Constraint::Length(9),
                    Constraint::Length(10),
                ],
            )
            .header(
                Row::new(["DESTINATION FILE", "ACTION", "FALLBACK", "SIZE"])
                    .style(muted())
                    .bottom_margin(0),
            )
            .block(border("Files · relative to archive"))
            .row_highlight_style(Style::default().bg(SELECTED).fg(CYAN))
            .highlight_symbol("› "),
            chunks[2],
            &mut app.preview_state,
        );
    } else {
        let rows: Vec<_> = preview_folder_rows(plan)
            .into_iter()
            .map(|row| match row {
                PreviewFolderRow::Header { exists: false } => Row::new(vec![
                    "📄→📂  New folders · will be created".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                ])
                .style(Style::default().fg(CYAN).add_modifier(Modifier::BOLD)),
                PreviewFolderRow::Header { exists: true } => Row::new(vec![
                    "📁  Existing folders · already in archive".into(),
                    String::new(),
                    String::new(),
                    String::new(),
                ])
                .style(Style::default().fg(GREEN).add_modifier(Modifier::BOLD)),
                PreviewFolderRow::Folder(folder) => {
                    let (icon, color) = if folder.exists {
                        ("📁", GREEN)
                    } else {
                        ("📂", CYAN)
                    };
                    Row::new(vec![
                        format!("{icon}  {}/", folder.path.display()),
                        folder.copy.to_string(),
                        folder.skip.to_string(),
                        human_bytes(folder.bytes),
                    ])
                    .style(Style::default().fg(color))
                }
            })
            .collect();
        frame.render_stateful_widget(
            Table::new(
                rows,
                [
                    Constraint::Min(25),
                    Constraint::Length(8),
                    Constraint::Length(8),
                    Constraint::Length(11),
                ],
            )
            .header(
                Row::new(["DESTINATION FOLDER", "COPY", "SKIP", "SIZE"])
                    .style(muted())
                    .bottom_margin(0),
            )
            .block(border("Folder structure · relative to archive"))
            .row_highlight_style(Style::default().bg(SELECTED).fg(CYAN))
            .highlight_symbol("› "),
            chunks[2],
            &mut app.preview_state,
        );
    }
    let detail = if app.show_files {
        app.preview_state
            .selected()
            .and_then(|index| plan.photos.get(index))
            .map(|photo| format!("Source: {}", photo.source.display()))
    } else {
        app.preview_state.selected().and_then(|index| {
            preview_folder_rows(plan).get(index).and_then(|row| match row {
                PreviewFolderRow::Folder(folder) => Some(format!(
                    "Destination: {}",
                    plan.archive.join(&folder.path).display()
                )),
                PreviewFolderRow::Header { .. } => None,
            })
        })
    }
    .unwrap_or_else(|| format!("Archive: {}", plan.archive.display()));
    frame.render_widget(
        Paragraph::new(detail)
            .style(muted())
            .wrap(Wrap { trim: false }),
        chunks[3],
    );
}

fn progress(frame: &mut Frame, app: &App, area: Rect) {
    let importing = app.screen == Screen::Importing;
    let block = border(if importing {
        "Importing · copy and verify"
    } else {
        "Scanning · reading dates and checking duplicates"
    });
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(2),
        Constraint::Length(3),
        Constraint::Min(2),
    ])
    .margin(2)
    .split(inner);
    let spinner = ["◐", "◓", "◑", "◒"][app.tick / 2 % 4];
    frame.render_widget(
        Paragraph::new(format!(
            "{spinner} {}",
            if importing {
                "Archiving your photos"
            } else {
                "Preparing your import preview"
            }
        ))
        .style(accent()),
        chunks[0],
    );
    if importing {
        let ratio = if app.total == 0 {
            0.0
        } else {
            (app.done as f64 / app.total as f64).min(1.0)
        };
        frame.render_widget(
            Gauge::default()
                .gauge_style(Style::default().fg(CYAN).bg(SELECTED))
                .ratio(ratio)
                .label(format!("{} / {} files", app.done, app.total)),
            chunks[1],
        );
    } else {
        frame.render_widget(
            Paragraph::new(format!("{} photos inspected", app.done))
                .style(Style::default().fg(GREEN)),
            chunks[1],
        );
    }
    let mut lines = vec![Line::from(app.current.as_str())];
    if importing {
        lines.push(Line::from(Span::styled(
            format!(
                "{} / {} processed",
                human_bytes(app.bytes),
                human_bytes(app.total_bytes)
            ),
            muted(),
        )));
    }
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: false }), chunks[2]);
    frame.render_widget(Paragraph::new(if importing {
        "Photos are verified before their final names appear.\nEsc cancels safely; completed copies stay in your archive."
    } else { "The scan reads your card and compares existing archive files.\nYou'll review exact destination folders before importing." }).style(muted()).wrap(Wrap { trim: false }), chunks[3]);
}

fn done(frame: &mut Frame, app: &App, area: Rect) {
    let Some(summary) = &app.summary else { return };
    let title = if summary.cancelled {
        "Import cancelled"
    } else if summary.failed > 0 {
        "Import finished with errors"
    } else {
        "Import complete"
    };
    let color = if summary.cancelled || summary.failed > 0 {
        YELLOW
    } else {
        GREEN
    };
    let mut lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            format!("  {title}"),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(format!(
            "  {} copied & verified    {} skipped    {} failed",
            summary.copied, summary.skipped, summary.failed
        )),
        Line::from(""),
        Line::from(format!(
            "  Archive: {}",
            app.import_archive
                .as_deref()
                .unwrap_or(&app.config.archive)
                .display()
        )),
        Line::from(Span::styled(
            "  Your original files remain on the card.",
            muted(),
        )),
        Line::from(""),
        Line::from("  Press Enter to start another import."),
    ];
    if app.completed_archive().is_some() {
        lines.push(Line::from("  Press o to open the archive folder."));
    }
    if summary.failed > 0 {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "  Errors · refresh the preview before retrying",
            Style::default().fg(RED),
        )));
        for error in &summary.errors {
            lines.push(Line::from(format!("  {error}")));
        }
    }
    frame.render_widget(
        Paragraph::new(lines)
            .block(border(title))
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        area,
    );
}

fn popup(frame: &mut Frame, width: u16, height: u16) -> Rect {
    let area = frame.area();
    let width = width.min(area.width.saturating_sub(4));
    let height = height.min(area.height.saturating_sub(4));
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, rect);
    rect
}

fn confirmation(frame: &mut Frame, app: &App) {
    let Some(plan) = &app.plan else { return };
    let area = popup(frame, 76, 15);
    let lines = vec![
        Line::from(""),
        Line::from(format!(
            "  Copy {} photos ({}) into {} folders?",
            plan.copy_count(),
            human_bytes(plan.bytes()),
            plan.folders().iter().filter(|folder| folder.copy > 0).count()
        )),
        Line::from(""),
        Line::from(format!("  From: {}", plan.source.display())),
        Line::from(format!("  To:   {}", plan.archive.display())),
        Line::from(""),
        Line::from(format!(
            "  {} identical files skipped · {} name conflicts renamed",
            plan.skip_count(),
            plan.rename_count()
        )),
        Line::from(Span::styled(
            format!(
                "  {} photos use file modification dates",
                plan.fallback_count()
            ),
            if plan.fallback_count() > 0 {
                Style::default().fg(YELLOW)
            } else {
                muted()
            },
        )),
        Line::from(""),
        Line::from(Span::styled("  Enter / y   Confirm import", accent())),
        Line::from(Span::styled("  Esc / n     Return to preview", muted())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(border("Confirm this import"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn custom_folder_dialog(frame: &mut Frame, app: &App) {
    let choosing = app.custom_folder_scope == CustomFolderScope::SelectedFolder;
    let list_height = if choosing {
        app.custom_folder_choices.len().clamp(1, 8) as u16 + 1
    } else {
        0
    };
    let area = popup(frame, 84, 18 + list_height);
    let block = border("📂 Custom folder");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(5),
        Constraint::Length(if choosing { list_height } else { 0 }),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
    ])
    .margin(1)
    .split(inner);
    frame.render_widget(custom_folder_scope_line(app), chunks[0]);
    let (blurb, clear_hint) = match app.custom_folder_scope {
        CustomFolderScope::SelectedFolder => (
            "Inserts a folder under year/month for the checked new folders only. Existing folders stay unchanged:",
            "↑↓ move · Space check/uncheck · x removes an applied custom name.",
        ),
        CustomFolderScope::AllNewFolders => (
            "Inserts the same folder under year/month for every new folder that will be created. Existing folders stay unchanged:",
            "Press x to remove an applied custom name.",
        ),
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(blurb),
            custom_folder_example(app),
            Line::from(Span::styled(clear_hint, muted())),
        ])
        .wrap(Wrap { trim: false }),
        chunks[1],
    );
    if choosing {
        frame.render_widget(custom_folder_picker(app), chunks[2]);
    }
    frame.render_widget(Paragraph::new("Folder name:").style(muted()), chunks[3]);
    let chars: Vec<_> = app.custom_folder_input.as_ref().unwrap().chars().collect();
    let available = chunks[4].width.saturating_sub(1) as usize;
    let input: String = chars[chars.len().saturating_sub(available)..]
        .iter()
        .collect();
    frame.render_widget(
        Paragraph::new(format!("{input}▏")).style(accent().bg(SELECTED)),
        chunks[4],
    );
    let error = if app.error {
        app.message.as_str()
    } else {
        ""
    };
    frame.render_widget(
        Paragraph::new(error)
            .style(Style::default().fg(RED))
            .wrap(Wrap { trim: false }),
        chunks[5],
    );
    let footer = if choosing {
        Line::from(vec![
            Span::styled("[←→]", accent().bg(SELECTED)),
            Span::raw(" Scope  "),
            Span::styled("[↑↓]", accent().bg(SELECTED)),
            Span::raw(" List  "),
            Span::styled("[Space]", accent().bg(SELECTED)),
            Span::raw(" Check  "),
            Span::styled("[Enter]", accent().bg(SELECTED)),
            Span::raw(" Apply  "),
            Span::styled("[x]", accent().bg(SELECTED)),
            Span::raw(" Remove  "),
            Span::styled("[Esc]", accent().bg(SELECTED)),
            Span::raw(" Back"),
        ])
    } else {
        Line::from(vec![
            Span::styled("[←→]", accent().bg(SELECTED)),
            Span::raw(" Scope  "),
            Span::styled("[Enter]", accent().bg(SELECTED)),
            Span::raw(" Apply  "),
            Span::styled("[x]", accent().bg(SELECTED)),
            Span::raw(" Remove  "),
            Span::styled("[Esc]", accent().bg(SELECTED)),
            Span::raw(" Back"),
        ])
    };
    frame.render_widget(
        Paragraph::new(footer).wrap(Wrap { trim: false }),
        chunks[6],
    );
}

fn custom_folder_scope_line(app: &App) -> Paragraph<'static> {
    let scope = app.custom_folder_scope;
    let chip = |active: bool| {
        if active {
            accent().bg(SELECTED)
        } else {
            muted()
        }
    };
    Paragraph::new(Line::from(vec![
        Span::styled("Scope: ", muted()),
        Span::styled(
            " Choose folders ",
            chip(scope == CustomFolderScope::SelectedFolder),
        ),
        Span::raw(" "),
        Span::styled(
            " All new folders ",
            chip(scope == CustomFolderScope::AllNewFolders),
        ),
    ]))
}

fn custom_folder_picker(app: &App) -> Paragraph<'static> {
    if app.custom_folder_choices.is_empty() {
        return Paragraph::new(Span::styled(
            "No new folders in this preview.",
            muted(),
        ));
    }
    let mut lines = vec![Line::from(Span::styled(
        "New folders · Space to include:",
        muted(),
    ))];
    for (index, path) in app.custom_folder_choices.iter().enumerate() {
        let checked = app
            .custom_folder_checked
            .get(index)
            .copied()
            .unwrap_or(false);
        let mark = if checked { "[x]" } else { "[ ]" };
        let focused = index == app.custom_folder_cursor;
        let style = if focused {
            accent().bg(SELECTED)
        } else if checked {
            Style::default().fg(CYAN)
        } else {
            muted()
        };
        lines.push(Line::from(Span::styled(
            format!(" {mark}  {}/", path.display()),
            style,
        )));
    }
    Paragraph::new(lines)
}

fn custom_folder_example(app: &App) -> Line<'static> {
    let typed = app
        .custom_folder_input
        .as_deref()
        .unwrap_or("")
        .trim();
    let name = if typed.is_empty() {
        "<CUSTOM FOLDER NAME>"
    } else {
        typed
    };
    let name_style = Style::default().fg(YELLOW).add_modifier(Modifier::BOLD);
    let archive = format!("{}", app.config.archive.display());
    let sample = app
        .checked_custom_folders()
        .into_iter()
        .next()
        .or_else(|| app.selected_new_folder_layout_path())
        .or_else(|| app.custom_folder_choices.first().cloned())
        .unwrap_or_else(|| {
            std::path::PathBuf::from(app.config.example())
                .strip_prefix(&app.config.archive)
                .map(|p| p.to_owned())
                .unwrap_or_else(|_| std::path::PathBuf::from("2026/10/2026-10-22-RAW"))
        });
    let parent = sample
        .parent()
        .map(|p| format!("{}/", p.display()))
        .unwrap_or_default();
    let leaf = sample
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "2026-10-22-RAW".into());
    Line::from(vec![
        Span::styled("📄→📂  ", Style::default().fg(CYAN)),
        Span::styled(format!("{archive}/"), Style::default().fg(CYAN)),
        Span::styled(parent, Style::default().fg(CYAN)),
        Span::styled(name.to_owned(), name_style),
        Span::styled(format!("/{leaf}/"), Style::default().fg(CYAN)),
    ])
}

fn source_dialog(frame: &mut Frame, input: &str, app: &App) {
    let area = popup(frame, 76, 11);
    let lines = vec![
        Line::from(""),
        Line::from("  Enter the card's mount point or a photo directory:"),
        Line::from(""),
        Line::from(Span::styled(format!("  {input}▏"), accent())),
        Line::from(""),
        Line::from(Span::styled(
            "  Tab complete · Ctrl+B browse · Enter select · Esc cancel",
            muted(),
        )),
        Line::from(Span::styled("  Ctrl+U clear · ~/ supported", muted())),
        Line::from(Span::styled(app.message.as_str(), Style::default().fg(RED))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(border("Choose source directory"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn directory_dialog(frame: &mut Frame, app: &mut App) {
    let title = if app.source_input.is_some() {
        "Browse source folders"
    } else {
        "Browse archive folders"
    };
    let screen = frame.area();
    let footer_height = shortcut_lines(app, screen.width.saturating_sub(2)).len() as u16;
    let available_height = screen.height.saturating_sub(footer_height + 1);
    let width = 90.min(screen.width.saturating_sub(4));
    let height = 26.min(available_height.saturating_sub(2));
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + (available_height - height) / 2,
        width,
        height,
    );
    frame.render_widget(Clear, area);
    let block = border(title);
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(3),
        Constraint::Min(3),
        Constraint::Length(2),
        Constraint::Length(3),
    ])
    .margin(1)
    .split(inner);
    let browser = app.directory_browser.as_mut().unwrap();
    let path = browser.current.display().to_string();
    let available = chunks[0].width.saturating_sub(6) as usize;
    let chars: Vec<_> = path.chars().collect();
    let path = if chars.len() > available {
        format!(
            "…{}",
            chars[chars.len().saturating_sub(available.saturating_sub(1))..]
                .iter()
                .collect::<String>()
        )
    } else {
        path
    };
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(format!("Use: {path}"), accent()))),
        chunks[0],
    );

    let mut ancestors: Vec<_> = browser.current.ancestors().take(3).collect();
    ancestors.reverse();
    let tree: Vec<_> = ancestors
        .iter()
        .enumerate()
        .map(|(index, path)| {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.display().to_string());
            Line::from(Span::styled(
                format!(
                    "{}{}{}",
                    "  ".repeat(index),
                    if index == 0 { "" } else { "└─ " },
                    name
                ),
                if *path == browser.current {
                    accent()
                } else {
                    muted()
                },
            ))
        })
        .collect();
    frame.render_widget(Paragraph::new(tree), chunks[1]);
    if browser.entries.is_empty() {
        frame.render_widget(
            Paragraph::new("  No subfolders. Space uses this folder.").style(muted()),
            chunks[2],
        );
    } else {
        let rows = browser.entries.iter().enumerate().map(|(index, path)| {
            Row::new(vec![format!(
                "    {} {}/",
                if index + 1 == browser.entries.len() {
                    "└─"
                } else {
                    "├─"
                },
                path.file_name().unwrap().to_string_lossy()
            )])
        });
        let table = Table::new(rows, [Constraint::Min(1)])
            .row_highlight_style(
                Style::default()
                    .bg(SELECTED)
                    .fg(CYAN)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("› ");
        frame.render_stateful_widget(table, chunks[2], &mut browser.state);
    }
    let status = if !browser.error.is_empty() {
        browser.error.clone()
    } else if !browser.filter.is_empty() {
        format!(
            "Matching: {} · Tab uses the selected folder",
            browser.filter
        )
    } else {
        format!(
            "{} folders · hidden folders {}",
            browser.entries.len(),
            if browser.show_hidden {
                "shown"
            } else {
                "hidden"
            }
        )
    };
    frame.render_widget(
        Paragraph::new(status)
            .style(if browser.error.is_empty() {
                muted()
            } else {
                Style::default().fg(RED)
            })
            .wrap(Wrap { trim: false }),
        chunks[3],
    );
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("↑↓ select · Enter/→ open · ←/Backspace parent"),
            Line::from("Space use current · Tab use selected · Esc cancel"),
            Line::from("h home · / root · . hidden folders · PgUp/PgDn scroll"),
        ])
        .style(muted()),
        chunks[4],
    );
    if browser.new_folder_input.is_some() {
        new_folder_dialog(frame, browser);
    }
}

fn new_folder_dialog(frame: &mut Frame, browser: &DirectoryBrowser) {
    let area = popup(frame, 76, 13);
    let block = border("Create new folder");
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(2),
        Constraint::Length(1),
    ])
    .margin(1)
    .split(inner);
    frame.render_widget(Paragraph::new("Create inside:").style(muted()), chunks[0]);
    frame.render_widget(
        Paragraph::new(browser.current.display().to_string()).wrap(Wrap { trim: false }),
        chunks[1],
    );
    frame.render_widget(Paragraph::new("Folder name:").style(muted()), chunks[2]);
    let chars: Vec<_> = browser.new_folder_input.as_ref().unwrap().chars().collect();
    let available = chunks[3].width.saturating_sub(1) as usize;
    let input: String = chars[chars.len().saturating_sub(available)..]
        .iter()
        .collect();
    frame.render_widget(
        Paragraph::new(format!("{input}▏")).style(accent().bg(SELECTED)),
        chunks[3],
    );
    frame.render_widget(
        Paragraph::new(browser.error.as_str())
            .style(Style::default().fg(RED))
            .wrap(Wrap { trim: false }),
        chunks[4],
    );
    frame.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("[Enter]", accent().bg(SELECTED)),
            Span::raw(" Create & open"),
            Span::styled("  │  ", muted()),
            Span::styled("[Esc]", accent().bg(SELECTED)),
            Span::raw(" Cancel"),
        ])),
        chunks[5],
    );
}

fn help(frame: &mut Frame) {
    let area = popup(frame, 80, 23);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  A quick path from card to archive",
            accent(),
        )),
        Line::from("  1. Set your archive; Tab completes paths, Enter browses folders."),
        Line::from("  2. Select a card, or s for a source (Tab completes, Ctrl+B browses)."),
        Line::from("  3. Press Enter to scan and review destination folders."),
        Line::from("  4. Press Enter again, then confirm the import."),
        Line::from(""),
        Line::from("  ↑↓ / j k    Select or scroll     c    Configuration"),
        Line::from("  s           Choose source       r    Refresh / rescan"),
        Line::from("  f / Tab     Folders / files      n    One-time folder"),
        Line::from("  Esc         Back / cancel        ?    This help"),
        Line::from("  q           Quit"),
        Line::from(""),
        Line::from(Span::styled("  Import behavior", accent())),
        Line::from("  Copies are verified; originals are preserved."),
        Line::from("  Identical files are skipped. Conflicts get a content suffix."),
        Line::from("  Missing capture EXIF uses file modification date, shown in preview."),
        Line::from("  n custom folder for new destinations; x removes an applied custom name."),
        Line::from("  Cancelling keeps finished photos and discards partial copies."),
        Line::from(""),
        Line::from(Span::styled("  Enter / Esc closes help", muted())),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(border("shotctl · keyboard guide"))
            .wrap(Wrap { trim: false }),
        area,
    );
}
