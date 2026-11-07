//! The dock as CupertinoKit runs it, headless: the model of `cupertino_dock::kit` behind a fake host, the session's
//! messages handed in by the test, and the pointer and the keys sent to the windows the runtime opened. The tests check
//! what the dock asks of the host and the server, and what it tells assistive technology.

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::path::Path;
use std::rc::Rc;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_defaults::dock::DockSettings;
use cupertino_dock::actions::{ActionMap, Trigger};
use cupertino_dock::desktop::{parse_entry, DesktopIndex};
use cupertino_dock::kit::{
    self, DockEvent, Host, IconKey, Message, Model, OutputInfo, Part, Placement, Request, Sized,
};
use cupertino_dock::overview_ui::{Mode, Workspace};
use cupertino_dock::session::Session;
use cupertino_dock::view::{
    IconSet, BAR_APP_ID, EDGE_APP_ID, FORCE_QUIT_APP_ID, GRID_APP_ID, OVERVIEW_APP_ID,
    SWITCHER_APP_ID,
};
use cupertino_dock::{Dock, DockConfig};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    next_resource_id, Application, Command, Key, PlaceholderIcons, Role, RuntimeOptions, Window,
};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer, ImageResource, PixelFormat};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use cupertino_wm::geometry::Rect as WmRect;

const EDITOR: &str = "org.example.Editor";
const TERMINAL: &str = "org.example.Terminal";

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;
const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;
const DOWN: u32 = 0xff54;
const TAB: u32 = 0xff09;
const SPACE: u32 = 0x20;

/// The main display and a second one.
const OUTPUTS: [OutputInfo; 2] = [
    OutputInfo {
        key: 1,
        width: 1280.0,
        height: 800.0,
        scale: 1.0,
    },
    OutputInfo {
        key: 2,
        width: 1024.0,
        height: 768.0,
        scale: 1.0,
    },
];

thread_local! {
    /// The model and the test's end of the message channel, taken by [`Run::init`].
    static START: RefCell<Option<(Model, async_channel::Receiver<Message>)>> = const { RefCell::new(None) };
    /// The message being handled came from the test, so the next one is listened for.
    static FROM_TEST: Cell<bool> = const { Cell::new(false) };
}

/// What the fake host was asked, and its clock.
#[derive(Default)]
struct Record {
    now: f64,
    wake: Option<u64>,
    /// How long after each wake the model asked to be woken, in seconds.
    wakes: Vec<f64>,
    requests: Vec<(u32, Request)>,
    placements: Vec<Placement>,
    keyboard: Vec<(Part, bool)>,
    spawned: Vec<(Vec<String>, Option<String>)>,
    pins: Vec<Vec<String>>,
    shell: Vec<String>,
}

struct FakeHost(Rc<RefCell<Record>>);

/// A square image `px` device pixels large: a solid square with a transparent margin of a tenth of its side, as an
/// icon leaves room around its shape.
fn square(px: u32) -> ImageSource {
    let margin = px / 10;
    let inside = |i: u32| (margin..px - margin).contains(&i);
    let pixels: Vec<u8> = (0..px * px)
        .flat_map(|i| {
            if inside(i % px) && inside(i / px) {
                [40u8, 90, 200, 255]
            } else {
                [0; 4]
            }
        })
        .collect();
    ImageSource {
        id: next_resource_id(),
        image: ImageResource {
            revision: 0,
            width: px,
            height: px,
            stride: px * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels),
        },
    }
}

impl Host for FakeHost {
    fn now(&self) -> f64 {
        self.0.borrow().now
    }
    fn wake(&mut self, after: f64, generation: u64) -> Vec<Command<Message>> {
        let mut record = self.0.borrow_mut();
        record.wake = Some(generation);
        record.wakes.push(after);
        drop(record);
        Vec::new()
    }
    fn request(&mut self, output: u32, request: Request) {
        self.0.borrow_mut().requests.push((output, request));
    }
    fn place_bar(&mut self, placement: Placement) {
        self.0.borrow_mut().placements.push(placement);
    }
    fn keyboard(&mut self, part: Part, on: bool) {
        self.0.borrow_mut().keyboard.push((part, on));
    }
    fn resize_force_quit(&mut self, _: f64, _: f64) {}
    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> bool {
        self.0.borrow_mut().spawned.push((argv, token));
        true
    }
    fn open(&mut self, _: &str) {}
    fn shell(&mut self, command: &str) {
        self.0.borrow_mut().shell.push(command.to_owned());
    }
    fn store_pins(&mut self, pins: &[String]) {
        self.0.borrow_mut().pins.push(pins.to_vec());
    }
    fn read_conf(&mut self, _: &DockSettings) -> Option<DockSettings> {
        None
    }
    fn terminal(&mut self) -> Option<String> {
        None
    }
    fn icon(&mut self, _: &DesktopIndex, _: &IconKey, side: f64) -> IconSet {
        [1.0, 2.0]
            .iter()
            .map(|scale| square((side * scale).ceil().max(1.0) as u32))
            .collect()
    }
    fn window_icon(&mut self, _: [String; 2], _: kit::WindowIcon) {}
    fn window_closed(&mut self, _: u32) {}
    fn trash_full(&mut self) -> bool {
        false
    }
}

struct Dockside;

struct State {
    model: Model,
    heard: async_channel::Receiver<Message>,
}

fn listen(heard: &async_channel::Receiver<Message>) -> Command<Message> {
    let heard = heard.clone();
    Command::Perform(Box::pin(async move {
        let message = heard.recv().await.expect("the test holds the sender");
        FROM_TEST.set(true);
        message
    }))
}

impl Application for Dockside {
    type Model = State;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (State, Vec<Command<Message>>) {
        let (model, heard) = START.with_borrow_mut(Option::take).expect("a run");
        let first = listen(&heard);
        (State { model, heard }, vec![first])
    }

    fn update(state: &mut State, message: Message) -> Vec<Command<Message>> {
        let from_test = FROM_TEST.replace(false);
        let mut commands = kit::update(&mut state.model, message);
        if from_test {
            commands.push(listen(&state.heard));
        }
        commands
    }

