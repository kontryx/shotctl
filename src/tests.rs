use std::{
    fs,
    path::Path,
    sync::{Arc, atomic::AtomicBool, mpsc},
};

use chrono::{Local, NaiveDate, TimeZone};
use tempfile::TempDir;

use crate::{
    config::{Config, DateBasis, Grouping, Layout},
    import::{self, Action, Plan, WorkerEvent},
    media,
};

fn fixture() -> (TempDir, Config) {
    let temp = TempDir::new().unwrap();
    fs::create_dir_all(temp.path().join("card/DCIM/100CAM")).unwrap();
    fs::create_dir(temp.path().join("archive")).unwrap();
    let config = Config {
        archive: temp.path().join("archive"),
        date_basis: DateBasis::Import,
        ..Config::default()
    };
    (temp, config)
}

fn plan(source: &Path, config: &Config) -> Plan {
    let (tx, _rx) = mpsc::channel::<WorkerEvent>();
    import::build_plan(source, config, &tx, &AtomicBool::new(false)).unwrap()
}

fn execute(plan: Plan) -> import::Summary {
    let (tx, _rx) = mpsc::channel();
    import::execute_plan(plan, &tx, Arc::new(AtomicBool::new(false)))
}

#[test]
fn every_layout_and_grouping_matches_the_documented_structure() {
    let date = NaiveDate::from_ymd_opt(2026, 10, 22).unwrap();
    for (layout, base) in [
        (Layout::MonthDate, "10/2026-10-22"),
        (Layout::YearMonthDate, "2026/10/2026-10-22"),
        (Layout::Date, "2026-10-22"),
    ] {
        for (grouping, suffix) in [
            (Grouping::Separate, "-RAW"),
            (Grouping::Subfolders, "/RAW"),
            (Grouping::Together, ""),
        ] {
            let config = Config {
                archive: "/example-archive".into(),
                layout,
                grouping,
                ..Config::default()
            };
            assert_eq!(
                config.directory(date, "RAW"),
                Path::new("/example-archive").join(format!("{base}{suffix}"))
            );
        }
    }
}

#[test]
fn first_import_verifies_bytes_and_second_import_skips_duplicates() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    let original = card.join("DCIM/100CAM/DSC_0001.ARW");
    fs::write(&original, b"raw photo content").unwrap();
    fs::write(card.join("DCIM/100CAM/DSC_0001.JPG"), b"jpeg photo content").unwrap();
    fs::write(card.join("notes.txt"), b"ignored").unwrap();
    let first = plan(&card, &config);
    assert_eq!(first.copy_count(), 2);
    assert_eq!(first.folders().len(), 2);
    assert_eq!(first.skipped_entries, 1);
    let destination = first
        .photos
        .iter()
        .find(|p| p.source == original.canonicalize().unwrap())
        .unwrap()
        .destination
        .clone();
    let summary = execute(first);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (2, 0, 0));
    assert_eq!(fs::read(destination).unwrap(), fs::read(original).unwrap());
    let second = plan(&card, &config);
    assert_eq!(second.copy_count(), 0);
    assert_eq!(second.skip_count(), 2);
    let summary = execute(second);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (0, 2, 0));
}

#[test]
fn filename_conflicts_are_stable_and_never_overwrite() {
    let (temp, mut config) = fixture();
    config.grouping = Grouping::Together;
    let card = temp.path().join("card");
    fs::create_dir(card.join("other")).unwrap();
    fs::write(card.join("DCIM/100CAM/IMG.JPG"), b"first image").unwrap();
    fs::write(card.join("other/IMG.JPG"), b"second image").unwrap();
    let first = plan(&card, &config);
    assert_eq!(first.rename_count(), 1);
    assert_ne!(first.photos[0].destination, first.photos[1].destination);
    let destinations: Vec<_> = first.photos.iter().map(|p| p.destination.clone()).collect();
    assert_eq!(execute(first).copied, 2);
    let again = plan(&card, &config);
    assert!(again.photos.iter().all(|p| p.action == Action::Skip));
    assert_eq!(
        again
            .photos
            .iter()
            .map(|p| p.destination.clone())
            .collect::<Vec<_>>(),
        destinations
    );
}

