//! The dock's model is ready to show its bar once everything the bar draws is at hand: the trash's state, the launcher's
//! history the recents come from, and every tile's icon. The component shows no window before that while the server
//! holds the bar's picture of the instance before, so a restarted dock's first picture takes the held one's place
//! whole.

use cupertino_defaults::dock::DockSettings;
use cupertino_dock::actions::ActionMap;
use cupertino_dock::conf::dock_config;
use cupertino_dock::desktop::DesktopIndex;
use cupertino_dock::dock::Dock;
use cupertino_dock::kit::{
    self, Host, IconKey, IconLoad, Message, Model, OutputInfo, Request, Started, WindowIcon,
};
use cupertino_dock::session::Session;
use cupertino_dock::view::IconSet;
use cupertino_kit::core::{Command, WindowKey};
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll, Waker};

/// The output the bar is on.
const OUTPUT: OutputInfo = OutputInfo {
    key: 1,
    width: 1440.0,
    height: 900.0,
    scale: 1.0,
};

/// A host that answers everything away from the model: the trash, full; the launcher's history, empty; every icon,
/// with no renditions.
struct Fixture;

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
        vec![Command::Perform(Box::pin(async {
            Message::RecentApps(Vec::new())
        }))]
    }
    fn preload(&mut self, _: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<IconLoad> {
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
        vec![Command::Perform(Box::pin(async {
            Message::TrashFull(true)
        }))]
    }
}

/// What `future` ends in: the fixture's answers are all ready at once.
fn answered(mut future: Pin<Box<dyn Future<Output = Message> + Send>>) -> Message {
    match future
        .as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
    {
        Poll::Ready(message) => message,
        Poll::Pending => panic!("an answer of the fixture is not ready at once"),
    }
}

/// Which of the host's answers a message is.
fn answer(message: &Message) -> Option<&'static str> {
    match message {
        Message::TrashFull(_) => Some("trash"),
        Message::RecentApps(_) => Some("history"),
        Message::IconsLoaded { .. } => Some("icons"),
        _ => None,
    }
}

/// A bar with a pinned application and the recents shown, on [`OUTPUT`], every answer of the host delivered but
/// `withheld`'s.
fn bar(withheld: Option<&str>) -> Model {
    let mut cfg = dock_config(None, OUTPUT.height);
    cfg.show_recents = true;
    let mut dock = Dock::new(cfg);
    dock.set_pinned(&["org.example.Mail"]);
    let mut model = Model::new(
        Session::new(dock, DesktopIndex::default()),
        ActionMap::default(),
        DockSettings::default(),
        Box::new(Fixture),
    );
    let mut pending = kit::update(&mut model, Message::Outputs(vec![OUTPUT]));
    assert!(!model.ready_to_show(), "nothing is in yet");
    while let Some(command) = pending.pop() {
        let Command::Perform(future) = command else {
            continue;
        };
        let message = answered(future);
        if answer(&message).is_some() && answer(&message) == withheld {
            continue;
        }
        pending.extend(kit::update(&mut model, message));
    }
    model
}

#[test]
fn the_bar_is_ready_to_show_once_all_it_draws_is_in() {
    // (the answer withheld, ready)
    for (withheld, ready) in [
        (None, true),
        (Some("trash"), false),
        (Some("history"), false),
        (Some("icons"), false),
    ] {
        assert_eq!(
            bar(withheld).ready_to_show(),
            ready,
            "{withheld:?} withheld"
        );
    }
}