    fn view(state: &State) -> Vec<Window<Message>> {
        kit::view(&state.model)
    }
}

fn entry(id: &str, name: &str) -> cupertino_dock::desktop::DesktopEntry {
    let exec = name.to_lowercase();
    parse_entry(
        id,
        Path::new("x"),
        &format!("[Desktop Entry]\nName={name}\nExec={exec} %U\n"),
    )
    .unwrap()
}

fn tokens(reduce_transparency: bool) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

struct Run {
    driver: TestDriver<Dockside>,
    send: async_channel::Sender<Message>,
    record: Rc<RefCell<Record>>,
}

impl Run {
    /// The dock with the editor and the terminal pinned, on two outputs, the terminal running.
    fn start() -> Self {
        Self::with(tokens(false), ActionMap::default())
    }

    fn with(theme: ThemeTokens, actions: ActionMap) -> Self {
        Self::on(theme, actions, true)
    }

    /// [`Run::with`] on a desktop that offers its materials (`app_effects`) or not, where the windows paint their fills.
    fn on(theme: ThemeTokens, actions: ActionMap, app_effects: bool) -> Self {
        // window ids start again with a new runtime: none of them is configured yet
        CONFIGURED.with_borrow_mut(Vec::clear);
        let record = Rc::new(RefCell::new(Record::default()));
        let mut dock = Dock::new(DockConfig::default());
        dock.set_pinned(&[EDITOR, TERMINAL]);
        let desktop = DesktopIndex {
            entries: vec![entry(EDITOR, "Editor"), entry(TERMINAL, "Terminal")],
        };
        let model = Model::new(
            Session::new(dock, desktop),
            actions,
            DockSettings::default(),
            Box::new(FakeHost(Rc::clone(&record))),
        );
        let (send, heard) = async_channel::unbounded();
        START.set(Some((model, heard)));
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 400,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 22,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: None,
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let capabilities = Capabilities {
            app_effects,
            ..Capabilities::default()
        };
        let driver = TestDriver::new(capabilities, options, ManualClock::new(Timestamp(0)));
        let mut run = Self {
            driver,
            send,
            record,
        };
        run.settle();
        run.message(Message::Outputs(OUTPUTS.to_vec()));
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::Window {
                id: 7,
                app_id: TERMINAL.to_owned(),
                title: "shell".to_owned(),
                minimized: false,
            }],
        });
        run.advance(1.0);
        run
    }

    /// Run until nothing is left to do, configuring every new window and drawing every requested frame.
    fn settle(&mut self) {
        for _ in 0..32 {
            self.driver.run_until_idle(budget()).unwrap();
            let mut busy = false;
            for window in self.driver.window_system_mut().windows() {
                let configured = self.configured(window);
                let system = self.driver.window_system_mut();
                if !system.unacknowledged(window).unwrap().is_empty() {
                    continue;
                }
                if system.frames(window).unwrap().is_empty() && !configured {
                    let active = WindowStates {
                        activated: true,
                        ..WindowStates::default()
                    };
                    system.configure(window, None, active).unwrap();
                    CONFIGURED.with_borrow_mut(|c| c.push(window));
                    busy = true;
                }
                let system = self.driver.window_system_mut();
                if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    busy = true;
                }
            }
            if !busy {
                return;
            }
        }
    }

    fn configured(&self, window: WindowId) -> bool {
        CONFIGURED.with_borrow(|c| c.contains(&window))
    }

    fn message(&mut self, message: Message) {
        self.send.try_send(message).unwrap();
        self.settle();
    }

    /// Move the host's clock on by `seconds` and wake the model.
    fn advance(&mut self, seconds: f64) {
        let generation = {
            let mut record = self.record.borrow_mut();
            record.now += seconds;
            record.wake.unwrap_or(0)
        };
        self.message(Message::Wake(generation));
    }

    /// The open window whose `app_id` is `app_id`.
    fn window(&mut self, app_id: &str) -> Option<WindowId> {
        let system = self.driver.window_system_mut();
        system
            .windows()
            .into_iter()
            .find(|&w| system.spec(w).is_ok_and(|s| s.app_id == app_id))
    }

    fn bar(&mut self) -> WindowId {
        self.window(&format!("{BAR_APP_ID}@1"))
            .expect("the bar is open")
    }

    /// Every accessible node of `window`: role, name, description.
    fn nodes(&self, window: WindowId) -> Vec<(Role, String, String, Rect)> {
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
        snapshot
            .accessibility
            .iter()
            .map(|(_, n)| {
                (
                    n.role,
                    n.name.clone().unwrap_or_default(),
                    n.description.clone().unwrap_or_default(),
                    n.bounds,
                )
            })
            .collect()
    }

    fn bounds(&self, window: WindowId, role: Role, name: &str) -> Rect {
        self.nodes(window)
            .into_iter()
            .find(|n| n.0 == role && n.1 == name)
            .unwrap_or_else(|| panic!("no {role:?} named {name}"))
            .3
    }

    fn focused(&self, window: WindowId) -> Option<String> {
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
        let focus = snapshot.focus?;
        snapshot
            .accessibility
            .iter()
            .find(|(id, _)| *id == focus)
            .and_then(|(_, n)| n.name.clone())
    }

    fn send_to(&mut self, window: WindowId, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(window, event.clone())
            .unwrap_or_else(|e| {
                panic!(
                    "{e:?} for {event:?}; open: {:?}",
                    self.driver.runtime().windows()
                )
            });
        self.settle();
    }

    fn pointer(&mut self, window: WindowId, event: PointerEvent) {
        self.send_to(window, WindowEvent::Pointer(event, Timestamp(0)));
    }

    fn button(&mut self, window: WindowId, position: Point, button: MouseButton, pressed: bool) {
        self.pointer(
            window,
            PointerEvent::Button {
                position,
                button,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            },
        );
    }

    fn click(&mut self, window: WindowId, position: Point) {
        self.pointer(window, PointerEvent::Enter { position });
        self.button(window, position, MouseButton::Left, true);
        self.button(window, position, MouseButton::Left, false);
    }

    /// Press and release `keysym`; the release is not sent when the press closed the window.
    fn key(&mut self, window: WindowId, keysym: u32) {
        for pressed in [true, false] {
            if !pressed && !self.driver.window_system_mut().windows().contains(&window) {
                break;
            }
            self.send_to(
                window,
                WindowEvent::Key(
                    KeyEvent {
                        code: 1,
                        keysym: Keysym(keysym),
                        text: None,
                        pressed,
                        repeat: false,
                        mods: Mods::NONE,
                    },
                    Timestamp(0),
                ),
            );
        }
    }

    /// Type `text` into `window`, one key per character.
    fn type_text(&mut self, window: WindowId, text: &str) {
        for c in text.chars() {
            for pressed in [true, false] {
                self.send_to(
                    window,
                    WindowEvent::Key(
                        KeyEvent {
                            code: 1,
                            keysym: Keysym(u32::from(c)),
                            text: pressed.then_some(c),
                            pressed,
                            repeat: false,
                            mods: Mods::NONE,
                        },
                        Timestamp(0),
                    ),
                );
            }
        }
    }

    fn requests(&self) -> Vec<(u32, Request)> {
        self.record.borrow().requests.clone()
    }
}