#[test]
fn identical_names_and_contents_within_a_card_copy_once() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::create_dir(card.join("second")).unwrap();
    fs::write(card.join("DCIM/100CAM/A.JPG"), b"identical").unwrap();
    fs::write(card.join("second/A.JPG"), b"identical").unwrap();
    let preview = plan(&card, &config);
    assert_eq!((preview.copy_count(), preview.skip_count()), (1, 1));
    let summary = execute(preview);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (1, 1, 0));
}

#[test]
fn source_changed_after_preview_is_rejected_and_partial_is_removed() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    let source = card.join("photo.JPG");
    fs::write(&source, b"original").unwrap();
    let preview = plan(&card, &config);
    let destination = preview.photos[0].destination.clone();
    fs::write(&source, b"modified").unwrap(); // Same length, different digest.
    let summary = execute(preview);
    assert_eq!(summary.failed, 1);
    assert!(!destination.exists());
    assert_eq!(
        fs::read_dir(destination.parent().unwrap()).unwrap().count(),
        0
    );
    assert!(source.exists());
}

#[test]
fn destination_created_after_preview_is_preserved() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("photo.JPG"), b"incoming photo").unwrap();
    let preview = plan(&card, &config);
    let destination = preview.photos[0].destination.clone();
    fs::create_dir_all(destination.parent().unwrap()).unwrap();
    fs::write(&destination, b"existing photo").unwrap();
    assert_eq!(execute(preview).failed, 1);
    assert_eq!(fs::read(destination).unwrap(), b"existing photo");
}

#[test]
fn a_duplicate_source_changed_after_preview_is_reported() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    let source = card.join("photo.JPG");
    fs::write(&source, b"original").unwrap();
    assert_eq!(execute(plan(&card, &config)).copied, 1);
    let preview = plan(&card, &config);
    assert_eq!(preview.skip_count(), 1);
    fs::write(&source, b"modified").unwrap();
    let summary = execute(preview);
    assert_eq!((summary.skipped, summary.failed), (0, 1));
}

#[test]
fn cancellation_does_not_publish_or_remove_photos() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("photo.JPG"), b"incoming photo").unwrap();
    let preview = plan(&card, &config);
    let destination = preview.photos[0].destination.clone();
    let (tx, _rx) = mpsc::channel();
    let summary = import::execute_plan(preview, &tx, Arc::new(AtomicBool::new(true)));
    assert!(summary.cancelled);
    assert_eq!(summary.copied, 0);
    assert!(!destination.exists());
    assert!(card.join("photo.JPG").exists());
}

#[test]
fn overlapping_source_and_archive_are_rejected() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    assert!(import::validate_roots(&card, &card).is_err());
    assert!(import::validate_roots(&card, &card.join("archive")).is_err());
    assert!(import::validate_roots(temp.path(), &config.archive).is_err());
}

#[test]
fn capture_metadata_and_modification_date_fallback_are_distinct() {
    let (temp, mut config) = fixture();
    config.date_basis = DateBasis::Capture;
    let card = temp.path().join("card");
    // Minimal little-endian TIFF with an EXIF IFD containing DateTimeOriginal.
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II");
    tiff.extend_from_slice(&42u16.to_le_bytes());
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x8769u16.to_le_bytes());
    tiff.extend_from_slice(&4u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&26u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&0x9003u16.to_le_bytes());
    tiff.extend_from_slice(&2u16.to_le_bytes());
    tiff.extend_from_slice(&20u32.to_le_bytes());
    tiff.extend_from_slice(&44u32.to_le_bytes());
    tiff.extend_from_slice(&0u32.to_le_bytes());
    tiff.extend_from_slice(b"2026:10:22 12:34:56\0");
    fs::write(card.join("with-exif.DNG"), tiff).unwrap();
    assert_eq!(
        media::capture_date(&card.join("with-exif.DNG")),
        NaiveDate::from_ymd_opt(2026, 10, 22)
    );
    let missing = card.join("no-exif.JPG");
    fs::write(&missing, b"no exif data").unwrap();
    let modified = Local.with_ymd_and_hms(2025, 3, 14, 12, 0, 0).unwrap();
    filetime::set_file_mtime(
        &missing,
        filetime::FileTime::from_unix_time(modified.timestamp(), 0),
    )
    .unwrap();
    let preview = plan(&card, &config);
    assert_eq!(preview.fallback_count(), 1);
    assert!(preview.photos.iter().any(|p| {
        p.fallback
            && p.destination
                .to_string_lossy()
                .contains("03/2025-03-14-JPEG")
    }));
    assert!(preview.photos.iter().any(|p| {
        !p.fallback
            && p.destination
                .to_string_lossy()
                .contains("10/2026-10-22-RAW")
    }));
}

