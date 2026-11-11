//! Desktop Control and Screen Capture as the window shows them: each app's Forget button and switch as assistive
//! technology and desktop automation read and press them, the keyboard's way through them, the file each one writes,
//! and an answer the compositor records while the page is open.

use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use cupertino_defaults::desktop_control::{App, Decision, DesktopControlConf, Service};
use cupertino_kit::core::{AccessibilityNode, Action};
use cupertino_kit::platform::PlatformEvent;
use cupertino_ui_test::AssistiveTechnology;

use super::*;
use crate::pages::SubPage;

const TAB: u32 = 0xff09;
/// How long a test waits for the file watch to bring another writer's change to the window.
const WATCH_WAIT: Duration = Duration::from_secs(10);

fn conf(dir: &Path) -> std::path::PathBuf {
    paths(dir).desktop_control.unwrap()
}

fn answers(dir: &Path, text: &str) {
    std::fs::write(conf(dir), text).unwrap();
    std::fs::set_permissions(conf(dir), std::fs::Permissions::from_mode(0o600)).unwrap();
}

fn decision(dir: &Path, service: Service, app: &App) -> Option<Decision> {
    DesktopControlConf::load(&conf(dir))
        .unwrap()
        .0
        .decision(service, app)
}

fn meeting() -> App {
    App::Desktop("org.example.Meeting".to_owned())
}

fn agent() -> App {
    App::Executable(std::path::PathBuf::from("/home/me/bin/agent"))
}

/// [`start_hosted`] with an assistive technology attached, which can press the window's controls.
fn start_assisted(
    dir: &Path,
    pane: PaneId,
) -> (TestDriver<SettingsApp>, WindowId, AssistiveTechnology) {
    let (wake, woken) = async_channel::bounded(1);
    let (notify, background) = background_channel(move || {
        let _ = wake.try_send(());
    });
    let services = Services::on(Bus::address(NOWHERE), Bus::address(NOWHERE));
    let state = State::new(paths(dir), services, pane, 1080.0, notify).unwrap();
    launch(Start {
        state,
        background,
        woken,
        activation: String::new(),
    });
    let at = AssistiveTechnology::new();
    let options = RuntimeOptions {
        accessibility: Some(at.adapter()),
        ..runtime_options()
    };
    let mut driver = TestDriver::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(budget()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    driver
        .window_system_mut()
        .configure(window, None, active)
        .unwrap();
    settle(&mut driver, window);
    (driver, window, at)
}

fn nodes(driver: &TestDriver<SettingsApp>, window: WindowId) -> Vec<(NodeId, AccessibilityNode)> {
    driver.runtime().snapshot(window).unwrap().accessibility
}

/// The switch named `name`.
fn switch(
    driver: &TestDriver<SettingsApp>,
    window: WindowId,
    name: &str,
) -> Option<AccessibilityNode> {
    nodes(driver, window)
        .into_iter()
        .map(|(_, n)| n)
        .find(|n| n.role == Role::Switch && n.name.as_deref() == Some(name))
}

/// The Forget button described by `app`, the name of its row.
fn forget(driver: &TestDriver<SettingsApp>, window: WindowId, app: &str) -> Option<NodeId> {
    nodes(driver, window)
        .into_iter()
        .find(|(_, n)| {
            n.role == Role::Button
                && n.name.as_deref() == Some("Forget")
                && n.description.as_deref() == Some(app)
        })
        .map(|(id, _)| id)
}

/// Assistive technology presses `target`.
fn press(
    driver: &mut TestDriver<SettingsApp>,
    window: WindowId,
    at: &AssistiveTechnology,
    target: NodeId,
) {
    driver.ask(at, target, Action::Click, None);
    settle(driver, window);
}

/// Lets the window take what other threads sent it (the file watch's reports) until `done` holds.
fn until(
    driver: &mut TestDriver<SettingsApp>,
    window: WindowId,
    what: &str,
    done: impl Fn(&TestDriver<SettingsApp>) -> bool,
) {
    let end = Instant::now() + WATCH_WAIT;
    while !done(driver) {
        assert!(Instant::now() < end, "{what}");
        std::thread::sleep(Duration::from_millis(20));
        let _ = driver.window_system_mut().enqueue(PlatformEvent::Wake);
        settle(driver, window);
    }
}

#[test]
fn the_privacy_pane_opens_desktop_control_and_screen_capture() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let (mut driver, window) = start(dir.path(), PaneId::Privacy);
    for sub in [SubPage::DesktopControl, SubPage::ScreenCapture] {
        let row = node(&driver, window, Role::Button, sub.title());
        click_node(&mut driver, window, row);
        assert_eq!(title(&mut driver, window), sub.title());
        key(&mut driver, window, BRACKET_LEFT, None, Mods::PRIMARY);
        assert_eq!(title(&mut driver, window), "Privacy & Security");
    }
}

