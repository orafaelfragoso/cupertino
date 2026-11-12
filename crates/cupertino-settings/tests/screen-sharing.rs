//! Screen Sharing in the Privacy pane: the displays apps may share again without asking, as the portal writes them to
//! `screen-sharing.conf`, and Forget, which takes one away.

use std::os::unix::fs::PermissionsExt;

use cupertino_apps::entry::{parse_desktop_entry, Locale, Visibility};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::screen_sharing::ScreenSharingConf;
use cupertino_settings::clipboard;
use cupertino_settings::controls::{Control, Value};
use cupertino_settings::live::Live;
use cupertino_settings::panes::{change, sections, ControlId, Effect, Row, Session};
use cupertino_settings::settings::{File, Paths, Settings};
use cupertino_settings::PaneId;

const LIGHT: Session = Session {
    dark: false,
    display: DisplayOptions {
        reduce_motion: false,
        reduce_transparency: false,
        increase_contrast: false,
        differentiate_without_color: false,
    },
};

const MEETING: &str = "6f1c0d2e9a8b4c3d5e6f708192a3b4c5";
const OTHER: &str = "0123456789abcdef0123456789abcdef";

fn paths(dir: &std::path::Path) -> Paths {
    Paths {
        dock: None,
        compositor: None,
        menubar: None,
        keyboard: None,
        power: None,
        accessibility: None,
        system_accessibility: None,
        data_control: None,
        screen_sharing: Some(dir.join("screen-sharing.conf")),
        notifications: None,
        desktop_control: None,
        appearance: None,
        launcher: None,
        screen_reader: None,
    }
}

fn write(paths: &Paths, text: &str) {
    let path = paths.screen_sharing.as_ref().unwrap();
    std::fs::write(path, text).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn live() -> Live {
    let vis = Visibility {
        skip_try_exec: true,
        ..Visibility::default()
    };
    let entry = parse_desktop_entry(
        "org.example.Meeting.desktop",
        "[Desktop Entry]\nType=Application\nName=Meeting\nIcon=org.example.Meeting\nExec=meeting\n",
        &Locale::parse("C"),
        &vis,
    )
    .unwrap();
    Live {
        apps: clipboard::installed(vec![entry], &[]),
        ..Live::default()
    }
}

/// The rows of the Screen Sharing section.
fn rows(settings: &Settings, live: &Live) -> Vec<Row> {
    sections(PaneId::Privacy, settings, LIGHT, live)
        .into_iter()
        .find(|s| s.header.as_deref() == Some("Screen Sharing"))
        .expect("the Privacy pane has Screen Sharing")
        .rows
}

#[test]
fn each_remembered_app_is_named_by_its_entry_with_its_display_and_forget() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        &format!(
            "grant = {MEETING} org.example.Meeting display DP-1\n\
             grant = {OTHER} org.example.Unknown display HDMI-A-1\n"
        ),
    );
    let settings = Settings::load(&paths, 1080.0).unwrap();
    let rows = rows(&settings, &live());
    let shown: Vec<_> = rows
        .iter()
        .map(|r| (r.label.as_str(), r.detail.as_deref(), r.icon.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            ("Meeting", Some("Display DP-1"), Some("org.example.Meeting")),
            ("org.example.Unknown", Some("Display HDMI-A-1"), Some("")),
        ]
    );
    assert_eq!(
        rows[0].control,
        Control::Button {
            title: "Forget".to_owned()
        }
    );
}