#[test]
fn configuration_persists_and_creates_archive() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("config/shotctl.toml");
    assert!(Config::load(&path).unwrap().is_none());
    let config = Config {
        archive: temp.path().join("new/archive"),
        layout: Layout::YearMonthDate,
        grouping: Grouping::Together,
        date_basis: DateBasis::Import,
    };
    config.save(&path).unwrap();
    let loaded = Config::load(&path).unwrap().unwrap();
    assert!(loaded.archive.is_dir());
    assert_eq!(loaded.layout, config.layout);
    assert_eq!(loaded.grouping, config.grouping);
    assert_eq!(loaded.date_basis, config.date_basis);
}

#[cfg(unix)]
#[test]
fn symlinks_cannot_route_imports_outside_the_archive() {
    use std::os::unix::fs::symlink;
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("photo.JPG"), b"incoming photo").unwrap();
    let outside = temp.path().join("outside");
    fs::create_dir(&outside).unwrap();
    let month = Local::now().format("%m").to_string();
    symlink(&outside, config.archive.join(month)).unwrap();
    let (tx, _rx) = mpsc::channel();
    assert!(import::build_plan(&card, &config, &tx, &AtomicBool::new(false)).is_err());
    assert_eq!(fs::read_dir(outside).unwrap().count(), 0);
    assert!(import::validate_roots(&card, &card.join("DCIM")).is_err());
}

#[test]
fn terminal_screens_keep_configuration_and_setup_controls_visible() {
    use crate::{
        app::{App, Screen},
        ui,
    };
    use ratatui::{Terminal, backend::TestBackend};

    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("photo.JPG"), b"jpeg data").unwrap();
    let mut app = App::new(temp.path().join("config.toml"), Some(card.clone())).unwrap();
    app.config = config.clone();
    app.draft = config.clone();
    app.archive_input = config.archive.display().to_string();
    app.plan = Some(plan(&card, &config));
    app.summary = Some(import::Summary {
        copied: 1,
        ..import::Summary::default()
    });
    for (width, height) in [(66, 22), (80, 24), (120, 40)] {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        for screen in [
            Screen::Settings,
            Screen::Sources,
            Screen::Scanning,
            Screen::Preview,
            Screen::Importing,
            Screen::Done,
        ] {
            app.screen = screen;
            terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
            let buffer = terminal.backend().buffer();
            let mut text = String::new();
            for y in 0..height {
                for x in 0..width {
                    text.push_str(buffer[(x, y)].symbol());
                }
                text.push('\n');
            }
            assert!(text.contains("Archive") && text.contains("Layout") && text.contains("Dates"));
            if screen == Screen::Settings {
                assert!(
                    text.contains("Save configuration"),
                    "Save control hidden at {width}×{height}:\n{text}"
                );
                assert!(text.contains("Organize by"));
            }
            if screen == Screen::Preview {
                let relative = app.plan.as_ref().unwrap().photos[0]
                    .destination
                    .parent()
                    .unwrap()
                    .strip_prefix(&app.plan.as_ref().unwrap().archive)
                    .unwrap()
                    .display()
                    .to_string();
                assert!(
                    text.contains(&relative),
                    "Folder hidden at {width}×{height}:\n{text}"
                );
                if width == 80 {
                    println!("Actual 80×24 preview:\n{text}");
                }
            }
        }
        app.help = true;
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        app.help = false;
        app.screen = Screen::Preview;
        app.confirming = true;
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        app.confirming = false;
        app.source_input = Some("/Volumes/CAMERA".into());
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        app.source_input = None;
    }
    let mut terminal = Terminal::new(TestBackend::new(30, 8)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
}