#[test]
fn each_app_reads_as_a_switch_and_a_forget_button_named_by_its_row() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    answers(
        dir.path(),
        "allow = control app:org.example.Meeting\n\
         deny = control exe:/home/me/bin/agent\n\
         allow = capture exe:/home/me/bin/agent\n",
    );
    let (mut driver, window) = start(dir.path(), PaneId::Privacy);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::DesktopControl),
    );
    let meeting = switch(&driver, window, "org.example.Meeting").expect("Meeting's switch");
    // the agent is listed on both pages, so its controls carry the page's title as well
    let agent = switch(&driver, window, "agent, Desktop Control").expect("the agent's switch");
    for (node, on) in [(&meeting, "on"), (&agent, "off")] {
        assert_eq!(
            cupertino_ui_test::describe(node),
            format!("Switch {:?} {on} focusable", node.name.as_deref().unwrap())
        );
        assert_eq!(node.actions, [Action::Click]);
    }
    for app in ["org.example.Meeting", "agent, Desktop Control"] {
        let button = forget(&driver, window, app).unwrap_or_else(|| panic!("{app}'s Forget"));
        let read = nodes(&driver, window)
            .into_iter()
            .find(|(id, _)| *id == button)
            .unwrap()
            .1;
        assert_eq!(
            cupertino_ui_test::describe(&read),
            "Button \"Forget\" focusable"
        );
        assert_eq!(read.actions, [Action::Click]);
    }
    let texts: Vec<String> = nodes(&driver, window)
        .into_iter()
        .filter(|(_, n)| n.role == Role::StaticText)
        .filter_map(|(_, n)| n.name)
        .collect();
    assert!(
        texts.iter().any(|t| t == "/home/me/bin/agent"),
        "the executable's path: {texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("Turning one off stops it at once")),
        "{texts:?}"
    );

    key(&mut driver, window, BRACKET_LEFT, None, Mods::PRIMARY);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::ScreenCapture),
    );
    assert!(
        switch(&driver, window, "agent, Screen Capture")
            .is_some_and(|n| cupertino_ui_test::describe(&n).contains(" on ")),
        "Screen Capture lists its own answers"
    );
    assert!(switch(&driver, window, "org.example.Meeting").is_none());
}

#[test]
fn assistive_technology_turns_an_app_off_and_forgets_another() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    answers(
        dir.path(),
        "allow = control app:org.example.Meeting\n\
         allow = control exe:/home/me/bin/agent\n\
         allow = capture exe:/home/me/bin/agent\n",
    );
    let (mut driver, window, at) = start_assisted(dir.path(), PaneId::Privacy);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::DesktopControl),
    );
    let (meeting_switch, _) = nodes(&driver, window)
        .into_iter()
        .find(|(_, n)| n.role == Role::Switch && n.name.as_deref() == Some("org.example.Meeting"))
        .unwrap();
    press(&mut driver, window, &at, meeting_switch);
    assert_eq!(
        decision(dir.path(), Service::Control, &meeting()),
        Some(Decision::Deny)
    );
    assert!(switch(&driver, window, "org.example.Meeting")
        .is_some_and(|n| cupertino_ui_test::describe(&n).contains(" off ")));

    let agent_forget = forget(&driver, window, "agent, Desktop Control").unwrap();
    press(&mut driver, window, &at, agent_forget);
    assert_eq!(decision(dir.path(), Service::Control, &agent()), None);
    assert_eq!(
        decision(dir.path(), Service::Capture, &agent()),
        Some(Decision::Allow),
        "Forget on Desktop Control leaves Screen Capture's answer"
    );
    for name in ["agent", "agent, Desktop Control"] {
        assert!(switch(&driver, window, name).is_none(), "its row went");
        assert!(forget(&driver, window, name).is_none());
    }
}

#[test]
fn tab_reaches_each_apps_forget_then_its_switch_in_row_order() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    answers(
        dir.path(),
        "allow = capture app:org.example.Meeting\n\
         deny = capture exe:/home/me/bin/agent\n",
    );
    let (mut driver, window) = start(dir.path(), PaneId::Privacy);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::ScreenCapture),
    );
    let mut reached = Vec::new();
    for _ in 0..24 {
        key(&mut driver, window, TAB, None, Mods::NONE);
        let snapshot = driver.runtime().snapshot(window).unwrap();
        let Some(focus) = snapshot.focus else {
            continue;
        };
        if let Some((_, n)) = snapshot.accessibility.iter().find(|(id, _)| *id == focus) {
            let what = match (&n.role, n.description.as_deref()) {
                (Role::Button, Some(row)) => format!("Forget {row}"),
                (role, _) => format!("{role:?} {}", n.name.as_deref().unwrap_or("")),
            };
            reached.push(what);
        }
    }
    let first = reached
        .iter()
        .position(|w| w.starts_with("Forget"))
        .unwrap_or_else(|| panic!("Tab reaches a Forget: {reached:?}"));
    assert_eq!(
        reached[first..first + 4],
        [
            "Forget org.example.Meeting",
            "Switch org.example.Meeting",
            "Forget agent",
            "Switch agent",
        ],
        "{reached:?}"
    );
}

