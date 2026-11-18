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

/// Rewrite a file the way a real edit would: new contents and a newer timestamp.
fn rewrite(path: &Path, contents: &[u8]) {
    let before = fs::metadata(path).unwrap().modified().unwrap();
    fs::write(path, contents).unwrap();
    filetime::set_file_mtime(
        path,
        filetime::FileTime::from_system_time(before + std::time::Duration::from_secs(5)),
    )
    .unwrap();
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
    assert!(
        raw.destination
            .parent()
            .unwrap()
            .ends_with(Path::new("Trip").join(raw.layout_relative.file_name().unwrap()))
    );
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
        paths
            .iter()
            .any(|path| path.contains("Trip/") && path.contains("RAW")),
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
    rewrite(&source, b"modified"); // Same length, different digest.
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
    rewrite(&source, b"modified");
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
    assert!(import::build_plan(&card, &config, None, &tx, &AtomicBool::new(false)).is_err());
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

#[test]
fn excluded_new_folders_are_left_out_of_the_import() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    fs::write(card.join("DCIM/100CAM/DSC_0001.JPG"), b"jpeg photo content").unwrap();
    let mut preview = plan(&card, &config);
    assert_eq!(preview.new_folder_layouts().len(), 2);
    assert!(preview.folders().iter().all(|folder| folder.included));

    let raw = preview
        .folders()
        .into_iter()
        .find(|folder| folder.path.to_string_lossy().contains("RAW"))
        .unwrap();
    assert!(!preview.toggle_new_folder(&raw.layout));
    assert_eq!((preview.copy_count(), preview.excluded_count()), (1, 1));
    assert!(preview.bytes() < "raw photo contentjpeg photo content".len() as u64);
    assert!(
        !preview
            .folders()
            .into_iter()
            .find(|folder| folder.path == raw.path)
            .unwrap()
            .included
    );

    let summary = execute(preview);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (1, 0, 0));
    let archive = temp.path().join("archive");
    let created: Vec<_> = walkdir::WalkDir::new(&archive)
        .into_iter()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().is_file())
        .collect();
    assert_eq!(created.len(), 1);
    assert!(created[0].path().to_string_lossy().contains("JPEG"));
    assert!(!archive.join(&raw.path).exists());
}

#[test]
fn all_new_folders_can_be_excluded_and_restored() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    fs::write(card.join("DCIM/100CAM/DSC_0001.JPG"), b"jpeg photo content").unwrap();
    let mut preview = plan(&card, &config);
    preview.set_all_new_folders(false);
    assert_eq!((preview.copy_count(), preview.excluded_count()), (0, 2));
    let summary = execute(preview);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (0, 0, 0));
    assert_eq!(
        fs::read_dir(temp.path().join("archive")).unwrap().count(),
        0
    );

    let mut preview = plan(&card, &config);
    preview.set_all_new_folders(false);
    preview.set_all_new_folders(true);
    assert_eq!((preview.copy_count(), preview.excluded_count()), (2, 0));
}

#[test]
fn existing_folders_cannot_be_excluded() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/old.JPG"), b"already archived").unwrap();
    execute(plan(&card, &config));
    fs::write(card.join("DCIM/100CAM/new.ARW"), b"brand new raw").unwrap();
    let mut preview = plan(&card, &config);
    let existing = preview
        .folders()
        .into_iter()
        .find(|folder| folder.exists)
        .unwrap();
    assert!(preview.toggle_new_folder(&existing.layout));
    assert!(preview.excluded.is_empty());
    preview.set_all_new_folders(false);
    assert_eq!(preview.excluded_count(), 1);
    assert_eq!(preview.skip_count(), 1);
}

#[test]
fn exclusions_survive_custom_folder_remapping() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    fs::write(card.join("DCIM/100CAM/DSC_0001.JPG"), b"jpeg photo content").unwrap();
    let mut preview = plan(&card, &config);
    let raw = preview
        .folders()
        .into_iter()
        .find(|folder| folder.path.to_string_lossy().contains("RAW"))
        .unwrap();
    preview.toggle_new_folder(&raw.layout);
    let insert = import::SegmentInsert::AllNew {
        name: "Trip".into(),
    };
    import::remap_plan_destinations(&mut preview, Some(&insert), &AtomicBool::new(false)).unwrap();
    assert_eq!((preview.copy_count(), preview.excluded_count()), (1, 1));
    let moved = preview
        .folders()
        .into_iter()
        .find(|folder| folder.path.to_string_lossy().contains("RAW"))
        .unwrap();
    assert!(moved.path.to_string_lossy().contains("Trip/"));
    assert!(!moved.included);
}