thread_local! {
    static CONFIGURED: RefCell<Vec<WindowId>> = const { RefCell::new(Vec::new()) };
}

fn centre(r: Rect) -> Point {
    Point::new(r.x() + r.width() / 2.0, r.y() + r.height() / 2.0)
}

#[test]
fn the_bar_is_a_toolbar_of_named_buttons_on_the_main_display() {
    let mut run = Run::start();
    let bar = run.bar();
    let nodes = run.nodes(bar);
    assert!(nodes.iter().any(|n| n.0 == Role::Toolbar && n.1 == "Dock"));
    let buttons: Vec<(String, String)> = nodes
        .iter()
        .filter(|n| n.0 == Role::Button)
        .map(|n| (n.1.clone(), n.2.clone()))
        .collect();
    assert_eq!(
        buttons,
        [
            ("Editor".to_owned(), String::new()),
            ("Terminal".to_owned(), kit::RUNNING.to_owned()),
            (kit::TRASH_NAME.to_owned(), String::new()),
        ],
        "every tile is a button named after it, the running one says so"
    );
    assert!(nodes.iter().any(|n| n.0 == Role::Separator));
    let placed = run.record.borrow().placements.last().copied().unwrap();
    assert_eq!(placed.output, 1, "the dock starts on the first output");
    assert!(
        run.window(&format!("{EDGE_APP_ID}@2")).is_some(),
        "the other output gets an edge strip"
    );
    assert!(run.window(&format!("{EDGE_APP_ID}@1")).is_none());
}

#[test]
fn a_click_activates_a_running_application_and_launches_an_idle_one_with_a_token() {
    let mut run = Run::start();
    let bar = run.bar();
    let terminal = run.bounds(bar, Role::Button, "Terminal");
    run.click(bar, centre(terminal));
    assert!(
        run.requests()
            .contains(&(1, Request::ActivateApp(TERMINAL.to_owned()))),
        "{:?}",
        run.requests()
    );
    let editor = run.bounds(bar, Role::Button, "Editor");
    run.click(bar, centre(editor));
    let spawned = run.record.borrow().spawned.clone();
    assert_eq!(spawned.len(), 1, "{spawned:?}");
    assert_eq!(spawned[0].0, ["editor"]);
    let token = spawned[0].1.as_deref().expect("an activation token");
    assert!(token.starts_with("headless-activation-"), "{token}");
    let nodes = run.nodes(bar);
    let editor = nodes.iter().find(|n| n.1 == "Editor").unwrap();
    assert_eq!(editor.2, kit::OPENING, "the launching tile says so");
}

#[test]
fn dragging_a_pinned_tile_off_the_bar_removes_it_and_stores_the_pins() {
    let mut run = Run::start();
    let bar = run.bar();
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.pointer(bar, PointerEvent::Enter { position: editor });
    run.button(bar, editor, MouseButton::Left, true);
    for step in 1..=6 {
        let p = Point::new(editor.x, editor.y - 40.0 * f64::from(step));
        run.pointer(bar, PointerEvent::Motion { position: p });
        run.advance(0.05);
    }
    run.advance(0.6);
    let off = Point::new(editor.x, editor.y - 240.0);
    run.pointer(bar, PointerEvent::Motion { position: off });
    run.button(bar, off, MouseButton::Left, false);
    let pins = run.record.borrow().pins.clone();
    assert_eq!(
        pins.last().cloned(),
        Some(vec![TERMINAL.to_owned()]),
        "{pins:?}"
    );
    run.advance(1.0);
    let names: Vec<String> = run
        .nodes(bar)
        .into_iter()
        .filter(|n| n.0 == Role::Button)
        .map(|n| n.1)
        .collect();
    assert_eq!(names, ["Terminal", kit::TRASH_NAME]);
}

#[test]
fn a_secondary_click_opens_the_tiles_menu_whose_items_reach_the_server() {
    let mut run = Run::start();
    let bar = run.bar();
    let terminal = centre(run.bounds(bar, Role::Button, "Terminal"));
    run.pointer(bar, PointerEvent::Enter { position: terminal });
    run.button(bar, terminal, MouseButton::Right, true);
    run.button(bar, terminal, MouseButton::Right, false);
    let menu = run
        .driver
        .runtime()
        .windows()
        .into_iter()
        .map(|(_, id)| id)
        .find(|&id| {
            id != bar
                && run
                    .driver
                    .runtime()
                    .snapshot(id)
                    .is_some_and(|s| s.accessibility.iter().any(|(_, n)| n.role == Role::Menu))
        })
        .expect("the menu opened");
    let items: Vec<String> = run
        .nodes(menu)
        .into_iter()
        .filter(|n| n.0 == Role::MenuItem)
        .map(|n| n.1)
        .collect();
    assert!(items.iter().any(|i| i == "Quit"), "{items:?}");
    let quit = run.bounds(menu, Role::MenuItem, "Quit");
    run.click(menu, centre(quit));
    assert!(
        run.requests()
            .contains(&(1, Request::App(TERMINAL.to_owned(), kit::AppAction::Quit))),
        "{:?}",
        run.requests()
    );
}

