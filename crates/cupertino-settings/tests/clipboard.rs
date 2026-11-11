//! Clipboard Access in the Privacy pane: what it shows of `data-control.conf` and what its buttons and sheets write.

use cupertino_apps::entry::{parse_desktop_entry, Locale, Visibility};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::data_control::{DataControlConf, Entry};
use cupertino_settings::clipboard::{self, Installed, Reason};
use cupertino_settings::controls::{Control, Value};
use cupertino_settings::live::Live;
use cupertino_settings::panes::{change, sections, ControlId, Effect, Row, Session};
use cupertino_settings::settings::{File, Paths, Settings};
use cupertino_settings::sheet::{Kind, Outcome, Sheet, SheetKey};
use cupertino_settings::PaneId;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

const LIGHT: Session = Session {
    dark: false,
    display: DisplayOptions {
        reduce_motion: false,
        reduce_transparency: false,
        increase_contrast: false,
        differentiate_without_color: false,
    },
};

struct Fixture {
    dir: tempfile::TempDir,
    paths: Paths,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("bin")).unwrap();
        let paths = Paths {
            dock: None,
            compositor: None,
            menubar: None,
            keyboard: None,
            power: None,
            accessibility: None,
            system_accessibility: None,
            data_control: Some(dir.path().join("data-control.conf")),
            screen_sharing: None,
            desktop_control: None,
            notifications: None,
            screen_reader: None,
        };
        Fixture { dir, paths }
    }

    fn conf(&self) -> PathBuf {
        self.paths.data_control.clone().unwrap()
    }

    /// An executable `bin/<name>` of this user holding `text`.
    fn program(&self, name: &str, text: &str) -> PathBuf {
        let path = self.dir.path().join("bin").join(name);
        std::fs::write(&path, text).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn write_conf(&self, text: &str) {
        std::fs::write(self.conf(), text).unwrap();
        std::fs::set_permissions(self.conf(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    fn settings(&self) -> Settings {
        Settings::load(&self.paths, 1080.0).unwrap()
    }

    fn installed(&self, entries: &[(&str, &str)]) -> Vec<Installed> {
        let vis = Visibility {
            skip_try_exec: true,
            ..Visibility::default()
        };
        let entries = entries
            .iter()
            .map(|(id, text)| parse_desktop_entry(id, text, &Locale::parse("C"), &vis).unwrap())
            .collect();
        clipboard::installed(entries, &[self.dir.path().join("bin")])
    }
}

fn desktop(name: &str, exec: &str) -> String {
    format!("[Desktop Entry]\nType=Application\nName={name}\nIcon={name}-icon\nExec={exec}\n")
}

fn clipboard_rows(settings: &Settings, live: &Live) -> Vec<Row> {
    sections(PaneId::Privacy, settings, LIGHT, live)
        .into_iter()
        .next()
        .unwrap()
        .rows
}

fn titles(control: &Control) -> Vec<String> {
    match control {
        Control::Button { title } => vec![title.clone()],
        Control::Buttons { titles } => titles.clone(),
        other => panic!("not a button: {other:?}"),
    }
}

#[test]
fn the_rows_are_the_entries_named_by_their_applications_or_their_file() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    let tool = f.program("tool", "x");
    f.write_conf(&format!(
        "allow = {}\nallow = {}\n",
        Entry::for_program(&clipd).unwrap().text(),
        Entry::for_program(&tool).unwrap().text()
    ));
    let live = Live {
        apps: f.installed(&[("clipd.desktop", &desktop("Clip Daemon", "clipd --watch %U"))]),
        ..Live::default()
    };
    let rows = clipboard_rows(&f.settings(), &live);
    let shown: Vec<(&str, Option<&str>, Option<&str>)> = rows
        .iter()
        .map(|r| (r.label.as_str(), r.icon.as_deref(), r.error.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            ("Clip Daemon", Some("Clip Daemon-icon"), None),
            ("tool", Some(""), None)
        ],
        "the second has no application: its file name and the generic icon"
    );
    assert_eq!(rows[0].detail.as_deref(), Some(clipd.to_str().unwrap()));
    assert_eq!(titles(&rows[0].control), ["Remove"]);
}

#[test]
fn remove_asks_first_and_only_the_confirmed_removal_writes() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    let mut settings = f.settings();
    settings.allow_clipboard(&f.paths, &clipd).unwrap();
    let before = std::fs::read(f.conf()).unwrap();

    let effects = change(
        ControlId::ClipboardApp(0),
        Value::Index(0),
        &mut settings,
        &Live::default(),
    );
    let [Effect::ConfirmUnallow { program, name }] = effects.as_slice() else {
        panic!("Remove asks first: {effects:?}");
    };
    assert_eq!(name, "clipd");
    let mut sheet = Sheet::unallow(program, name);
    assert_eq!(
        sheet.title(),
        "Remove \u{201c}clipd\u{201d} from Clipboard Access?"
    );
    assert_eq!(
        sheet.clone().key(SheetKey::Return),
        Outcome::Cancel,
        "Return cancels"
    );
    assert_eq!(
        std::fs::read(f.conf()).unwrap(),
        before,
        "cancelled: the same bytes"
    );

    let l = sheet.layout(&cupertino_settings::geom::Rect::new(
        216.0, 52.0, 499.0, 573.0,
    ));
    let Outcome::Unallow(program) = sheet.press(&l, l.action.center()) else {
        panic!("the action removes");
    };
    settings.unallow_clipboard(&f.paths, &program).unwrap();
    assert_eq!(DataControlConf::load(&f.conf()).unwrap().0.allowed, []);
}

#[test]
fn allowing_stores_a_system_program_by_path_and_a_users_program_pinned_by_its_canonical_path() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    let link = f.dir.path().join("bin").join("clip-link");
    std::os::unix::fs::symlink(&clipd, &link).unwrap();
    let mut settings = f.settings();
    settings.allow_clipboard(&f.paths, &link).unwrap();
    let system = std::fs::canonicalize("/usr/bin/true").unwrap();
    settings.allow_clipboard(&f.paths, &system).unwrap();

    let allowed = DataControlConf::load(&f.conf()).unwrap().0.allowed;
    assert_eq!(allowed[0].program, clipd, "the link's file, not the link");
    assert!(allowed[0].pin.is_some(), "this user can change it: pinned");
    assert_eq!(
        allowed[1],
        Entry {
            program: system,
            pin: None
        }
    );
}

