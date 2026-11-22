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
/// An application whose windows name none of its entry's id, `StartupWMClass` or program.
const EDITOR: &str = "org.example.Editor";
const EDITOR_WINDOWS: &str = "electron-editor";

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
        entry(EDITOR, "/opt/editor/start-editor %F", false),
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

/// A hidden entry stands for the shown one its program is past the programs that only run another (`env`, `flatpak
/// run`, a shell, an interpreter), and only when exactly one shown entry runs it.
#[test]
fn a_hidden_entry_stands_for_the_one_shown_entry_its_wrapped_program_is() {
    let index = DesktopIndex {
        entries: vec![
            entry("org.kde.dolphin", "env LANGUAGE=en_GB:en dolphin %u", false),
            entry("org.kde.dolphin.handler", "env -u QT_SCALE_FACTOR dolphin --select %u", true),
            entry("org.example.Helper", "env GDK_BACKEND=x11 /usr/lib/example-helper", true),
            entry(
                "org.mozilla.firefox",
                "/usr/bin/flatpak run --branch=stable --command=firefox org.mozilla.firefox @@u %u @@",
                false,
            ),
            entry("org.mozilla.firefox.handler", "flatpak run org.mozilla.firefox --new-window %u", true),
            entry("org.example.Sandboxed", "/usr/bin/flatpak run org.example.Sandboxed", true),
            entry("org.example.Shelled", "sh -c \"cd /opt/a && ./run\"", false),
            entry("org.example.OtherShelled", "sh -c \"cd /opt/b && ./run\"", true),
            entry("org.example.Script", "python3 /opt/a/main.py", false),
            entry("org.example.OtherScript", "python3.12 -X dev /opt/b/tool.py", true),
            entry("org.example.ScriptHandler", "sh -c \"exec python3 /opt/a/main.py %u\"", true),
            entry("org.example.SuiteWriter", "/opt/suite/launch --writer", false),
            entry("org.example.SuiteSheets", "/opt/suite/launch --sheets", false),
            entry("org.example.SuiteHandler", "/opt/suite/launch %U", true),
        ],
    };
    for (hidden, shown) in [
        ("org.kde.dolphin.handler", "org.kde.dolphin"),
        ("org.example.Helper", "org.example.Helper"),
        ("org.mozilla.firefox.handler", "org.mozilla.firefox"),
        ("org.example.Sandboxed", "org.example.Sandboxed"),
        ("org.example.OtherShelled", "org.example.OtherShelled"),
        ("org.example.OtherScript", "org.example.OtherScript"),
        ("org.example.ScriptHandler", "org.example.Script"),
        ("org.example.SuiteHandler", "org.example.SuiteHandler"),
    ] {
        assert_eq!(
            index.find(hidden).map(|e| e.id.as_str()),
            Some(shown),
            "{hidden}"
        );
    }
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

#[test]
fn a_window_whose_launch_named_an_application_runs_on_its_tile_and_ends_its_bounce() {
    let mut s = session(&[]);
    s.launching(EDITOR, 0.0);
    s.window_launch(1, EDITOR, 1.0);
    s.window(1, EDITOR_WINDOWS, "Untitled", false, 1.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some(EDITOR));
    assert!(s.dock.app_tile(EDITOR_WINDOWS).is_none());
    let later = 2.0 + s.dock.bounce.launch_timeout;
    s.dock.tick(later);
    let tile = s.dock.app_tile(EDITOR).expect("the launched tile");
    assert!(tile.running && !tile.is_leaving());
    assert!(tile
        .launch
        .is_none_or(|b| !b.is_active(&s.dock.bounce, later)));
    // raising it names the window's own application id to the server
    assert_eq!(
        s.opening(EDITOR),
        cupertino_dock::session::Activation::ActivateApp(EDITOR_WINDOWS.to_owned())
    );
    s.window_closed(1, later);
    assert!(s.dock.app_tile(EDITOR).is_none_or(|t| !t.running));
}

#[test]
fn a_window_shown_before_its_launch_was_named_moves_to_the_launched_tile() {
    let mut s = session(&[EDITOR]);
    s.window(1, EDITOR_WINDOWS, "Untitled", false, 0.0);
    assert!(s.dock.app_tile(EDITOR_WINDOWS).is_some_and(|t| t.running));
    s.window_launch(1, EDITOR, 1.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some(EDITOR));
    assert!(s.dock.app_tile(EDITOR).is_some_and(|t| t.running));
    assert!(s
        .dock
        .app_tile(EDITOR_WINDOWS)
        .is_none_or(|t| !t.running && t.is_leaving()));
}

#[test]
fn a_launch_that_names_no_application_leaves_the_window_where_it_was() {
    let mut s = session(&[]);
    s.window_launch(1, "org.example.Portal", 0.0);
    s.window(1, EDITOR_WINDOWS, "Untitled", false, 0.0);
    assert_eq!(
        s.window_info(1).map(|w| w.key.as_str()),
        Some(EDITOR_WINDOWS)
    );
}

/// A token can name another application than the one it starts (Qt names the requester's): a window whose own
/// application id names an application stays on that application's tile.
#[test]
fn a_window_of_an_application_stays_on_its_tile_whatever_its_launch_named() {
    let mut s = session(&[]);
    s.window_launch(1, EDITOR, 0.0);
    s.window(1, "foot", "Terminal", false, 0.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some("foot"));
    // a window shown before stays too
    s.window(2, T3_HIDDEN, "T3 Code", false, 1.0);
    s.window_launch(2, EDITOR, 1.0);
    assert_eq!(s.window_info(2).map(|w| w.key.as_str()), Some(T3_SHOWN));
    assert!(s.dock.app_tile(EDITOR).is_none_or(|t| !t.running));
    s.set_desktop(desktop(), 2.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some("foot"));
    assert_eq!(s.window_info(2).map(|w| w.key.as_str()), Some(T3_SHOWN));
}

/// Opening a resident application without windows asks it for a window: the launcher's token for it is announced, and
/// neither bounces the tile nor lets the launch timeout take the running tile away.
#[test]
fn launching_an_application_that_runs_without_a_window_keeps_its_tile_unbounced() {
    // the terminal ran before the dock: its window is in the server's first list, and does not bounce
    let mut s = Session::new(Dock::new(dock_config(None, 1080.0)), desktop());
    s.window(1, "foot", "Terminal", false, 0.0);
    s.list_taken();
    s.set_resident("foot", true, 0.0);
    s.window_closed(1, 1.0);
    s.launching("foot", 2.0);
    let tile = s.dock.app_tile("foot").expect("the running tile");
    assert!(tile.running);
    assert!(tile.launch.is_none(), "no bounce");
    let later = 3.0 + s.dock.bounce.launch_timeout;
    s.dock.tick(later);
    let tile = s.dock.app_tile("foot").expect("the running tile");
    assert!(tile.running && !tile.is_leaving());
}

/// A launch that times out takes away an unpinned tile only when nothing of the application runs.
#[test]
fn the_launch_timeout_keeps_a_running_tile() {
    let mut s = session(&[]);
    s.dock.app_started("foot", 0.0);
    s.dock.app_launching("foot", 1.0);
    let later = 2.0 + s.dock.bounce.launch_timeout;
    s.dock.tick(later);
    let tile = s.dock.app_tile("foot").expect("the running tile");
    assert!(tile.running && !tile.is_leaving());
    // a launch of what does not run still fades
    s.dock.app_launching("htop", later);
    let after = later + s.dock.bounce.launch_timeout + 1.0;
    s.dock.tick(after);
    assert!(s.dock.app_tile("htop").is_none_or(|t| t.is_leaving()));
}

/// The server reports a window again only when it changes: one that got no tile when it was reported is placed when
/// the entry it stands for is shown later, and leaves its tile when that entry is hidden again.
#[test]
fn indexing_the_entries_again_places_a_window_that_had_no_tile() {
    // T3 Code's hidden handler entry was written before the entry people start it by
    let only_hidden = || DesktopIndex {
        entries: vec![entry(T3_HIDDEN, T3_EXEC, true)],
    };
    let mut s = Session::new(Dock::new(dock_config(None, 1080.0)), only_hidden());
    s.list_taken();
    s.window(1, T3_HIDDEN, "T3 Code", false, 0.0);
    assert!(
        s.window_info(1).is_none(),
        "a hidden entry's window has no tile"
    );
    s.set_desktop(desktop(), 1.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some(T3_SHOWN));
    assert!(s.dock.app_tile(T3_SHOWN).is_some_and(|t| t.running));
    s.set_desktop(only_hidden(), 2.0);
    assert!(s.window_info(1).is_none());
    assert!(s.dock.app_tile(T3_SHOWN).is_none_or(|t| !t.running));
}

/// `window_launch` for a window the dock holds no tile for (its own application id is a hidden entry's) places it,
/// though the server does not report the window again.
#[test]
fn a_launch_named_for_a_window_without_a_tile_places_it() {
    let mut s = session(&[]);
    s.window(1, "org.example.Portal", "Untitled", false, 0.0);
    assert!(s.window_info(1).is_none());
    s.window_launch(1, EDITOR, 1.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some(EDITOR));
    assert!(s.dock.app_tile(EDITOR).is_some_and(|t| t.running));
    s.window_closed(1, 2.0);
    assert!(s.dock.app_tile(EDITOR).is_none_or(|t| !t.running));
    // a launch named after the window closed places nothing
    s.window_launch(1, EDITOR, 3.0);
    assert!(s.window_info(1).is_none());
}

#[test]
fn indexing_the_entries_again_keeps_a_window_on_the_tile_its_launch_named() {
    let mut s = session(&[]);
    s.window_launch(1, EDITOR, 0.0);
    s.window(1, EDITOR_WINDOWS, "Untitled", false, 0.0);
    s.set_desktop(desktop(), 1.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some(EDITOR));
    assert!(s.dock.app_tile(EDITOR).is_some_and(|t| t.running));
    // the launch is still the window's after the index changed under it
    s.window(1, EDITOR_WINDOWS, "Saved", false, 2.0);
    assert_eq!(s.window_info(1).map(|w| w.key.as_str()), Some(EDITOR));
}
