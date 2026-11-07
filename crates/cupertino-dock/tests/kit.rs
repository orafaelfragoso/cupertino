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
    SwitcherPaint,
};
use cupertino_dock::overview_ui::OverviewUi;
use cupertino_dock::session::Session;
use cupertino_dock::view::{IconSet, BAR_APP_ID, EDGE_APP_ID, FORCE_QUIT_APP_ID, GRID_APP_ID};
use cupertino_dock::{Dock, DockConfig};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    next_resource_id, Application, Command, PlaceholderIcons, Role, RuntimeOptions, Window,
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
    requests: Vec<(u32, Request)>,
    placements: Vec<Placement>,
    keyboard: Vec<(Part, bool)>,
    spawned: Vec<(Vec<String>, Option<String>)>,
    pins: Vec<Vec<String>>,
    shell: Vec<String>,
}

struct FakeHost(Rc<RefCell<Record>>);

/// A solid square `px` device pixels large.
fn square(px: u32) -> ImageSource {
    let pixels: Vec<u8> = [40u8, 90, 200, 255]
        .iter()
        .copied()
        .cycle()
        .take((px * px * 4) as usize)
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
    fn wake(&mut self, _after: f64, generation: u64) -> Vec<Command<Message>> {
        self.0.borrow_mut().wake = Some(generation);
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
    fn paint_switcher(&mut self, _: f64, _: &SwitcherPaint<'_>) -> Option<ImageSource> {
        Some(square(8))
    }
    fn paint_overview(
        &mut self,
        _: f64,
        _: (f64, f64),
        _: &OverviewUi,
        _: f64,
        _: bool,
    ) -> Option<ImageSource> {
        Some(square(8))
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
            app_effects: true,
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

#[test]
fn with_reduce_transparency_the_bar_asks_for_no_material() {
    for (reduce, material) in [(false, true), (true, false)] {
        let mut run = Run::with(tokens(reduce), ActionMap::default());
        let bar = run.bar();
        let backdrop = run
            .driver
            .window_system_mut()
            .requests(bar)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                WindowRequest::Backdrop(b) => Some(b.clone()),
                _ => None,
            })
            .flatten();
        assert_eq!(backdrop.is_some(), material, "reduce transparency {reduce}");
        if let Some(b) = backdrop {
            assert!(
                b.bounds.is_some(),
                "the material lies under the bar's rectangle only"
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