#[test]
fn an_answer_the_compositor_records_shows_while_the_page_is_open_and_a_switch_keeps_it() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    answers(dir.path(), "allow = control app:org.example.Meeting\n");
    let (mut driver, window) = start(dir.path(), PaneId::Privacy);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::DesktopControl),
    );
    assert!(switch(&driver, window, "agent").is_none());

    // the compositor records an app that asked, under the file's lock
    DesktopControlConf::update(&conf(dir.path()), |conf| {
        conf.set(Service::Control, &agent(), Decision::Allow)
            .map_err(std::io::Error::other)
    })
    .unwrap();
    until(&mut driver, window, "the new app's row shows", |d| {
        switch(d, window, "agent").is_some()
    });

    // and turns one off from elsewhere: the switch follows
    DesktopControlConf::update(&conf(dir.path()), |conf| {
        conf.set(Service::Control, &meeting(), Decision::Deny)
            .map_err(std::io::Error::other)
    })
    .unwrap();
    until(&mut driver, window, "the switch shows off", |d| {
        switch(d, window, "org.example.Meeting")
            .is_some_and(|n| cupertino_ui_test::describe(&n).contains(" off "))
    });

    let agent_switch = node(&driver, window, Role::Switch, "agent");
    click_node(&mut driver, window, agent_switch);
    assert_eq!(
        decision(dir.path(), Service::Control, &agent()),
        Some(Decision::Deny)
    );
    assert_eq!(
        decision(dir.path(), Service::Control, &meeting()),
        Some(Decision::Deny),
        "the compositor's answer is kept"
    );
}

#[test]
fn a_refused_file_shows_why_and_offers_nothing_to_switch_or_forget() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    answers(dir.path(), "allow = control app:org.example.Meeting\n");
    std::fs::set_permissions(conf(dir.path()), std::fs::Permissions::from_mode(0o666)).unwrap();
    let (mut driver, window) = start(dir.path(), PaneId::Privacy);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::DesktopControl),
    );
    let read = nodes(&driver, window);
    assert!(
        read.iter().all(|(_, n)| n.role != Role::Switch),
        "no switch"
    );
    assert!(forget(&driver, window, "org.example.Meeting").is_none());
    let texts: Vec<String> = read
        .into_iter()
        .filter(|(_, n)| n.role == Role::StaticText)
        .filter_map(|(_, n)| n.name)
        .collect();
    assert!(
        texts.iter().any(|t| t == "The list can't be read"),
        "{texts:?}"
    );
    assert!(
        texts
            .iter()
            .any(|t| t.contains("other users can write the file")),
        "why it was refused: {texts:?}"
    );
    assert_eq!(
        std::fs::read_to_string(conf(dir.path())).unwrap(),
        "allow = control app:org.example.Meeting\n"
    );
}

/// With `CUPERTINO_LOOK_DIR` set, pictures of the pages for review, light and dark: the Privacy pane, Desktop Control
/// with an app on, one off and an executable, an empty Screen Capture, and a refused file.
#[test]
fn pictures_for_review() {
    let _turn = WINDOW.lock().unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    answers(
        dir.path(),
        "allow = control app:org.example.Meeting\n\
         deny = control app:org.example.Notes\n\
         allow = control exe:/home/me/bin/agent\n",
    );
    let (mut driver, window) = start(dir.path(), PaneId::Privacy);
    driver.look(window, "settings-privacy").unwrap();
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::DesktopControl),
    );
    driver.look(window, "settings-desktop-control").unwrap();
    key(&mut driver, window, BRACKET_LEFT, None, Mods::PRIMARY);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::ScreenCapture),
    );
    driver
        .look(window, "settings-screen-capture-empty")
        .unwrap();

    let refused = tempfile::tempdir().unwrap();
    answers(refused.path(), "allow = control app:org.example.Meeting\n");
    std::fs::set_permissions(conf(refused.path()), std::fs::Permissions::from_mode(0o666)).unwrap();
    let (mut driver, window) = start(refused.path(), PaneId::Privacy);
    click(
        &mut driver,
        window,
        &format!("open-{:?}", SubPage::DesktopControl),
    );
    driver
        .look(window, "settings-desktop-control-refused")
        .unwrap();
}
