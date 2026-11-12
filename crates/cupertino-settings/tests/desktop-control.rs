//! Desktop Control and Screen Capture in the Privacy pane: the apps the compositor recorded in `desktop-control.conf`
//! when they first asked, each with a switch that allows or denies it and Forget, which makes it ask again.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use cupertino_apps::entry::{parse_desktop_entry, Locale, Visibility};
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_defaults::desktop_control::{App, Decision, DesktopControlConf, Service};
use cupertino_settings::clipboard;
use cupertino_settings::controls::{Control, Value};
use cupertino_settings::desktop_control::{identity, Answers, Identity};
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

fn paths(dir: &Path) -> Paths {
    Paths {
        dock: None,
        compositor: None,
        menubar: None,
        keyboard: None,
        power: None,
        accessibility: None,
        system_accessibility: None,
        data_control: None,
        screen_sharing: None,
        notifications: None,
        desktop_control: Some(dir.join("desktop-control.conf")),
        appearance: None,
        launcher: None,
        screen_reader: None,
    }
}

fn conf(paths: &Paths) -> &Path {
    paths.desktop_control.as_deref().unwrap()
}

fn write(paths: &Paths, text: &str) {
    std::fs::write(conf(paths), text).unwrap();
    std::fs::set_permissions(conf(paths), std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn meeting() -> App {
    App::Desktop("org.example.Meeting".to_owned())
}

fn agent() -> App {
    App::Executable(PathBuf::from("/home/me/bin/agent"))
}

fn unknown() -> App {
    App::Desktop("org.example.Unknown".to_owned())
}

/// The answer the file holds now.
fn decision(paths: &Paths, service: Service, app: &App) -> Option<Decision> {
    DesktopControlConf::load(conf(paths))
        .unwrap()
        .0
        .decision(service, app)
}

/// The installed applications: one desktop entry, Meeting.
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

/// The rows of the section titled `title`.
fn rows(settings: &Settings, live: &Live, title: &str) -> Vec<Row> {
    sections(PaneId::Privacy, settings, LIGHT, live)
        .into_iter()
        .find(|s| s.header.as_deref() == Some(title))
        .unwrap_or_else(|| panic!("the Privacy pane has {title}"))
        .rows
}

fn control_rows(settings: &Settings, live: &Live) -> Vec<Row> {
    rows(settings, live, "Desktop Control")
}

fn capture_rows(settings: &Settings, live: &Live) -> Vec<Row> {
    rows(settings, live, "Screen Capture")
}

/// Each row as its name and whether its switch is on.
fn switches(rows: &[Row]) -> Vec<(&str, bool)> {
    rows.iter()
        .map(|r| match &r.control {
            Control::ButtonAndSwitch { on, .. } => (r.label.as_str(), *on),
            other => panic!("{}: {other:?}", r.label),
        })
        .collect()
}

#[test]
fn an_app_is_named_by_its_desktop_entry_else_its_id_and_an_executable_by_its_file() {
    let installed = live().apps;
    assert_eq!(
        identity(&meeting(), &installed),
        Identity {
            name: "Meeting".to_owned(),
            icon: Some("org.example.Meeting".to_owned()),
            detail: None,
        }
    );
    assert_eq!(
        identity(&unknown(), &installed),
        Identity {
            name: "org.example.Unknown".to_owned(),
            icon: None,
            detail: None,
        },
        "an app without a desktop entry shows its id"
    );
    assert_eq!(
        identity(&agent(), &installed),
        Identity {
            name: "agent".to_owned(),
            icon: None,
            detail: Some("/home/me/bin/agent".to_owned()),
        },
        "an executable shows its file name over its whole path"
    );
}

#[test]
fn each_service_lists_its_apps_in_file_order_with_a_switch_and_forget() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        "allow = control app:org.example.Unknown\n\
         deny = control exe:/home/me/bin/agent\n\
         allow = capture app:org.example.Meeting\n\
         allow = control app:org.example.Meeting\n\
         # a line that answers nothing is not listed\n\
         allow = record app:org.example.Meeting\n",
    );
    let settings = Settings::load(&paths, 1080.0).unwrap();
    let live = live();
    let control = control_rows(&settings, &live);
    let shown: Vec<_> = control
        .iter()
        .map(|r| (r.label.as_str(), r.detail.as_deref(), r.icon.as_deref()))
        .collect();
    assert_eq!(
        shown,
        [
            ("org.example.Unknown", None, Some("")),
            ("agent", Some("/home/me/bin/agent"), Some("")),
            ("Meeting", None, Some("org.example.Meeting")),
        ],
        "in the order the file lists them"
    );
    assert_eq!(
        switches(&control),
        [
            ("org.example.Unknown", true),
            ("agent", false),
            ("Meeting", true)
        ]
    );
    assert_eq!(
        control[0].control,
        Control::ButtonAndSwitch {
            title: "Forget".to_owned(),
            on: true
        }
    );
    assert!(control
        .iter()
        .all(|r| r.disabled.is_none() && r.error.is_none()));
    assert_eq!(
        switches(&capture_rows(&settings, &live)),
        [("Meeting", true)]
    );
    let ids: Vec<_> = control.iter().map(|r| r.id).collect();
    assert_eq!(
        ids,
        [0, 1, 2].map(|i| ControlId::DesktopControlApp(Service::Control, i))
    );
}

