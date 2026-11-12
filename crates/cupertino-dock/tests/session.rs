use cupertino_dock::desktop::{parse_entry, DesktopIndex};
use cupertino_dock::session::{Activation, Launch, Session};
use cupertino_dock::{Dock, DockConfig, TileId, TileKind, TileLifecycle};
use cupertino_wm::geometry::Rect;
use std::path::Path;

const SCREEN: Rect = Rect::new(0.0, 0.0, 1920.0, 1080.0);

fn entry(id: &str, extra: &str) -> cupertino_dock::desktop::DesktopEntry {
    parse_entry(
        id,
        Path::new("x"),
        &format!("[Desktop Entry]\nName={id}\nExec={id} %U\n{extra}"),
    )
    .unwrap()
}

fn session(pinned: &[&str], entries: Vec<cupertino_dock::desktop::DesktopEntry>) -> Session {
    let mut dock = Dock::new(DockConfig::default());
    dock.set_pinned(pinned);
    Session::new(dock, DesktopIndex { entries })
}

fn tile_of(s: &Session, key: &str) -> TileId {
    s.dock.app_tile(key).unwrap().id
}

fn lifecycle(s: &Session, key: &str, now: f64) -> TileLifecycle {
    let id = tile_of(s, key);
    s.dock
        .layout(SCREEN, now)
        .tiles
        .iter()
        .find(|t| t.id == id)
        .unwrap()
        .lifecycle
}

#[test]
fn an_application_runs_from_its_first_window_to_its_last() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    assert_eq!(lifecycle(&s, "foot", 0.0), TileLifecycle::Idle);
    s.window(1, "foot", "one", false, 0.0);
    s.window(2, "foot", "two", false, 0.0);
    assert_eq!(lifecycle(&s, "foot", 0.0), TileLifecycle::Running);
    s.window_closed(1, 1.0);
    assert_eq!(
        lifecycle(&s, "foot", 1.0),
        TileLifecycle::Running,
        "another window is still open"
    );
    s.window_closed(2, 2.0);
    assert_eq!(lifecycle(&s, "foot", 2.0), TileLifecycle::Idle);
    assert!(s.dock.app_tile("foot").is_some(), "a pinned tile stays");
}

#[test]
fn an_unpinned_application_gets_a_tile_that_leaves_with_its_last_window() {
    let mut s = session(&["foot"], vec![entry("org.example.editor", "")]);
    s.window(7, "org.example.editor", "notes", false, 0.0);
    assert!(s.dock.app_tile("org.example.editor").is_some());
    s.window_closed(7, 1.0);
    assert!(
        s.dock.app_tile("org.example.editor").is_none(),
        "the tile fades out"
    );
}

#[test]
fn windows_find_the_pinned_tile_of_their_desktop_entry() {
    let mut s = session(
        &["org.mozilla.firefox"],
        vec![entry("org.mozilla.firefox", "StartupWMClass=firefox\n")],
    );
    s.window(3, "firefox", "page", false, 0.0);
    assert_eq!(s.key_of("firefox"), "org.mozilla.firefox");
    assert_eq!(
        lifecycle(&s, "org.mozilla.firefox", 0.0),
        TileLifecycle::Running
    );
    assert!(s.dock.app_tile("firefox").is_none(), "no second tile");
}

#[test]
fn a_server_launch_bounces_the_tile_until_a_window_appears() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    s.launching("foot", 0.0);
    assert_eq!(lifecycle(&s, "foot", 0.1), TileLifecycle::Launching);
    s.window(1, "foot", "shell", false, 0.2);
    assert_eq!(lifecycle(&s, "foot", 5.0), TileLifecycle::Running);
}

#[test]
fn a_minimized_window_has_a_tile_that_restores_it() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    s.window(4, "foot", "shell", false, 0.0);
    s.window(4, "foot", "shell", true, 1.0);
    let tile = s
        .dock
        .tiles()
        .into_iter()
        .find(|t| t.kind == TileKind::MinimizedWindow)
        .unwrap()
        .id;
    assert_eq!(s.activation(tile), Some(Activation::RestoreWindow(4)));
    s.window(4, "foot", "shell", false, 2.0);
    assert!(s
        .dock
        .tiles()
        .into_iter()
        .filter(|t| t.kind == TileKind::MinimizedWindow)
        .all(|t| t.is_leaving()));
}

#[test]
fn closing_a_minimized_window_removes_its_tile_but_not_the_running_state() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    s.window(1, "foot", "a", false, 0.0);
    s.window(2, "foot", "b", true, 0.0);
    s.window_closed(2, 1.0);
    assert!(s
        .dock
        .tiles()
        .into_iter()
        .filter(|t| t.kind == TileKind::MinimizedWindow)
        .all(|t| t.is_leaving()));
    assert_eq!(lifecycle(&s, "foot", 1.0), TileLifecycle::Running);
}