#[test]
fn a_changed_file_shows_as_changed_and_allow_again_pins_it_anew() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    let mut settings = f.settings();
    settings.allow_clipboard(&f.paths, &clipd).unwrap();
    let old_pin = settings.clipboard.conf.allowed[0].pin;
    f.program("clipd", "a different program");

    let row = &clipboard_rows(&settings, &Live::default())[0];
    assert_eq!(row.error.as_deref(), Some("Changed since you allowed it"));
    assert_eq!(titles(&row.control), ["Allow Again", "Remove"]);

    let effects = change(
        ControlId::ClipboardApp(0),
        Value::Index(0),
        &mut settings,
        &Live::default(),
    );
    assert_eq!(effects, [Effect::Store(File::DataControl)]);
    settings.store(&f.paths, File::DataControl).unwrap();
    let allowed = DataControlConf::load(&f.conf()).unwrap().0.allowed;
    assert_ne!(allowed[0].pin, old_pin);
    assert_eq!(clipboard::state(&allowed[0]), clipboard::State::Allowed);
}

#[test]
fn a_path_only_entry_this_user_can_change_shows_as_changed_and_a_missing_one_as_not_found() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    f.write_conf(&format!(
        "allow = {}\nallow = /opt/gone/clipman\n",
        clipd.display()
    ));
    let errors: Vec<Option<String>> = clipboard_rows(&f.settings(), &Live::default())
        .into_iter()
        .map(|r| r.error)
        .collect();
    assert_eq!(
        errors,
        [
            Some("Changed since you allowed it".to_string()),
            Some("Not found".to_string())
        ]
    );
}

