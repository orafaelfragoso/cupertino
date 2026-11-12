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
    IconSet, BAR_APP_ID, EDGE_APP_ID, FORCE_QUIT_APP_ID, OVERVIEW_APP_ID, SWITCHER_APP_ID,
};
use cupertino_dock::{Dock, DockConfig};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    next_resource_id, Action, Application, Command, Key, NodeId, PlaceholderIcons, Role,
    RuntimeOptions, Window,
};
use cupertino_kit::geometry::{Point, Rect};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    semantics, Capabilities, KeyEvent, Keysym, MaterialRegion, Mods, MouseButton, PointerEvent,
    Timestamp, WindowEvent, WindowId, WindowRequest, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer, ImageResource, PixelFormat};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{
    semantic_outline, unannounced, whole_tree, AssistiveTechnology, ManualClock, TestDriver,
};
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
    /// What the next read of `dock.conf` finds.
    conf: Option<DockSettings>,
    /// The desktop file IDs the launcher's history lists, the most recent first.
    recent: Vec<String>,
    /// The `org.freedesktop.Application.Activate` calls: the bus name and the activation token.
    activated: Vec<(String, Option<String>)>,
    /// The application does not answer on the bus: an activation over it fails.
    no_bus: bool,
    /// The icons loaded, with the side asked for, in order.
    icon_loads: Vec<(IconKey, f64)>,
    /// The host loads icons in the background ([`Host::preload`]).
    background: bool,
    /// The icons asked for in the background, a list a load.
    preloads: Vec<Vec<(IconKey, f64)>>,
    /// The icons a background load leaves out, for the model to load itself.
    leave_out: Vec<IconKey>,
    /// The ends of the programs started, in order: a send makes it exit, failed or not.
    exits: Vec<async_channel::Sender<bool>>,
    /// The folders shown as stacks, with their icons, and the entries every folder lists.
    folders: Vec<(String, String)>,
    folder_entries: Vec<cupertino_dock::stack::StackEntry>,
    /// Listings wait for a send here before they end, as a slow folder does; otherwise they end at once.
    hold_listings: bool,
    listings: Vec<async_channel::Sender<()>>,
    /// What was opened with the session's opener, in order.
    opened: Vec<String>,
}