#[test]
fn each_page_says_what_its_apps_can_do_and_what_turning_off_and_forget_do() {
    let dir = tempfile::tempdir().unwrap();
    let settings = Settings::load(&paths(dir.path()), 1080.0).unwrap();
    for (title, does) in [
        ("Desktop Control", "use the pointer and keyboard"),
        ("Screen Capture", "take pictures of your screen"),
    ] {
        let section = sections(PaneId::Privacy, &settings, LIGHT, &Live::default())
            .into_iter()
            .find(|s| s.header.as_deref() == Some(title))
            .unwrap();
        let footer = section.footer.unwrap();
        for words in [
            does,
            "Turning one off stops it at once",
            "Forget one to be asked",
        ] {
            assert!(footer.contains(words), "{title}: {footer}");
        }
        let [empty] = &section.rows[..] else {
            panic!("{title}: {:?}", section.rows);
        };
        assert_eq!(empty.label, "No applications");
        assert!(
            empty
                .detail
                .as_deref()
                .is_some_and(|d| d.contains("after they ask")),
            "{title}: {:?}",
            empty.detail
        );
    }
}

#[test]
fn the_switch_writes_allow_or_deny_for_that_app_and_service_only() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        "allow = control app:org.example.Meeting\n\
         deny = control exe:/home/me/bin/agent\n\
         allow = capture app:org.example.Meeting\n",
    );
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    let live = live();

    let effects = change(
        ControlId::DesktopControlApp(Service::Control, 0),
        Value::Bool(false),
        &mut settings,
        &live,
    );
    assert_eq!(
        effects,
        [Effect::DecideApp {
            service: Service::Control,
            app: meeting(),
            decision: Decision::Deny,
        }]
    );
    assert_eq!(
        switches(&control_rows(&settings, &live))[0],
        ("Meeting", false),
        "the switch shows the change at once"
    );
    settings
        .decide_app(&paths, Service::Control, &meeting(), Decision::Deny)
        .unwrap();
    assert_eq!(
        decision(&paths, Service::Control, &meeting()),
        Some(Decision::Deny)
    );
    assert_eq!(
        decision(&paths, Service::Capture, &meeting()),
        Some(Decision::Allow),
        "the other service keeps its answer"
    );

    let effects = change(
        ControlId::DesktopControlApp(Service::Control, 1),
        Value::Bool(true),
        &mut settings,
        &live,
    );
    assert_eq!(
        effects,
        [Effect::DecideApp {
            service: Service::Control,
            app: agent(),
            decision: Decision::Allow,
        }]
    );
    settings
        .decide_app(&paths, Service::Control, &agent(), Decision::Allow)
        .unwrap();
    assert_eq!(
        decision(&paths, Service::Control, &agent()),
        Some(Decision::Allow)
    );
    assert_eq!(
        std::fs::read_to_string(conf(&paths)).unwrap(),
        "deny = control app:org.example.Meeting\n\
         allow = control exe:/home/me/bin/agent\n\
         allow = capture app:org.example.Meeting\n",
        "each answer keeps its place"
    );
    let mode = std::fs::metadata(conf(&paths))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
    assert_eq!(
        switches(&control_rows(&settings, &live)),
        [("Meeting", false), ("agent", true)]
    );
}

