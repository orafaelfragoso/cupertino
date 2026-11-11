//! A menu desktop control opens before the application had any input opens without a grab and closes as a grab would
//! close it; a popup the window system refuses leaves the application running.

use super::super::Runtime;
use crate::test_support::{options, spec, FakeSystem};
use crate::{
    Action, Application, Change, Command, CommandRegistry, Element, Event, EventCx, MeasureCx,
    PaintCx, Popup, PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_geometry::{Point, Size};
use cupertino_semantics as wire;
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{
    Configure, ConfigureId, DecorationMode, KeyEvent, Keysym, Mods, MouseButton, PlatformEvent,
    PointerEvent, Timestamp, WindowEvent, WindowId, WindowSpec, WindowStates, WmCapabilities,
};
use std::cell::RefCell;
use std::num::NonZeroU32;
use std::rc::Rc;

/// A control named `name` that offers `action` and says what it hears: the action as `message`, a press and a key.
struct Actor {
    name: &'static str,
    action: Action,
    message: &'static str,
}

impl Widget<String> for Actor {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(10.0, 10.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, String>) -> Propagation {
        let heard = match event {
            Event::Action(request) if request.action == self.action => self.message.to_owned(),
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _) => {
                format!("{} pressed", self.name)
            }
            Event::Key(key, _) if key.pressed => format!("{} heard a key", self.name),
            _ => return Propagation::Continue,
        };
        cx.emit(heard);
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.name);
        cx.action(self.action);
    }
}

fn actor(name: &'static str, action: Action, message: &'static str) -> Element<String> {
    Element::new(Actor {
        name,
        action,
        message,
    })
}

/// A window with an opener that shows a menu, whose `More` opens a submenu.
struct Menus;

#[derive(Default)]
struct Shown {
    menu: bool,
    submenu: bool,
    /// Keeps declaring the menu after it is dismissed.
    keep: bool,
    /// Every message, in order.
    heard: Vec<String>,
}

fn window(key: &str, content: Element<String>, popup: Option<Popup<String>>) -> Window<String> {
    Window {
        key: WindowKey(key.to_owned().into()),
        spec: WindowSpec {
            title: key.to_owned(),
            app_id: "test.menus".into(),
            size: Size::new(200.0, 100.0),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Preferred,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        popup,
        background: WindowBackground::Theme,
        content,
        menus: None,
    }
}

fn popup(parent: &str, dismissed: &str) -> Option<Popup<String>> {
    Some(Popup {
        parent: WindowKey(parent.to_owned().into()),
        anchor: PopupAnchor::At(Point::new(0.0, 20.0)),
        grab: true,
        on_dismiss: dismissed.to_owned(),
    })
}

impl Application for Menus {
    type Model = Shown;
    type Message = String;

    fn init() -> (Shown, Vec<Command<String>>) {
        (Shown::default(), Vec::new())
    }

    fn update(model: &mut Shown, message: String) -> Vec<Command<String>> {
        match message.as_str() {
            "show menu" => model.menu = true,
            "more" => model.submenu = true,
            "keep" => model.keep = true,
            "menu dismissed" if !model.keep => {
                model.menu = false;
                model.submenu = false;
            }
            "submenu dismissed" => model.submenu = false,
            _ => {}
        }
        // the row around the main window's controls tells of every press and key it passes on
        if !message.starts_with("group:") {
            model.heard.push(message);
        }
        Vec::new()
    }

    fn view(model: &Shown) -> Vec<Window<String>> {
        let mut windows = vec![window(
            "main",
            Element::new(crate::test_support::row(vec![
                actor("Opener", Action::ShowMenu, "show menu"),
                actor("Keep", Action::Click, "keep"),
            ])),
            None,
        )];
        if model.menu {
            windows.push(window(
                "menu",
                actor("More", Action::Expand, "more"),
                popup("main", "menu dismissed"),
            ));
        }
        if model.submenu {
            windows.push(window(
                "submenu",
                actor("Deep", Action::Click, "deep"),
                popup("menu", "submenu dismissed"),
            ));
        }
        windows
    }
}

fn steps() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}

/// The application on a window system that publishes semantics, its main window shown.
struct Desk {
    rt: Runtime<Menus, FakeSystem>,
    configures: u32,
    next_call: u64,
    /// Every window error reported: the window and the error.
    errors: Rc<RefCell<Vec<(WindowId, String)>>>,
}

impl Desk {
    /// `input` says whether the window system has had an input event; `popups` whether it opens popups at all.
    fn new(input: bool, popups: bool) -> Self {
        let errors: Rc<RefCell<Vec<(WindowId, String)>>> = Rc::default();
        let mut options = options(&Rc::default());
        let reported = Rc::clone(&errors);
        options.on_window_error = Some(Box::new(move |window, error| {
            reported.borrow_mut().push((window, format!("{error:?}")));
        }));
        let mut system = FakeSystem::default();
        system.popups = popups;
        system.no_input = !input;
        system.semantics = true;
        let mut rt = Runtime::new(system, options);
        rt.step(steps()).unwrap();
        let mut desk = Self {
            rt,
            configures: 0,
            next_call: 0,
            errors,
        };
        desk.show("main");
        desk
    }