#[test]
fn the_chooser_disables_what_cannot_be_allowed_with_its_reason_and_the_arrows_skip_it() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    f.program("python3", "x");
    f.program("flatpak", "x");
    f.program("hello.sh", "#!/bin/sh\necho hi\n");
    let editor = f.program("editor", "x");
    let mut settings = f.settings();
    settings.allow_clipboard(&f.paths, &clipd).unwrap();
    let installed = f.installed(&[
        ("a.desktop", &desktop("A Clipboard", "clipd")),
        ("b.desktop", &desktop("B Python", "python3 -m tool")),
        (
            "c.desktop",
            &desktop("C Flatpak", "flatpak run org.example.App"),
        ),
        ("d.desktop", &desktop("D Script", "hello.sh")),
        ("e.desktop", &desktop("E Editor", "editor %F")),
        ("f.desktop", &desktop("F Snap", "env BAMF=1 /snap/bin/f")),
        ("g.desktop", &desktop("G Editor Again", "env LANG=C editor")),
    ]);
    let apps = clipboard::apps(&installed, &settings.clipboard, str::cmp);
    let shown: Vec<(&str, Option<Reason>)> =
        apps.iter().map(|a| (a.name.as_str(), a.reason)).collect();
    assert_eq!(
        shown,
        [
            ("A Clipboard", Some(Reason::AlreadyAllowed)),
            ("B Python", Some(Reason::RunsScripts)),
            ("C Flatpak", Some(Reason::Sandboxed)),
            ("D Script", Some(Reason::RunsScripts)),
            ("E Editor", None),
            ("F Snap", Some(Reason::Sandboxed)),
        ],
        "one row per program: the second editor entry is the same file"
    );

    let mut sheet = Sheet::apps(apps);
    for key in [SheetKey::Down, SheetKey::Down, SheetKey::Up, SheetKey::Home] {
        assert_eq!(sheet.key(key), Outcome::Stay);
        let Kind::Apps { selected, .. } = &sheet.kind else {
            unreachable!()
        };
        assert_eq!(*selected, Some(4), "only E Editor can be selected");
    }
    // Return in the list grants nothing (§3)
    assert_eq!(sheet.key(SheetKey::Return), Outcome::Stay);
    assert_eq!(
        std::fs::read_to_string(f.conf()).unwrap().lines().count(),
        1
    );

    let l = sheet.layout(&cupertino_settings::geom::Rect::new(
        216.0, 52.0, 499.0, 573.0,
    ));
    assert_eq!(sheet.press(&l, l.action.center()), Outcome::Program(editor));
}

#[test]
fn a_script_picked_as_a_program_is_refused_with_its_reason() {
    let f = Fixture::new();
    let script = f.program("hello.sh", "#!/bin/sh\necho hi\n");
    let mut settings = f.settings();
    assert_eq!(
        settings.allow_clipboard(&f.paths, &script),
        Err("Scripts can't be allowed: the clipboard would see their interpreter.".to_string())
    );
    assert!(!f.conf().exists(), "nothing written");
}

#[test]
fn a_line_that_allows_nothing_shows_with_its_message_and_remove_drops_only_it() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    f.write_conf(&format!(
        "allow = python3\nallow = {}\nsize = 4\n",
        Entry::for_program(&clipd).unwrap().text()
    ));
    let mut settings = f.settings();
    let rows = clipboard_rows(&settings, &Live::default());
    let lines: Vec<(&str, bool)> = rows[1..]
        .iter()
        .map(|r| (r.label.as_str(), r.error.is_some()))
        .collect();
    assert_eq!(lines, [("allow = python3", true), ("size = 4", true)]);
    assert_eq!(rows[1].id, ControlId::ClipboardUnusable(0));

    let effects = change(
        ControlId::ClipboardUnusable(0),
        Value::Index(0),
        &mut settings,
        &Live::default(),
    );
    assert_eq!(effects, [Effect::Store(File::DataControl)]);
    settings.store(&f.paths, File::DataControl).unwrap();
    assert_eq!(
        std::fs::read_to_string(f.conf()).unwrap(),
        format!(
            "allow = {}\nsize = 4\n",
            Entry::for_program(&clipd).unwrap().text()
        )
    );
}

#[test]
fn a_refused_file_disables_adding_and_is_never_written() {
    let f = Fixture::new();
    let clipd = f.program("clipd", "x");
    f.write_conf("allow = /usr/bin/env\n");
    std::fs::set_permissions(f.conf(), std::fs::Permissions::from_mode(0o666)).unwrap();
    let (bytes, mtime) = (
        std::fs::read(f.conf()).unwrap(),
        std::fs::metadata(f.conf()).unwrap().modified().unwrap(),
    );
    let mut settings = f.settings();
    let sections = sections(PaneId::Privacy, &settings, LIGHT, &Live::default());
    assert_eq!(sections[0].rows.len(), 1);
    assert_eq!(sections[0].rows[0].label, "The list can't be read");
    assert_eq!(
        sections[1].rows[0].disabled.as_deref(),
        Some("The list can't be read")
    );

    assert!(settings.allow_clipboard(&f.paths, &clipd).is_err());
    assert!(settings.store(&f.paths, File::DataControl).is_err());
    assert_eq!(std::fs::read(f.conf()).unwrap(), bytes);
    assert_eq!(
        std::fs::metadata(f.conf()).unwrap().modified().unwrap(),
        mtime
    );
}

#[test]
fn an_empty_list_says_so() {
    let f = Fixture::new();
    let rows = clipboard_rows(&f.settings(), &Live::default());
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "No applications");
}