#[test]
fn the_focus_dock_shortcut_gives_the_bar_the_keyboard_and_escape_gives_it_back() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 2,
        events: vec![DockEvent::Engage],
    });
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), true)),
        "the bar takes the keyboard, whichever role was told"
    );
    let bar = run.bar();
    run.send_to(bar, WindowEvent::Focus(true));
    assert_eq!(
        run.focused(bar).as_deref(),
        Some("Editor"),
        "the first tile has the focus"
    );
    run.key(bar, RIGHT);
    assert_eq!(run.focused(bar).as_deref(), Some("Terminal"));
    run.key(bar, RIGHT);
    run.key(bar, LEFT);
    run.key(bar, RETURN);
    assert!(run
        .requests()
        .contains(&(1, Request::ActivateApp(TERMINAL.to_owned()))));
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), false)),
        "activating gives the keyboard back"
    );
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Engage],
    });
    run.key(bar, ESCAPE);
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), false))
    );
}

/// The grid and the Force Quit dialog hold the keyboard while they are up: a Focus Dock meanwhile is dropped, so the
/// bar does not take the keyboard from the dialog, nor from the window the grid launched when it closes.
#[test]
fn the_focus_dock_shortcut_is_dropped_while_the_grid_or_the_dialog_has_the_keyboard() {
    let mut run = Run::start();
    let bar_took = |run: &Run| {
        run.record
            .borrow()
            .keyboard
            .iter()
            .any(|&(part, on)| part == Part::Bar(1) && on)
    };
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ToggleAppGrid],
    });
    let grid = run.window(&format!("{GRID_APP_ID}@1")).expect("the grid");
    run.message(Message::Sized(Sized::Grid, 1280.0, 800.0));
    run.advance(1.0);
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Engage],
    });
    run.send_to(grid, WindowEvent::Focus(true));
    run.key(grid, ESCAPE);
    run.advance(1.0);
    assert!(run.window(&format!("{GRID_APP_ID}@1")).is_none());
    assert!(!bar_took(&run), "{:?}", run.record.borrow().keyboard);

    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ForceQuitDialog],
    });
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Engage],
    });
    run.advance(1.0);
    assert!(!bar_took(&run), "{:?}", run.record.borrow().keyboard);
}

/// The minimize target goes on the bar's role, which the host has once the bar is made and sized: a target worked out
/// before that would be lost, so it is sent when the bar is sized.
#[test]
fn the_minimize_target_is_sent_once_the_bar_is_sized() {
    let mut run = Run::start();
    let before = run.requests().len();
    // the size the server gives the bar is the one it asked for: the output's width and the bar's extent
    let extent = run
        .record
        .borrow()
        .placements
        .last()
        .expect("placed")
        .extent;
    run.message(Message::Sized(Sized::Bar(1), OUTPUTS[0].width, extent));
    let after: Vec<(u32, Request)> = run.requests()[before..].to_vec();
    assert!(
        after
            .iter()
            .any(|(output, r)| *output == 1 && matches!(r, Request::MinimizeTarget(_))),
        "{after:?}"
    );
}

/// An auto-hidden bar shows while Focus Dock has given it the keyboard, and hides again once it gives it back.
#[test]
fn the_focus_dock_shortcut_shows_an_auto_hidden_bar_until_escape() {
    let mut run = Run::start();
    let editor_y = |run: &mut Run| {
        let bar = run.bar();
        run.bounds(bar, Role::Button, "Editor").y()
    };
    let shown = editor_y(&mut run);
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ToggleAutoHide],
    });
    for _ in 0..4 {
        run.advance(0.5);
    }
    let hidden = editor_y(&mut run);
    assert!(hidden > shown, "the bar slid out: {hidden} > {shown}");
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Engage],
    });
    for _ in 0..4 {
        run.advance(0.5);
    }
    assert_eq!(editor_y(&mut run), shown, "the engaged bar is shown");
    let bar = run.bar();
    run.send_to(bar, WindowEvent::Focus(true));
    run.key(bar, ESCAPE);
    for _ in 0..4 {
        run.advance(0.5);
    }
    assert_eq!(editor_y(&mut run), hidden, "and hides once it lets go");
}

#[test]
fn resting_the_pointer_on_another_displays_edge_moves_the_dock_there() {
    let mut run = Run::start();
    let edge = run
        .window(&format!("{EDGE_APP_ID}@2"))
        .expect("an edge strip");
    run.pointer(
        edge,
        PointerEvent::Enter {
            position: Point::new(500.0, 0.5),
        },
    );
    run.advance(0.1);
    run.pointer(edge, PointerEvent::Leave);
    run.advance(kit::EDGE_DWELL);
    assert_eq!(
        run.record.borrow().placements.last().unwrap().output,
        1,
        "a pass is not a rest"
    );
    let edge = run
        .window(&format!("{EDGE_APP_ID}@2"))
        .expect("an edge strip");
    run.pointer(
        edge,
        PointerEvent::Enter {
            position: Point::new(500.0, 0.5),
        },
    );
    run.advance(kit::EDGE_DWELL + 0.05);
    assert_eq!(run.record.borrow().placements.last().unwrap().output, 2);
    assert!(
        run.window(&format!("{BAR_APP_ID}@2")).is_some(),
        "the bar opened on the second output"
    );
    assert!(run.window(&format!("{BAR_APP_ID}@1")).is_none());
    assert!(
        run.window(&format!("{EDGE_APP_ID}@1")).is_some(),
        "the first output now has the strip"
    );
}

