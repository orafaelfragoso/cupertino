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