#[test]
fn forget_takes_the_answer_out_so_the_app_asks_again_and_keeps_the_others() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        "allow = control app:org.example.Meeting\n\
         deny = control exe:/home/me/bin/agent\n\
         allow = capture app:org.example.Meeting\n",
    );
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    let live = live();
    let effects = change(
        ControlId::DesktopControlApp(Service::Control, 0),
        Value::Index(0),
        &mut settings,
        &live,
    );
    assert_eq!(
        effects,
        [Effect::ForgetApp {
            service: Service::Control,
            app: meeting(),
        }]
    );
    assert_eq!(
        switches(&control_rows(&settings, &live)),
        [("agent", false)],
        "the row goes at once"
    );
    settings
        .forget_app(&paths, Service::Control, &meeting())
        .unwrap();
    assert_eq!(decision(&paths, Service::Control, &meeting()), None);
    assert_eq!(
        decision(&paths, Service::Control, &agent()),
        Some(Decision::Deny)
    );
    assert_eq!(
        decision(&paths, Service::Capture, &meeting()),
        Some(Decision::Allow)
    );
    assert!(
        change(
            ControlId::DesktopControlApp(Service::Control, 5),
            Value::Index(0),
            &mut settings,
            &live
        )
        .is_empty(),
        "a row that is gone"
    );
}

#[test]
fn a_refused_file_shows_why_lists_nothing_and_is_never_written() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(&paths, "allow = control app:org.example.Meeting\n");
    std::fs::set_permissions(conf(&paths), std::fs::Permissions::from_mode(0o666)).unwrap();
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    assert!(settings.desktop_control.unreadable.is_some());
    let live = live();
    for rows in [
        control_rows(&settings, &live),
        capture_rows(&settings, &live),
    ] {
        let [refused] = &rows[..] else {
            panic!("only the refusal: {rows:?}");
        };
        assert!(refused.error.is_some(), "the refusal is shown");
        assert_eq!(
            refused.id,
            ControlId::Reading,
            "nothing to switch or forget"
        );
    }
    assert_eq!(
        change(
            ControlId::DesktopControlApp(Service::Control, 0),
            Value::Bool(false),
            &mut settings,
            &live,
        ),
        [Effect::Refuse("The list can't be read".to_owned())]
    );
    assert!(settings
        .decide_app(&paths, Service::Control, &meeting(), Decision::Deny)
        .is_err());
    assert!(settings
        .forget_app(&paths, Service::Control, &meeting())
        .is_err());
    assert!(
        settings.store(&paths, File::DesktopControl).is_err(),
        "the list is never written whole"
    );
    assert_eq!(
        std::fs::read_to_string(conf(&paths)).unwrap(),
        "allow = control app:org.example.Meeting\n"
    );
}

#[test]
fn a_change_keeps_an_answer_the_compositor_recorded_after_the_pane_read_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(&paths, "allow = control app:org.example.Meeting\n");
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    let live = live();
    // the compositor records another app's answer while the pane shows the older list
    DesktopControlConf::update(conf(&paths), |conf| {
        conf.set(Service::Control, &agent(), Decision::Allow)
            .map_err(std::io::Error::other)
    })
    .unwrap();
    assert_eq!(control_rows(&settings, &live).len(), 1, "not read yet");

    let effects = change(
        ControlId::DesktopControlApp(Service::Control, 0),
        Value::Bool(false),
        &mut settings,
        &live,
    );
    let [Effect::DecideApp {
        service,
        app,
        decision: answer,
    }] = &effects[..]
    else {
        panic!("expected an answer, got {effects:?}");
    };
    settings.decide_app(&paths, *service, app, *answer).unwrap();
    assert_eq!(
        decision(&paths, Service::Control, &meeting()),
        Some(Decision::Deny)
    );
    assert_eq!(
        decision(&paths, Service::Control, &agent()),
        Some(Decision::Allow),
        "the answer recorded since is kept"
    );
    assert_eq!(
        switches(&control_rows(&settings, &live)),
        [("Meeting", false), ("agent", true)],
        "the pane shows the file as it is now"
    );
}