thread_local! {
    /// The next run's host loads icons in the background, leaving these out.
    static BACKGROUND: RefCell<Option<Vec<IconKey>>> = const { RefCell::new(None) };
    /// The next run's host shows these folders as stacks.
    static FOLDERS: RefCell<Vec<(String, String)>> = const { RefCell::new(Vec::new()) };
    /// The next run's windows are read by this assistive technology.
    static READER: RefCell<Option<AssistiveTechnology>> = const { RefCell::new(None) };
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
    fn spawn(&mut self, argv: Vec<String>, token: Option<String>) -> Option<kit::Exited> {
        let mut record = self.0.borrow_mut();
        record.spawned.push((argv, token));
        let (exit, exited) = async_channel::bounded::<bool>(1);
        record.exits.push(exit);
        Some(Box::pin(
            async move { exited.recv().await.unwrap_or(false) },
        ))
    }
    fn open(&mut self, target: &str) {
        self.0.borrow_mut().opened.push(target.to_owned());
    }
    fn folders(&mut self) -> Vec<(String, String)> {
        self.0.borrow().folders.clone()
    }
    fn folder_entries(&mut self, _: &str) -> kit::FolderListing {
        let mut record = self.0.borrow_mut();
        let entries = record.folder_entries.clone();
        if !record.hold_listings {
            return Box::pin(std::future::ready(entries));
        }
        let (done, listed) = async_channel::bounded::<()>(1);
        record.listings.push(done);
        Box::pin(async move {
            let _ = listed.recv().await;
            entries
        })
    }
    fn shell(&mut self, command: &str) {
        self.0.borrow_mut().shell.push(command.to_owned());
    }
    fn store_pins(&mut self, pins: &[String]) {
        self.0.borrow_mut().pins.push(pins.to_vec());
    }
    fn read_conf(&mut self, _: &DockSettings) -> Option<DockSettings> {
        self.0.borrow_mut().conf.take()
    }
    fn terminal(&mut self) -> Option<String> {
        None
    }
    fn recent_apps(&mut self) -> Vec<String> {
        self.0.borrow().recent.clone()
    }
    fn dbus_activate(&mut self, bus_name: &str, token: Option<String>) -> Option<kit::BusCall> {
        let mut record = self.0.borrow_mut();
        record.activated.push((bus_name.to_owned(), token));
        let ok = !record.no_bus;
        Some(Box::pin(async move { ok }))
    }
    fn icon(&mut self, _: &DesktopIndex, icon: &IconKey, side: f64) -> IconSet {
        self.0.borrow_mut().icon_loads.push((icon.clone(), side));
        [1.0, 2.0]
            .iter()
            .map(|scale| square((side * scale).ceil().max(1.0) as u32))
            .collect()
    }
    fn preload(&mut self, _: &DesktopIndex, icons: Vec<(IconKey, f64)>) -> Option<kit::IconLoad> {
        let mut record = self.0.borrow_mut();
        if !record.background {
            return None;
        }
        record.preloads.push(icons.clone());
        let loaded: Vec<(IconKey, u32, IconSet)> = icons
            .into_iter()
            .filter(|(key, _)| !record.leave_out.contains(key))
            .map(|(key, side)| {
                let set = [1.0, 2.0]
                    .iter()
                    .map(|scale| square((side * scale).ceil().max(1.0) as u32))
                    .collect();
                (key, side.ceil() as u32, set)
            })
            .collect();
        Some(Box::pin(std::future::ready(loaded)))
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

/// A desktop entry with more `key=value` lines.
fn entry_with(id: &str, name: &str, extra: &str) -> cupertino_dock::desktop::DesktopEntry {
    let exec = name.to_lowercase();
    parse_entry(
        id,
        Path::new("x"),
        &format!("[Desktop Entry]\nName={name}\nExec={exec} %U\n{extra}"),
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
        Self::on_desktop(theme, actions, app_effects, Vec::new())
    }

    /// [`Run::start`] with the viewer's desktop entry besides, not pinned.
    fn with_viewer() -> Self {
        Self::on_desktop(
            tokens(false),
            ActionMap::default(),
            true,
            vec![entry_with(VIEWER, "Viewer", "")],
        )
    }

    /// [`Run::on`] with `more` desktop entries besides the editor's and the terminal's.
    fn on_desktop(
        theme: ThemeTokens,
        actions: ActionMap,
        app_effects: bool,
        more: Vec<cupertino_dock::desktop::DesktopEntry>,
    ) -> Self {
        // window ids start again with a new runtime: none of them is configured yet
        CONFIGURED.with_borrow_mut(Vec::clear);
        let leave_out = BACKGROUND.take();
        let record = Rc::new(RefCell::new(Record {
            background: leave_out.is_some(),
            leave_out: leave_out.unwrap_or_default(),
            folders: FOLDERS.take(),
            ..Record::default()
        }));
        let mut dock = Dock::new(DockConfig::default());
        dock.set_pinned(&[EDITOR, TERMINAL]);
        let mut entries = vec![
            entry_with(EDITOR, "Editor", "Categories=Development;TextEditor;\n"),
            entry_with(
                TERMINAL,
                "Terminal",
                "Categories=System;TerminalEmulator;\nDBusActivatable=true\n",
            ),
        ];
        entries.extend(more);
        // the terminal's entry is D-Bus activatable, the editor's is not
        let desktop = DesktopIndex { entries };
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
            accessibility: READER.take().map(|reader| reader.adapter()),
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

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Dock");
    }

    /// Run until nothing is left to do, configuring every new window and drawing every requested frame.
    fn settle_frames(&mut self) {
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

    /// Move the host's clock on by `seconds`, and the toolkit's with it, and wake the model.
    fn advance(&mut self, seconds: f64) {
        let (generation, now) = {
            let mut record = self.record.borrow_mut();
            record.now += seconds;
            (record.wake.unwrap_or(0), record.now)
        };
        self.driver
            .advance_to(Timestamp((now * 1e9).round() as u64))
            .unwrap();
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

/// The names of the bar's tiles, in order.
fn tile_names(run: &Run, bar: WindowId) -> Vec<String> {
    run.nodes(bar)
        .into_iter()
        .filter(|n| n.0 == Role::Button)
        .map(|n| n.1)
        .collect()
}

/// The names of the application tiles, ahead of the trash: an application that quit is listed after it, among the
/// recent ones.
fn applications(run: &Run, bar: WindowId) -> Vec<String> {
    let names = tile_names(run, bar);
    let trash = names.iter().position(|n| n == kit::TRASH_NAME).unwrap();
    names[..trash].to_vec()
}

/// A secondary click on the tile named `tile`, then a click on its menu's item named `item`, and the flash it acts
/// after.
fn choose_on_tile(run: &mut Run, bar: WindowId, tile: &str, item: &str) {
    let at = centre(run.bounds(bar, Role::Button, tile));
    run.pointer(bar, PointerEvent::Enter { position: at });
    run.button(bar, at, MouseButton::Right, true);
    run.button(bar, at, MouseButton::Right, false);
    let menu = open_menu(run, bar).expect("the menu opened");
    let row = run.bounds(menu, Role::MenuItem, item);
    run.click(menu, centre(row));
    run.advance(MENU_FLASH);
    assert!(
        open_menu(run, bar).is_none(),
        "choosing {item} closes the menu"
    );
}

#[test]
fn remove_from_dock_unpins_at_once_and_a_running_tile_stays_until_its_application_quits() {
    let mut run = Run::start();
    let bar = run.bar();
    choose_on_tile(&mut run, bar, "Terminal", "Remove from Dock");
    assert_eq!(
        run.record.borrow().pins.last().cloned(),
        Some(vec![EDITOR.to_owned()]),
        "the pins are stored without the terminal"
    );
    run.advance(1.0);
    assert_eq!(
        tile_names(&run, bar),
        ["Editor", "Terminal", kit::TRASH_NAME],
        "the running terminal keeps its tile"
    );
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(7)],
    });
    run.advance(1.0);
    assert_eq!(
        applications(&run, bar),
        ["Editor"],
        "its tile goes as it quits"
    );
    choose_on_tile(&mut run, bar, "Editor", "Remove from Dock");
    assert_eq!(
        run.record.borrow().pins.last().cloned(),
        Some(Vec::new()),
        "the pins are stored without the editor"
    );
    run.advance(1.0);
    assert!(
        applications(&run, bar).is_empty(),
        "an application that is not running leaves at once"
    );
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
    // the chosen item flashes before it acts
    run.advance(MENU_FLASH);
    assert!(
        run.requests()
            .contains(&(1, Request::App(TERMINAL.to_owned(), kit::AppAction::Quit))),
        "{:?}",
        run.requests()
    );
}

/// Longer than a chosen menu item flashes before it acts.
const MENU_FLASH: f64 = 0.2;

/// The dock read by an assistive technology, before anything gave it an input event.
fn untouched() -> (Run, AssistiveTechnology) {
    let reader = AssistiveTechnology::new();
    READER.set(Some(reader.clone()));
    let mut run = Run::start();
    run.driver.window_system_mut().set_no_input_yet();
    (run, reader)
}

/// The node of `window` with `role` named `name`.
fn node(run: &Run, window: WindowId, role: Role, name: &str) -> NodeId {
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("no {role:?} named {name}"))
        .0
}

/// The open menu, if any.
fn open_menu(run: &Run, bar: WindowId) -> Option<WindowId> {
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

/// The reader shows the menu of the terminal's tile.
fn show_menu(run: &mut Run, reader: &AssistiveTechnology, bar: WindowId) -> WindowId {
    let tile = node(run, bar, Role::Button, "Terminal");
    run.driver.ask(reader, tile, Action::ShowMenu, None);
    run.settle();
    open_menu(run, bar).expect("the menu opened")
}

#[test]
fn a_menu_an_assistive_action_shows_before_any_input_opens_without_a_grab_and_its_items_act() {
    let (mut run, reader) = untouched();
    let bar = run.bar();
    let menu = show_menu(&mut run, &reader, bar);
    let placed = run.driver.window_system_mut().placement(menu).unwrap();
    assert_eq!(placed.map(|p| p.grab), Some(false));
    let quit = node(&run, menu, Role::MenuItem, "Quit");
    run.driver.ask(&reader, quit, Action::Click, None);
    run.settle();
    // the chosen item flashes before it acts
    run.advance(MENU_FLASH);
    assert!(
        run.requests()
            .contains(&(1, Request::App(TERMINAL.to_owned(), kit::AppAction::Quit))),
        "{:?}",
        run.requests()
    );
    assert_eq!(
        open_menu(&run, bar),
        None,
        "choosing an item closes the menu"
    );
}

/// The y of the terminal's tile on the bar: larger while an auto-hidden bar is out of sight.
fn terminal_y(run: &mut Run) -> f64 {
    let bar = run.bar();
    run.bounds(bar, Role::Button, "Terminal").y()
}

/// Where the popup `window` is placed now: its first placement, or the last place it was moved to.
fn placed_at(run: &mut Run, window: WindowId) -> Rect {
    let system = run.driver.window_system_mut();
    system
        .requests(window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Reposition(spec) => Some(spec.anchor_rect),
            _ => None,
        })
        .or_else(|| system.placement(window).unwrap().map(|p| p.anchor_rect))
        .expect("a popup")
}

#[test]
fn a_menu_shown_on_an_auto_hidden_bar_slides_the_bar_in_and_follows_its_tile() {
    let (mut run, reader) = untouched();
    let shown = terminal_y(&mut run);
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ToggleAutoHide],
    });
    for _ in 0..4 {
        run.advance(0.5);
    }
    let hidden = terminal_y(&mut run);
    assert!(hidden > shown, "the bar slid out: {hidden} > {shown}");
    let bar = run.bar();
    let tile = node(&run, bar, Role::Button, "Terminal");
    run.driver.ask(&reader, tile, Action::ShowMenu, None);
    run.settle();
    // the menu stands over the bar, so it opens as the bar slides in
    for _ in 0..4 {
        run.advance(0.5);
    }
    let menu = open_menu(&run, bar).expect("the menu opened");
    assert_eq!(terminal_y(&mut run), shown, "the bar slid in for its menu");
    let tile = run.bounds(bar, Role::Button, "Terminal");
    let anchor = placed_at(&mut run, menu);
    assert!(
        (centre(anchor).x - centre(tile).x).abs() < 0.5 && anchor.y() <= tile.y(),
        "the menu stands over the tile where it is drawn: {anchor:?} for {tile:?}"
    );
    let quit = node(&run, menu, Role::MenuItem, "Quit");
    run.driver.ask(&reader, quit, Action::Click, None);
    run.settle();
    for _ in 0..4 {
        run.advance(0.5);
    }
    assert_eq!(open_menu(&run, bar), None);
    assert_eq!(
        terminal_y(&mut run),
        hidden,
        "and slides out again once the menu closed"
    );
}