    fn id(&self, key: &str) -> Option<WindowId> {
        self.rt
            .windows()
            .into_iter()
            .find(|(k, _)| *k == WindowKey(key.to_owned().into()))
            .map(|(_, id)| id)
    }

    /// Configures window `key` and lets every window draw.
    fn show(&mut self, key: &str) {
        self.configures += 1;
        let id = self.id(key).unwrap_or_else(|| panic!("no window {key}"));
        self.rt.window_system_mut().send(
            id,
            WindowEvent::Configure(Configure {
                id: ConfigureId(self.configures),
                size: Some(Size::new(200.0, 100.0)),
                states: WindowStates {
                    activated: true,
                    ..WindowStates::default()
                },
                bounds: None,
                server_decorations: true,
                wm_capabilities: WmCapabilities {
                    window_menu: true,
                    maximize: true,
                    fullscreen: true,
                    minimize: true,
                },
            }),
        );
        self.settle();
    }

    /// Runs, handing every window a frame, until nothing is left to do.
    fn settle(&mut self) {
        for _ in 0..4 {
            self.rt.step(steps()).unwrap();
            for (_, id) in self.rt.windows() {
                self.rt
                    .window_system_mut()
                    .send(id, WindowEvent::Frame(Timestamp(0)));
            }
        }
        self.rt.step(steps()).unwrap();
    }

    /// Desktop control asks for `action` on the node named `name` of window `key`; how it ended.
    fn act(&mut self, key: &str, name: &str, action: wire::Action) -> Option<wire::Outcome> {
        let window = self.id(key).unwrap_or_else(|| panic!("no window {key}"));
        let snapshot = self
            .rt
            .window_system()
            .published
            .iter()
            .find(|(w, _)| *w == window)
            .map(|(_, s)| s.clone())
            .unwrap_or_else(|| panic!("{key} published nothing"));
        let node = snapshot
            .nodes
            .iter()
            .find(|n| n.name.as_deref() == Some(name))
            .unwrap_or_else(|| panic!("no {name} in {key}"))
            .id;
        self.next_call += 1;
        let call = wire::ActionCall {
            id: self.next_call,
            epoch: 1,
            revision: snapshot.revision,
            node,
            action,
            data: None,
        };
        let system = self.rt.window_system_mut();
        system.semantic_calls.push((window, call));
        system.queue.push_back(PlatformEvent::Wake);
        self.settle();
        let id = self.next_call;
        self.rt
            .window_system()
            .semantic_outcomes
            .iter()
            .find(|(_, call, _)| *call == id)
            .map(|(_, _, outcome)| *outcome)
    }

    fn send(&mut self, key: &str, event: WindowEvent) {
        let id = self.id(key).unwrap_or_else(|| panic!("no window {key}"));
        self.rt.window_system_mut().send(id, event);
        self.settle();
    }

    fn button(&mut self, key: &str, position: Point, pressed: bool) {
        let event = PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        };
        self.send(key, WindowEvent::Pointer(event, Timestamp(0)));
    }

    fn key(&mut self, key: &str) {
        let event = KeyEvent {
            code: 108,
            keysym: Keysym(0xff54),
            text: None,
            pressed: true,
            repeat: false,
            mods: Mods::NONE,
        };
        self.send(key, WindowEvent::Key(event, Timestamp(0)));
    }

    /// Whether each popup asked for so far took a grab, in order.
    fn grabs(&self) -> Vec<bool> {
        self.rt
            .window_system()
            .popups_created
            .iter()
            .map(|(_, _, placed)| placed.grab)
            .collect()
    }

    /// What the application heard since `from` messages.
    fn heard_since(&self, from: usize) -> Vec<String> {
        self.rt.model().heard[from..].to_vec()
    }

    /// The menu shown by desktop control.
    fn open_menu(&mut self) {
        let outcome = self.act("main", "Opener", wire::Action::ShowMenu);
        assert_eq!(outcome, Some(wire::Outcome::Applied));
        assert!(self.id("menu").is_some(), "the menu opened");
        self.show("menu");
    }
}

/// The opener, where a press lands on it.
const OPENER: Point = Point::new(5.0, 5.0);

#[test]
fn a_menu_shown_before_any_input_opens_without_a_grab_and_its_submenu_too() {
    let mut desk = Desk::new(false, true);
    desk.open_menu();
    let outcome = desk.act("menu", "More", wire::Action::Expand);
    assert_eq!(outcome, Some(wire::Outcome::Applied));
    assert!(desk.id("submenu").is_some(), "the submenu opened");
    // the fake system fails the test when a grabbing popup opens from one without a grab
    assert_eq!(desk.grabs(), [false, false]);
    assert!(desk.errors.borrow().is_empty(), "{:?}", desk.errors);
    // the submenu's item still acts
    desk.show("submenu");
    assert_eq!(
        desk.act("submenu", "Deep", wire::Action::Click),
        Some(wire::Outcome::Applied)
    );
    assert_eq!(
        desk.rt.model().heard.last().map(String::as_str),
        Some("deep")
    );
}