#[test]
fn clicks_activate_launch_open_or_do_nothing() {
    let mut s = session(&["foot", "mpv"], vec![entry("foot", ""), entry("mpv", "")]);
    s.window(1, "foot", "shell", false, 0.0);
    assert_eq!(
        s.activation(tile_of(&s, "foot")),
        Some(Activation::ActivateApp("foot".into()))
    );
    assert_eq!(
        s.activation(tile_of(&s, "mpv")),
        Some(Activation::Launch("mpv".into()))
    );
    let kind = |k: TileKind| s.dock.tiles().into_iter().find(|t| t.kind == k).unwrap().id;
    assert_eq!(
        s.activation(kind(TileKind::Trash)),
        Some(Activation::OpenTrash)
    );
    assert_eq!(s.activation(kind(TileKind::Separator)), None);
    assert_eq!(s.activation(TileId(9999)), None);
}

#[test]
fn a_click_activates_with_the_application_id_the_server_reported() {
    let mut s = session(
        &["org.mozilla.firefox"],
        vec![entry("org.mozilla.firefox", "StartupWMClass=firefox\n")],
    );
    s.window(3, "firefox", "page", false, 0.0);
    assert_eq!(
        s.activation(tile_of(&s, "org.mozilla.firefox")),
        Some(Activation::ActivateApp("firefox".into()))
    );
}

#[test]
fn launching_uses_the_desktop_entry_or_the_key_as_a_command() {
    let s = session(
        &[],
        vec![entry("foot", "Terminal=true\n"), entry("mpv", "")],
    );
    assert_eq!(
        s.launch("foot"),
        Launch {
            argv: vec!["foot".into()],
            terminal: true
        }
    );
    assert_eq!(
        s.launch("mpv"),
        Launch {
            argv: vec!["mpv".into()],
            terminal: false
        }
    );
    assert_eq!(
        s.launch("unknown-tool"),
        Launch {
            argv: vec!["unknown-tool".into()],
            terminal: false
        }
    );
}

fn lift(s: &Session, key: &str, now: f64) -> f64 {
    let id = tile_of(s, key);
    s.dock
        .layout(SCREEN, now)
        .tiles
        .iter()
        .find(|t| t.id == id)
        .unwrap()
        .lift
}

/// whoever starts a program, its tile bounces; with no launch signal the first window is
/// the only sign, so the bounce is the one cycle that stop rule leaves.
#[test]
fn an_application_that_appears_without_a_launch_signal_bounces_for_one_cycle() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    s.list_taken();
    s.window(1, "foot", "one", false, 10.0);
    let peak = s.dock.bounce.period / 2.0;
    assert!(
        lift(&s, "foot", 10.0 + peak) > 5.0,
        "the tile is raised at the top of the arc"
    );
    assert_eq!(
        lift(&s, "foot", 10.0 + s.dock.bounce.period + 0.01),
        0.0,
        "and is down after one cycle"
    );
    assert_eq!(lifecycle(&s, "foot", 10.0 + peak), TileLifecycle::Running);
    s.window(2, "foot", "two", false, 20.0);
    assert_eq!(
        lift(&s, "foot", 20.0 + peak),
        0.0,
        "a second window of a running application does not bounce"
    );
}

#[test]
fn an_application_in_the_first_list_was_running_before_the_dock_and_does_not_bounce() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    s.window(1, "foot", "one", false, 0.0);
    s.list_taken();
    assert_eq!(lift(&s, "foot", s.dock.bounce.period / 2.0), 0.0);
}

#[test]
fn a_launch_that_was_announced_keeps_its_bounce_until_the_window_and_is_not_doubled() {
    let mut s = session(&["foot"], vec![entry("foot", "")]);
    s.list_taken();
    s.launching("foot", 0.0);
    s.launching("foot", 0.2);
    let peak = s.dock.bounce.period / 2.0;
    let raised = lift(&s, "foot", peak);
    assert!(raised > 0.0);
    assert!(
        lift(&s, "foot", 0.55) < 0.9 * raised,
        "the second announcement did not restart the arc"
    );
    s.window(1, "foot", "one", false, 5.0);
    assert!(lift(&s, "foot", 5.0 + 0.1) >= 0.0);
    assert_eq!(lift(&s, "foot", 5.0 + 2.0 * s.dock.bounce.period), 0.0);
}