#[test]
fn a_press_on_the_bar_closes_a_menu_shown_without_a_grab_and_launches_nothing() {
    let (mut run, reader) = untouched();
    let bar = run.bar();
    show_menu(&mut run, &reader, bar);
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.click(bar, editor);
    assert_eq!(open_menu(&run, bar), None);
    assert!(
        run.record.borrow().spawned.is_empty(),
        "the press that closed the menu opened nothing: {:?}",
        run.record.borrow().spawned
    );
    // the next click is the bar's again
    run.click(bar, editor);
    assert_eq!(run.record.borrow().spawned.len(), 1);
}

/// The dock watched by desktop control, before anything gave it an input event.
fn controlled() -> Run {
    watched(Run::start())
}

/// `run` watched by desktop control, before anything gave it an input event.
fn watched(mut run: Run) -> Run {
    let system = run.driver.window_system_mut();
    system.set_no_input_yet();
    system.observe_semantics();
    run.settle();
    run
}

/// Desktop control asks for `action` on the node of `window` with `role` named `name`, with a grab serial when `grab`
/// (a compositor that hands one out) or without (an older one); how the action ended.
fn perform(
    run: &mut Run,
    window: WindowId,
    (role, name): (&str, &str),
    action: semantics::Action,
    grab: bool,
) -> Option<semantics::Outcome> {
    let system = run.driver.window_system_mut();
    let node = system
        .published(window)
        .and_then(|s| {
            s.nodes
                .iter()
                .find(|n| n.role == role && n.name.as_deref() == Some(name))
        })
        .unwrap_or_else(|| panic!("no {role} named {name} published"))
        .id;
    let call = system.perform(window, node, action, None, grab);
    run.settle();
    run.driver.window_system_mut().outcome(call)
}

/// Desktop control shows the terminal tile's menu, the action coming with a grab serial.
fn show_menu_grabbing(run: &mut Run, bar: WindowId) -> WindowId {
    let outcome = perform(
        run,
        bar,
        ("button", "Terminal"),
        semantics::Action::ShowMenu,
        true,
    );
    assert_eq!(outcome, Some(semantics::Outcome::Applied));
    let menu = open_menu(run, bar).expect("the menu opened");
    let system = run.driver.window_system_mut();
    assert_eq!(system.placement(menu).unwrap().map(|p| p.grab), Some(true));
    assert!(
        system.grabbed_by_action(menu),
        "it grabbed with the action's serial"
    );
    menu
}

#[test]
fn a_menu_desktop_control_shows_before_any_input_grabs_and_escape_closes_it() {
    let mut run = controlled();
    let bar = run.bar();
    let menu = show_menu_grabbing(&mut run, bar);
    // the grab gives the menu the keyboard: the arrows walk its items and Escape closes it
    run.send_to(menu, WindowEvent::Focus(true));
    run.key(menu, DOWN);
    let first = run.focused(menu).expect("an item has the focus");
    run.key(menu, DOWN);
    let second = run.focused(menu).expect("an item has the focus");
    assert_ne!(first, second, "the arrows move through the items");
    run.key(menu, ESCAPE);
    assert_eq!(open_menu(&run, bar), None, "Escape closes it");
    assert!(run.record.borrow().spawned.is_empty());
}

#[test]
fn a_menu_desktop_control_shows_closes_when_the_compositor_ends_its_grab_and_an_item_still_acts() {
    let mut run = controlled();
    let bar = run.bar();
    // a press anywhere outside it, in another application too: the compositor dismisses the grabbing menu
    let menu = show_menu_grabbing(&mut run, bar);
    run.send_to(menu, WindowEvent::PopupDone);
    assert_eq!(open_menu(&run, bar), None);
    // the closed menu fades out
    run.advance(0.5);
    // shown again, its item acts through desktop control as through a click
    let menu = show_menu_grabbing(&mut run, bar);
    // the menu's opening frames end before the action
    run.advance(0.5);
    let outcome = perform(
        &mut run,
        menu,
        ("menu-item", "Quit"),
        semantics::Action::Click,
        true,
    );
    assert_eq!(outcome, Some(semantics::Outcome::Applied));
    // the chosen item flashes before it acts
    run.advance(MENU_FLASH);
    assert!(
        run.requests()
            .contains(&(1, Request::App(TERMINAL.to_owned(), kit::AppAction::Quit))),
        "{:?}",
        run.requests()
    );
    assert_eq!(open_menu(&run, bar), None);
}

/// From a compositor that hands out no grab serial, a menu shown before any input still opens without a grab.
#[test]
fn without_a_grab_serial_a_menu_desktop_control_shows_before_any_input_opens_without_a_grab() {
    let mut run = controlled();
    let bar = run.bar();
    let outcome = perform(
        &mut run,
        bar,
        ("button", "Terminal"),
        semantics::Action::ShowMenu,
        false,
    );
    assert_eq!(outcome, Some(semantics::Outcome::Applied));
    let menu = open_menu(&run, bar).expect("the menu opened");
    let system = run.driver.window_system_mut();
    assert_eq!(system.placement(menu).unwrap().map(|p| p.grab), Some(false));
    assert!(!system.grabbed_by_action(menu));
}

/// The downloads stack of two files, its folder listed only once [`end_listings`] says so, watched by desktop control
/// before anything gave it an input event.
fn controlled_stack() -> Run {
    let run = watched(stack_run(2));
    run.record.borrow_mut().hold_listings = true;
    run
}

/// Every folder listing still running ends.
fn end_listings(run: &mut Run) {
    let listings = std::mem::take(&mut run.record.borrow_mut().listings);
    for listed in listings {
        let _ = listed.send_blocking(());
    }
    run.settle();
    run.advance(0.5);
}

/// A click desktop control sends to the downloads tile opens the stack once its folder is listed, after the click was
/// answered: the stack still grabs with the click's serial, so Escape and a press elsewhere close it.
#[test]
fn the_stack_a_desktop_control_click_opens_grabs_once_its_folder_is_listed() {
    let mut run = controlled_stack();
    let bar = run.bar();
    let tile = ("button", "Downloads");
    let outcome = perform(&mut run, bar, tile, semantics::Action::Click, true);
    assert_eq!(outcome, Some(semantics::Outcome::Applied));
    assert!(stack_window(&run).is_none(), "the folder is still listed");
    end_listings(&mut run);
    let stack = stack_window(&run).expect("the stack opened");
    let system = run.driver.window_system_mut();
    assert_eq!(system.placement(stack).unwrap().map(|p| p.grab), Some(true));
    assert!(system.grabbed_by_action(stack));
    run.send_to(stack, WindowEvent::Focus(true));
    run.key(stack, ESCAPE);
    run.settle();
    assert!(stack_window(&run).is_none(), "Escape closes it");

    // shown again: a press elsewhere, which the compositor answers by dismissing it, closes it too
    perform(&mut run, bar, tile, semantics::Action::Click, true);
    end_listings(&mut run);
    let stack = stack_window(&run).expect("the stack opened again");
    assert!(run.driver.window_system_mut().grabbed_by_action(stack));
    run.send_to(stack, WindowEvent::PopupDone);
    run.settle();
    assert!(stack_window(&run).is_none());
}