#[test]
fn new_photos_are_not_hashed_until_they_are_copied() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/a.JPG"), b"first").unwrap();
    fs::write(card.join("DCIM/100CAM/b.ARW"), b"second").unwrap();
    let preview = plan(&card, &config);
    assert!(preview.photos.iter().all(|photo| photo.hash.is_none()));
    let summary = execute(preview);
    assert_eq!((summary.copied, summary.failed), (2, 0));
}

#[test]
fn importing_a_subset_does_not_reread_duplicates() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/old.JPG"), b"already archived").unwrap();
    execute(plan(&card, &config));
    fs::write(card.join("DCIM/100CAM/new.ARW"), b"brand new raw").unwrap();
    let preview = plan(&card, &config);
    assert_eq!((preview.copy_count(), preview.skip_count()), (1, 1));
    // The duplicate was proven identical during the scan; the import only copies
    // the new photo and counts the duplicate without reading it again.
    let summary = execute(preview);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (1, 1, 0));
}

#[test]
fn a_repeat_scan_reuses_what_the_first_import_learned() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/a.JPG"), b"first photo").unwrap();
    fs::write(card.join("DCIM/100CAM/b.ARW"), b"second photo").unwrap();
    let cache = Arc::new(import::ScanCache::default());
    let (tx, _rx) = mpsc::channel::<WorkerEvent>();
    let cancel = AtomicBool::new(false);
    let first = import::build_plan_cached(&card, &config, None, &tx, &cancel, &cache).unwrap();
    assert_eq!(execute(first).copied, 2);

    let again = import::build_plan_cached(&card, &config, None, &tx, &cancel, &cache).unwrap();
    assert!(
        again
            .photos
            .iter()
            .all(|photo| photo.action == Action::Skip)
    );
    assert!(again.photos.iter().all(|photo| photo.hash.is_some()));
    // A changed archive file must never be trusted from the cache.
    let destination = again.photos[0].destination.clone();
    rewrite(&destination, b"tampered.....");
    let rescanned = import::build_plan_cached(&card, &config, None, &tx, &cancel, &cache).unwrap();
    assert_ne!(rescanned.photos[0].action, Action::Skip);
}

#[test]
fn a_custom_folder_that_already_exists_cannot_stay_deselected() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    let mut preview = plan(&card, &config);
    let layout = preview.photos[0].layout_relative.clone();
    preview.toggle_new_folder(&layout);
    assert_eq!((preview.copy_count(), preview.excluded_count()), (0, 1));
    // The archive already has a folder where the custom name would put the photo.
    fs::create_dir_all(
        preview
            .archive
            .join(import::insert_segment_before_leaf(&layout, "Trip")),
    )
    .unwrap();
    let insert = import::SegmentInsert::AllNew {
        name: "Trip".into(),
    };
    import::remap_plan_destinations(&mut preview, Some(&insert), &AtomicBool::new(false)).unwrap();
    assert!(preview.folders().iter().all(|folder| folder.exists));
    assert!(preview.excluded.is_empty());
    assert_eq!((preview.copy_count(), preview.excluded_count()), (1, 0));
}

/// Minimal TIFF: `DateTimeOriginal` in the Exif sub-IFD, then a large unrelated
/// payload past the metadata, like a real raw file.
fn tiff_with_capture_date(big_endian: bool, date: &str, padding: usize) -> Vec<u8> {
    let u16b = |v: u16| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let u32b = |v: u32| {
        if big_endian {
            v.to_be_bytes()
        } else {
            v.to_le_bytes()
        }
    };
    let mut out = Vec::new();
    out.extend(if big_endian { b"MM\0*" } else { b"II*\0" });
    out.extend(u32b(8)); // IFD0
    out.extend(u16b(1)); // one entry: ExifIFD pointer
    out.extend(u16b(0x8769));
    out.extend(u16b(4));
    out.extend(u32b(1));
    out.extend(u32b(26)); // Exif IFD offset
    out.extend(u32b(0)); // no next IFD
    out.extend(u16b(1)); // Exif IFD: DateTimeOriginal
    out.extend(u16b(0x9003));
    out.extend(u16b(2));
    out.extend(u32b(20));
    out.extend(u32b(44)); // value offset
    out.extend(u32b(0));
    assert_eq!(out.len(), 44);
    out.extend(date.as_bytes());
    out.push(0);
    out.extend(std::iter::repeat_n(7u8, padding));
    out
}