/// A launch announced for an application that already shows a window (the launcher asks for its token before the
/// server brings the application forward) does not bounce its tile: no first window will come to end the bounce. Once
/// its last window has closed, a launch bounces again.
#[test]
fn a_launch_announced_for_an_application_that_shows_a_window_does_not_bounce() {
    let mut s = session(
        &["org.mozilla.firefox"],
        vec![entry("org.mozilla.firefox", "StartupWMClass=firefox\n")],
    );
    s.list_taken();
    s.window(1, "firefox", "page", false, 0.0);
    let peak = s.dock.bounce.period / 2.0;
    s.launching("org.mozilla.firefox.desktop", 5.0);
    assert_eq!(lift(&s, "org.mozilla.firefox", 5.0 + peak), 0.0);
    assert_eq!(lift(&s, "org.mozilla.firefox", 30.0), 0.0);
    s.window_closed(1, 40.0);
    s.launching("org.mozilla.firefox.desktop", 50.0);
    assert!(lift(&s, "org.mozilla.firefox", 50.0 + peak) > 0.0);
}

/// a refused activation reaches the tile of the application's desktop entry, however the
/// server names the application.
#[test]
fn a_refused_activation_makes_the_pinned_tile_of_the_entry_hop() {
    let mut s = session(
        &["org.mozilla.firefox"],
        vec![entry("org.mozilla.firefox", "StartupWMClass=firefox\n")],
    );
    s.window(1, "firefox", "page", false, 0.0);
    s.attention("firefox", 5.0);
    assert!(
        lift(&s, "org.mozilla.firefox", 5.0 + 0.25) > 0.0,
        "the first hop (500 ms) is up"
    );
    assert_eq!(
        lift(&s, "org.mozilla.firefox", 5.0 + 1.5),
        0.0,
        "and it plays once"
    );
}

/// A resident application the desktop shows keeps its tile and dot after its last window; a helper process (no desktop
/// entry, or a `NoDisplay` one, like a portal's dialogs) stays connected too, but leaves with its last window.
#[test]
fn only_an_application_the_desktop_shows_stays_in_the_dock_without_windows() {
    let mut s = session(
        &[],
        vec![
            entry("org.example.Notes", ""),
            entry("org.example.Helper", "NoDisplay=true\n"),
        ],
    );
    for app in [
        "org.example.Notes",
        "org.example.Helper",
        "org.example.Portal",
    ] {
        s.window(1, app, "dialog", false, 0.0);
        s.set_resident(app, true, 0.0);
        s.window_closed(1, 1.0);
    }
    assert_eq!(
        lifecycle(&s, "org.example.Notes", 1.0),
        TileLifecycle::Running
    );
    assert!(s.dock.app_tile("org.example.Helper").is_none());
    assert!(s.dock.app_tile("org.example.Portal").is_none());
    s.set_resident("org.example.Notes", false, 6.0);
    assert!(s.dock.app_tile("org.example.Notes").is_none());
}

#[test]
fn only_main_application_windows_make_tiles_one_per_application() {
    let mut s = session(
        &["org.example.Pinned"],
        vec![
            entry("org.example.Browser", "StartupWMClass=browser\n"),
            entry("org.example.Editor", ""),
            entry("xdg-desktop-portal-gtk", "NoDisplay=true\n"),
            entry("org.example.Agent", "NoDisplay=true\n"),
        ],
    );
    // the windows the server lists: two of the browser (one by its window class), one of the editor, a portal's
    // chooser, an authentication agent, a program with no desktop entry, a window with no application id and one of
    // a pinned program that has no entry
    let windows = [
        (1, "org.example.Browser"),
        (2, "browser"),
        (3, "org.example.Editor"),
        (4, "xdg-desktop-portal-gtk"),
        (5, "org.example.Agent"),
        (6, "some-helper"),
        (7, ""),
        (8, "org.example.Pinned"),
    ];
    for (id, app) in windows {
        s.window(id, app, "w", false, 0.0);
        s.set_resident(app, true, 0.0);
    }
    s.set_resident("background-service", true, 0.0);
    let running: Vec<String> = s
        .dock
        .tiles()
        .into_iter()
        .filter(|t| t.kind == TileKind::App && s.has_windows(&t.key))
        .map(|t| t.key.clone())
        .collect();
    assert_eq!(
        running,
        [
            "org.example.Pinned",
            "org.example.Browser",
            "org.example.Editor"
        ]
    );
    assert_eq!(
        s.window_ids(),
        [1, 2, 3, 8],
        "only application windows count"
    );
    for app in [
        "xdg-desktop-portal-gtk",
        "org.example.Agent",
        "some-helper",
        "background-service",
    ] {
        assert!(s.dock.app_tile(app).is_none(), "{app} has no tile");
    }
    // a window that changes to a helper's id leaves its application
    s.window(3, "some-helper", "w", false, 1.0);
    assert!(!s.has_windows("org.example.Editor"));
}