/// A stack listed once the moment after the answer passed has no action serial left: the grab it would take is the
/// input's, and before any input there is none, so it opens without one.
#[test]
fn a_stack_listed_after_the_answer_s_moment_passed_opens_without_the_action_s_grab() {
    let mut run = controlled_stack();
    let bar = run.bar();
    let tile = ("button", "Downloads");
    perform(&mut run, bar, tile, semantics::Action::Click, true);
    run.driver
        .window_system_mut()
        .expire_answered_action_grabs();
    end_listings(&mut run);
    let stack = stack_window(&run).expect("the stack opened");
    let system = run.driver.window_system_mut();
    assert!(!system.grabbed_by_action(stack));
    assert_eq!(
        system.placement(stack).unwrap().map(|p| p.grab),
        Some(false)
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

/// The Force Quit dialog holds the keyboard while it is up: a Focus Dock meanwhile is dropped, so the bar does not take
/// the keyboard from the dialog.
#[test]
fn the_focus_dock_shortcut_is_dropped_while_the_dialog_has_the_keyboard() {
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

/// The app-grid action (by default the pinch, or a hot corner mapped to it) opens the launcher's application browser:
/// the dock asks the server for the action on the role of the output it fired on, and opens no window of its own. A
/// `toggle_app_grid` from an older server opens nothing either.
#[test]
fn the_app_grid_trigger_asks_the_server_for_the_launcher_s_browser() {
    let mut run = Run::start();
    let windows = run.driver.runtime().windows().len();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Trigger(Trigger::Pinch)],
    });
    assert_eq!(
        run.requests()
            .iter()
            .filter(|r| **r == (1, Request::RunAction(kit::APP_GRID_ACTION.to_owned())))
            .count(),
        1,
        "{:?}",
        run.requests()
    );
    assert_eq!(kit::APP_GRID_ACTION, "app-grid", "the server's action name");
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ToggleAppGrid],
    });
    run.advance(1.0);
    assert_eq!(
        run.driver.runtime().windows().len(),
        windows,
        "the dock opens no window for it"
    );
    assert!(
        run.record.borrow().keyboard.is_empty(),
        "nor takes the keyboard: {:?}",
        run.record.borrow().keyboard
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

/// The bar's node asks for glass under the bar's own rectangle, not under the whole window (which is deeper and as
/// wide as the output); under Reduce Transparency it asks for the Dock's own material instead, a body in the
/// wallpaper's colour.
#[test]
fn with_reduce_transparency_the_bar_has_its_own_body() {
    for (reduce, material, glass) in [(false, "BarGlass", true), (true, "Dock", false)] {
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
        let regions = last_materials(requests);
        // in the light appearance the running dot paints itself
        assert_eq!(regions.len(), 1, "{regions:?}");
        assert_eq!(
            regions[0].material, material,
            "reduce transparency {reduce}"
        );
        assert_eq!(
            regions[0].glass.is_some(),
            glass,
            "reduce transparency {reduce}"
        );
        assert!(
            regions[0].rect.width() < width,
            "the material lies under the bar's rectangle only: {regions:?}"
        );
    }
}

/// In the dark appearance a running dot asks for the material that lightens what lies behind it, in front of the
/// bar's, with and without Reduce Transparency.
#[test]
fn in_the_dark_the_running_dot_lightens_the_bar_behind_it() {
    for reduce in [false, true] {
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance: Appearance::Dark,
            reduce_transparency: reduce,
            reduce_motion: false,
        };
        let tokens = ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap();
        let mut run = Run::with(tokens, ActionMap::default());
        let bar = run.bar();
        let requests = run.driver.window_system_mut().requests(bar).unwrap();
        let regions = last_materials(requests);
        let marks = indicators(&run, bar);
        assert_eq!(marks.len(), 1, "only the terminal runs");
        assert_eq!(regions.len(), 2, "{regions:?}");
        assert_eq!(
            regions[1].material, "RunningIndicator",
            "reduce transparency {reduce}"
        );
        assert!(regions[1].glass.is_none());
        let dot = regions[1].rect;
        assert!(
            regions[0].rect.contains(dot.center()),
            "the dot lies on the bar: {regions:?}"
        );
        assert!(dot.width() <= marks[0].width() + 1e-9 && dot.height() <= marks[0].height() + 1e-9);
    }
}

/// The materials of the last request that set them.
fn last_materials(requests: &[WindowRequest]) -> Vec<MaterialRegion> {
    requests
        .iter()
        .rev()
        .find_map(|r| match r {
            WindowRequest::Materials(m) => Some(m.clone()),
            _ => None,
        })
        .unwrap_or_default()
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
fn the_force_quit_dialog_reads_its_text_and_list_and_the_selected_row_takes_the_focus() {
    let mut run = Run::start();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::ForceQuitDialog],
    });
    let dialog = run.window(&format!("{FORCE_QUIT_APP_ID}@1")).unwrap();
    run.send_to(dialog, WindowEvent::Focus(true));
    let snapshot = run.driver.runtime().snapshot(dialog).unwrap();
    let tree = whole_tree(dialog, snapshot.accessibility, snapshot.focus);
    let items: Vec<String> = tree
        .nodes
        .iter()
        .filter(|(_, n)| n.role == Role::ListItem)
        .map(|(_, n)| n.name.clone().unwrap())
        .collect();
    assert!(!items.is_empty());
    let list = format!("List \"Applications\" {} items", items.len());
    let first = format!("ListItem {:?}", items[0]);
    assert_eq!(
        unannounced(
            &tree,
            &[
                "StaticText \"If an application does not respond for a while, select its name and click Force Quit.\"",
                &list,
                &first,
                "StaticText \"You can open this window by pressing Super-Alt-Escape.\"",
            ]
        ),
        Vec::<&str>::new(),
        "{}",
        semantic_outline(&tree).join("\n")
    );
    run.key(dialog, DOWN);
    assert_eq!(
        run.focused(dialog),
        Some(items[0].clone()),
        "the row Down selects takes the focus"
    );
    run.key(dialog, TAB);
    assert_eq!(
        run.focused(dialog).as_deref(),
        Some("Force Quit"),
        "Tab leaves the list for the button, and the selected row does not take it back"
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

fn kit_rect(r: WmRect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

/// The names of the nodes of `role` in `window`, in order.
fn named(run: &Run, window: WindowId, role: Role) -> Vec<String> {
    run.nodes(window)
        .into_iter()
        .filter(|n| n.0 == role)
        .map(|n| n.1)
        .collect()
}

// ---- residency --------------------------------------------------------------------------------------------------

fn resident(run: &mut Run, output: u32, app_id: &str, on: bool) {
    run.message(Message::Dock {
        output,
        events: vec![DockEvent::Resident {
            app_id: app_id.to_owned(),
            resident: on,
        }],
    });
}

fn description(run: &Run, bar: WindowId, name: &str) -> String {
    run.nodes(bar)
        .into_iter()
        .find(|n| n.0 == Role::Button && n.1 == name)
        .unwrap_or_else(|| panic!("no tile {name}"))
        .2
}

/// An application whose last window closed while its process runs keeps its running dot; opening it asks that process
/// for a window over D-Bus with an activation token, once, without a launch bounce, and the dot goes when the process
/// exits.
#[test]
fn a_resident_application_keeps_its_dot_and_is_asked_over_the_bus_for_a_window() {
    let mut run = Run::start();
    let bar = run.bar();
    resident(&mut run, 1, TERMINAL, true);
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(7)],
    });
    run.advance(1.0);
    assert_eq!(indicators(&run, bar).len(), 1, "the dot stays");
    assert_eq!(description(&run, bar, "Terminal"), kit::RUNNING);

    let terminal = centre(run.bounds(bar, Role::Button, "Terminal"));
    run.click(bar, terminal);
    run.click(bar, terminal);
    let activated = run.record.borrow().activated.clone();
    assert_eq!(activated.len(), 1, "asked once: {activated:?}");
    assert_eq!(activated[0].0, TERMINAL, "by the desktop file ID");
    let token = activated[0].1.as_deref().expect("an activation token");
    assert!(token.starts_with("headless-activation-"), "{token}");
    assert!(run.record.borrow().spawned.is_empty(), "nothing started");
    assert!(
        !run.requests()
            .iter()
            .any(|r| matches!(&r.1, Request::ActivateApp(_))),
        "no window to raise"
    );
    assert_eq!(
        description(&run, bar, "Terminal"),
        kit::RUNNING,
        "a running application does not bounce"
    );

    // the window comes; the process exits with it
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 8,
            app_id: TERMINAL.to_owned(),
            title: "shell".to_owned(),
            minimized: false,
        }],
    });
    resident(&mut run, 1, TERMINAL, false);
    run.advance(1.0);
    assert_eq!(indicators(&run, bar).len(), 1, "it still shows a window");
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(8)],
    });
    run.advance(1.0);
    assert!(indicators(&run, bar).is_empty(), "the dot went with it");
    assert_eq!(description(&run, bar, "Terminal"), "");
}

