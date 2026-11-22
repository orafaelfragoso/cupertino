//! The session as the dock models it: which tile a window, a launch and a resident process belong to.

use cupertino_dock::conf::dock_config;
use cupertino_dock::desktop::{DesktopEntry, DesktopIndex};
use cupertino_dock::dock::Dock;
use cupertino_dock::session::Session;

/// The Electron application whose windows name the hidden entry: the entry people start it by
/// (`T3-Code-x86_64`, no `StartupWMClass`) and the URL handler named by its windows' id (`NoDisplay`).
const T3_SHOWN: &str = "T3-Code-x86_64";
const T3_HIDDEN: &str = "com.t3tools.T3Code";
const T3_EXEC: &str = "\"/home/user/.local/bin/T3-Code-x86_64.AppImage\" %U";

fn entry(id: &str, exec: &str, no_display: bool) -> DesktopEntry {
    DesktopEntry {
        id: id.to_owned(),
        name: id.to_owned(),
        exec: exec.to_owned(),
        no_display,
        ..DesktopEntry::default()
    }
}

/// The desktop of the person whose T3 Code was missing from the dock, with a portal's hidden entry that has no shown
/// sibling.
fn desktop() -> DesktopIndex {
    let mut entries = vec![
        entry(T3_SHOWN, T3_EXEC, false),
        entry(T3_HIDDEN, T3_EXEC, true),
        entry(
            "org.example.Portal",
            "/usr/lib/example-portal --replace",
            true,
        ),
        entry("foot", "foot", false),
        entry("htop", "htop", false),
    ];
    entries[4].terminal = true;
    entries.sort_by(|a, b| a.id.cmp(&b.id));
    DesktopIndex { entries }
}

fn session(pinned: &[&str]) -> Session {
    let mut dock = Dock::new(dock_config(None, 1080.0));
    dock.set_pinned(pinned);
    let mut session = Session::new(dock, desktop());
    session.list_taken();
    session
}

#[test]
fn a_hidden_entry_stands_for_the_shown_entry_that_runs_its_program() {
    let index = desktop();
    assert_eq!(index.find(T3_HIDDEN).map(|e| e.id.as_str()), Some(T3_SHOWN));
    assert_eq!(index.find(T3_SHOWN).map(|e| e.id.as_str()), Some(T3_SHOWN));
    // a helper without a shown sibling stays itself, hidden
    let portal = index
        .find("org.example.Portal")
        .expect("the portal's entry");
    assert!(portal.no_display);
}

#[test]
fn a_window_named_by_the_hidden_entry_runs_on_the_shown_entrys_tile() {
    let mut s = session(&[]);
    s.window(1, T3_HIDDEN, "T3 Code", false, 0.0);
    assert!(s.has_windows(&s.key_of(T3_HIDDEN)));
    let tile = s
        .dock
        .app_tile(T3_SHOWN)
        .expect("a tile for the application");
    assert!(tile.running);
    assert!(s.is_application(T3_HIDDEN));
}

#[test]
fn a_pinned_shown_entry_receives_the_window_of_its_hidden_entry() {
    let mut s = session(&[T3_SHOWN]);
    s.window(1, T3_HIDDEN, "T3 Code", false, 0.0);
    assert_eq!(s.key_of(T3_HIDDEN), T3_SHOWN);
    assert!(s.dock.app_tile(T3_SHOWN).is_some_and(|t| t.running));
    assert!(s.dock.app_tile(T3_HIDDEN).is_none());
}

#[test]
fn the_launch_of_the_shown_entry_ends_with_the_window_of_its_hidden_entry() {
    let mut s = session(&[]);
    s.launching(T3_SHOWN, 0.0);
    s.window(1, T3_HIDDEN, "T3 Code", false, 1.0);
    let past_timeout = 1.0 + s.dock.bounce.launch_timeout + 1.0;
    s.dock.tick(past_timeout);
    let tile = s.dock.app_tile(T3_SHOWN).expect("the launched tile");
    assert!(tile.running);
    assert!(
        !tile.is_leaving(),
        "the running tile stays past the launch timeout"
    );
    assert!(
        tile.launch
            .is_none_or(|b| !b.is_active(&s.dock.bounce, past_timeout)),
        "the bounce ended"
    );
}

#[test]
fn a_window_of_a_lone_hidden_entry_gets_no_tile() {
    let mut s = session(&[]);
    s.window(1, "org.example.Portal", "Choose a File", false, 0.0);
    assert!(s.dock.app_tile("org.example.Portal").is_none());
    assert!(!s.has_windows("org.example.Portal"));
}

#[test]
fn launching_a_program_that_is_no_application_adds_no_tile() {
    let mut s = session(&[]);
    for identity in ["org.example.Portal", "some-script", ""] {
        s.launching(identity, 0.0);
        assert!(
            s.dock.app_tile(identity).is_none(),
            "{identity:?} has no tile"
        );
    }
    // an application the desktop shows still bounces
    s.launching("foot", 0.0);
    assert!(s.dock.app_tile("foot").is_some_and(|t| t.launch.is_some()));
}

#[test]
fn launching_a_pinned_program_without_an_entry_bounces_its_tile() {
    let mut s = session(&["some-script"]);
    s.launching("some-script", 0.0);
    assert!(s
        .dock
        .app_tile("some-script")
        .is_some_and(|t| t.launch.is_some()));
}
