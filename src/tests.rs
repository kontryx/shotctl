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
    plan_with_segment(source, config, None)
}

fn plan_with_segment(
    source: &Path,
    config: &Config,
    segment_insert: Option<&import::SegmentInsert>,
) -> Plan {
    let (tx, _rx) = mpsc::channel::<WorkerEvent>();
    import::build_plan(source, config, segment_insert, &tx, &AtomicBool::new(false)).unwrap()
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
    assert!(first.folders().iter().all(|folder| !folder.exists));
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
    assert!(second.folders().iter().all(|folder| folder.exists));
    let summary = execute(second);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (0, 2, 0));
}

#[test]
fn custom_folder_name_rejects_path_separators() {
    assert!(
        import::validate_one_time_folder("bad/name").is_err(),
        "path separators must be rejected"
    );
}

#[test]
fn remapping_custom_folder_leaves_unaffected_photos_unchanged() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/keep.JPG"), b"jpeg").unwrap();
    fs::write(card.join("DCIM/100CAM/move.ARW"), b"raw").unwrap();
    let mut preview = plan(&card, &config);
    let jpeg = preview
        .photos
        .iter()
        .find(|p| p.source.extension().unwrap() == "JPG")
        .unwrap();
    let jpeg_destination = jpeg.destination.clone();
    let jpeg_action = jpeg.action;
    let raw_layout = preview
        .photos
        .iter()
        .find(|p| p.source.extension().unwrap() == "ARW")
        .unwrap()
        .layout_relative
        .clone();
    let insert = import::SegmentInsert::Selected {
        folders: vec![raw_layout],
        name: "Trip".into(),
    };
    import::remap_plan_destinations(&mut preview, Some(&insert), &AtomicBool::new(false)).unwrap();
    let jpeg = preview
        .photos
        .iter()
        .find(|p| p.source.extension().unwrap() == "JPG")
        .unwrap();
    assert_eq!(jpeg.destination, jpeg_destination);
    assert_eq!(jpeg.action, jpeg_action);
    let raw = preview
        .photos
        .iter()
        .find(|p| p.source.extension().unwrap() == "ARW")
        .unwrap();
    assert!(raw
        .destination
        .parent()
        .unwrap()
        .ends_with(Path::new("Trip").join(raw.layout_relative.file_name().unwrap())));
}

#[test]
fn selected_folder_inserts_segment_before_leaf() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    let baseline = plan(&card, &config);
    let from = baseline.photos[0]
        .destination
        .parent()
        .unwrap()
        .strip_prefix(&baseline.archive)
        .unwrap()
        .to_owned();
    let insert = import::SegmentInsert::Selected {
        folders: vec![from.clone()],
        name: "CustomShoot".into(),
    };
    let updated = plan_with_segment(&card, &config, Some(&insert));
    let relative = updated.photos[0]
        .destination
        .parent()
        .unwrap()
        .strip_prefix(&updated.archive)
        .unwrap();
    assert_eq!(
        relative,
        import::insert_segment_before_leaf(&from, "CustomShoot")
    );
}

#[test]
fn all_new_folders_insert_skips_existing_destinations() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/old.JPG"), b"already archived").unwrap();
    execute(plan(&card, &config));
    fs::write(card.join("DCIM/100CAM/new.ARW"), b"brand new raw").unwrap();
    let insert = import::SegmentInsert::AllNew {
        name: "Trip".into(),
    };
    let preview = plan_with_segment(&card, &config, Some(&insert));
    let paths: Vec<_> = preview
        .folders()
        .into_iter()
        .map(|folder| folder.path.display().to_string())
        .collect();
    assert!(
        paths.iter().any(|path| path.contains("Trip/") && path.contains("RAW")),
        "new RAW folder should include the custom segment: {paths:?}"
    );
    assert!(
        paths
            .iter()
            .any(|path| !path.contains("Trip/") && path.contains("JPEG")),
        "existing JPEG folder should keep its natural path: {paths:?}"
    );
}

#[test]
fn scans_prune_system_folders_and_keep_ordinary_hidden_photo_folders() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/photo.JPG"), b"camera photo").unwrap();
    fs::create_dir(card.join(".hidden-shoot")).unwrap();
    fs::write(card.join(".hidden-shoot/hidden.JPG"), b"hidden photo").unwrap();
    let system_folders = [
        ".Trashes",
        ".Spotlight-V100",
        ".fseventsd",
        ".TemporaryItems",
        ".DocumentRevisions-V100",
        "$RECYCLE.BIN",
        "System Volume Information",
    ];
    for name in system_folders {
        let directory = card.join(name).join("nested");
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("unwanted.JPG"), b"discarded photo").unwrap();
    }
    let preview = plan(&card, &config);
    assert_eq!(preview.copy_count(), 2);
    assert_eq!(preview.skipped_entries, system_folders.len());
    assert!(
        preview
            .photos
            .iter()
            .all(|photo| photo.source.file_name().unwrap() != "unwanted.JPG")
    );
    assert!(
        preview
            .photos
            .iter()
            .any(|photo| photo.source.ends_with(".hidden-shoot/hidden.JPG"))
    );
}