/// A resident application whose entry is not D-Bus activatable, or whose bus call cannot be made, is asked for a window
/// by starting its entry again with an activation token; a second click while that is under way starts nothing more.
#[test]
fn a_resident_application_without_the_bus_is_started_again_once() {
    let mut run = Run::start();
    let bar = run.bar();
    resident(&mut run, 1, EDITOR, true);
    run.advance(1.0);
    assert_eq!(
        indicators(&run, bar).len(),
        2,
        "a resident application has a dot"
    );
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.click(bar, editor);
    run.click(bar, editor);
    let spawned = run.record.borrow().spawned.clone();
    assert_eq!(spawned.len(), 1, "{spawned:?}");
    assert_eq!(spawned[0].0, ["editor"]);
    assert!(spawned[0].1.is_some(), "with an activation token");
    assert_eq!(description(&run, bar, "Editor"), kit::RUNNING);
    resident(&mut run, 1, EDITOR, false);
    run.advance(1.0);
    assert_eq!(indicators(&run, bar).len(), 1, "the dot went");

    run.record.borrow_mut().no_bus = true;
    resident(&mut run, 1, TERMINAL, true);
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(7)],
    });
    run.advance(1.0);
    run.click(bar, centre(run.bounds(bar, Role::Button, "Terminal")));
    let spawned = run.record.borrow().spawned.clone();
    assert_eq!(spawned.len(), 2, "{spawned:?}");
    assert_eq!(spawned[1].0, ["terminal"]);
    assert!(spawned[1].1.is_some(), "with an activation token");
}

/// A launch asked for again before its window came starts nothing more.
#[test]
fn a_launch_is_not_asked_for_twice() {
    let mut run = Run::start();
    let bar = run.bar();
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.click(bar, editor);
    run.advance(0.5);
    run.click(bar, editor);
    assert_eq!(run.record.borrow().spawned.len(), 1);
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 8,
            app_id: EDITOR.to_owned(),
            title: "notes".to_owned(),
            minimized: false,
        }],
    });
    run.click(bar, centre(run.bounds(bar, Role::Button, "Editor")));
    assert_eq!(run.record.borrow().spawned.len(), 1);
    assert!(run
        .requests()
        .contains(&(1, Request::ActivateApp(EDITOR.to_owned()))));
}

/// The model alone, the test handing it messages with no runtime around it: the commands it gives are not carried out,
/// so an activation token it asks for never comes. With the key of the editor's tile.
fn bare() -> (Model, Rc<RefCell<Record>>, u64) {
    let record = Rc::new(RefCell::new(Record::default()));
    let mut dock = Dock::new(DockConfig::default());
    dock.set_pinned(&[EDITOR, TERMINAL]);
    let editor = dock
        .tiles()
        .iter()
        .find(|t| t.key == EDITOR)
        .expect("the editor's tile")
        .id
        .0;
    let desktop = DesktopIndex {
        entries: vec![entry_with(EDITOR, "Editor", "")],
    };
    let mut model = Model::new(
        Session::new(dock, desktop),
        ActionMap::default(),
        DockSettings::default(),
        Box::new(FakeHost(Rc::clone(&record))),
    );
    kit::update(&mut model, Message::Outputs(OUTPUTS.to_vec()));
    (model, record, editor)
}

/// Move the bare model's clock on by `seconds` and wake it.
fn bare_advance(model: &mut Model, record: &Rc<RefCell<Record>>, seconds: f64) {
    let generation = {
        let mut record = record.borrow_mut();
        record.now += seconds;
        record.wake.unwrap_or(0)
    };
    kit::update(model, Message::Wake(generation));
}

/// The wait for an activation token is bounded: one that does not come within two seconds no longer holds the launch
/// or the guard against a second one; the program starts without a token, and one that comes later is dropped.
#[test]
fn a_launch_whose_activation_token_never_comes_goes_ahead_without_one_after_2_s() {
    let (mut model, record, editor) = bare();
    kit::update(
        &mut model,
        Message::Input(cupertino_dock::view::Input::Activate(editor)),
    );
    assert!(record.borrow().spawned.is_empty(), "waiting for the token");
    let wakes = record.borrow().wakes.clone();
    assert!(
        wakes.iter().any(|w| (w - kit::TOKEN_TIMEOUT).abs() < 1e-9),
        "the model asks to be woken when the wait ends: {wakes:?}"
    );
    bare_advance(&mut model, &record, 1.0);
    kit::update(
        &mut model,
        Message::Input(cupertino_dock::view::Input::Activate(editor)),
    );
    assert!(
        record.borrow().spawned.is_empty(),
        "a second ask within the wait is the same launch"
    );
    bare_advance(&mut model, &record, 1.0);
    {
        let spawned = &record.borrow().spawned;
        assert_eq!(spawned.len(), 1, "started at the end of the wait");
        assert_eq!(spawned[0].1, None, "without a token");
    }
    for launch in 0..4 {
        kit::update(&mut model, Message::Token(launch, Some("late".to_owned())));
    }
    assert_eq!(
        record.borrow().spawned.len(),
        1,
        "a late token starts nothing"
    );
}

/// A program that fails before it shows a window, as one that crashes at once does, can be opened again straight away:
/// the guard against a second launch ends with its process.
#[test]
fn a_program_that_exits_at_once_can_be_opened_again_straight_away() {
    let mut run = Run::start();
    let bar = run.bar();
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.click(bar, editor);
    run.advance(0.5);
    run.click(bar, editor);
    assert_eq!(
        run.record.borrow().spawned.len(),
        1,
        "guarded while it starts"
    );
    let exit = run.record.borrow().exits[0].clone();
    exit.send_blocking(true).unwrap();
    run.settle();
    run.advance(0.1);
    run.click(bar, editor);
    assert_eq!(
        run.record.borrow().spawned.len(),
        2,
        "started again after it exited"
    );
}

/// A launcher that exits successfully before the window comes (one that starts the application in the background, or
/// hands the open to an instance already running) leaves the guard in place: a second click opens nothing more.
#[test]
fn a_launcher_that_exits_successfully_before_the_window_leaves_the_guard() {
    let mut run = Run::start();
    let bar = run.bar();
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.click(bar, editor);
    run.advance(0.5);
    let exit = run.record.borrow().exits[0].clone();
    exit.send_blocking(false).unwrap();
    run.settle();
    run.advance(0.1);
    run.click(bar, editor);
    assert_eq!(
        run.record.borrow().spawned.len(),
        1,
        "still guarded: its window is on the way"
    );
    run.advance(kit::OPEN_GUARD);
    run.click(bar, editor);
    assert_eq!(
        run.record.borrow().spawned.len(),
        2,
        "the guard ends in its own time"
    );
}