#[test]
fn the_window_feed_comes_from_the_bars_role_and_a_trigger_from_any() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 2,
        events: vec![
            DockEvent::Window {
                id: 9,
                app_id: EDITOR.to_owned(),
                title: "notes".to_owned(),
                minimized: false,
            },
            DockEvent::Trigger(Trigger::SwipeUp),
        ],
    });
    let bar = run.bar();
    let editor = run
        .nodes(bar)
        .into_iter()
        .find(|n| n.1 == "Editor")
        .unwrap();
    assert_eq!(editor.2, "", "the edge role's copy of the feed is ignored");
    assert!(
        run.requests()
            .contains(&(2, Request::RunAction("overview".to_owned()))),
        "the gesture on the second output runs there: {:?}",
        run.requests()
    );
}

/// The bar's node asks for the menu material under the bar's own rectangle, not under the whole window (which is
/// deeper and as wide as the output); under Reduce Transparency it asks for none and paints the opaque fill.
#[test]
fn with_reduce_transparency_the_bar_asks_for_no_material() {
    for (reduce, material) in [(false, true), (true, false)] {
        let mut run = Run::with(tokens(reduce), ActionMap::default());
        let bar = run.bar();
        let width = run.driver.window_system_mut().spec(bar).unwrap().size.width;
        let requests = run.driver.window_system_mut().requests(bar).unwrap();
        assert!(
            !requests
                .iter()
                .any(|r| matches!(r, WindowRequest::Backdrop(Some(_)))),
            "no material under the whole window"
        );
        let regions = requests
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Materials(m) => Some(m.clone()),
                _ => None,
            })
            .unwrap_or_default();
        assert_eq!(
            !regions.is_empty(),
            material,
            "reduce transparency {reduce}"
        );
        if material {
            assert_eq!(regions.len(), 1, "{regions:?}");
            assert_eq!(regions[0].material, "Menu");
            assert!(
                regions[0].rect.width() < width,
                "the material lies under the bar's rectangle only: {regions:?}"
            );
        }
    }
}

#[test]
fn the_force_quit_dialog_is_driven_from_the_keyboard() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ForceQuitDialog],
    });
    let dialog = run
        .window(&format!("{FORCE_QUIT_APP_ID}@1"))
        .expect("the dialog opened on the dock's output");
    let nodes = run.nodes(dialog);
    assert!(nodes.iter().any(|n| n.1 == "Terminal"), "{nodes:?}");
    run.send_to(dialog, WindowEvent::Focus(true));
    run.key(dialog, DOWN);
    run.key(dialog, RETURN);
    assert!(
        run.requests()
            .iter()
            .all(|(_, r)| !matches!(r, Request::ForceQuitApp(_))),
        "Return asks first"
    );
    let dialog = run
        .window(&format!("{FORCE_QUIT_APP_ID}@1"))
        .expect("the question is shown");
    for _ in 0..3 {
        if run.focused(dialog).as_deref() == Some("Force Quit") {
            break;
        }
        run.key(dialog, TAB);
    }
    assert_eq!(
        run.focused(dialog).as_deref(),
        Some("Force Quit"),
        "Tab reaches the button"
    );
    run.key(dialog, SPACE);
    assert!(
        run.requests()
            .iter()
            .any(|(_, r)| matches!(r, Request::ForceQuitApp(a) if a == TERMINAL)),
        "{:?}",
        run.requests()
    );
}

#[test]
fn locking_the_session_closes_the_menu_and_the_dialog_and_hides_the_strips() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ForceQuitDialog],
    });
    assert!(run.window(&format!("{FORCE_QUIT_APP_ID}@1")).is_some());
    run.message(Message::Locked(true));
    run.advance(1.0);
    assert!(run.window(&format!("{FORCE_QUIT_APP_ID}@1")).is_none());
    assert!(run.window(&format!("{EDGE_APP_ID}@2")).is_none());
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Engage],
    });
    assert!(
        !run.record.borrow().keyboard.iter().any(|&(_, on)| on),
        "the locked dock never takes the keyboard"
    );
}

#[test]
fn the_application_grid_takes_the_keyboard_filters_as_one_types_and_launches_with_return() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ToggleAppGrid],
    });
    let grid = run
        .window(&format!("{GRID_APP_ID}@1"))
        .expect("the grid opened on the dock's output");
    run.message(Message::Sized(Sized::Grid, 1280.0, 800.0));
    run.advance(1.0);
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Grid, true))
    );
    let names: Vec<String> = run
        .nodes(grid)
        .into_iter()
        .filter(|n| n.0 == Role::Button)
        .map(|n| n.1)
        .collect();
    assert!(
        names.contains(&"Editor".to_owned()) && names.contains(&"Terminal".to_owned()),
        "every application is a named item: {names:?}"
    );
    run.send_to(grid, WindowEvent::Focus(true));
    run.type_text(grid, "edi");
    run.key(grid, RETURN);
    let spawned = run.record.borrow().spawned.clone();
    assert_eq!(spawned.len(), 1, "{spawned:?}");
    assert_eq!(spawned[0].0, ["editor"]);
    assert!(spawned[0].1.is_some(), "with an activation token");
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Grid, false)),
        "the grid closes and gives the keyboard back"
    );
}

// ---- the bar drawn through Kit ---------------------------------------------------------------------------------

/// The server's Focus Dock, on the role of `output`.
fn engage(run: &mut Run, output: u32) {
    run.message(Message::Dock {
        output,
        events: vec![DockEvent::Engage],
    });
}

/// The rectangles of the bar's running indicators (its square marks), in window points.
fn indicators(run: &Run, bar: WindowId) -> Vec<Rect> {
    let snapshot = run.driver.runtime().snapshot(bar).unwrap();
    snapshot
        .nodes
        .iter()
        .filter(|n| matches!(&n.key, Some(Key::Str(k)) if k.starts_with("mark-")))
        .map(|n| n.rect)
        .filter(|r| r.width() == r.height())
        .collect()
}

/// How many paint operations the tile called `name` has: its icon, and its focus ring when that shows.
fn tile_ops(run: &Run, bar: WindowId, name: &str) -> usize {
    let tile = centre(run.bounds(bar, Role::Button, name));
    let snapshot = run.driver.runtime().snapshot(bar).unwrap();
    snapshot
        .display_list
        .nodes
        .iter()
        .filter(|n| n.parent.is_some() && n.hit.width() > 0.0)
        .find(|n| (centre(n.hit).x - tile.x).abs() < 0.5 && (centre(n.hit).y - tile.y).abs() < 0.5)
        .unwrap_or_else(|| panic!("no paint node for {name}"))
        .ops
        .len()
}