#[cfg(unix)]
#[test]
fn protected_trash_does_not_block_scans_but_unreadable_photo_folders_do() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/photo.JPG"), b"camera photo").unwrap();
    let trash = card.join(".Trashes");
    fs::create_dir(&trash).unwrap();
    fs::write(trash.join("deleted.JPG"), b"deleted photo").unwrap();
    let original_permissions = fs::metadata(&trash).unwrap().permissions();
    fs::set_permissions(&trash, fs::Permissions::from_mode(0o0)).unwrap();
    let (tx, _rx) = mpsc::channel();
    let result = import::build_plan(&card, &config, None, &tx, &AtomicBool::new(false));
    fs::set_permissions(&trash, original_permissions).unwrap();
    let preview = result.unwrap();
    assert_eq!(preview.copy_count(), 1);
    assert_eq!(preview.skipped_entries, 1);

    let shoot = card.join("Unreadable Shoot");
    fs::create_dir(&shoot).unwrap();
    fs::write(shoot.join("photo.JPG"), b"another photo").unwrap();
    let original_permissions = fs::metadata(&shoot).unwrap().permissions();
    fs::set_permissions(&shoot, fs::Permissions::from_mode(0o0)).unwrap();
    let permission_denied = fs::read_dir(&shoot).is_err();
    let result = import::build_plan(&card, &config, None, &tx, &AtomicBool::new(false));
    fs::set_permissions(&shoot, original_permissions).unwrap();
    if permission_denied {
        assert!(format!("{:#}", result.unwrap_err()).contains("Unreadable Shoot"));
    }
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
    assert!(
        import::build_plan(&card, &config, None, &tx, &AtomicBool::new(false)).is_err()
    );
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
    app.import_archive = Some(config.archive.clone());
    app.summary = Some(import::Summary {
        copied: 1,
        ..import::Summary::default()
    });
    for (width, height) in [(66, 22), (80, 24), (120, 30), (120, 40)] {
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
            if screen == Screen::Done {
                assert!(
                    text.contains("[o] Open archive"),
                    "Open archive shortcut hidden at {width}×{height}:\n{text}"
                );
                for (cancelled, failed) in [(true, 0), (false, 1)] {
                    let summary = app.summary.as_mut().unwrap();
                    summary.cancelled = cancelled;
                    summary.failed = failed;
                    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
                    let buffer = terminal.backend().buffer();
                    let text = (0..height)
                        .map(|y| {
                            (0..width)
                                .map(|x| buffer[(x, y)].symbol())
                                .collect::<String>()
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                    assert!(!text.contains("[o] Open archive"));
                    assert!(!text.contains("Press o to open"));
                }
                let summary = app.summary.as_mut().unwrap();
                summary.cancelled = false;
                summary.failed = 0;
            }
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
        app.screen = Screen::Settings;
        app.directory_browser = Some(
            crate::directories::DirectoryBrowser::open(temp.path().to_str().unwrap()).unwrap(),
        );
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Browse archive folders",
            "archive/",
            "card/",
            "Space use current",
            "Tab use selected",
            "Esc cancel",
            "Backspace parent",
            "[n] New folder here",
        ] {
            assert!(
                text.contains(expected),
                "Browser control hidden at {width}×{height}: {expected}\n{text}"
            );
        }
        let footer = (0..width)
            .map(|x| buffer[(x, height - 2)].symbol())
            .collect::<String>();
        assert!(
            footer.contains("[n] New folder here"),
            "New-folder shortcut missing from footer at {width}×{height}:\n{text}"
        );
        assert_eq!(text.matches("[n] New folder here").count(), 1);
        assert!(!text.contains("Complete path"));
        for expected in [
            "[↑↓] Select",
            "[Enter/→] Open",
            "[←/Backspace] Parent",
            "[Space] Use current",
            "[Tab] Use selected",
            "[Esc] Cancel",
        ] {
            assert!(
                text.contains(expected),
                "Browser shortcut obscured at {width}×{height}: {expected}\n{text}"
            );
        }
        let browser = app.directory_browser.as_mut().unwrap();
        browser.new_folder_input = Some("New Photo Archive".into());
        browser.error = "A file or folder named 'Existing' already exists.".into();
        terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
        let buffer = terminal.backend().buffer();
        let text = (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n");
        for expected in [
            "Create new folder",
            "Create inside:",
            "Folder name:",
            "New Photo Archive",
            "already exists",
            "[Enter] Create & open",
            "[Esc] Cancel",
        ] {
            assert!(
                text.contains(expected),
                "New folder control hidden at {width}×{height}: {expected}\n{text}"
            );
        }
        app.directory_browser = None;
    }
    let mut terminal = Terminal::new(TestBackend::new(30, 8)).unwrap();
    terminal.draw(|frame| ui::draw(frame, &mut app)).unwrap();
}