/// Only the process of the launch a guard is for ends it: an earlier one that exits after a later launch leaves the
/// later guard in place.
#[test]
fn an_earlier_launch_exiting_leaves_a_later_launch_guarded() {
    let mut run = Run::start();
    let bar = run.bar();
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.click(bar, editor);
    run.advance(kit::OPEN_GUARD + 1.0);
    run.click(bar, editor);
    assert_eq!(
        run.record.borrow().spawned.len(),
        2,
        "the guard of the first ended in time"
    );
    let first = run.record.borrow().exits[0].clone();
    first.send_blocking(true).unwrap();
    run.settle();
    run.advance(0.1);
    run.click(bar, editor);
    assert_eq!(
        run.record.borrow().spawned.len(),
        2,
        "the second launch is still guarded"
    );
}

/// A program that showed a window is running: its exit afterwards changes nothing about how its tile opens.
#[test]
fn an_exit_after_a_window_showed_keeps_the_running_application_activated() {
    let mut run = Run::start();
    let bar = run.bar();
    run.click(bar, centre(run.bounds(bar, Role::Button, "Editor")));
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 8,
            app_id: EDITOR.to_owned(),
            title: "notes".to_owned(),
            minimized: false,
        }],
    });
    let exit = run.record.borrow().exits[0].clone();
    exit.send_blocking(true).unwrap();
    run.settle();
    run.click(bar, centre(run.bounds(bar, Role::Button, "Editor")));
    assert_eq!(run.record.borrow().spawned.len(), 1);
    assert!(run
        .requests()
        .contains(&(1, Request::ActivateApp(EDITOR.to_owned()))));
}

/// With a host that loads in the background, the bar loads only its resting side on the spot: the magnified side
/// of its icons is asked for in the background, each once. An icon the background leaves out (an application with
/// only a window's icon) loads on the spot when the rest arrive.
#[test]
fn icons_load_in_the_background_so_the_first_magnification_waits_for_none() {
    BACKGROUND.set(Some(Vec::new()));
    let run = Run::start();
    let large = DockConfig::default().large_size;
    let tile = DockConfig::default().tile_size;
    let preloads = run.record.borrow().preloads.clone();
    assert!(!preloads.is_empty(), "a background load was asked for");
    let asked: Vec<(IconKey, f64)> = preloads.concat();
    for app in [EDITOR, TERMINAL] {
        assert!(
            asked.contains(&(IconKey::App(app.to_owned()), large)),
            "{app} magnified"
        );
    }
    let mut unique = asked.clone();
    unique.dedup();
    assert_eq!(unique.len(), asked.len(), "each icon is asked for once");
    let loads = run.record.borrow().icon_loads.clone();
    assert!(
        loads.iter().all(|(_, side)| *side == tile),
        "only the resting side on the spot: {loads:?}"
    );
}

#[test]
fn an_icon_the_background_leaves_out_loads_on_the_spot() {
    BACKGROUND.set(Some(vec![IconKey::App(EDITOR.to_owned())]));
    let mut run = Run::with_viewer();
    let large = DockConfig::default().large_size;
    let loads = run.record.borrow().icon_loads.clone();
    assert!(
        loads.contains(&(IconKey::App(EDITOR.to_owned()), large)),
        "{loads:?}"
    );
    assert!(
        !loads.contains(&(IconKey::App(TERMINAL.to_owned()), large)),
        "{loads:?}"
    );
    // a tile that comes later is asked for in the background as it comes
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 9,
            app_id: "org.example.Viewer".to_owned(),
            title: "picture".to_owned(),
            minimized: false,
        }],
    });
    let last = run
        .record
        .borrow()
        .preloads
        .last()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        last,
        vec![(IconKey::App("org.example.Viewer".to_owned()), large)]
    );
    let loads = run.record.borrow().icon_loads.clone();
    assert!(loads.contains(&(IconKey::App("org.example.Viewer".to_owned()), 64.0)));
}

const VIEWER: &str = "org.example.Viewer";

/// The tiles of the bar by name, the separators as "|", in order.
fn bar_row(run: &mut Run) -> Vec<String> {
    let bar = run.bar();
    let mut nodes: Vec<(Role, String, String, Rect)> = run
        .nodes(bar)
        .into_iter()
        .filter(|n| matches!(n.0, Role::Button | Role::Separator))
        .collect();
    nodes.sort_by(|a, b| a.3.x().total_cmp(&b.3.x()));
    nodes
        .into_iter()
        .map(|n| {
            if n.0 == Role::Separator {
                "|".to_owned()
            } else {
                n.1
            }
        })
        .collect()
}

fn viewer_run() -> Run {
    Run::on_desktop(
        tokens(false),
        ActionMap::default(),
        true,
        vec![
            entry_with(VIEWER, "Viewer", ""),
            entry_with("org.example.Mail", "Mail", ""),
            entry_with("org.example.Music", "Music", ""),
            entry_with("org.example.Maps", "Maps", ""),
        ],
    )
}

/// An application that quits, not pinned, comes into the recents: their own section after the applications, between
/// two separators, before the trash. Opening it from there starts it; while it runs it is among the applications.
#[test]
fn an_application_that_quits_comes_into_the_recents_between_two_separators() {
    let mut run = viewer_run();
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::Window {
            id: 9,
            app_id: VIEWER.to_owned(),
            title: "picture".to_owned(),
            minimized: false,
        }],
    });
    run.advance(1.0);
    let row = bar_row(&mut run);
    assert_eq!(&row[..3], ["Editor", "Terminal", "Viewer"], "{row:?}");
    run.message(Message::Dock {
        output: 1,
        events: vec![DockEvent::WindowClosed(9)],
    });
    run.advance(1.0);
    let row = bar_row(&mut run);
    let trash = row.last().cloned().unwrap();
    assert_eq!(
        row,
        ["Editor", "Terminal", "|", "Viewer", "|", trash.as_str()]
    );
    let bar = run.bar();
    run.click(bar, centre(run.bounds(bar, Role::Button, "Viewer")));
    let spawned = run.record.borrow().spawned.clone();
    assert_eq!(spawned.len(), 1);
    assert_eq!(spawned[0].0[0], "viewer");
}

/// With the launcher's history, the recents are the applications that quit, then the history's, the most recent first:
/// none pinned or running, none twice, at most three.
#[test]
fn the_recents_are_those_that_quit_then_the_launchers_history_at_most_three() {
    let mut run = viewer_run();
    run.record.borrow_mut().recent = vec![
        format!("{EDITOR}.desktop"),
        "org.example.Maps.desktop".to_owned(),
        "org.example.Music.desktop".to_owned(),
        "org.example.Mail.desktop".to_owned(),
        "org.example.Gone.desktop".to_owned(),
    ];
    for (id, closed) in [(9, false), (9, true)] {
        let events = if closed {
            vec![DockEvent::WindowClosed(id)]
        } else {
            vec![DockEvent::Window {
                id,
                app_id: "org.example.Music".to_owned(),
                title: "song".to_owned(),
                minimized: false,
            }]
        };
        run.message(Message::Dock { output: 1, events });
        run.advance(1.0);
    }
    let row = bar_row(&mut run);
    assert_eq!(
        &row[..7],
        ["Editor", "Terminal", "|", "Music", "Maps", "Mail", "|"],
        "{row:?}"
    );
}