/// Whether `window` asks for its next frame. [`Run::settle`] can end on a frame just handed out, so the runtime takes
/// it first.
fn asks_for_frames(run: &mut Run, window: WindowId) -> bool {
    run.driver.run_until_idle(budget()).unwrap();
    run.driver
        .window_system_mut()
        .frame_requested(window)
        .unwrap()
}

/// The open context menu: a window of the runtime other than the bar with a menu in it.
fn open_menu_window(run: &Run, bar: WindowId) -> Option<WindowId> {
    run.driver
        .runtime()
        .windows()
        .into_iter()
        .map(|(_, id)| id)
        .find(|&id| {
            id != bar
                && run
                    .driver
                    .runtime()
                    .snapshot(id)
                    .is_some_and(|s| s.accessibility.iter().any(|(_, n)| n.role == Role::Menu))
        })
}

#[test]
fn a_running_application_has_an_indicator_under_its_tile() {
    let mut run = Run::start();
    let bar = run.bar();
    let under = |run: &Run, mark: Rect, name: &str| {
        let tile = run.bounds(bar, Role::Button, name);
        (centre(mark).x - centre(tile).x).abs() < 0.5 && mark.y() >= tile.y() + tile.height()
    };
    let marks = indicators(&run, bar);
    assert_eq!(marks.len(), 1, "the terminal runs: {marks:?}");
    assert!(
        under(&run, marks[0], "Terminal"),
        "on the screen-edge side of its tile"
    );

    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 8,
            app_id: EDITOR.to_owned(),
            title: "notes".to_owned(),
            minimized: false,
        }],
    });
    run.advance(1.0);
    let marks = indicators(&run, bar);
    assert_eq!(marks.len(), 2, "{marks:?}");
    assert!(marks.iter().any(|&m| under(&run, m, "Editor")));

    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(8)],
    });
    run.advance(1.0);
    let marks = indicators(&run, bar);
    assert_eq!(marks.len(), 1, "the editor stopped: {marks:?}");
    assert!(under(&run, marks[0], "Terminal"));
}

/// A bounce runs on the bar's frame callbacks: the tile rises while it lasts, the model is never woken more often than
/// its slow fallback, and the tile rests again once it ends, with no frame asked for.
#[test]
fn an_attention_bounce_moves_the_tile_on_frames_and_brings_it_back_to_rest() {
    let mut run = Run::start();
    let bar = run.bar();
    let y = |run: &Run| run.bounds(bar, Role::Button, "Terminal").y();
    let rest = y(&run);
    assert!(!asks_for_frames(&mut run, bar));
    let woken = run.record.borrow().wakes.len();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Attention(TERMINAL.to_owned())],
    });
    assert!(
        asks_for_frames(&mut run, bar),
        "the bounce asks for the bar's next frame"
    );
    let mut highest = rest;
    for _ in 0..12 {
        run.advance(0.05);
        highest = highest.min(y(&run));
    }
    assert!(highest < rest - 4.0, "the tile rose: {highest} < {rest}");
    let wakes = run.record.borrow().wakes[woken..].to_vec();
    assert!(
        wakes
            .iter()
            .all(|&after| after >= kit::FRAME_FALLBACK - 1e-9),
        "no timer faster than the fallback while the frames drive it: {wakes:?}"
    );
    for _ in 0..4 {
        run.advance(1.0);
    }
    assert_eq!(y(&run), rest, "back at rest");
    assert!(!asks_for_frames(&mut run, bar), "and asks for no frame");
}

/// Launching from the bar bounces the tile until the application's window comes.
#[test]
fn a_launch_bounces_its_tile_until_the_window_comes() {
    let mut run = Run::start();
    let bar = run.bar();
    let y = |run: &Run| run.bounds(bar, Role::Button, "Editor").y();
    let rest = y(&run);
    run.click(bar, centre(run.bounds(bar, Role::Button, "Editor")));
    let mut highest = rest;
    for _ in 0..12 {
        run.advance(0.05);
        highest = highest.min(y(&run));
    }
    assert!(highest < rest - 4.0, "the tile rose: {highest} < {rest}");
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 8,
            app_id: EDITOR.to_owned(),
            title: "notes".to_owned(),
            minimized: false,
        }],
    });
    for _ in 0..4 {
        run.advance(1.0);
    }
    assert_eq!(y(&run), rest, "the window came: the tile rests");
}

/// The tile's menu is a grabbing popup of the bar anchored to the tile itself, below it, so the server flips it above
/// the bar at the bottom of the output.
#[test]
fn the_tiles_menu_is_a_popup_anchored_to_its_tile() {
    let mut run = Run::start();
    let bar = run.bar();
    let terminal = run.bounds(bar, Role::Button, "Terminal");
    run.pointer(
        bar,
        PointerEvent::Enter {
            position: centre(terminal),
        },
    );
    run.button(bar, centre(terminal), MouseButton::Right, true);
    run.button(bar, centre(terminal), MouseButton::Right, false);
    let menu = open_menu_window(&run, bar).expect("the menu opened");
    let system = run.driver.window_system_mut();
    assert_eq!(system.parent(menu).unwrap(), Some(bar));
    let spec = system.placement(menu).unwrap().expect("placed").clone();
    assert!(
        spec.grab,
        "the menu takes the keyboard and closes on a press outside"
    );
    assert!(
        (centre(spec.anchor_rect).x - centre(terminal).x).abs() < 0.5,
        "centred on the tile: {:?} for {terminal:?}",
        spec.anchor_rect
    );
    assert!(
        spec.anchor_rect.y() >= terminal.y() - 0.5
            && spec.anchor_rect.y() + spec.anchor_rect.height()
                <= terminal.y() + terminal.height() + 0.5,
        "{:?} for {terminal:?}",
        spec.anchor_rect
    );
}