#[test]
fn a_row_commits_the_app_it_shows_even_when_the_file_changed_since() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        "allow = control app:org.example.Meeting\n\
         allow = control exe:/home/me/bin/agent\n",
    );
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    // another writer puts the agent first; a commit reads the other files again first
    write(
        &paths,
        "allow = control exe:/home/me/bin/agent\n\
         allow = control app:org.example.Meeting\n",
    );
    settings.reload(&paths).unwrap();
    let effects = change(
        ControlId::DesktopControlApp(Service::Control, 0),
        Value::Bool(false),
        &mut settings,
        &live(),
    );
    assert_eq!(
        effects,
        [Effect::DecideApp {
            service: Service::Control,
            app: meeting(),
            decision: Decision::Deny,
        }],
        "the first row showed Meeting"
    );
    settings.reload_desktop_control(&paths);
    assert_eq!(
        switches(&control_rows(&settings, &live())),
        [("agent", true), ("Meeting", true)],
        "reading the file again shows it as it is"
    );
}

#[test]
fn an_app_removed_meanwhile_stays_removed_when_its_old_row_is_switched() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(&paths, "allow = control app:org.example.Meeting\n");
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    DesktopControlConf::update(conf(&paths), |conf| {
        conf.forget(Service::Control, &meeting());
        Ok(())
    })
    .unwrap();
    let effects = change(
        ControlId::DesktopControlApp(Service::Control, 0),
        Value::Bool(false),
        &mut settings,
        &live(),
    );
    assert_eq!(effects.len(), 1);
    settings
        .decide_app(&paths, Service::Control, &meeting(), Decision::Deny)
        .unwrap();
    assert_eq!(
        decision(&paths, Service::Control, &meeting()),
        None,
        "it asks again at its next request"
    );
    assert_eq!(control_rows(&settings, &live())[0].label, "No applications");
}

#[test]
fn a_change_that_fails_shows_the_answer_the_file_still_holds() {
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(&paths, "allow = control app:org.example.Meeting\n");
    let mut settings = Settings::load(&paths, 1080.0).unwrap();
    let live = live();
    // the compositor keeps the file's lock for longer than Settings waits
    let held = cupertino_defaults::lock::FileLock::acquire(conf(&paths)).unwrap();
    change(
        ControlId::DesktopControlApp(Service::Control, 0),
        Value::Bool(false),
        &mut settings,
        &live,
    );
    assert!(settings
        .decide_app(&paths, Service::Control, &meeting(), Decision::Deny)
        .is_err());
    assert!(settings
        .forget_app(&paths, Service::Control, &meeting())
        .is_err());
    drop(held);
    assert_eq!(
        decision(&paths, Service::Control, &meeting()),
        Some(Decision::Allow)
    );
    assert_eq!(
        switches(&control_rows(&settings, &live)),
        [("Meeting", true)],
        "the answer still in the file is shown"
    );
}

#[test]
fn a_missing_file_holds_no_answer_and_an_unreadable_one_says_why() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("desktop-control.conf");
    assert_eq!(Answers::load(&path), Answers::default());
    std::os::unix::fs::symlink(dir.path().join("elsewhere.conf"), &path).unwrap();
    let linked = Answers::load(&path);
    assert!(linked.conf.entries.is_empty());
    assert!(linked.unreadable.is_some(), "a link is refused");
}

#[test]
fn the_privacy_pane_links_each_page_and_each_page_holds_its_service_alone() {
    use cupertino_settings::pages::{blocks, Block, Page, SubPage};
    let dir = tempfile::tempdir().unwrap();
    let paths = paths(dir.path());
    write(
        &paths,
        "allow = control app:org.example.Meeting\n\
         deny = capture exe:/home/me/bin/agent\n",
    );
    let settings = Settings::load(&paths, 1080.0).unwrap();
    let live = live();
    let all = || sections(PaneId::Privacy, &settings, LIGHT, &live);
    assert_eq!(
        blocks(Page::pane(PaneId::Privacy), all()),
        [Block::Links(vec![
            SubPage::ClipboardAccess,
            SubPage::ScreenSharing,
            SubPage::DesktopControl,
            SubPage::ScreenCapture,
        ])]
    );
    for (sub, name) in [
        (SubPage::DesktopControl, "Meeting"),
        (SubPage::ScreenCapture, "agent"),
    ] {
        let page = Page {
            pane: PaneId::Privacy,
            sub: Some(sub),
        };
        let [Block::Section(section)] = &blocks(page, all())[..] else {
            panic!("{sub:?} holds one section");
        };
        assert_eq!(section.header, None, "the toolbar shows the title");
        let names: Vec<_> = section.rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(names, [name], "{sub:?}");
    }
    assert!(
        cupertino_settings::pages::description(PaneId::Privacy, None)
            .contains("control the desktop")
    );
}