/// With the recents off (`recents = off`), the bar has no recents section.
#[test]
fn without_recents_the_bar_has_one_separator() {
    let mut run = viewer_run();
    run.record.borrow_mut().conf = Some(DockSettings {
        recents: Some(false),
        ..DockSettings::default()
    });
    run.message(Message::ConfChanged);
    for events in [
        vec![DockEvent::Window {
            id: 9,
            app_id: VIEWER.to_owned(),
            title: "picture".to_owned(),
            minimized: false,
        }],
        vec![DockEvent::WindowClosed(9)],
    ] {
        run.message(Message::Dock { output: 1, events });
        run.advance(1.0);
    }
    let row = bar_row(&mut run);
    assert_eq!(row.len(), 4, "{row:?}");
    assert_eq!(&row[..3], ["Editor", "Terminal", "|"]);
}

const DOWNLOADS: &str = "/fixture/Downloads";

/// A run whose host shows the downloads folder as a stack of `count` files.
fn stack_run(count: usize) -> Run {
    FOLDERS.set(vec![(DOWNLOADS.to_owned(), "folder-download".to_owned())]);
    let run = Run::start();
    run.record.borrow_mut().folder_entries = (0..count)
        .map(|i| cupertino_dock::stack::StackEntry {
            name: format!("file-{i}.txt"),
            path: format!("{DOWNLOADS}/file-{i}.txt").into(),
            icon: "text-x-generic".to_owned(),
        })
        .collect();
    run
}

/// The stack's popup window, while it is open.
fn stack_window(run: &Run) -> Option<WindowId> {
    run.driver
        .runtime()
        .windows()
        .into_iter()
        .map(|(_, id)| id)
        .find(|&id| {
            run.driver.runtime().snapshot(id).is_some_and(|s| {
                s.accessibility
                    .iter()
                    .any(|(_, n)| n.role == Role::Dialog && n.name.as_deref() == Some("Downloads"))
            })
        })
}

/// The downloads folder is a tile after the applications, before the trash; a click opens its stack, a grabbing
/// popup over it with the folder's name, its newest files as buttons and a button that opens the folder. A click on
/// a file opens it and closes the stack; a second click on the tile closes it too.
#[test]
fn the_downloads_folder_is_a_tile_before_the_trash_whose_click_opens_its_stack() {
    let mut run = stack_run(7);
    let row = bar_row(&mut run);
    assert_eq!(
        &row[..4],
        ["Editor", "Terminal", "|", "Downloads"],
        "{row:?}"
    );
    assert_eq!(row.len(), 5, "then the trash: {row:?}");
    assert!(stack_window(&run).is_none());
    let bar = run.bar();
    let tile = centre(run.bounds(bar, Role::Button, "Downloads"));
    run.click(bar, tile);
    run.advance(0.5);
    let stack = stack_window(&run).expect("the stack opened");
    let buttons = named(&run, stack, Role::Button);
    assert_eq!(buttons.len(), 8, "{buttons:?}");
    assert!(buttons.contains(&"Open in Files".to_owned()));
    let layout = cupertino_dock::stack::StackLayout::new(7);
    // the newest file takes the first cell
    let first = run.bounds(stack, Role::Button, "file-0.txt");
    assert!(
        kit_rect(layout.cell(0)).contains(centre(first)),
        "{first:?} in {:?}",
        layout.cell(0)
    );
    let bounds = run.bounds(stack, Role::Button, "Open in Files");
    assert_eq!(
        bounds,
        kit_rect(layout.button),
        "the button where the layout puts it"
    );
    run.click(stack, centre(run.bounds(stack, Role::Button, "file-2.txt")));
    assert_eq!(
        run.record.borrow().opened,
        [format!("{DOWNLOADS}/file-2.txt")]
    );
    run.settle();
    assert!(
        stack_window(&run).is_none(),
        "opening a file closes the stack"
    );

    run.click(bar, tile);
    run.advance(0.5);
    assert!(stack_window(&run).is_some());
    run.click(bar, tile);
    run.settle();
    assert!(stack_window(&run).is_none(), "a second click closes it");
}

/// Escape closes the stack; its button opens the folder in the file manager (here the session's opener, as no file
/// manager is installed) and closes it.
#[test]
fn escape_closes_the_stack_and_its_button_opens_the_folder() {
    let mut run = stack_run(0);
    let bar = run.bar();
    let tile = centre(run.bounds(bar, Role::Button, "Downloads"));
    run.click(bar, tile);
    run.advance(0.5);
    // an empty folder fans out only its button
    let stack = stack_window(&run).expect("the stack opened");
    assert_eq!(named(&run, stack, Role::Button), ["Open in Files"]);
    run.send_to(stack, WindowEvent::Focus(true));
    run.key(stack, ESCAPE);
    // the fan folds back into its tile
    run.advance(0.5);
    assert!(stack_window(&run).is_none());
    run.click(bar, tile);
    run.advance(0.5);
    let stack = stack_window(&run).expect("the stack opened again");
    run.click(
        stack,
        centre(run.bounds(stack, Role::Button, "Open in Files")),
    );
    run.advance(0.5);
    assert_eq!(run.record.borrow().opened, [DOWNLOADS.to_owned()]);
    assert!(stack_window(&run).is_none());
}

/// The folder is listed away from the dock: the stack shows only once its listing ends, a click on the tile while it
/// is listed closes it for good (the listing that ends later opens nothing).
#[test]
fn a_stack_shows_once_its_folder_is_listed_and_a_listing_for_a_closed_stack_opens_nothing() {
    let mut run = stack_run(3);
    run.record.borrow_mut().hold_listings = true;
    let bar = run.bar();
    let tile = centre(run.bounds(bar, Role::Button, "Downloads"));
    run.click(bar, tile);
    run.advance(0.5);
    assert!(stack_window(&run).is_none(), "still being listed");
    assert_eq!(run.record.borrow().listings.len(), 1);
    let first = run.record.borrow().listings[0].clone();
    first.send_blocking(()).unwrap();
    run.settle();
    run.advance(0.5);
    let stack = stack_window(&run).expect("shown once listed");
    assert_eq!(
        named(&run, stack, Role::Button).len(),
        4,
        "three files and the button"
    );

    run.click(bar, tile);
    run.settle();
    assert!(stack_window(&run).is_none());
    run.click(bar, tile);
    run.click(bar, tile);
    run.settle();
    let listings = run.record.borrow().listings.clone();
    assert_eq!(listings.len(), 2, "a listing for the second open");
    listings[1].send_blocking(()).unwrap();
    run.settle();
    run.advance(0.5);
    assert!(
        stack_window(&run).is_none(),
        "closed while it was listed, it stays closed"
    );
}

/// The label popup, while it shows: its window and the name it says.
fn label(run: &Run) -> Option<(WindowId, String)> {
    run.driver
        .runtime()
        .windows()
        .into_iter()
        .map(|(_, id)| id)
        .find_map(|id| {
            let snapshot = run.driver.runtime().snapshot(id)?;
            snapshot
                .accessibility
                .iter()
                .find(|(_, n)| n.role == Role::Tooltip)
                .map(|(_, n)| (id, n.name.clone().unwrap_or_default()))
        })
}

