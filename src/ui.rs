use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Gauge, Paragraph, Row, Table, Wrap},
};

use crate::{
    app::{App, Screen},
    import::{Action, human_bytes},
};

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
    let regions = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(if area.height < 32 { 5 } else { 7 }),
        Constraint::Length(1),
        Constraint::Min(6),
        Constraint::Length(2),
        Constraint::Length(1),
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
    let hints = match app.screen {
        Screen::Settings => " Tab/↑↓ field   ←→ option   Ctrl+S save   Esc cancel",
        Screen::Sources => {
            " ↑↓/jk select   Enter preview   s source   r refresh   c config   ? help   q quit"
        }
        Screen::Preview => {
            " ↑↓/jk scroll   f folders/files   Enter confirm   r rescan   c config   Esc back"
        }
        Screen::Scanning | Screen::Importing => " Esc / Ctrl+C cancel safely   ? help",
        Screen::Done => " Enter new import   ↑↓ scroll   c config   ? help   q quit",
    };
    frame.render_widget(Paragraph::new(hints).style(muted()), regions[5]);
    if app.help {
        help(frame);
    }
    if let Some(input) = &app.source_input {
        source_dialog(frame, input, app);
    }
    if app.confirming {
        confirmation(frame, app);
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
    .margin(1)
    .split(inner);
    frame.render_widget(
        Paragraph::new("Choose where your photos live and how dates become folders.")
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
    let chunks = Layout::vertical([
        Constraint::Length(3),
        Constraint::Length(if has_warning { 2 } else { 0 }),
        Constraint::Min(4),
        Constraint::Length(1),
    ])
    .split(area);
    let metrics = Line::from(vec![
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
    ]);
    frame.render_widget(
        Paragraph::new(metrics).block(border("Import preview · no files copied yet")),
        chunks[0],
    );
    if has_warning {
        let text = format!(
            "{} photos use file modification dates (capture EXIF unavailable). {} unsupported files / symlinks ignored. Press f to inspect each photo.",
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
        let rows: Vec<_> = plan
            .folders()
            .into_iter()
            .map(|(path, copy, skip, bytes)| {
                Row::new(vec![
                    format!("{}/", path.display()),
                    copy.to_string(),
                    skip.to_string(),
                    human_bytes(bytes),
                ])
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
            plan.folders().get(index).map(|(path, _, _, _)| {
                format!("Destination: {}", plan.archive.join(path).display())
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
        Line::from(format!("  Archive: {}", app.config.archive.display())),
        Line::from(Span::styled(
            "  Your original files remain on the card.",
            muted(),
        )),
        Line::from(""),
        Line::from("  Press Enter to start another import."),
    ];
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
            plan.folders()
                .iter()
                .filter(|(_, copy, _, _)| *copy > 0)
                .count()
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

fn source_dialog(frame: &mut Frame, input: &str, app: &App) {
    let area = popup(frame, 76, 10);
    let lines = vec![
        Line::from(""),
        Line::from("  Enter the card's mount point or a photo directory:"),
        Line::from(""),
        Line::from(Span::styled(format!("  {input}▏"), accent())),
        Line::from(""),
        Line::from(Span::styled(
            "  Enter select · Esc cancel · Ctrl+U clear · ~/ supported",
            muted(),
        )),
        Line::from(Span::styled(app.message.as_str(), Style::default().fg(RED))),
    ];
    frame.render_widget(
        Paragraph::new(lines)
            .block(border("Choose source directory"))
            .wrap(Wrap { trim: false }),
        area,
    );
}

fn help(frame: &mut Frame) {
    let area = popup(frame, 80, 22);
    let lines = vec![
        Line::from(""),
        Line::from(Span::styled(
            "  A quick path from card to archive",
            accent(),
        )),
        Line::from("  1. Set your archive, folder layout, grouping, and date basis."),
        Line::from("  2. Select a detected card, or press s to enter a source."),
        Line::from("  3. Press Enter to scan and review destination folders."),
        Line::from("  4. Press Enter again, then confirm the import."),
        Line::from(""),
        Line::from("  ↑↓ / j k    Select or scroll     c    Configuration"),
        Line::from("  s           Choose source       r    Refresh / rescan"),
        Line::from("  f / Tab     Folders / files      q    Quit"),
        Line::from("  Esc         Back / cancel        ?    This help"),
        Line::from(""),
        Line::from(Span::styled("  Import behavior", accent())),
        Line::from("  Copies are verified; originals are preserved."),
        Line::from("  Identical files are skipped. Conflicts get a content suffix."),
        Line::from("  Missing capture EXIF uses file modification date, shown in preview."),
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