/// From a window system whose actions come with a grab of their own, a menu an action shows before any input grabs as
/// one a press opened would, and so does the submenu a later action opens; presses are then the compositor's to judge.
#[test]
fn a_menu_an_action_with_its_own_grab_shows_before_any_input_grabs_and_its_submenu_too() {
    let mut desk = Desk::new(false, true);
    desk.rt.window_system_mut().action_grabs = true;
    desk.open_menu();
    assert_eq!(desk.grabs(), [true]);
    let outcome = desk.act("menu", "More", wire::Action::Expand);
    assert_eq!(outcome, Some(wire::Outcome::Applied));
    assert!(desk.id("submenu").is_some(), "the submenu opened");
    assert_eq!(desk.grabs(), [true, true]);
    assert!(desk.errors.borrow().is_empty(), "{:?}", desk.errors);
    let from = desk.rt.model().heard.len();
    desk.button("main", OPENER, true);
    assert_eq!(desk.heard_since(from), ["Opener pressed"]);
    assert!(desk.id("menu").is_some());
}

#[test]
fn a_press_outside_an_ungrabbed_menu_closes_it_and_goes_no_further() {
    let mut desk = Desk::new(false, true);
    desk.open_menu();
    desk.act("menu", "More", wire::Action::Expand);
    desk.show("submenu");
    let from = desk.rt.model().heard.len();
    desk.button("main", OPENER, true);
    let heard = desk.heard_since(from);
    assert!(heard.contains(&"menu dismissed".to_owned()), "{heard:?}");
    assert!(heard.contains(&"submenu dismissed".to_owned()), "{heard:?}");
    assert!(
        !heard.contains(&"Opener pressed".to_owned()),
        "a press that closes the menu reaches nothing: {heard:?}"
    );
    assert_eq!(desk.id("menu"), None);
    assert_eq!(desk.id("submenu"), None);
    // its release, and the next press, reach the window as usual
    desk.button("main", OPENER, false);
    desk.button("main", OPENER, true);
    assert_eq!(
        desk.rt.model().heard.last().map(String::as_str),
        Some("Opener pressed")
    );
}

#[test]
fn a_press_in_an_ungrabbed_menu_reaches_it() {
    let mut desk = Desk::new(false, true);
    desk.open_menu();
    let from = desk.rt.model().heard.len();
    desk.button("menu", OPENER, true);
    assert_eq!(desk.heard_since(from), ["More pressed"]);
    assert!(desk.id("menu").is_some());
}

#[test]
fn keys_of_the_opener_go_to_its_ungrabbed_menu_until_it_loses_the_keyboard() {
    let mut desk = Desk::new(false, true);
    desk.open_menu();
    // input comes afterwards: the menu stays as it opened
    desk.rt.window_system_mut().no_input = false;
    desk.send("main", WindowEvent::Focus(true));
    let from = desk.rt.model().heard.len();
    desk.key("main");
    assert_eq!(desk.heard_since(from), ["More heard a key"]);
    desk.send("main", WindowEvent::Focus(false));
    assert_eq!(
        desk.heard_since(from),
        ["More heard a key", "menu dismissed"]
    );
    assert_eq!(desk.id("menu"), None);
}

#[test]
fn with_input_a_menu_takes_its_grab_and_the_runtime_leaves_presses_alone() {
    let mut desk = Desk::new(true, true);
    desk.open_menu();
    assert_eq!(desk.grabs(), [true]);
    let from = desk.rt.model().heard.len();
    // the compositor dismisses a grabbed menu itself; what it delivers reaches the window
    desk.button("main", OPENER, true);
    assert_eq!(desk.heard_since(from), ["Opener pressed"]);
    assert!(desk.id("menu").is_some());
}

#[test]
fn a_popup_the_window_system_refuses_is_dismissed_reported_and_answered_unavailable() {
    let mut desk = Desk::new(true, false);
    let main = desk.id("main").unwrap();
    let outcome = desk.act("main", "Opener", wire::Action::ShowMenu);
    assert_eq!(outcome, Some(wire::Outcome::Unavailable));
    assert_eq!(desk.rt.model().heard, ["show menu", "menu dismissed"]);
    assert_eq!(desk.id("menu"), None);
    assert_eq!(desk.errors.borrow().len(), 1);
    assert_eq!(
        desk.errors.borrow()[0].0,
        main,
        "reported against the parent"
    );
    assert!(!desk.rt.exited());
    // a model that keeps declaring it is not asked for it again, nor told again, until it stops
    desk.act("main", "Keep", wire::Action::Click);
    let outcome = desk.act("main", "Opener", wire::Action::ShowMenu);
    assert_eq!(outcome, Some(wire::Outcome::Unavailable));
    assert_eq!(desk.errors.borrow().len(), 2);
    let from = desk.rt.model().heard.len();
    desk.act("main", "Keep", wire::Action::Click);
    assert_eq!(desk.heard_since(from), ["keep"]);
    assert_eq!(desk.errors.borrow().len(), 2);
    assert_eq!(desk.id("menu"), None);
}
