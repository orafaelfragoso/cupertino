//! The capture's files and command line (`cupertino_screenshot::files`, `settings`, `shot::Options`): the folder
//! each location sends files to, from `user-dirs.dirs`; time-stamped PNG files that never replace one already there;
//! the settings file read back as it was stored; and the arguments the server starts a capture with.
#![expect(
    clippy::disallowed_methods,
    reason = "a test makes and reads its own files on its own thread"
)]

use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cupertino_kit::geometry::Rect;
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_screenshot::files::{read_settings, store_settings, write_captures, write_new};
use cupertino_screenshot::settings::{Location, Mode, Settings};
use cupertino_screenshot::shot::{Options, StartMode};

/// A folder of the test's own, empty.
fn scratch(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("screenshot-files-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn a_location_sends_files_to_the_folder_user_dirs_names_else_the_default() {
    let home = Path::new("/home/someone");
    let dirs = vec![
        ("PICTURES".to_owned(), PathBuf::from("/home/someone/Old")),
        ("PICTURES".to_owned(), PathBuf::from("/home/someone/Shots")),
    ];
    assert_eq!(
        Location::Pictures.directory(home, &dirs),
        Some(PathBuf::from("/home/someone/Shots")),
        "the last assignment wins"
    );
    assert_eq!(
        Location::Desktop.directory(home, &dirs),
        Some(home.join("Desktop"))
    );
    assert_eq!(
        Location::Documents.directory(home, &[]),
        Some(home.join("Documents"))
    );
    assert_eq!(
        Location::Directory(PathBuf::from("/srv/shots")).directory(home, &dirs),
        Some(PathBuf::from("/srv/shots"))
    );
    assert_eq!(Location::Clipboard.directory(home, &dirs), None);
}

#[test]
fn captures_are_written_as_numbered_pngs_to_the_pictures_folder() {
    let home = scratch("captures");
    std::fs::create_dir_all(home.join(".config")).unwrap();
    std::fs::write(
        home.join(".config/user-dirs.dirs"),
        "XDG_PICTURES_DIR=\"$HOME/Shots\"\n",
    )
    .unwrap();
    // the only test of this binary that reads the environment
    std::env::set_var("HOME", &home);
    std::env::remove_var("XDG_CONFIG_HOME");
    let image = ImageResource {
        revision: 1,
        width: 3,
        height: 2,
        stride: 12,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: Arc::from(vec![255; 24]),
    };
    let written = write_captures(&Location::Pictures, &[image.clone(), image]).expect("written");
    assert_eq!(written.len(), 2);
    for path in &written {
        assert_eq!(
            path.parent(),
            Some(home.join("Shots").as_path()),
            "created in the folder user-dirs.dirs names"
        );
        let name = path.file_name().unwrap().to_string_lossy();
        assert!(
            name.starts_with("Screenshot ") && name.ends_with(".png"),
            "{name}"
        );
        let png = std::fs::read(path).unwrap();
        let decoded = cupertino_kit::image::decode(&png, u32::MAX).expect("a PNG");
        assert_eq!((decoded.width, decoded.height), (3, 2));
    }
    assert_ne!(
        written[0], written[1],
        "the second of one capture is numbered"
    );
}

#[test]
fn a_new_file_never_replaces_one_already_there() {
    let dir = scratch("new");
    let path = dir.join("Screenshot.png");
    write_new(&path, b"first").expect("written");
    let again = write_new(&path, b"second").expect_err("not replaced");
    assert_eq!(again.kind(), ErrorKind::AlreadyExists);
    assert_eq!(std::fs::read(&path).unwrap(), b"first");
    let left: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .map(|e| e.file_name())
        .collect();
    assert_eq!(left.len(), 1, "no hidden file stays: {left:?}");
}

#[test]
fn the_settings_are_read_back_as_they_were_stored() {
    let dir = scratch("settings");
    let path = dir.join("nested/screenshot.conf");
    let settings = Settings {
        location: Location::Directory(PathBuf::from("/srv/shots")),
        timer: 5,
        show_thumbnail: false,
        show_pointer: true,
        remember_selection: true,
        mode: Mode::Window,
        selection: Some(Rect::new(10.0, 20.0, 300.0, 200.0)),
    };
    store_settings(&path, &settings).expect("stored");
    let (read, problems) = read_settings(&path).expect("read");
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(read, settings);

    let (parsed, problems) = Settings::parse("location = nowhere\ntimer = 5\n");
    assert_eq!(problems.len(), 1, "the bad line is reported: {problems:?}");
    assert_eq!(
        parsed.location,
        Settings::default().location,
        "and left at its default"
    );
    assert_eq!(parsed.timer, 5);
}

#[test]
fn a_capture_starts_with_the_mode_and_the_clipboard_the_server_names() {
    let parse = |args: &[&str]| Options::parse(args.iter().map(|a| a.to_string()));
    assert_eq!(
        parse(&["--mode", "screen", "--clipboard"]),
        Ok(Options {
            mode: StartMode::Screen,
            clipboard: true
        })
    );
    assert_eq!(
        parse(&["--mode", "toolbar"]),
        Ok(Options {
            mode: StartMode::Toolbar,
            clipboard: false
        })
    );
    assert_eq!(
        parse(&["--clipboard", "--mode", "selection"]).map(|o| o.mode),
        Ok(StartMode::Selection)
    );
    assert!(parse(&[]).is_err(), "a mode is required");
    assert!(parse(&["--mode"]).is_err());
    assert!(parse(&["--mode", "window"]).is_err());
    assert!(parse(&["--serve"]).is_err(), "the service is not a capture");
}
