//! The launcher's Kit model (`kit::Model`, `kit::update`, `kit::view`) through a test host: the panel's windows come
//! and go with it, the server closing them or the session lock hides it at once, the launch history read as the
//! component starts takes the place of a clean one, and a changed history is handed to the host to write when the
//! panel hides.

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_kit::core::Command;
use cupertino_kit::geometry::Size;
use cupertino_launcher::component::Shell;
use cupertino_launcher::engine::ResultItem;
use cupertino_launcher::files::FileSearchConfig;
use cupertino_launcher::kit::{self, Host, Message, Model};
use cupertino_launcher::panel::Now;
use cupertino_launcher::{view, CommandSpec, Frecency, Launcher, LauncherConfig};

/// What the test host was asked.
#[derive(Default)]
struct Asked {
    /// The launch histories handed over to write.
    saved: Vec<Frecency>,
}

struct TestHost(Rc<RefCell<Asked>>);

impl Host for TestHost {
    fn run(&mut self, _: CommandSpec, _: Option<String>) {}
    fn open(&mut self, _: &str) {}
    fn index(&mut self, _: FileSearchConfig) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn resize(&mut self, _: Size) {}
    fn icon(&mut self, _: &ResultItem) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn preview(&mut self, _: &ResultItem) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn save_history(&mut self, history: Frecency) -> Vec<Command<Message>> {
        self.0.borrow_mut().saved.push(history);
        Vec::new()
    }
    fn now(&self) -> Now {
        Now {
            mono: 0.0,
            unix: UNIX_NOW,
        }
    }
}

/// The wall clock of the tests, in Unix seconds.
const UNIX_NOW: f64 = 1_794_650_400.0;
/// The size of the output the panel opens on.
const OUTPUT: Size = Size::new(1512.0, 982.0);

/// A model with no applications whose transitions end at once, and what its host was asked.
fn launcher_model() -> (Model, Rc<RefCell<Asked>>) {
    let asked = Rc::new(RefCell::new(Asked::default()));
    let launcher = Launcher::new(LauncherConfig::default(), Vec::new(), Frecency::in_memory());
    let mut model = Model::new(launcher, Box::new(TestHost(Rc::clone(&asked))));
    model.settled = true;
    kit::update(&mut model, Message::Output(OUTPUT));
    (model, asked)
}

fn keys(model: &Model) -> Vec<cupertino_kit::core::WindowKey> {
    kit::view(model).into_iter().map(|w| w.key).collect()
}

#[test]
fn the_hidden_launcher_has_no_window() {
    let (model, _) = launcher_model();
    assert!(!model.presenting());
    assert!(kit::view(&model).is_empty());
}

#[test]
fn toggle_shows_the_panel_and_its_catcher_and_a_second_hides_them() {
    let (mut model, _) = launcher_model();
    kit::update(&mut model, Message::Toggle);
    assert!(model.nav.is_visible());
    assert_eq!(
        keys(&model),
        [view::panel_window_key(), view::catcher_key()]
    );
    kit::update(&mut model, Message::Toggle);
    assert!(!model.nav.is_visible());
    assert!(!model.presenting());
    assert!(kit::view(&model).is_empty());
}

#[test]
fn the_server_closing_the_panel_or_the_session_lock_hides_it_at_once() {
    for message in [Message::Gone, Message::Locked] {
        let (mut model, _) = launcher_model();
        kit::update(&mut model, Message::Toggle);
        assert!(model.presenting());
        kit::update(&mut model, message.clone());
        assert!(!model.presenting(), "{message:?} hides the panel");
        assert!(kit::view(&model).is_empty(), "{message:?} leaves no window");
    }
}

#[test]
fn what_the_component_hears_changes_nothing_in_the_model() {
    let (mut model, _) = launcher_model();
    let commands = kit::update(&mut model, Message::Shell(Shell::Saved));
    assert!(commands.is_empty());
    assert!(kit::view(&model).is_empty());
}

#[test]
fn the_history_read_at_start_takes_the_place_of_a_clean_one_only() {
    let mut read = Frecency::in_memory();
    read.record("app:org.example.Read.desktop", "", UNIX_NOW);

    let (mut model, _) = launcher_model();
    kit::update(&mut model, Message::History(read.clone()));
    assert_eq!(
        model
            .launcher
            .frecency
            .launches("app:org.example.Read.desktop"),
        1
    );

    // a launch made before the history was read is kept until the next start
    let (mut model, _) = launcher_model();
    model
        .launcher
        .frecency
        .record("app:org.example.Early.desktop", "", UNIX_NOW);
    kit::update(&mut model, Message::History(read));
    assert_eq!(
        model
            .launcher
            .frecency
            .launches("app:org.example.Early.desktop"),
        1
    );
    assert_eq!(
        model
            .launcher
            .frecency
            .launches("app:org.example.Read.desktop"),
        0
    );
}

#[test]
fn hiding_hands_a_changed_history_to_the_host_once() {
    let (mut model, asked) = launcher_model();
    model
        .launcher
        .frecency
        .record("app:org.example.Used.desktop", "us", UNIX_NOW);
    kit::update(&mut model, Message::Toggle);
    kit::update(&mut model, Message::Toggle);
    {
        let asked = asked.borrow();
        assert_eq!(asked.saved.len(), 1, "the changed history is handed over");
        assert_eq!(asked.saved[0].launches("app:org.example.Used.desktop"), 1);
    }
    assert!(
        !model.launcher.frecency.is_dirty(),
        "the model's copy counts as written"
    );

    kit::update(&mut model, Message::Toggle);
    kit::update(&mut model, Message::Gone);
    assert_eq!(
        asked.borrow().saved.len(),
        1,
        "an unchanged history is not written again"
    );
}