#[test]
fn tiff_capture_dates_are_read_without_loading_the_file() {
    let temp = TempDir::new().unwrap();
    for (big_endian, name) in [(false, "le.DNG"), (true, "be.ARW")] {
        let path = temp.path().join(name);
        fs::write(
            &path,
            tiff_with_capture_date(big_endian, "2026:10:12 14:07:31", 4 << 20),
        )
        .unwrap();
        assert_eq!(
            media::capture_date(&path),
            NaiveDate::from_ymd_opt(2026, 10, 12),
            "{name}"
        );
    }
    // A TIFF with no date, or a damaged one, is "no date" rather than an error.
    let broken = temp.path().join("broken.DNG");
    fs::write(&broken, b"II*\0\xff\xff\xff\xff").unwrap();
    assert_eq!(media::capture_date(&broken), None);
}

#[cfg(unix)]
#[test]
fn remembered_hashes_survive_a_restart_without_rereading_files() {
    use std::os::unix::fs::PermissionsExt;
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    let photo = card.join("DCIM/100CAM/photo.JPG");
    fs::write(&photo, b"already archived").unwrap();
    execute(plan(&card, &config));
    let cache_file = temp.path().join("hash-cache.tsv");
    let cache = Arc::new(import::ScanCache::default());
    let (tx, _rx) = mpsc::channel::<WorkerEvent>();
    let cancel = AtomicBool::new(false);
    let learned = import::build_plan_cached(&card, &config, None, &tx, &cancel, &cache).unwrap();
    let archived = learned.photos[0].destination.clone();
    assert_eq!(learned.skip_count(), 1);
    cache.save(&cache_file);

    // Make both copies unreadable: only remembered hashes can still prove a match.
    for path in [&photo, &archived] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o000)).unwrap();
    }
    let restarted = Arc::new(import::ScanCache::load(&cache_file));
    let again = import::build_plan_cached(&card, &config, None, &tx, &cancel, &restarted);
    let unreadable = fs::File::open(&photo).is_err(); // false when running as root
    let without_memory = import::build_plan_cached(
        &card,
        &config,
        None,
        &tx,
        &cancel,
        &Arc::new(import::ScanCache::default()),
    );
    for path in [&photo, &archived] {
        fs::set_permissions(path, fs::Permissions::from_mode(0o644)).unwrap();
    }
    assert_eq!(again.unwrap().skip_count(), 1);
    if unreadable {
        assert!(without_memory.is_err(), "a cold scan has to read the files");
    }
}

#[test]
fn a_saved_hash_is_not_trusted_after_the_file_changes() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/photo.JPG"), b"original").unwrap();
    execute(plan(&card, &config));
    let cache = Arc::new(import::ScanCache::default());
    let (tx, _rx) = mpsc::channel::<WorkerEvent>();
    let cancel = AtomicBool::new(false);
    let learned = import::build_plan_cached(&card, &config, None, &tx, &cancel, &cache).unwrap();
    let cache_file = temp.path().join("hash-cache.tsv");
    cache.save(&cache_file);
    rewrite(&learned.photos[0].destination, b"changed!"); // Same size, newer date.
    let restarted = Arc::new(import::ScanCache::load(&cache_file));
    let again = import::build_plan_cached(&card, &config, None, &tx, &cancel, &restarted).unwrap();
    assert_ne!(again.photos[0].action, Action::Skip);
}

#[test]
fn a_missing_or_damaged_hash_cache_is_just_empty() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("hash-cache.tsv");
    for contents in [
        "",
        "not a cache",
        "shotctl-hash-cache 1\ngarbage\nmore\tgarbage\n",
    ] {
        fs::write(&path, contents).unwrap();
        import::ScanCache::load(&path).save(&path); // Neither step may panic.
    }
    import::ScanCache::load(&temp.path().join("absent.tsv"));
}