/// The tile under the pointer shows its name over it at once, and the name follows the pointer from tile to tile; it
/// goes when the pointer leaves the bar and while the tile's menu is open. A separator has none.
#[test]
fn the_tile_under_the_pointer_shows_its_name_over_it_at_once() {
    let mut run = Run::start();
    let bar = run.bar();
    assert!(label(&run).is_none());
    let editor = centre(run.bounds(bar, Role::Button, "Editor"));
    run.pointer(bar, PointerEvent::Enter { position: editor });
    run.settle();
    assert_eq!(label(&run).map(|l| l.1).as_deref(), Some("Editor"));
    let terminal = centre(run.bounds(bar, Role::Button, "Terminal"));
    run.pointer(bar, PointerEvent::Motion { position: terminal });
    run.settle();
    assert_eq!(label(&run).map(|l| l.1).as_deref(), Some("Terminal"));
    run.button(bar, terminal, MouseButton::Right, true);
    run.button(bar, terminal, MouseButton::Right, false);
    run.settle();
    assert!(label(&run).is_none(), "not with the menu open");
    run.message(Message::Input(cupertino_dock::view::Input::MenuClosed));
    run.pointer(bar, PointerEvent::Leave);
    run.advance(1.0);
    assert!(label(&run).is_none(), "gone with the pointer");
}

/// A new bar role's first group lists every resident application: one the dock still holds that it does not list
/// exited meanwhile, and loses its dot.
#[test]
fn a_resident_application_the_new_role_does_not_list_loses_its_dot() {
    let mut run = Run::start();
    resident(&mut run, 1, EDITOR, true);
    run.advance(1.0);
    let first = run.bar();
    assert_eq!(indicators(&run, first).len(), 2);
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
    let bar = run
        .window(&format!("{BAR_APP_ID}@2"))
        .expect("the bar moved");
    run.message(Message::Dock {
        output: 2,
        events: vec![DockEvent::Window {
            id: 7,
            app_id: TERMINAL.to_owned(),
            title: "shell".to_owned(),
            minimized: false,
        }],
    });
    run.advance(1.0);
    assert_eq!(indicators(&run, bar).len(), 1, "only the terminal runs");
    assert_eq!(description(&run, bar, "Editor"), "");
}

// ---- Reduce Motion ----------------------------------------------------------------------------------------------

/// Under Reduce Motion a launch dims its tile in place instead of bouncing it (the magnification and the dim are
/// tested on the model, `tests/dock.rs`).
#[test]
fn under_reduce_motion_a_launch_does_not_bounce() {
    let mut run = Run::start();
    run.message(Message::ReduceMotion(true));
    let bar = run.bar();
    let rest = run.bounds(bar, Role::Button, "Editor");
    run.click(bar, centre(rest));
    assert_eq!(run.record.borrow().spawned.len(), 1);
    let mut moved = false;
    for _ in 0..12 {
        run.advance(0.05);
        moved |= run.bounds(bar, Role::Button, "Editor").y() != rest.y();
    }
    assert!(!moved, "the tile stays in place");
    assert_eq!(description(&run, bar, "Editor"), kit::OPENING);
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
    // the anchor runs from the bar's top down, so the menu stands above the bar, over the tile
    assert!(
        spec.anchor_rect.y() <= terminal.y() + 0.5
            && spec.anchor_rect.y() + spec.anchor_rect.height()
                >= terminal.y() + terminal.height() - 0.5,
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
    let mut run = Run::with_viewer();
    // without recents the tile of the application that quit leaves the bar
    run.record.borrow_mut().conf = Some(DockSettings {
        recents: Some(false),
        ..DockSettings::default()
    });
    run.message(Message::ConfChanged);
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

/// Every change the bar shows reaches the screen as a new frame of the bar's window: a running application, an
/// attention bounce, a new output scale, Focus Dock, and a `dock.conf` that turns magnification on (only the
/// surface's depth changes) or changes the tile size, the position or auto-hide; an
/// output plugged in shows its strip, and the bar moved off an unplugged output shows on the next. The bar draws no
/// revision that changes with every update, so each must change what it paints or its surface.
#[test]
fn every_change_the_bar_shows_presents_a_new_frame() {
    use cupertino_defaults::dock::Position;
    let mut run = Run::start();
    let presented = |run: &mut Run| {
        let bar = run.bar();
        run.driver.window_system_mut().frames(bar).unwrap().len()
    };
    let check = |run: &mut Run, what: &str, change: &dyn Fn(&mut Run)| {
        run.advance(2.0);
        let before = presented(run);
        change(run);
        run.advance(2.0);
        assert!(presented(run) > before, "{what} presents a new frame");
    };
    check(&mut run, "an application starting", &|run| {
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::Window {
                id: 8,
                app_id: EDITOR.to_owned(),
                title: "notes".to_owned(),
                minimized: false,
            }],
        });
    });
    check(&mut run, "an application quitting", &|run| {
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::WindowClosed(8)],
        });
    });
    check(&mut run, "an attention bounce", &|run| {
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::Attention(TERMINAL.to_owned())],
        });
    });
    check(&mut run, "a new output scale", &|run| {
        let bar = run.bar();
        run.send_to(bar, WindowEvent::Scale(2.0));
    });
    let conf = |run: &mut Run, settings: DockSettings| {
        run.record.borrow_mut().conf = Some(settings);
        run.message(Message::ConfChanged);
    };
    // these change only the bar's surface (its depth, its keyboard interactivity), which applies with its next commit
    let placements = run.record.borrow().placements.len();
    check(&mut run, "magnification turned on", &|run| {
        conf(
            run,
            DockSettings {
                magnification: Some(true),
                ..DockSettings::default()
            },
        );
    });
    assert!(
        run.record.borrow().placements.len() > placements,
        "the bar's depth changed"
    );
    check(&mut run, "Focus Dock", &|run| engage(run, 1));
    assert_eq!(
        run.record.borrow().keyboard.last(),
        Some(&(Part::Bar(1), true))
    );
    check(&mut run, "a new tile size", &|run| {
        conf(
            run,
            DockSettings {
                tile_size: Some(40.0),
                ..DockSettings::default()
            },
        );
    });
    check(&mut run, "a new position", &|run| {
        conf(
            run,
            DockSettings {
                tile_size: Some(40.0),
                position: Some(Position::Left),
                ..DockSettings::default()
            },
        );
    });
    check(&mut run, "auto-hide", &|run| {
        conf(
            run,
            DockSettings {
                tile_size: Some(40.0),
                position: Some(Position::Left),
                autohide: Some(true),
                ..DockSettings::default()
            },
        );
    });

    // an output plugged in shows its edge strip; the bar's output unplugged moves the bar to the next one, shown there
    let mut outputs = OUTPUTS.to_vec();
    outputs.push(OutputInfo {
        key: 3,
        width: 800.0,
        height: 600.0,
        scale: 1.0,
    });
    run.message(Message::Outputs(outputs.clone()));
    run.advance(2.0);
    let strip = run
        .window(&format!("{EDGE_APP_ID}@3"))
        .expect("the new output's strip");
    // it draws nothing: shown as a transparent colour, without a frame
    let system = run.driver.window_system_mut();
    assert!(system.frames(strip).unwrap().is_empty());
    assert!(!system.solids(strip).unwrap().is_empty());
    outputs.remove(0);
    run.message(Message::Outputs(outputs));
    run.advance(2.0);
    let moved = run
        .window(&format!("{BAR_APP_ID}@2"))
        .expect("the bar on the next output");
    assert!(!run
        .driver
        .window_system_mut()
        .frames(moved)
        .unwrap()
        .is_empty());
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
        run.message(Message::Dock {
            output: 1,
            events: vec![DockEvent::OverviewClosed],
        });
        for _ in 0..12 {
            run.advance(0.1);
        }
    }
}
