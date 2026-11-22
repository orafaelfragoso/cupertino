//! Launching from the dock as the toolkit runs it: the tile that bounces, and the window that ends the bounce.

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_defaults::dock::DockSettings;
use cupertino_dock::actions::ActionMap;
use cupertino_dock::conf::dock_config;
use cupertino_dock::desktop::{DesktopEntry, DesktopIndex};
use cupertino_dock::dock::Dock;
use cupertino_dock::kit::{
    self, DockEvent, Host, IconKey, IconLoad, Message, Model, OutputInfo, Request, Started,
    WindowIcon,
};
use cupertino_dock::session::Session;
use cupertino_dock::view::Input;
use cupertino_kit::core::{Command, WindowKey};

/// The output the bar is on.
const OUTPUT: OutputInfo = OutputInfo {
    key: 1,
    width: 1440.0,
    height: 900.0,
    scale: 1.0,
};

/// What the host saw: the clock it gives the model, and the programs it was asked to start.
#[derive(Default)]
struct Seen {
    now: f64,
    spawned: Vec<Vec<String>>,
}

struct TestHost(Rc<RefCell<Seen>>);

impl Host for TestHost {
    fn now(&self) -> f64 {
        self.0.borrow().now
    }
    fn wake(&mut self, _: f64, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn request(&mut self, _: &WindowKey, _: Request) {}
    fn spawn(&mut self, argv: Vec<String>, _: Option<String>) -> Started {
        self.0.borrow_mut().spawned.push(argv);
        Started::Untracked
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
        Some("foot".to_owned())
    }
    fn recent_apps(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn preload(&mut self, _: &DesktopIndex, _: Vec<(IconKey, f64)>) -> Option<IconLoad> {
        None
    }
    fn window_icon(&mut self, _: [String; 2], _: WindowIcon) {}
    fn window_closed(&mut self, _: u32) {}
    fn trash_full(&mut self) -> Vec<Command<Message>> {
        Vec::new()
    }
}

fn entry(id: &str, exec: &str, terminal: bool) -> DesktopEntry {
    DesktopEntry {
        id: id.to_owned(),
        name: id.to_owned(),
        exec: exec.to_owned(),
        terminal,
        ..DesktopEntry::default()
    }
}

/// A dock with `htop` (an entry that runs in a terminal) pinned, the terminal `foot` installed.
fn model() -> (Model, Rc<RefCell<Seen>>) {
    let seen = Rc::new(RefCell::new(Seen::default()));
    let mut dock = Dock::new(dock_config(None, 900.0));
    dock.set_pinned(&["htop"]);
    let desktop = DesktopIndex {
        entries: vec![entry("foot", "foot", false), entry("htop", "htop", true)],
    };
    let mut session = Session::new(dock, desktop);
    session.list_taken();
    let mut model = Model::new(
        session,
        ActionMap::default(),
        DockSettings::default(),
        Box::new(TestHost(Rc::clone(&seen))),
    );
    kit::update(&mut model, Message::Outputs(vec![OUTPUT]));
    (model, seen)
}

/// Run `message` and hand every activation token it asks for over at once.
fn run(model: &mut Model, message: Message) {
    let mut pending = kit::update(model, message);
    while let Some(command) = pending.pop() {
        if let Command::ActivationToken { on_token, .. } = command {
            pending.extend(kit::update(model, on_token(Some("token".to_owned()))));
        }
    }
}

fn bouncing(model: &Model, key: &str, now: f64) -> bool {
    model
        .session
        .dock
        .app_tile(key)
        .and_then(|t| t.launch)
        .is_some_and(|b| b.is_active(&model.session.dock.bounce, now))
}

#[test]
fn an_entry_that_runs_in_a_terminal_bounces_the_terminal_until_its_window_shows() {
    let (mut model, seen) = model();
    let htop = model
        .session
        .dock
        .app_tile("htop")
        .expect("the pinned tile")
        .id;
    run(&mut model, Message::Input(Input::Activate(htop.0)));
    assert_eq!(seen.borrow().spawned, [["foot", "-e", "htop"]]);
    assert!(bouncing(&model, "foot", 0.5), "the terminal's tile bounces");
    assert!(!bouncing(&model, "htop", 0.5), "the entry's tile does not");

    seen.borrow_mut().now = 1.0;
    run(
        &mut model,
        Message::Dock {
            output: 1,
            events: vec![DockEvent::Window {
                id: 7,
                app_id: "foot".to_owned(),
                title: "htop".to_owned(),
                minimized: false,
            }],
        },
    );
    let later = 2.0 + model.session.dock.bounce.launch_timeout;
    model.session.dock.tick(later);
    let foot = model
        .session
        .dock
        .app_tile("foot")
        .expect("the terminal's tile");
    assert!(foot.running && !foot.is_leaving());
    assert!(
        !bouncing(&model, "foot", later),
        "the terminal's window ended the bounce"
    );
}

#[test]
fn opening_an_entry_in_a_terminal_that_shows_a_window_bounces_nothing() {
    let (mut model, seen) = model();
    run(
        &mut model,
        Message::Dock {
            output: 1,
            events: vec![DockEvent::Window {
                id: 7,
                app_id: "foot".to_owned(),
                title: "~".to_owned(),
                minimized: false,
            }],
        },
    );
    seen.borrow_mut().now = 10.0;
    let htop = model
        .session
        .dock
        .app_tile("htop")
        .expect("the pinned tile")
        .id;
    run(&mut model, Message::Input(Input::Activate(htop.0)));
    assert_eq!(seen.borrow().spawned.len(), 1);
    assert!(!bouncing(&model, "foot", 10.5));
}