fn import_into_custom_folder(card: &Path, config: &Config, name: &str) -> Plan {
    let insert = import::SegmentInsert::AllNew { name: name.into() };
    let preview = plan_with_segment(card, config, Some(&insert));
    let archive = preview.archive.clone();
    let destinations: Vec<_> = preview
        .photos
        .iter()
        .map(|p| p.destination.clone())
        .collect();
    assert_eq!(execute(preview).failed, 0);
    let again = plan(card, config);
    assert!(destinations.iter().all(|d| d.starts_with(&archive)));
    again
}

#[test]
fn photos_imported_into_a_custom_folder_are_recognised_as_existing() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    fs::write(card.join("DCIM/100CAM/DSC_0002.ARW"), b"another raw photo").unwrap();
    // Imported earlier under `demo-trip`; this scan applies no custom folder at all.
    let preview = import_into_custom_folder(&card, &config, "demo-trip");
    assert_eq!((preview.copy_count(), preview.skip_count()), (0, 2));
    let folders = preview.folders();
    assert_eq!(folders.len(), 1);
    assert!(folders[0].exists && folders[0].path.to_string_lossy().contains("demo-trip"));
    assert_eq!((folders[0].copy, folders[0].skip), (0, 2));
    assert!(preview.new_folder_layouts().is_empty());
    let summary = execute(preview);
    assert_eq!((summary.copied, summary.skipped, summary.failed), (0, 2, 0));
}

#[test]
fn a_different_file_with_the_same_name_in_a_custom_folder_is_not_a_duplicate() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    let photo = card.join("DCIM/100CAM/DSC_0001.ARW");
    fs::write(&photo, b"raw photo content").unwrap();
    import_into_custom_folder(&card, &config, "demo-trip");
    // Same name and size, different bytes: a new photo that must not be skipped.
    rewrite(&photo, b"RAW PHOTO CONTENT");
    let preview = plan(&card, &config);
    assert_eq!((preview.copy_count(), preview.skip_count()), (1, 0));
    assert!(preview.folders().iter().all(|folder| !folder.exists));
}

#[test]
fn new_photos_still_get_a_new_folder_next_to_existing_custom_ones() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    import_into_custom_folder(&card, &config, "demo-trip");
    fs::write(card.join("DCIM/100CAM/DSC_0002.ARW"), b"brand new raw").unwrap();
    let preview = plan(&card, &config);
    assert_eq!((preview.copy_count(), preview.skip_count()), (1, 1));
    let folders = preview.folders();
    assert_eq!(folders.len(), 2);
    assert_eq!(folders.iter().filter(|folder| folder.exists).count(), 1);
    assert_eq!(preview.new_folder_layouts().len(), 1);
}

#[test]
fn applying_another_custom_name_still_recognises_the_earlier_import() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/DSC_0001.ARW"), b"raw photo content").unwrap();
    let mut preview = import_into_custom_folder(&card, &config, "demo-trip");
    let insert = import::SegmentInsert::AllNew {
        name: "other".into(),
    };
    import::remap_plan_destinations(&mut preview, Some(&insert), &AtomicBool::new(false)).unwrap();
    assert_eq!((preview.copy_count(), preview.skip_count()), (0, 1));
    assert!(preview.folders().iter().all(|folder| folder.exists));
}

#[test]
fn existing_folders_report_when_they_were_created() {
    let (temp, config) = fixture();
    let card = temp.path().join("card");
    fs::write(card.join("DCIM/100CAM/old.JPG"), b"already archived").unwrap();
    execute(plan(&card, &config));
    fs::write(card.join("DCIM/100CAM/new.ARW"), b"brand new raw").unwrap();
    let preview = plan(&card, &config);
    let today = Local::now().date_naive();
    let supported = fs::metadata(temp.path()).unwrap().created().is_ok();
    for folder in preview.folders() {
        if folder.exists {
            // Created by the import above. Platforms without creation dates say so.
            assert_eq!(folder.imported, supported.then_some(today));
        } else {
            assert_eq!(folder.imported, None);
        }
    }
}