#[test]
fn forget_takes_the_grant_out_of_the_file_and_keeps_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        &format!(
            "grant = {MEETING} org.example.Meeting display DP-1\n\
             grant = {OTHER} org.example.Other display DP-2\n"
        ),
    );
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    let effects = change(
        ControlId::ScreenSharingGrant(0),
        Value::Index(0),
        &mut settings,
        &live(),
    );
    assert_eq!(effects, [Effect::ForgetGrant(MEETING.to_owned())]);
    assert_eq!(rows(&settings, &live()).len(), 1, "the row goes at once");
    settings.forget_grant(&paths, MEETING).unwrap();
    let path = paths.screen_sharing.as_ref().unwrap();
    let conf = ScreenSharingConf::load(path).unwrap().0;
    assert!(conf.find(MEETING, "org.example.Meeting").is_none());
    assert!(conf.find(OTHER, "org.example.Other").is_some());
    let mode = std::fs::metadata(path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);

    settings.reload_screen_sharing(&paths);
    assert_eq!(rows(&settings, &live()).len(), 1);
    assert!(
        change(
            ControlId::ScreenSharingGrant(5),
            Value::Index(0),
            &mut settings,
            &live()
        )
        .is_empty(),
        "a row that is gone"
    );
}

#[test]
fn no_grant_shows_no_applications_and_a_refused_file_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    let settings = Settings::load(&paths, 1080.0).unwrap();
    let empty = rows(&settings, &Live::default());
    assert_eq!(empty.len(), 1);
    assert_eq!(empty[0].label, "No applications");

    write(
        &paths,
        &format!("grant = {MEETING} org.example.Meeting display DP-1\n"),
    );
    let path = paths.screen_sharing.as_ref().unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o666)).unwrap();
    let settings = Settings::load(&paths, 1080.0).unwrap();
    let refused = rows(&settings, &Live::default());
    assert!(refused[0].error.is_some(), "the refusal is shown");
    let mut settings = settings;
    assert!(settings.forget_grant(&paths, MEETING).is_err());
    assert!(
        settings.store(&paths, File::ScreenSharing).is_err(),
        "the list is never written whole"
    );
    assert!(std::fs::read_to_string(path).unwrap().contains(MEETING));
}

#[test]
fn forget_keeps_a_grant_the_portal_added_after_the_pane_read_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        &format!("grant = {MEETING} org.example.Meeting display DP-1\n"),
    );
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    // the portal remembers another share while the pane shows the older list
    let path = paths.screen_sharing.as_ref().unwrap();
    ScreenSharingConf::update(path, |conf| {
        conf.grants.push(
            cupertino_defaults::screen_sharing::Grant::parse(&format!(
                "{OTHER} org.example.Other display DP-2"
            ))
            .unwrap(),
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(rows(&settings, &live()).len(), 1, "the pane has not reread");

    let effects = change(
        ControlId::ScreenSharingGrant(0),
        Value::Index(0),
        &mut settings,
        &live(),
    );
    let [Effect::ForgetGrant(id)] = &effects[..] else {
        panic!("expected a Forget, got {effects:?}");
    };
    settings.forget_grant(&paths, id).unwrap();
    let conf = ScreenSharingConf::load(path).unwrap().0;
    assert!(conf.find(MEETING, "org.example.Meeting").is_none());
    assert!(
        conf.find(OTHER, "org.example.Other").is_some(),
        "the grant added since is kept"
    );
    assert_eq!(
        rows(&settings, &live())[0].detail.as_deref(),
        Some("Display DP-2"),
        "the pane shows the file as it is now"
    );
}

#[test]
fn a_forget_that_fails_shows_the_grant_again() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        &format!("grant = {MEETING} org.example.Meeting display DP-1\n"),
    );
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    let path = paths.screen_sharing.as_ref().unwrap();
    // another writer keeps the file's lock for longer than Settings waits
    let held = cupertino_defaults::lock::FileLock::acquire(path).unwrap();
    let effects = change(
        ControlId::ScreenSharingGrant(0),
        Value::Index(0),
        &mut settings,
        &live(),
    );
    assert_eq!(effects, [Effect::ForgetGrant(MEETING.to_owned())]);
    assert!(settings.forget_grant(&paths, MEETING).is_err());
    drop(held);
    assert!(std::fs::read_to_string(path).unwrap().contains(MEETING));
    let shown = rows(&settings, &live());
    assert_eq!(
        shown[0].detail.as_deref(),
        Some("Display DP-1"),
        "the grant still in the file is not hidden"
    );
}
