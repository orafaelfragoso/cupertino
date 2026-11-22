//! The desktop entries the dock reads again after it started: a window of an application installed since gets the
//! entry's icon, and an `applications` directory created after the start is heard.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

use cupertino_defaults::dock::DockSettings;
use cupertino_dock::actions::ActionMap;
use cupertino_dock::conf::dock_config;
use cupertino_dock::desktop::{parse_entry, DesktopIndex, WatchPlan};
use cupertino_dock::dock::Dock;
use cupertino_dock::kit::{
    self, DockEvent, Host, IconKey, IconLoad, Message, Model, OutputInfo, Request, Started,
    WindowIcon,
};
use cupertino_dock::session::Session;
use cupertino_dock::view::IconSet;
use cupertino_kit::core::{Command, WindowKey};

/// The output the bar is on.
const OUTPUT: OutputInfo = OutputInfo {
    key: 1,
    width: 1440.0,
    height: 900.0,
    scale: 1.0,
};

/// The application of the window, which no entry names at first.
const APP: &str = "org.example.Fresh";

/// What the host was asked to load: the application, and the theme icon its entry named then.
type Loads = Arc<Mutex<Vec<(String, Option<String>)>>>;

/// A host that loads every icon with no renditions, and notes the entry icon each load was asked with.
struct Fixture(Loads);

impl Host for Fixture {
    fn now(&self) -> f64 {
        0.0
    }
    fn wake(&mut self, _: f64, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn request(&mut self, _: &WindowKey, _: Request) {}
    fn spawn(&mut self, _: Vec<String>, _: Option<String>) -> Started {
        Started::No
    }
    fn open(&mut self, _: &str) {}
    fn shell(&mut self, _: &str) {}
    fn store_pins(&mut self, _: &[String]) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn read_conf(&mut self, _: &DockSettings) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn terminal(&mut self) -> Option<String> {
        None
    }
    fn recent_apps(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn preload(&mut self, desktop: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<IconLoad> {
        for (key, _) in &icons {
            if let IconKey::App(app) = key {
                self.0
                    .lock()
                    .unwrap()
                    .push((app.clone(), desktop.entry_icon(app).map(str::to_owned)));
            }
        }
        Some(Box::pin(async move {
            icons
                .into_iter()
                .map(|(key, side)| (key, side as u32, IconSet::new()))
                .collect()
        }))
    }
    fn window_icon(&mut self, _: [String; 2], _: WindowIcon) {}
    fn window_closed(&mut self, _: u32) {}
    fn trash_full(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
}

/// Deliver `message` and every answer the model's commands end in that is ready at once.
fn deliver(model: &mut Model, message: Message) {
    let mut pending = kit::update(model, message);
    while let Some(command) = pending.pop() {
        let Command::Perform(mut future) = command else {
            continue;
        };
        if let Poll::Ready(message) = future
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop()))
        {
            pending.extend(kit::update(model, message));
        }
    }
}

/// An index of one entry, `id` with the theme icon `icon`.
fn index(id: &str, icon: &str) -> DesktopIndex {
    let text = format!("[Desktop Entry]\nType=Application\nName=Fresh\nExec=fresh\nIcon={icon}\n");
    DesktopIndex {
        entries: vec![parse_entry(id, Path::new("/nowhere/fresh.desktop"), &text).unwrap()],
    }
}

#[test]
fn a_window_of_an_application_installed_after_the_start_gets_its_entry_icon() {
    let loads = Loads::default();
    let mut model = Model::new(
        Session::new(
            Dock::new(dock_config(None, OUTPUT.height)),
            DesktopIndex::default(),
        ),
        ActionMap::default(),
        DockSettings::default(),
        Box::new(Fixture(Loads::clone(&loads))),
    );
    deliver(&mut model, Message::Outputs(vec![OUTPUT]));
    let window = DockEvent::Window {
        id: 7,
        app_id: APP.to_owned(),
        title: "Fresh".to_owned(),
        minimized: false,
    };
    deliver(
        &mut model,
        Message::Dock {
            output: OUTPUT.key,
            events: vec![window],
        },
    );
    assert!(model.session.has_windows(APP), "the window has a tile");
    assert!(
        loads
            .lock()
            .unwrap()
            .iter()
            .all(|(app, icon)| app != APP || icon.is_none()),
        "no entry is known yet"
    );

    deliver(
        &mut model,
        Message::Desktop(Box::new(index(APP, "fresh-icon"))),
    );

    assert!(
        model.session.desktop().find(APP).is_some(),
        "the entry is known"
    );
    assert!(model.session.has_windows(APP), "the window keeps its tile");
    assert!(
        loads
            .lock()
            .unwrap()
            .iter()
            .any(|(app, icon)| app == APP && icon.as_deref() == Some("fresh-icon")),
        "the icon is loaded again from the entry: {:?}",
        loads.lock().unwrap()
    );
}

/// A data directory whose `applications` directory does not exist when the dock starts (a fresh home's
/// `~/.local/share`) is watched from the nearest directory above it, for the creations on the way to it only.
#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the test makes and removes its own directories; it has no UI thread"
)]
fn a_missing_applications_directory_is_watched_from_the_nearest_directory_above_it() {
    let root = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("dock-watch-plan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let system = root.join("usr/share");
    std::fs::create_dir_all(system.join("applications")).unwrap();
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let local = home.join(".local/share");
    let flatpak = home.join(".local/flatpak/exports/share");
    let plan = WatchPlan::of(&[local.clone(), system.clone(), flatpak.clone()]);

    assert_eq!(plan.entries, [system.join("applications")]);
    assert_eq!(
        plan.above,
        std::slice::from_ref(&home),
        "one watch above both"
    );
    assert!(plan.waits());
    assert!(plan.concerns(&home.join(".local")));
    assert!(plan.concerns(&local.join("applications")));
    assert!(plan.concerns(&local.join("applications/org.example.Fresh.desktop")));
    assert!(plan.concerns(&system.join("applications/org.example.Other.desktop")));
    assert!(
        !plan.concerns(&home.join(".bash_history")),
        "another file of the directory above concerns nothing"
    );

    std::fs::create_dir_all(local.join("applications")).unwrap();
    std::fs::create_dir_all(&flatpak).unwrap();
    let plan = WatchPlan::of(&[local.clone(), system.clone(), flatpak.clone()]);
    std::fs::remove_dir_all(&root).unwrap();
    assert_eq!(
        plan.entries,
        [local.join("applications"), system.join("applications")]
    );
    assert_eq!(plan.above, [flatpak], "the watch follows it down");
    assert!(plan.waits());
}