/// Every Focus Dock starts on the first tile and shows the ring at once, whatever was focused when the bar last gave
/// the keyboard back.
#[test]
fn every_focus_dock_starts_on_the_first_tile_with_the_ring_shown() {
    let mut run = Run::start();
    let bar = run.bar();
    run.send_to(bar, WindowEvent::Focus(true));
    for _ in 0..2 {
        engage(&mut run, 1);
        assert_eq!(run.focused(bar).as_deref(), Some("Editor"));
        assert!(
            tile_ops(&run, bar, "Editor") > tile_ops(&run, bar, "Terminal"),
            "the focused tile draws its ring"
        );
        run.key(bar, RIGHT);
        assert_eq!(run.focused(bar).as_deref(), Some("Terminal"));
        run.key(bar, ESCAPE);
        assert_eq!(
            run.record.borrow().keyboard.last().copied(),
            Some((Part::Bar(1), false))
        );
    }
}

/// The focused tile going away hands the focus to a tile beside it, never to the bar itself.
#[test]
fn the_focus_moves_to_a_neighbour_when_its_tile_goes() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 9,
            app_id: "org.example.Viewer".to_owned(),
            title: "picture".to_owned(),
            minimized: false,
        }],
    });
    run.advance(1.0);
    let bar = run.bar();
    let viewer = run
        .nodes(bar)
        .into_iter()
        .filter(|n| n.0 == Role::Button)
        .map(|n| n.1)
        .find(|n| n != "Editor" && n != "Terminal" && n != kit::TRASH_NAME)
        .expect("the viewer's tile");
    run.send_to(bar, WindowEvent::Focus(true));
    engage(&mut run, 1);
    for _ in 0..4 {
        if run.focused(bar).as_deref() == Some(viewer.as_str()) {
            break;
        }
        run.key(bar, RIGHT);
    }
    assert_eq!(run.focused(bar), Some(viewer.clone()));
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(9)],
    });
    run.advance(1.0);
    assert!(
        !run.nodes(bar).iter().any(|n| n.1 == viewer),
        "the tile went"
    );
    let focused = run.focused(bar).expect("a tile keeps the focus");
    assert!(
        [kit::TRASH_NAME, "Terminal"].contains(&focused.as_str()),
        "a neighbour: {focused}"
    );
    assert!(tile_ops(&run, bar, &focused) > tile_ops(&run, bar, "Editor"));
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), true)),
        "still engaged"
    );
}

/// The menu taking the keyboard keeps Focus Dock; the bar losing the keyboard otherwise, a menu open or not (a press
/// outside that closes it), ends it.
#[test]
fn losing_the_keyboard_ends_focus_dock_except_to_its_own_menu() {
    const MENU: u32 = 0xff67;
    let mut run = Run::start();
    let bar = run.bar();
    run.send_to(bar, WindowEvent::Focus(true));
    engage(&mut run, 1);
    run.key(bar, MENU);
    assert!(open_menu_window(&run, bar).is_some(), "the menu opened");
    run.message(Message::KeyboardLeft(1));
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), true)),
        "the menu took the keyboard: still engaged"
    );
    // the server gives the keyboard to a window: the bar is told it left once more, the menu still open
    run.message(Message::KeyboardLeft(1));
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), false))
    );

    engage(&mut run, 1);
    run.message(Message::KeyboardLeft(1));
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Bar(1), false)),
        "without a menu, any leave ends it"
    );
}

/// With magnification off, the pointer moving over one tile changes nothing the bar draws, so the bar presents no new
/// frame for it.
#[test]
fn pointer_motion_over_a_tile_presents_no_new_frame() {
    let mut run = Run::start();
    let bar = run.bar();
    let terminal = centre(run.bounds(bar, Role::Button, "Terminal"));
    run.pointer(bar, PointerEvent::Enter { position: terminal });
    run.advance(1.0);
    let before = run.driver.window_system_mut().frames(bar).unwrap().len();
    for step in 0..12 {
        let dx = f64::from(step % 5) - 2.0;
        let position = Point::new(terminal.x + dx, terminal.y + dx / 2.0);
        run.pointer(bar, PointerEvent::Motion { position });
    }
    let after = run.driver.window_system_mut().frames(bar).unwrap().len();
    assert_eq!(after, before, "no repaint for motion that changes nothing");
}

// ---- the switcher and the overview through the model -----------------------------------------------------------

/// The switch's panel opens on the output the server names, keyed and named for it, lists the applications by their
/// desktop names, and a click on one asks the server to switch to it.
#[test]
fn the_switcher_opens_on_the_named_output_and_a_click_switches() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 2,
        events: vec![DockEvent::Switcher {
            output: 2,
            apps: vec![
                TERMINAL.to_owned(),
                EDITOR.to_owned(),
                "org.example.Unknown".to_owned(),
            ],
            selected: 1,
        }],
    });
    for _ in 0..4 {
        run.advance(0.1);
    }
    assert!(run.window(&format!("{SWITCHER_APP_ID}@1")).is_none());
    let panel = run
        .window(&format!("{SWITCHER_APP_ID}@2"))
        .expect("the panel on the second output");
    let names: Vec<String> = run
        .nodes(panel)
        .into_iter()
        .filter(|n| n.0 == Role::Button || n.0 == Role::ListItem || n.0 == Role::Tab)
        .map(|n| n.1)
        .collect();
    assert!(
        names.iter().any(|n| n == "Terminal") && names.iter().any(|n| n == "Editor"),
        "{names:?}"
    );
    assert!(
        names.iter().any(|n| n == "org.example.Unknown"),
        "an application without a desktop entry goes by its id: {names:?}"
    );
    let editor = run
        .nodes(panel)
        .into_iter()
        .find(|n| n.1 == "Editor")
        .unwrap()
        .3;
    run.click(panel, centre(editor));
    assert!(
        run.requests()
            .iter()
            .any(|(o, r)| *o == 2 && matches!(r, Request::SwitcherActivate(1))),
        "{:?}",
        run.requests()
    );
    // every role is told the switch ended; the dock takes it from the bar's
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::SwitcherClosed { committed: true }],
    });
    for _ in 0..10 {
        run.advance(0.1);
    }
    assert!(
        run.window(&format!("{SWITCHER_APP_ID}@2")).is_none(),
        "the panel faded out"
    );
}

/// The overview's chrome opens on its output, takes the keyboard while it is up, lays out the three windows and tells
/// the server where each goes; Escape asks to close it, and once the server closes it the keyboard goes back.
#[test]
fn the_overview_takes_the_keyboard_and_sends_the_window_frames() {
    let mut run = Run::start();
    for (id, title) in [(7, "shell"), (8, "notes"), (9, "todo")] {
        let app_id = if id == 7 { TERMINAL } else { EDITOR };
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::Window {
                id,
                app_id: app_id.to_owned(),
                title: title.to_owned(),
                minimized: false,
            }],
        });
    }
    let before = run.requests().len();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Overview {
            mode: Mode::All,
            area: WmRect::new(0.0, 0.0, 1280.0, 800.0),
            windows: vec![
                (7, WmRect::new(40.0, 60.0, 600.0, 400.0)),
                (8, WmRect::new(300.0, 100.0, 700.0, 500.0)),
                (9, WmRect::new(500.0, 200.0, 500.0, 400.0)),
            ],
            workspaces: vec![
                Workspace {
                    id: 101,
                    current: true,
                },
                Workspace {
                    id: 102,
                    current: false,
                },
            ],
        }],
    });
    run.advance(0.1);
    let chrome = run
        .window(&format!("{OVERVIEW_APP_ID}@1"))
        .expect("the overview's chrome");
    assert!(
        run.record
            .borrow()
            .keyboard
            .contains(&(Part::Overview(1), true)),
        "{:?}",
        run.record.borrow().keyboard
    );
    for _ in 0..10 {
        run.advance(0.1);
    }
    let framed: std::collections::BTreeSet<u32> = run.requests()[before..]
        .iter()
        .filter_map(|(o, r)| match r {
            Request::OverviewFrame(id, _) if *o == 1 => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(
        framed,
        [7, 8, 9].into(),
        "every window is told where it goes"
    );
    run.send_to(chrome, WindowEvent::Focus(true));
    run.key(chrome, ESCAPE);
    assert!(
        run.requests()
            .iter()
            .any(|(o, r)| *o == 1 && *r == Request::OverviewClose),
        "{:?}",
        run.requests()
    );
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::OverviewClosed],
    });
    for _ in 0..10 {
        run.advance(0.1);
    }
    assert!(run.window(&format!("{OVERVIEW_APP_ID}@1")).is_none());
    assert_eq!(
        run.record.borrow().keyboard.last().copied(),
        Some((Part::Overview(1), false))
    );
}

/// With `CUPERTINO_LOOK_DIR` set, pictures of the dock as its model drives it, light and dark at 1x and 2x: the bar with
/// a running application, a tile's menu, the switcher panel and the overview's chrome over three windows. The desktop
/// offers no materials here, so the windows show their opaque fills where the compositor would draw a material.
#[test]
fn look_captures() {
    let Some(dir) = std::env::var_os("CUPERTINO_LOOK_DIR") else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let looks = [Appearance::Light, Appearance::Dark];
    for scale in [1.0, 2.0] {
        let mut run = Run::on(tokens(false), ActionMap::default(), false);
        let bar = run.bar();
        run.driver
            .look_into(&dir, bar, "dock-bar", scale, &looks)
            .unwrap();
        let terminal = centre(run.bounds(bar, Role::Button, "Terminal"));
        run.pointer(bar, PointerEvent::Enter { position: terminal });
        run.button(bar, terminal, MouseButton::Right, true);
        run.button(bar, terminal, MouseButton::Right, false);
        let menu = open_menu_window(&run, bar).expect("the menu opened");
        // the compositor gives a popup the size it was placed at
        let system = run.driver.window_system_mut();
        let size = system.placement(menu).unwrap().expect("placed").size;
        system
            .configure(menu, Some(size), WindowStates::default())
            .unwrap();
        run.settle();
        run.driver
            .look_into(&dir, menu, "dock-menu", scale, &looks)
            .unwrap();
        run.key(menu, ESCAPE);

        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::Switcher {
                output: 1,
                apps: vec![
                    TERMINAL.to_owned(),
                    EDITOR.to_owned(),
                    "org.example.Viewer".to_owned(),
                ],
                selected: 1,
            }],
        });
        for _ in 0..6 {
            run.advance(0.1);
        }
        let panel = run
            .window(&format!("{SWITCHER_APP_ID}@1"))
            .expect("the switcher");
        run.driver
            .look_into(&dir, panel, "dock-switcher", scale, &looks)
            .unwrap();
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::SwitcherClosed { committed: false }],
        });

        for (id, title) in [(7, "shell"), (8, "Meeting Notes"), (9, "Plan")] {
            let app_id = if id == 7 { TERMINAL } else { EDITOR };
            run.message(Message::Dock {
                output: 1,
                events: vec![DockEvent::Window {
                    id,
                    app_id: app_id.to_owned(),
                    title: title.to_owned(),
                    minimized: false,
                }],
            });
        }
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::Overview {
                mode: Mode::All,
                area: WmRect::new(0.0, 0.0, 1280.0, 800.0),
                windows: vec![
                    (7, WmRect::new(40.0, 60.0, 600.0, 400.0)),
                    (8, WmRect::new(300.0, 100.0, 700.0, 500.0)),
                    (9, WmRect::new(500.0, 200.0, 500.0, 400.0)),
                ],
                workspaces: vec![
                    Workspace {
                        id: 101,
                        current: true,
                    },
                    Workspace {
                        id: 102,
                        current: false,
                    },
                ],
            }],
        });
        for _ in 0..12 {
            run.advance(0.1);
        }
        let chrome = run
            .window(&format!("{OVERVIEW_APP_ID}@1"))
            .expect("the overview");
        run.driver
            .look_into(&dir, chrome, "dock-overview-three-windows", scale, &looks)
            .unwrap();
    }
}
