//! The launcher's windows run headless on the test launcher's entries: the Inspector dump of the panel (layout,
//! semantics and display list) compared with `tests/goldens/<name>.txt`, and what a screen reader reads of it
//! compared with `tests/fixtures/screen-reader/<name>.txt`. `CUPERTINO_UPDATE_GOLDENS=1` re-records both and then
//! fails, so the variable can never pass a run.

use super::*;

mod frames;
use crate::browse::Zone;
use crate::engine::tests_support::test_launcher;
use crate::exec::CommandSpec;
use crate::files::FileSearchConfig;
use crate::kit::{self, Host, Message, Model, Running, SearchRequest};
use crate::nav::View;
use crate::panel::Now;
use crate::providers::{Provider, Reply, Update};
use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityNode, AccessibleValue, Application, Checked, Command, FrameSnapshot, NodeId,
    PlaceholderIcons, RuntimeOptions,
};
use cupertino_kit::geometry::Point;
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{AssistiveTechnology, Inspector, ManualClock, TestDriver};
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

const OUTPUT: Size = Size {
    width: 1000.0,
    height: 700.0,
};

thread_local! {
    /// What the test host was asked to do, in order.
    static HOST: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The size the model last gave the shown panel's window.
    static EXTENT: RefCell<Option<Size>> = const { RefCell::new(None) };
    /// Messages a test sends the model as the session would (`Toggle` from the bus, `Locked` from the server).
    static INBOX: RefCell<Option<(async_channel::Sender<Message>, async_channel::Receiver<Message>)>> =
        const { RefCell::new(None) };
    /// The message being handled came from [`INBOX`] (so the inbox is listened to again).
    static INBOXED: RefCell<bool> = const { RefCell::new(false) };
    /// While set, the file searches the model asks for wait here instead of answering at once.
    static HELD: RefCell<Option<Vec<SearchRequest>>> = const { RefCell::new(None) };
    /// The server can be asked to bring an application forward ([`Host::bring_forward`]).
    static FORWARDS: Cell<bool> = const { Cell::new(false) };
    /// The launch the server was last asked about.
    static ASKED: Cell<Option<u64>> = const { Cell::new(None) };
    /// The entries are D-Bus activatable, and their `Activate` call ends in this answer.
    static BUS_ANSWER: Cell<Option<bool>> = const { Cell::new(None) };
}

/// Holds the file searches from now on, for a test to answer as it likes.
fn hold_searches() {
    HELD.set(Some(Vec::new()));
}

/// The file searches held since the last call.
fn held_searches() -> Vec<SearchRequest> {
    HELD.with_borrow_mut(|held| held.as_mut().map(std::mem::take).unwrap_or_default())
}

/// Waits for the next message of [`INBOX`].
fn listen() -> Command<Message> {
    let inbox = INBOX.with_borrow(|inbox| inbox.as_ref().map(|(_, r)| r.clone()));
    let inbox = inbox.expect("the test app started");
    Command::Perform(Box::pin(async move {
        let message = inbox.recv().await.unwrap_or(Message::Unheard);
        INBOXED.set(true);
        message
    }))
}

fn log(entry: String) {
    HOST.with_borrow_mut(|log| log.push(entry));
}

/// What the test host was asked to do, less the previews it was asked for.
fn launches() -> Vec<String> {
    HOST.with_borrow(|log| {
        log.iter()
            .filter(|entry| !entry.starts_with("preview "))
            .cloned()
            .collect()
    })
}

/// Records what the model asks of the session; its clock stands still.
struct TestHost;

impl Host for TestHost {
    fn run(&mut self, spec: CommandSpec, token: Option<String>) {
        log(format!(
            "run {:?} {:?} token={}",
            spec.desktop_id,
            std::iter::once(spec.program.as_str())
                .chain(spec.args.iter().map(String::as_str))
                .collect::<Vec<_>>(),
            token.is_some()
        ));
    }

    fn open(&mut self, target: &str) {
        log(format!("open {target}"));
    }

    fn index(&mut self, _: FileSearchConfig) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn resize(&mut self, size: Size) {
        EXTENT.set(Some(size));
    }

    fn frame(&mut self, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn icon(&mut self, _: &ResultItem) -> Vec<ImageSource> {
        Vec::new()
    }

    fn preview(&mut self, item: &ResultItem) -> Vec<Command<Message>> {
        log(format!("preview {}", item.id));
        Vec::new()
    }

    fn search(&mut self, request: SearchRequest) -> Vec<Command<Message>> {
        let held = HELD.with_borrow_mut(|held| match held {
            Some(held) => {
                held.push(request.clone());
                true
            }
            None => false,
        });
        if held {
            return Vec::new();
        }
        let update = request.run();
        vec![Command::Perform(Box::pin(async move {
            Message::Provided(update)
        }))]
    }

    fn now(&self) -> Now {
        Now {
            mono: 0.0,
            unix: 1_000_000.0,
        }
    }

    fn bring_forward(&mut self, desktop_id: &str, launch: u64) -> bool {
        if !FORWARDS.get() {
            return false;
        }
        log(format!("bring forward {desktop_id}"));
        ASKED.set(Some(launch));
        true
    }

    fn ask_for_window(
        &mut self,
        bus_name: &str,
        _: Option<&std::path::Path>,
        token: Option<String>,
    ) -> Option<kit::BusCall> {
        let answer = BUS_ANSWER.get()?;
        log(format!("ask {bus_name} token={}", token.is_some()));
        Some(Box::pin(async move { answer }))
    }
}

/// The launcher on the test entries, shown at once with its appear animation settled.
struct TestApp;

impl Application for TestApp {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        HOST.with_borrow_mut(Vec::clear);
        EXTENT.set(None);
        HELD.set(None);
        FORWARDS.set(false);
        ASKED.set(None);
        BUS_ANSWER.set(None);
        INBOXED.set(false);
        let mut model = Model::new(test_launcher(), Box::new(TestHost));
        model.settled = true;
        INBOX.set(Some(async_channel::unbounded()));
        let mut commands = kit::update(&mut model, Message::Toggle);
        commands.push(listen());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        // what a test delivers comes from the inbox, which is listened to again
        let inbox = INBOXED.replace(false);
        let mut commands = kit::update(model, message);
        if inbox {
            commands.push(listen());
        }
        commands
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        kit::view(model)
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

fn theme() -> ThemeTokens {
    theme_in(Appearance::Light)
}

fn theme_in(appearance: Appearance) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

struct Run {
    driver: TestDriver<TestApp>,
    configured: BTreeSet<WindowId>,
    /// The height the panel window was last configured at.
    extent: Option<Size>,
    /// Each input and what it left, for the keyboard golden.
    trace: String,
    /// A screen reader following the windows.
    reader: AssistiveTechnology,
    /// The windows are layer surfaces, active while they have the keyboard rather than by their configures' state.
    layers: bool,
}

fn active() -> WindowStates {
    WindowStates {
        activated: true,
        ..WindowStates::default()
    }
}

impl Run {
    fn new() -> Run {
        Run::with(Appearance::Light, false)
    }

    /// A run in `appearance`, on a desktop that draws the panel's glass when `app_effects`.
    fn with(appearance: Appearance, app_effects: bool) -> Run {
        Run::build(appearance, app_effects, false)
    }

    /// A run whose windows are layer surfaces, as on the desktop: their configures carry no activated state, and the
    /// panel takes the keyboard once it is mapped.
    fn on_layer_surfaces() -> Run {
        let mut run = Run::build(Appearance::Light, false, true);
        let panel = run.panel();
        run.send(panel, WindowEvent::Focus(true));
        run
    }

    fn build(appearance: Appearance, app_effects: bool, layers: bool) -> Run {
        let reader = AssistiveTechnology::new();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme_in(appearance)),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 1000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 22,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(reader.adapter()),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let driver = TestDriver::new(
            Capabilities {
                app_effects,
                ..Capabilities::default()
            },
            options,
            ManualClock::new(Timestamp(0)),
        );
        let mut run = Run {
            driver,
            configured: BTreeSet::new(),
            extent: None,
            trace: String::new(),
            reader,
            layers,
        };
        run.settle();
        run
    }

    fn window(&self, key: &'static str) -> Option<WindowId> {
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| *k == WindowKey(Key::from(key)))
            .map(|(_, id)| id)
    }

    /// The window kept mapped while the panel is hidden, of the model's current generation.
    fn hidden(&self) -> Option<WindowId> {
        let key = WindowKey(Key::from(format!("hidden-{}", self.model().hidden)));
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, id)| id)
    }

    /// Delivers `message` to the model as the session would, and settles.
    fn deliver(&mut self, message: Message) {
        INBOX.with_borrow(|inbox| inbox.as_ref().unwrap().0.try_send(message).unwrap());
        self.settle();
    }

    fn panel(&self) -> WindowId {
        self.window("panel").expect("the panel is shown")
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Launcher");
    }

    /// Runs until no window waits for a configure or a frame. New windows are configured as the server would: the
    /// panel at the height the model gave it, the catcher over the output; a new height configures the panel again.
    fn settle_frames(&mut self) {
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let mut busy = false;
            for (key, id) in self.driver.runtime().windows() {
                let panel = key == WindowKey(Key::from("panel"));
                let catcher = key == WindowKey(Key::from("catcher"));
                let size = match () {
                    () if panel => {
                        let extent =
                            EXTENT.with_borrow(|e| e.unwrap_or(window_extent(compact_shape())));
                        if self.configured.contains(&id) && self.extent == Some(extent) {
                            continue;
                        }
                        self.extent = Some(extent);
                        Some(extent)
                    }
                    _ if self.configured.contains(&id) => continue,
                    () if catcher => Some(OUTPUT),
                    _ => None,
                };
                self.configured.insert(id);
                let system = self.driver.window_system_mut();
                if self.layers {
                    system.activate_by_keyboard(id).unwrap();
                }
                system.configure(id, size, active()).unwrap();
                busy = true;
            }
            self.driver.run_until_idle(budget()).unwrap();
            for (_, id) in self.driver.runtime().windows() {
                let system = self.driver.window_system_mut();
                if system.frame_requested(id).unwrap() {
                    system.frame_ready(id, Timestamp(0)).unwrap();
                    busy = true;
                }
            }
            if !busy {
                return;
            }
        }
        panic!("the launcher keeps asking for frames");
    }

    /// Sends `event` to `window`; nothing when the window has closed meanwhile (a key release after the press that
    /// closed the panel).
    fn send(&mut self, window: WindowId, event: WindowEvent) {
        if !self.driver.window_system_mut().windows().contains(&window) {
            return;
        }
        self.driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
        self.settle();
    }

    /// Presses and releases a key on the panel with `mods` held, and notes what it left in the trace.
    fn chord(&mut self, name: &str, code: u32, keysym: u32, text: Option<char>, mods: Mods) {
        let window = self.panel();
        for pressed in [true, false] {
            self.send(
                window,
                WindowEvent::Key(
                    KeyEvent {
                        code,
                        keysym: Keysym(keysym),
                        text: text.filter(|_| pressed),
                        pressed,
                        repeat: false,
                        mods,
                    },
                    Timestamp(0),
                ),
            );
        }
        let note = self.note();
        let _ = writeln!(self.trace, "> {name}\n  {note}");
    }

    fn key(&mut self, name: &str, code: u32, keysym: u32) {
        self.chord(name, code, keysym, None, Mods::NONE);
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.chord(&format!("{c:?}"), 0, c as u32, Some(c), Mods::NONE);
        }
    }

    /// The query, the keyboard's owner and the selected result.
    fn note(&self) -> String {
        let model = self.model();
        let panel = &model.panel;
        if !panel.is_visible() {
            return "closed".into();
        }
        if let Some(b) = model
            .browser
            .as_ref()
            .filter(|_| model.nav.view == View::Browse)
        {
            return format!(
                "browse query={:?} zone={:?} selected={:?} category={:?} {:?} nested={:?}",
                b.query(),
                b.zone(),
                b.selected_app().map(|a| a.name.as_str()),
                b.category().map(|c| c.name()),
                b.arrangement(),
                model.nav.nested,
            );
        }
        format!(
            "query={:?} focus={:?} selected={:?} scroll={}",
            panel.query(),
            panel.focus(),
            panel.selected_item().map(|i| i.title.as_str()),
            panel.scroll_row()
        )
    }

    fn click(&mut self, window: WindowId, position: Point) {
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.send(window, pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            self.send(
                window,
                pointer(PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    serial: 1,
                    mods: Mods::NONE,
                }),
            );
        }
    }

    fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    fn snapshot(&self) -> FrameSnapshot {
        self.driver.runtime().snapshot(self.panel()).unwrap()
    }

    /// The centre of the node `key` names in the panel.
    fn centre(&self, key: impl Into<Key>) -> Point {
        let runtime = self.driver.runtime();
        let id = runtime.find(self.panel(), &key.into()).unwrap();
        let rect = self
            .snapshot()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .rect;
        Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        )
    }

    /// The trace, then the panel's Inspector dump.
    fn dump(&self) -> String {
        let dump = Inspector::default().dump(&self.snapshot());
        if self.trace.is_empty() {
            dump
        } else {
            format!("{}\n{dump}", self.trace)
        }
    }

    /// What a screen reader reads of the panel, after the trace.
    fn reading(&self) -> String {
        let mut reading = self.trace.clone();
        if !reading.is_empty() {
            reading.push('\n');
        }
        reading.push_str(&reading_of(&self.snapshot()));
        reading
    }

    /// Compares the panel's dump with its golden and its reading with its screen-reader fixture.
    fn fixtures(&self, name: &str) {
        let recorded = [
            compare(&golden_path(name), &renumbered(&self.dump())),
            compare(&reader_path(name), &self.reading()),
        ];
        if recorded.contains(&true) {
            panic!("{name}: fixtures re-recorded; run again without CUPERTINO_UPDATE_GOLDENS");
        }
    }
}

/// `dump` with its image resources numbered by first appearance: the ids come from a counter of the whole process,
/// which the tests running beside each other share.
fn renumbered(dump: &str) -> String {
    const MARK: &str = "ResourceId(";
    let mut seen: Vec<String> = Vec::new();
    let mut out = String::with_capacity(dump.len());
    let mut rest = dump;
    while let Some(at) = rest.find(MARK) {
        let (before, after) = rest.split_at(at + MARK.len());
        out.push_str(before);
        let end = after.find(')').unwrap_or(after.len());
        let id = &after[..end];
        let n = seen.iter().position(|s| s == id).unwrap_or_else(|| {
            seen.push(id.to_owned());
            seen.len() - 1
        });
        let _ = write!(out, "#{n}");
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

fn golden_path(name: &str) -> String {
    format!("{}/tests/goldens/{name}.txt", env!("CARGO_MANIFEST_DIR"))
}

fn reader_path(name: &str) -> String {
    format!(
        "{}/tests/fixtures/screen-reader/{name}.txt",
        env!("CARGO_MANIFEST_DIR")
    )
}

/// Compares `actual` with the file at `path`; with `CUPERTINO_UPDATE_GOLDENS=1` writes it there instead and returns
/// `true`.
fn compare(path: &str, actual: &str) -> bool {
    if std::env::var("CUPERTINO_UPDATE_GOLDENS").as_deref() == Ok("1") {
        std::fs::create_dir_all(std::path::Path::new(path).parent().unwrap()).unwrap();
        std::fs::write(path, actual).unwrap();
        return true;
    }
    let expected = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("{path}: {e} (record it with CUPERTINO_UPDATE_GOLDENS=1)"));
    if let Some(difference) = Inspector::first_difference(&expected, actual) {
        panic!("{path} differs: {difference}");
    }
    false
}

/// What a screen reader reads of the window, in reading order: each object's role, name, description, value and
/// states, indented by its depth, after the object that has the focus.
fn reading_of(snapshot: &FrameSnapshot) -> String {
    let nodes: std::collections::HashMap<NodeId, &AccessibilityNode> = snapshot
        .accessibility
        .iter()
        .map(|(id, node)| (*id, node))
        .collect();
    let describe = |node: &AccessibilityNode| {
        let mut line = format!("{:?}", node.role);
        if let Some(name) = &node.name {
            let _ = write!(line, " {name:?}");
        }
        if let Some(description) = &node.description {
            let _ = write!(line, ", {description:?}");
        }
        match &node.value {
            Some(cupertino_kit::core::AccessibleValue::Text { value, .. }) => {
                let _ = write!(line, " value={value:?}");
            }
            Some(other) => {
                let _ = write!(line, " value={other:?}");
            }
            None => {}
        }
        if let Some(count) = node.item_count {
            let _ = write!(line, " items={count}");
        }
        if let Some(index) = node.item_index {
            let _ = write!(line, " item={}", index + 1);
        }
        let s = node.states;
        let states: Vec<&str> = [
            (s.focused, "focused"),
            (s.selected, "selected"),
            (s.disabled, "disabled"),
        ]
        .into_iter()
        .filter_map(|(on, name)| on.then_some(name))
        .collect();
        if !states.is_empty() {
            let _ = write!(line, " [{}]", states.join(", "));
        }
        if !node.actions.is_empty() {
            let _ = write!(line, " actions={:?}", node.actions);
        }
        line
    };
    let mut out = String::new();
    let focus = snapshot
        .focus
        .and_then(|id| nodes.get(&id))
        .map_or_else(|| "none".to_owned(), |node| describe(node));
    let _ = writeln!(out, "focus: {focus}");
    fn walk(
        out: &mut String,
        nodes: &std::collections::HashMap<NodeId, &AccessibilityNode>,
        id: NodeId,
        depth: usize,
        describe: &dyn Fn(&AccessibilityNode) -> String,
    ) {
        let Some(node) = nodes.get(&id) else { return };
        let _ = writeln!(out, "{}{}", "  ".repeat(depth), describe(node));
        for &child in &node.children {
            walk(out, nodes, child, depth + 1, describe);
        }
    }
    // the root is no node's child (the snapshot lists the nodes in no particular order)
    let children: BTreeSet<NodeId> = nodes
        .values()
        .flat_map(|n| n.children.iter().copied())
        .collect();
    let mut roots: Vec<NodeId> = nodes
        .keys()
        .copied()
        .filter(|id| !children.contains(id))
        .collect();
    roots.sort();
    for root in roots {
        walk(&mut out, &nodes, root, 0, &describe);
    }
    out
}

const BACKSPACE_CODE: u32 = 14;
const TAB_CODE: u32 = 15;
const RETURN_CODE: u32 = 28;
const ESCAPE_CODE: u32 = 1;
const UP_CODE: u32 = 103;
const DOWN_CODE: u32 = 108;
const RIGHT_CODE: u32 = 106;

#[test]
fn the_search_field_takes_the_keyboard_when_the_panel_opens() {
    let run = Run::new();
    let model = run.model();
    assert!(model.panel.is_visible());
    let snapshot = run.snapshot();
    let field = snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.role == Role::TextInput)
        .expect("the query field is in the tree");
    assert_eq!(field.1.name.as_deref(), Some(PLACEHOLDER));
    assert_eq!(snapshot.focus, Some(field.0), "the field has the keyboard");
    assert!(run.window("catcher").is_some());
    assert!(run.hidden().is_none());
    run.fixtures("search-field");
}

#[test]
fn typing_lists_the_ranked_results_in_their_sections() {
    let mut run = Run::new();
    run.type_text("fire");
    let model = run.model();
    assert_eq!(model.panel.query(), "fire");
    assert_eq!(
        model.panel.selected_item().map(|i| i.title.as_str()),
        Some("Firefox")
    );
    let shape = search_shape(&model.panel, &model.status(), None);
    assert_eq!(shape.height, on_shape_grid(panel_height(&model.panel)));
    assert_eq!(
        run.extent,
        Some(window_extent(shape)),
        "the panel window takes the height of its rows"
    );
    assert!(shape.height >= MIN_EXPANDED_HEIGHT);
    let snapshot = run.snapshot();
    let mut items: Vec<(Option<usize>, Option<&str>, bool)> = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::ListItem)
        .map(|(_, n)| (n.item_index, n.name.as_deref(), n.states.selected))
        .collect();
    items.sort();
    let items: Vec<(Option<&str>, bool)> = items.into_iter().map(|(_, n, s)| (n, s)).collect();
    assert_eq!(items[0], (Some("Firefox"), true), "{items:?}");
    assert!(items[1..].iter().all(|(_, selected)| !selected));
    run.fixtures("results-list");
}

#[test]
fn the_keyboard_moves_through_the_results_and_back_to_the_field() {
    let mut run = Run::new();
    run.type_text("fire");
    run.trace.clear();
    run.key("Down", DOWN_CODE, DOWN);
    run.key("Down", DOWN_CODE, DOWN);
    assert_eq!(run.model().panel.focus(), Focus::List);
    run.chord("Super+Up", UP_CODE, UP, None, Mods::PRIMARY);
    run.chord("Super+Down", DOWN_CODE, DOWN, None, Mods::PRIMARY);
    run.key("Up", UP_CODE, UP);
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert_eq!(run.model().panel.focus(), Focus::Field);
    run.key("Tab", TAB_CODE, TAB);
    run.key("Backspace", BACKSPACE_CODE, BACKSPACE);
    // the field still has the keyboard: the panel's keys never moved Kit's focus
    let snapshot = run.snapshot();
    let focused = snapshot
        .focus
        .and_then(|id| snapshot.accessibility.iter().find(|(n, _)| *n == id))
        .map(|(_, n)| n.role);
    assert_eq!(focused, Some(Role::TextInput));
    run.fixtures("keyboard-navigation");
}

#[test]
fn return_launches_the_selection_with_its_token_and_closes() {
    let mut run = Run::new();
    run.type_text("fire");
    run.key("Return", RETURN_CODE, RETURN);
    assert!(!run.model().panel.is_visible());
    assert!(run.window("panel").is_none() && run.window("catcher").is_none());
    assert!(run.hidden().is_some(), "the hidden window stays mapped");
    let host = launches();
    assert_eq!(host.len(), 1, "{host:?}");
    assert!(
        host[0].starts_with("run Some(\"browser.desktop\") [\"firefox\"]"),
        "{host:?}"
    );
}

impl Run {
    /// The server answers `running` for the application it was last asked to bring forward.
    fn answer(&mut self, running: Running) {
        let launch = ASKED.take().expect("the server was asked");
        self.deliver(Message::Opened(launch, running));
    }
}

#[test]
fn an_application_that_runs_comes_forward_from_the_search_and_the_browser_without_a_second_copy() {
    let mut run = Run::new();
    FORWARDS.set(true);
    run.type_text("fire");
    run.key("Return", RETURN_CODE, RETURN);
    assert!(!run.model().panel.is_visible(), "the panel closes at once");
    assert_eq!(launches(), ["bring forward browser.desktop"]);
    run.answer(Running::Raised);
    assert_eq!(
        launches(),
        ["bring forward browser.desktop"],
        "nothing is started"
    );

    // the panel is hidden already: the Dock's request opens the browser
    run.deliver(Message::Browse);
    run.key("Down", DOWN_CODE, DOWN);
    // the recents row leads with the application just opened
    assert_eq!(run.selected(Role::Button), ["Firefox"]);
    run.key("Return", RETURN_CODE, RETURN);
    assert!(run.window("panel").is_none());
    run.answer(Running::Raised);
    assert_eq!(
        launches(),
        [
            "bring forward browser.desktop",
            "bring forward browser.desktop"
        ],
        "nothing is started"
    );
}

#[test]
fn an_application_that_does_not_run_starts_with_its_token_once_the_server_says_so() {
    let mut run = Run::new();
    FORWARDS.set(true);
    run.type_text("fire");
    run.key("Return", RETURN_CODE, RETURN);
    assert_eq!(launches(), ["bring forward browser.desktop"]);
    run.answer(Running::NotRunning);
    let host = launches();
    assert_eq!(host.len(), 2, "{host:?}");
    assert!(
        host[1].starts_with("run Some(\"browser.desktop\") [\"firefox\"]")
            && host[1].ends_with("token=true"),
        "{host:?}"
    );
    // a late or repeated answer starts nothing more
    run.deliver(Message::Opened(0, Running::NotRunning));
    assert_eq!(launches().len(), 2);
}

#[test]
fn a_resident_application_is_asked_for_a_window_over_the_bus_and_started_again_when_it_cannot_be() {
    // D-Bus activatable and answering: asked, not started
    let mut run = Run::new();
    FORWARDS.set(true);
    BUS_ANSWER.set(Some(true));
    run.type_text("fire");
    run.key("Return", RETURN_CODE, RETURN);
    run.answer(Running::Resident);
    assert_eq!(
        launches(),
        ["bring forward browser.desktop", "ask browser token=true"]
    );

    // D-Bus activatable but not answering: started again with the token
    let mut run = Run::new();
    FORWARDS.set(true);
    BUS_ANSWER.set(Some(false));
    run.type_text("fire");
    run.key("Return", RETURN_CODE, RETURN);
    run.answer(Running::Resident);
    let host = launches();
    assert_eq!(host.len(), 3, "{host:?}");
    assert_eq!(host[1], "ask browser token=true");
    assert!(
        host[2].starts_with("run Some(\"browser.desktop\")") && host[2].ends_with("token=true")
    );

    // not D-Bus activatable: started again, and a running instance opens a window
    let mut run = Run::new();
    FORWARDS.set(true);
    run.type_text("fire");
    run.key("Return", RETURN_CODE, RETURN);
    run.answer(Running::Resident);
    let host = launches();
    assert_eq!(host.len(), 2, "{host:?}");
    assert!(host[1].starts_with("run Some(\"browser.desktop\")"));
}

#[test]
fn a_desktop_action_runs_its_command_without_asking_the_server() {
    let mut run = Run::new();
    FORWARDS.set(true);
    run.type_text("fire");
    run.key("Tab", TAB_CODE, TAB);
    run.key("Tab", TAB_CODE, TAB);
    run.key("Return", RETURN_CODE, RETURN);
    let host = launches();
    assert_eq!(host.len(), 1, "{host:?}");
    assert!(host[0].contains("--private-window"), "{host:?}");
}

#[test]
fn escape_clears_the_query_and_then_closes() {
    let mut run = Run::new();
    run.type_text("fire");
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert_eq!(run.model().panel.query(), "");
    assert!(run.model().panel.is_visible());
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert!(!run.model().panel.is_visible());
    assert!(run.hidden().is_some());
}

#[test]
fn hover_selects_a_row_and_a_click_activates_it() {
    let mut run = Run::new();
    run.type_text("fire");
    let row = run.centre("item:app:firewall.desktop");
    let panel = run.panel();
    run.send(
        panel,
        WindowEvent::Pointer(PointerEvent::Motion { position: row }, Timestamp(0)),
    );
    assert_eq!(
        run.model().panel.selected_item().map(|i| i.title.as_str()),
        Some("Firewall Configuration")
    );
    run.click(panel, row);
    assert!(!run.model().panel.is_visible());
    let host = launches();
    assert!(
        host.iter().any(|h| h.contains("firewall-config")),
        "{host:?}"
    );
}

#[test]
fn a_screen_reader_s_click_on_a_result_launches_it_and_closes_the_panel() {
    let mut run = Run::new();
    run.type_text("fire");
    let tree = run
        .reader
        .tree(run.panel())
        .expect("the reader holds the panel");
    let row = tree
        .nodes
        .iter()
        .find(|(_, n)| {
            n.role == Role::ListItem && n.name.as_deref() == Some("Firewall Configuration")
        })
        .map(|(id, _)| *id)
        .expect("the reader reads the result");
    run.driver
        .ask(&run.reader, row, cupertino_kit::core::Action::Click, None);
    run.settle();
    assert!(!run.model().panel.is_visible());
    let host = launches();
    assert!(
        host.iter().any(|h| h.contains("firewall-config")),
        "{host:?}"
    );
}

#[test]
fn a_press_outside_the_panel_closes_it() {
    let mut run = Run::new();
    let catcher = run.window("catcher").unwrap();
    // the catcher over the output is one transparent colour, shown without a frame of the output's size, and all of
    // it takes the pointer
    let system = run.driver.runtime().window_system();
    assert!(system.frames(catcher).unwrap().is_empty());
    let solids = system.solids(catcher).unwrap();
    assert_eq!(solids.len(), 1);
    assert_eq!(solids[0].color.a, 0.0);
    assert_eq!(system.spec(catcher).unwrap().input_region, None);
    run.click(catcher, Point::new(10.0, 650.0));
    assert!(!run.model().panel.is_visible());
    assert!(run.hidden().is_some());
}

#[test]
fn the_panel_closes_when_its_window_loses_the_keyboard() {
    let mut run = Run::new();
    let panel = run.panel();
    // the keyboard's leave, as the Wayland window system reports it
    run.send(panel, WindowEvent::Focus(false));
    assert!(!run.model().panel.is_visible());
    assert!(run.hidden().is_some());
}

#[test]
fn the_calculator_result_goes_on_the_clipboard() {
    let mut run = Run::new();
    run.type_text("6*7");
    assert_eq!(
        run.model().panel.selected_item().map(|i| i.title.as_str()),
        Some("42")
    );
    run.key("Return", RETURN_CODE, RETURN);
    assert!(!run.model().panel.is_visible());
    let clipboard = run
        .driver
        .window_system_mut()
        .clipboard(cupertino_kit::platform::ClipboardKind::Clipboard)
        .map(|data| data.bytes.clone());
    assert_eq!(clipboard.as_deref(), Some(&b"42"[..]));
}

#[test]
fn a_panel_window_the_server_closes_hides_the_panel() {
    let mut run = Run::new();
    run.type_text("fire");
    let panel = run.panel();
    run.send(panel, WindowEvent::CloseRequested);
    assert!(!run.model().panel.is_visible());
    assert!(run.window("panel").is_none());
    assert!(run.hidden().is_some());
}

#[test]
fn a_hidden_window_the_server_closes_is_opened_again() {
    let mut run = Run::new();
    let panel = run.panel();
    run.send(panel, WindowEvent::CloseRequested);
    let hidden = run.hidden().expect("the hidden window is mapped");
    // its output went away
    run.send(hidden, WindowEvent::CloseRequested);
    assert!(!run.driver.runtime().exited(), "the launcher keeps running");
    let again = run.hidden().expect("a new hidden window is mapped");
    assert_ne!(again, hidden);
    assert!(!run.driver.window_system_mut().windows().contains(&hidden));
    // and the panel still opens
    run.deliver(Message::Toggle);
    assert!(run.model().panel.is_visible());
}

#[test]
fn locking_the_session_closes_the_panel() {
    let mut run = Run::new();
    run.type_text("fire");
    run.deliver(Message::Locked);
    assert!(!run.model().panel.is_visible());
    assert!(run.window("panel").is_none() && run.window("catcher").is_none());
    assert!(run.hidden().is_some());
    // a lock while hidden changes nothing
    run.deliver(Message::Locked);
    assert!(!run.model().panel.is_visible());
}

fn key_event(keysym: u32, text: Option<char>, mods: Mods) -> KeyEvent {
    KeyEvent {
        code: 0,
        keysym: Keysym(keysym),
        text,
        pressed: true,
        repeat: false,
        mods,
    }
}

#[test]
fn named_keys_keep_their_modifiers() {
    assert_eq!(
        panel_key(&key_event(RETURN, None, Mods::NONE)),
        Some((PanelKey::Enter, PanelMods::NONE))
    );
    assert_eq!(
        panel_key(&key_event(LEFT_TAB, None, Mods::SHIFT)),
        Some((
            PanelKey::Tab,
            PanelMods {
                shift: true,
                ..PanelMods::NONE
            }
        ))
    );
    assert_eq!(
        panel_key(&key_event(PAGE_DOWN, None, Mods::NONE)),
        Some((PanelKey::PageDown, PanelMods::NONE))
    );
    let release = KeyEvent {
        pressed: false,
        ..key_event(RETURN, None, Mods::NONE)
    };
    assert_eq!(panel_key(&release), None, "only presses are keys");
}

#[test]
fn typing_uses_the_produced_character_and_chords_the_lower_case_letter() {
    assert_eq!(
        panel_key(&key_event('a' as u32, Some('a'), Mods::NONE)),
        Some((PanelKey::Char('a'), PanelMods::NONE))
    );
    assert_eq!(
        panel_key(&key_event('A' as u32, Some('A'), Mods::SHIFT)),
        Some((
            PanelKey::Char('A'),
            PanelMods {
                shift: true,
                ..PanelMods::NONE
            }
        ))
    );
    let ctrl_shift = Mods::CTRL.union(Mods::SHIFT);
    assert_eq!(
        panel_key(&key_event('A' as u32, Some('A'), ctrl_shift)),
        Some((
            PanelKey::Char('a'),
            PanelMods {
                ctrl: true,
                shift: true,
                ..PanelMods::NONE
            }
        ))
    );
    assert_eq!(
        panel_key(&key_event('1' as u32, Some('1'), Mods::ALT)),
        Some((PanelKey::Char('1'), PanelMods::ALT))
    );
    assert_eq!(
        panel_key(&key_event('y' as u32, None, Mods::PRIMARY)),
        Some((PanelKey::Char('y'), PanelMods::PRIMARY))
    );
    // F5 types nothing and has no panel name
    assert_eq!(panel_key(&key_event(0xffc2, None, Mods::NONE)), None);
}

#[test]
fn the_panel_is_collapsed_without_results_and_expanded_with_them() {
    let launcher = test_launcher();
    let mut panel = Panel::new();
    let now = Now {
        mono: 0.0,
        unix: 1_000_000.0,
    };
    panel.show(&launcher, now);
    assert!(
        panel.results().is_empty(),
        "no launch history, no suggestions"
    );
    assert_eq!(panel_height(&panel), COLLAPSED_HEIGHT);
    panel.set_query("e", &launcher, now);
    assert!(visible_rows(&panel).len() <= MAX_ROWS);
    assert!(panel_height(&panel) >= MIN_EXPANDED_HEIGHT);
    assert_eq!(
        placement(OUTPUT, 430.0),
        Rect::new(180.0, 140.0, WIDTH, 430.0)
    );
    // the window keeps room for the shadow around the panel, which alone takes input
    let shape = PanelShape::new(WIDTH, 430.0, CORNER_RADIUS);
    let window = window_extent(shape);
    let frame = Frame {
        shape,
        alpha: 1.0,
        scale: 1.0,
        content: 0.0,
        header: Header::Search,
    };
    let panel = enclosure_rect(window, &frame);
    assert_eq!(
        (window.width - panel.width(), window.height - panel.height()),
        (2.0 * SHADOW_SIDE, SHADOW_TOP + SHADOW_BOTTOM)
    );
    assert_eq!(panel.origin, Point::new(SHADOW_SIDE, SHADOW_TOP));
    // scaled as it opens, about its top centre
    let opening = enclosure_rect(
        window,
        &Frame {
            scale: 0.5,
            ..frame
        },
    );
    assert_eq!(opening.min_y(), SHADOW_TOP);
    assert_eq!(opening.center().x, panel.center().x);
    assert_eq!(opening.width(), WIDTH / 2.0);
}

/// A file index of a folder holding `notes.txt`, and the folder.
fn notes_index() -> (tempfile::TempDir, Arc<crate::files::FileIndex>) {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "first line\nsecond line\n").unwrap();
    let index = crate::files::FileIndex::build(FileSearchConfig::new(dir.path()));
    (dir, Arc::new(index))
}

impl Run {
    /// Asks the screen reader to press the object of `role` named `name` in the panel.
    fn press(&mut self, role: Role, name: &str) {
        let tree = self
            .reader
            .tree(self.panel())
            .expect("the reader holds the panel");
        let node = tree
            .nodes
            .iter()
            .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
            .map(|(id, _)| *id)
            .unwrap_or_else(|| panic!("the reader reads {role:?} {name:?}"));
        self.driver
            .ask(&self.reader, node, cupertino_kit::core::Action::Click, None);
        self.settle();
    }

    /// The names of the panel's objects of `role`, by their item index.
    fn names(&self, role: Role) -> Vec<String> {
        let mut named: Vec<(Option<usize>, String)> = self
            .snapshot()
            .accessibility
            .iter()
            .filter(|(_, n)| n.role == role)
            .filter_map(|(_, n)| Some((n.item_index, n.name.clone()?)))
            .collect();
        named.sort();
        named.into_iter().map(|(_, name)| name).collect()
    }
}

#[test]
fn the_selected_file_shows_its_preview_once_the_host_reads_it() {
    let (_dir, index) = notes_index();
    let mut run = Run::new();
    run.deliver(Message::Indexed(index));
    run.type_text("notes");
    let item = run
        .model()
        .panel
        .selected_item()
        .cloned()
        .expect("a result");
    assert!(item.id.starts_with("file:"), "{item:?}");
    assert!(
        HOST.with_borrow(|log| log.contains(&format!("preview {}", item.id))),
        "the host is asked for the selected file's preview"
    );
    // until it arrives the pane shows the result itself and its actions
    assert_eq!(
        run.names(Role::Button),
        ["Open", "Reveal in Files", "Copy Path"]
    );
    run.deliver(Message::Previewed(
        item.id.clone(),
        Arc::new(Preview {
            body: PreviewBody::Text("first line\nsecond line".into()),
            icon: Vec::new(),
            kind: "Plain text".into(),
            details: vec![("Size".into(), "23 bytes".into())],
        }),
    ));
    let texts = run.names(Role::StaticText);
    for expected in [
        "notes.txt",
        "Plain text",
        "Size",
        "23 bytes",
        "first line\nsecond line",
    ] {
        assert!(
            texts.iter().any(|t| t == expected),
            "{expected:?} in {texts:?}"
        );
    }
    // the pane takes the width the list leaves, to the panel's trailing edge, and centres the name in it
    let bounds = |role: Role, name: &str| {
        run.snapshot()
            .accessibility
            .iter()
            .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
            .map(|(_, n)| n.bounds)
            .unwrap_or_else(|| panic!("{role:?} {name:?}"))
    };
    let pane = bounds(Role::Group, "Preview");
    let edge = enclosure_rect(run.model().extent(), &run.model().presented()).max_x();
    assert!((pane.max_x() - edge).abs() < 1.0, "{pane:?} ends at {edge}");
    assert!(pane.width() > WIDTH - LIST_WIDTH - 2.0, "{pane:?}");
    let name = bounds(Role::StaticText, "notes.txt");
    assert!(
        ((name.min_x() + name.max_x()) / 2.0 - (pane.min_x() + pane.max_x()) / 2.0).abs() < 1.0,
        "{name:?} is centred in {pane:?}"
    );
    // a detail's label ends short of the pane's middle and its value starts past it
    let middle = (pane.min_x() + pane.max_x()) / 2.0;
    assert!(bounds(Role::StaticText, "Size").max_x() <= middle);
    assert!(bounds(Role::StaticText, "23 bytes").min_x() >= middle);
    assert!(run.names(Role::Group).iter().any(|g| g == "Preview"));
    // a preview of a result no longer selected is dropped
    run.deliver(Message::Previewed(
        "file:/elsewhere".into(),
        Arc::new(Preview {
            body: PreviewBody::Icon,
            icon: Vec::new(),
            kind: "Elsewhere".into(),
            details: Vec::new(),
        }),
    ));
    assert!(!run.names(Role::StaticText).iter().any(|t| t == "Elsewhere"));
}

#[test]
fn reveal_in_files_opens_the_folder_holding_the_file_and_closes() {
    let (dir, index) = notes_index();
    let mut run = Run::new();
    run.deliver(Message::Indexed(index));
    run.type_text("notes");
    run.press(Role::Button, "Reveal in Files");
    assert!(!run.model().panel.is_visible());
    let folder = dir.path().to_string_lossy().into_owned();
    // the test entries have no file manager of the session: the folder opens with the open command
    assert_eq!(
        launches(),
        [format!("run None [\"xdg-open\", {folder:?}] token=true")]
    );
}

#[test]
fn the_arrows_choose_an_action_and_return_carries_it_out() {
    let mut run = Run::new();
    run.type_text("fire");
    run.key("Down", DOWN_CODE, DOWN);
    run.key("Up", UP_CODE, UP);
    assert_eq!(run.model().panel.focus(), Focus::List);
    run.key("Right", RIGHT_CODE, RIGHT);
    run.key("Right", RIGHT_CODE, RIGHT);
    let mut buttons: Vec<(Option<usize>, String, bool)> = run
        .snapshot()
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::Button)
        .map(|(_, n)| {
            (
                n.item_index,
                n.name.clone().unwrap_or_default(),
                n.states.selected,
            )
        })
        .collect();
    buttons.sort();
    let chosen: Vec<(String, bool)> = buttons.into_iter().map(|(_, n, s)| (n, s)).collect();
    assert_eq!(
        chosen,
        [
            ("Open".into(), false),
            ("New Window".into(), false),
            ("New Private Window".into(), true)
        ]
    );
    run.key("Return", RETURN_CODE, RETURN);
    let host = launches();
    assert!(
        host.len() == 1 && host[0].contains("--private-window"),
        "{host:?}"
    );
}

#[test]
fn a_conversion_lists_its_value_and_copies_it() {
    let mut run = Run::new();
    run.type_text("10 km in mi");
    let item = run
        .model()
        .panel
        .selected_item()
        .cloned()
        .expect("a result");
    assert_eq!(item.kind, ItemKind::Conversion);
    assert!(run.names(Role::Group).iter().any(|g| g == "Conversion"));
    assert!(run.names(Role::StaticText).contains(&item.title));
    assert_eq!(run.names(Role::Button), ["Copy"]);
    run.press(Role::Button, "Copy");
    assert!(!run.model().panel.is_visible());
}

const LEFT_CODE: u32 = 105;
const SPACE_CODE: u32 = 57;
const SPACE: u32 = 0x20;

/// The applications the browser lists, by name.
const ALL_APPS: [&str; 9] = [
    "Calculator",
    "Files",
    "Firefox",
    "Firewall Configuration",
    "GIMP",
    "Htop",
    "LibreOffice Writer",
    "Terminal",
    "Visual Studio Code",
];

impl Run {
    /// The query field's accessible name (its placeholder says which view it heads).
    fn field_name(&self) -> Option<String> {
        self.snapshot()
            .accessibility
            .iter()
            .find(|(_, n)| n.role == Role::TextInput)
            .and_then(|(_, n)| n.name.clone())
    }

    /// The names of the panel's objects of `role` that are selected.
    fn selected(&self, role: Role) -> Vec<String> {
        self.snapshot()
            .accessibility
            .iter()
            .filter(|(_, n)| n.role == role && n.states.selected)
            .filter_map(|(_, n)| n.name.clone())
            .collect()
    }

    /// What the panel announces as it changes (its live regions).
    fn announced(&self) -> Vec<String> {
        self.snapshot()
            .accessibility
            .iter()
            .filter(|(_, n)| n.live.is_some())
            .filter_map(|(_, n)| n.name.clone())
            .collect()
    }

    /// The query has stood for its settling time: the timer the host would have fired.
    fn settle_query(&mut self) {
        let change = self.model().query_change();
        self.deliver(Message::Settled(change));
    }

    /// The browser's filter has stood for its settling time: the timer the host would have fired.
    fn settle_browse(&mut self) {
        let change = self.model().browse_change();
        self.deliver(Message::BrowseSettled(change));
    }

    /// What a screen reader said since the last call, oldest first.
    fn said(&self) -> Vec<String> {
        self.reader
            .take_announcements()
            .into_iter()
            .map(|a| a.text)
            .collect()
    }

    /// Hides the panel, then asks for the applications as the Dock's hot corner does (through the server).
    fn browse_from_hidden(&mut self) {
        self.key("Escape", ESCAPE_CODE, ESCAPE);
        assert!(self.window("panel").is_none());
        self.deliver(Message::Browse);
    }
}

#[test]
fn the_dock_s_request_opens_the_application_browser_and_the_arrows_move_through_it() {
    let mut run = Run::new();
    run.browse_from_hidden();
    assert_eq!(run.model().nav.view, View::Browse);
    assert_eq!(run.field_name().as_deref(), Some("Applications"));
    assert_eq!(
        run.model().extent(),
        window_extent(browse_shape(BrowseLayout::HEIGHT)),
        "the window holds the browser and its shadow"
    );
    assert_eq!(run.names(Role::Button), ALL_APPS);
    assert_eq!(
        run.names(Role::CheckBox),
        [
            "Creativity",
            "Development",
            "Internet",
            "Other",
            "Utilities"
        ],
        "only the categories the applications are in, the uncategorised ones as Other"
    );
    assert!(
        run.selected(Role::Button).is_empty(),
        "it opens with nothing selected"
    );
    // the arrows that move along the query stay the field's until an application is selected
    run.key("Right", RIGHT_CODE, RIGHT);
    assert!(run.selected(Role::Button).is_empty());
    run.key("Down", DOWN_CODE, DOWN);
    assert_eq!(run.selected(Role::Button), ["Calculator"]);
    run.key("Right", RIGHT_CODE, RIGHT);
    assert_eq!(run.selected(Role::Button), ["Files"]);
    assert!(
        run.announced().contains(&"Files".to_owned()),
        "{:?}",
        run.announced()
    );
    run.key("Down", DOWN_CODE, DOWN);
    assert_eq!(
        run.selected(Role::Button),
        ["Visual Studio Code"],
        "a column down, the last row's nearest item"
    );
    run.key("Left", LEFT_CODE, LEFT);
    assert_eq!(run.selected(Role::Button), ["Terminal"]);
    // the field keeps the keyboard throughout
    let snapshot = run.snapshot();
    let focused = snapshot
        .focus
        .and_then(|id| snapshot.accessibility.iter().find(|(n, _)| *n == id))
        .map(|(_, n)| n.role);
    assert_eq!(focused, Some(Role::TextInput));
    run.fixtures("browser-grid");
    run.key("Return", RETURN_CODE, RETURN);
    assert!(
        run.window("panel").is_none(),
        "launching closes the browser"
    );
    let host = launches();
    assert_eq!(host.len(), 1, "{host:?}");
    assert!(
        host[0].starts_with("run Some(\"terminal.desktop\")") && host[0].ends_with("token=true"),
        "{host:?}"
    );
}

#[test]
fn typing_filters_the_applications_and_escape_folds_the_browser_before_it_closes() {
    let mut run = Run::new();
    run.browse_from_hidden();
    run.type_text("fire");
    let names = run.names(Role::Button);
    assert!(names.contains(&"Firefox".to_owned()), "{names:?}");
    assert!(!names.contains(&"GIMP".to_owned()), "{names:?}");
    run.type_text("zzz");
    assert!(run.names(Role::Button).is_empty());
    assert!(
        !run.announced().contains(&"No Results".to_owned()),
        "not while typing: {:?}",
        run.announced()
    );
    run.settle_browse();
    assert!(
        run.announced().contains(&"No Results".to_owned()),
        "{:?}",
        run.announced()
    );
    // Escape clears the query, then folds the browser into the search field, then closes
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert_eq!(run.model().panel.query(), "");
    assert_eq!(run.names(Role::Button), ALL_APPS);
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert_eq!(run.model().nav.view, View::Search);
    assert_eq!(run.field_name().as_deref(), Some("Search"));
    assert!(run.names(Role::Button).is_empty());
    assert_eq!(run.model().extent(), window_extent(compact_shape()));
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert!(run.window("panel").is_none());
    assert!(run.hidden().is_some());
}

#[test]
fn the_shortcut_unfolds_the_search_into_the_browser_keeping_the_query_and_back() {
    let mut run = Run::new();
    run.type_text("ter");
    run.chord("Super+1", 2, '1' as u32, Some('1'), Mods::PRIMARY);
    assert_eq!(run.model().nav.view, View::Browse);
    assert_eq!(run.model().panel.query(), "ter", "the query carries over");
    assert_eq!(run.names(Role::Button), ["Terminal"]);
    // the Dock's request while browsing closes it
    run.deliver(Message::Browse);
    assert!(run.window("panel").is_none());
}

#[test]
fn a_category_filter_narrows_the_applications_and_clearing_it_brings_them_back() {
    let mut run = Run::new();
    run.browse_from_hidden();
    run.press(Role::CheckBox, "Internet");
    assert_eq!(run.names(Role::Button), ["Firefox"]);
    let checked = |run: &Run, name: &str| {
        run.snapshot()
            .accessibility
            .iter()
            .find(|(_, n)| n.role == Role::CheckBox && n.name.as_deref() == Some(name))
            .and_then(|(_, n)| n.value.clone())
    };
    assert_eq!(
        checked(&run, "Internet"),
        Some(AccessibleValue::Toggle(Checked::True))
    );
    assert_eq!(
        checked(&run, "Creativity"),
        Some(AccessibleValue::Toggle(Checked::False))
    );
    // Escape clears the filter before it folds the browser
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert_eq!(run.model().nav.view, View::Browse);
    assert_eq!(run.names(Role::Button), ALL_APPS);
    run.press(Role::CheckBox, "Creativity");
    assert_eq!(run.names(Role::Button), ["GIMP"]);
    run.press(Role::CheckBox, "Creativity");
    assert_eq!(
        run.names(Role::Button),
        ALL_APPS,
        "pressing it again clears it"
    );
}

impl Run {
    /// The view options menu's popup, while it is open.
    fn options_menu(&self) -> Option<WindowId> {
        let key = options_menu_key();
        self.driver
            .runtime()
            .windows()
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, id)| id)
    }

    /// Presses and releases a key in the open view options menu, which holds the keyboard while it is open.
    fn menu_key(&mut self, name: &str, code: u32, keysym: u32) {
        let menu = self.options_menu().expect("the view options are open");
        for pressed in [true, false] {
            self.send(
                menu,
                WindowEvent::Key(
                    KeyEvent {
                        code,
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
        let note = self.note();
        let _ = writeln!(self.trace, "> {name} (menu)\n  {note}");
    }

    /// The rectangle of the node `key` names in `window`.
    fn rect_in(&self, window: WindowId, key: impl Into<Key>) -> Rect {
        let runtime = self.driver.runtime();
        let id = runtime.find(window, &key.into()).expect("the node");
        let r = runtime
            .snapshot(window)
            .unwrap()
            .nodes
            .iter()
            .find(|n| n.id == id)
            .unwrap()
            .rect;
        Rect::new(r.origin.x, r.origin.y, r.size.width, r.size.height)
    }

    /// Opens the view options from the keyboard: Tab to the filters, Tab to the button, Return.
    fn open_options_by_keys(&mut self) {
        self.key("Tab", TAB_CODE, TAB);
        assert_eq!(
            self.model().browser.as_ref().unwrap().zone(),
            Zone::Categories
        );
        assert_eq!(self.selected(Role::CheckBox), ["Development"]);
        self.key("Tab", TAB_CODE, TAB);
        assert_eq!(self.model().browser.as_ref().unwrap().zone(), Zone::Options);
        self.key("Return", RETURN_CODE, RETURN);
        assert!(self.options_menu().is_some(), "Return opens the menu");
    }

    /// Whether the view options button draws its focus ring (a stroke; the button is otherwise filled dots).
    fn options_ring(&self) -> bool {
        let id = self
            .driver
            .runtime()
            .find(self.panel(), &Key::from(browse::OPTIONS_BUTTON))
            .expect("the button shows");
        let paint = cupertino_kit::render::PaintNodeId::from(id);
        self.snapshot()
            .display_list
            .nodes
            .iter()
            .filter(|n| n.id == paint)
            .flat_map(|n| &n.ops)
            .any(|op| matches!(op, PaintOp::Stroke { .. }))
    }
}

#[test]
fn the_view_options_lay_the_applications_out_as_a_list() {
    let mut run = Run::new();
    run.browse_from_hidden();
    // Tab walks from the applications to the filters and on to the view options
    run.open_options_by_keys();
    run.menu_key("Down", DOWN_CODE, DOWN);
    run.menu_key("Return", RETURN_CODE, RETURN);
    assert!(run.options_menu().is_none(), "choosing closes the menu");
    assert_eq!(run.names(Role::ListItem), ALL_APPS);
    assert!(run.names(Role::Button).is_empty());
    run.fixtures("browser-list");
    // the choice lasts to the next opening
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    run.deliver(Message::Browse);
    assert_eq!(run.names(Role::ListItem), ALL_APPS);
}

/// The view options are a menu in a popup of the panel's window that takes the keyboard, opened below the button
/// and free of the panel's bounds: nothing of it is drawn in the panel, where the header would clip it.
#[test]
fn the_view_options_open_in_a_popup_below_their_button() {
    let mut run = Run::new();
    run.browse_from_hidden();
    let panel = run.panel();
    let button = run.rect_in(panel, browse::OPTIONS_BUTTON);
    run.click(panel, run.centre(browse::OPTIONS_BUTTON));
    let menu = run.options_menu().expect("the press opened the menu");
    assert!(
        run.names(Role::Menu).is_empty(),
        "the panel draws no menu of its own"
    );
    let system = run.driver.window_system_mut();
    assert_eq!(system.parent(menu).unwrap(), Some(panel));
    let spec = system.placement(menu).unwrap().expect("placed").clone();
    assert!(
        spec.grab,
        "it takes the keyboard and closes on a press outside"
    );
    assert_eq!(spec.anchor_rect, button, "anchored to the button");
    assert_eq!(
        (spec.anchor, spec.gravity),
        (
            cupertino_kit::platform::PopupEdge::Bottom,
            cupertino_kit::platform::PopupEdge::Bottom
        ),
        "opening below it"
    );
    assert!(spec.offset.y > 0.0, "a gap under the button");
    // its two rows are whole: the first row's top lies below the menu's own top edge and padding
    let first = run.rect_in(menu, 0u64);
    let second = run.rect_in(menu, 1u64);
    assert!(first.y() > 0.0, "{first:?}");
    assert!(second.max_y() < spec.size.height, "{second:?} in {spec:?}");
}

/// On layer surfaces the keyboard leaves the panel for the menu's popup when it opens, and comes back when it
/// closes: the panel stays open throughout, as it does for nothing else that takes the keyboard.
#[test]
fn the_panel_stays_open_while_its_view_options_menu_holds_the_keyboard() {
    let mut run = Run::on_layer_surfaces();
    run.deliver(Message::Browse);
    let panel = run.panel();
    run.click(panel, run.centre(browse::OPTIONS_BUTTON));
    let menu = run.options_menu().expect("the press opened the menu");
    // the keyboard's leave from the panel, then its enter on the popup, as the compositor sends them
    run.send(panel, WindowEvent::Focus(false));
    run.send(menu, WindowEvent::Focus(true));
    assert!(run.model().panel.is_visible(), "the panel stayed open");
    assert!(run.options_menu().is_some(), "and its menu");
    // opened with the pointer, nothing is selected: the first Down selects Grid, the second List
    run.menu_key("Down", DOWN_CODE, DOWN);
    run.menu_key("Down", DOWN_CODE, DOWN);
    run.menu_key("Return", RETURN_CODE, RETURN);
    assert!(run.options_menu().is_none(), "Return chose");
    run.send(panel, WindowEvent::Focus(true));
    assert!(run.model().panel.is_visible());
    assert_eq!(
        run.model().browser.as_ref().unwrap().arrangement(),
        Arrangement::List
    );
    // with the menu closed, losing the keyboard closes the panel as before
    run.send(panel, WindowEvent::Focus(false));
    assert!(!run.model().panel.is_visible());
}

/// The compositor ends the menu's grab when something else takes the keyboard: the popup is dismissed, the keyboard
/// comes back to the panel and leaves it at once. The panel's leave comes after the menu closed, so the panel closes;
/// a menu dismissed by a press outside, which gives the keyboard back to the panel and nothing more, leaves it open.
#[test]
fn the_panel_closes_when_the_keyboard_leaves_it_after_its_menu_was_dismissed() {
    for taken in [false, true] {
        let mut run = Run::on_layer_surfaces();
        run.deliver(Message::Browse);
        let panel = run.panel();
        run.click(panel, run.centre(browse::OPTIONS_BUTTON));
        let menu = run.options_menu().expect("the press opened the menu");
        run.send(panel, WindowEvent::Focus(false));
        run.send(menu, WindowEvent::Focus(true));
        assert!(run.model().panel.is_visible());
        let mut batch = vec![
            (menu, WindowEvent::PopupDone),
            (menu, WindowEvent::Focus(false)),
            (panel, WindowEvent::Focus(true)),
        ];
        if taken {
            batch.push((panel, WindowEvent::Focus(false)));
        }
        let system = run.driver.window_system_mut();
        for (window, event) in batch {
            system.enqueue_window(window, event).unwrap();
        }
        run.settle();
        assert!(run.options_menu().is_none(), "the menu closed");
        assert_eq!(
            run.model().panel.is_visible(),
            !taken,
            "the panel closes only when the keyboard went elsewhere (taken: {taken})"
        );
    }
}

/// A window system hands over every event it read at once, and the model hears what they emit in that order before
/// the view is built again: the menu's close and then the panel's leave, while the panel's window still shows the
/// menu's keys. The leave is judged after the close, so it closes the panel.
#[test]
fn a_leave_after_the_menu_closed_in_one_batch_closes_the_panel() {
    for taken in [false, true] {
        let mut model = Model::new(test_launcher(), Box::new(TestHost));
        kit::update(&mut model, Message::Browse);
        kit::update(&mut model, Message::Input(Input::ViewOptions));
        assert_eq!(model.nav.nested, Some(crate::nav::Nested::ViewOptions));
        // the menu's popup takes the keyboard from the panel's window
        kit::update(&mut model, Message::Input(Input::KeyboardLeft));
        assert!(model.panel.is_visible(), "the menu holds the keyboard");
        kit::update(&mut model, Message::Input(Input::CloseMenu));
        if taken {
            kit::update(&mut model, Message::Input(Input::KeyboardLeft));
        }
        assert_eq!(model.nav.nested, None);
        assert_eq!(
            model.panel.is_visible(),
            !taken,
            "the panel closes only when the keyboard left it after the menu (taken: {taken})"
        );
    }
}

/// The view options are drawn as every menu of the shell is: a menu on the menu material, which a desktop with glass
/// shows as glass under the popup.
#[test]
fn the_view_options_are_a_menu_on_the_menu_material() {
    for appearance in [Appearance::Light, Appearance::Dark] {
        let mut run = Run::with(appearance, true);
        run.browse_from_hidden();
        let panel = run.panel();
        run.click(panel, run.centre(browse::OPTIONS_BUTTON));
        let menu = run.options_menu().expect("the press opened the menu");
        let reader = run.reader.tree(menu).expect("the reader holds the menu");
        let root = reader
            .nodes
            .iter()
            .map(|(_, n)| n)
            .find(|n| n.role == Role::Menu)
            .expect("a menu");
        assert_eq!(root.name.as_deref(), Some(browse::VIEW_OPTIONS));
        let items: Vec<_> = reader
            .nodes
            .iter()
            .map(|(_, n)| n)
            .filter(|n| n.role == Role::MenuItem || n.role == Role::MenuItemCheckBox)
            .filter_map(|n| n.name.clone())
            .collect();
        assert_eq!(items.len(), 2, "{items:?}");
        let material = theme_in(appearance)
            .surface(Surface::Menu)
            .material
            .expect("the menu surface has a material")
            .to_owned();
        let backdrop = run
            .driver
            .window_system_mut()
            .requests(menu)
            .unwrap()
            .iter()
            .rev()
            .find_map(|r| match r {
                cupertino_kit::platform::WindowRequest::Backdrop(b) => Some(b.clone()),
                _ => None,
            })
            .flatten()
            .expect("the menu asks for a material");
        assert_eq!(backdrop.material, material, "{appearance:?}");
        assert!(backdrop.radius > 0.0);
    }
}

/// A choice made with the pointer leaves no focus ring on the button that opened the menu; one made from the
/// keyboard leaves the ring where the keys are.
#[test]
fn only_a_keyboard_choice_leaves_the_focus_ring_on_the_view_options_button() {
    let mut run = Run::new();
    run.browse_from_hidden();
    let panel = run.panel();
    assert!(!run.options_ring());
    run.click(panel, run.centre(browse::OPTIONS_BUTTON));
    let menu = run.options_menu().expect("the press opened the menu");
    let list = run.rect_in(menu, 1u64);
    run.click(menu, list.center());
    assert!(run.options_menu().is_none(), "the click chose");
    assert_eq!(
        run.model().browser.as_ref().unwrap().arrangement(),
        Arrangement::List
    );
    assert!(!run.options_ring(), "no ring after a pointer choice");
    // from the keyboard the menu opens on its first item, Grid
    run.open_options_by_keys();
    run.menu_key("Return", RETURN_CODE, RETURN);
    assert!(run.options_menu().is_none(), "Return chose");
    assert_eq!(
        run.model().browser.as_ref().unwrap().arrangement(),
        Arrangement::Grid
    );
    assert!(run.options_ring(), "the ring after a keyboard choice");
    // Escape in the menu closes it and leaves the arrangement
    run.key("Return", RETURN_CODE, RETURN);
    run.menu_key("Escape", ESCAPE_CODE, ESCAPE);
    assert!(run.options_menu().is_none());
    assert_eq!(
        run.model().browser.as_ref().unwrap().arrangement(),
        Arrangement::Grid
    );
    assert!(run.model().panel.is_visible(), "only the menu closed");
}

#[test]
fn a_late_file_search_is_dropped_and_one_past_its_deadline_says_the_results_may_be_incomplete() {
    let (_dir, index) = notes_index();
    let mut run = Run::new();
    run.deliver(Message::Indexed(index));
    hold_searches();
    run.type_text("notes");
    let searches = held_searches();
    assert_eq!(searches.len(), 5, "one search per query");
    let (older, newer) = (&searches[3], &searches[4]);
    assert!(
        searches[..4].iter().all(|s| s.cancel.is_cancelled()),
        "a newer query cancels the older searches"
    );
    assert!(!newer.cancel.is_cancelled());
    assert_eq!(
        run.announced(),
        Vec::<String>::new(),
        "nothing said while typing"
    );
    // the query stood past the deadline with no result yet: said once
    run.settle_query();
    assert_eq!(run.announced(), [SEARCHING]);
    run.settle_query();
    assert_eq!(run.announced(), [SEARCHING]);
    // the older answer comes last: it is dropped, the newer one shows
    run.deliver(Message::Provided(newer.run()));
    run.deliver(Message::Provided(Update {
        generation: older.generation,
        provider: Provider::Files,
        reply: Reply::Sections(Vec::new()),
    }));
    assert!(
        run.names(Role::ListItem).contains(&"notes.txt".to_owned()),
        "{:?}",
        run.names(Role::ListItem)
    );
    // a search that misses its deadline: what shows says it may be incomplete, not that nothing was found
    run.type_text("tesq");
    let late = held_searches().pop().expect("a search");
    run.deliver(Message::Overdue(late.generation));
    assert!(
        late.cancel.is_cancelled(),
        "the overdue search is told to stop"
    );
    assert_eq!(run.announced(), Vec::<String>::new(), "not settled yet");
    run.settle_query();
    assert_eq!(run.announced(), [INCOMPLETE]);
    run.deliver(Message::Provided(late.run()));
    assert_eq!(run.announced(), [INCOMPLETE], "its late answer is dropped");
    // a search that failed is not "no results" either
    run.type_text("u");
    let failing = held_searches().pop().expect("a search");
    run.deliver(Message::Provided(Update {
        generation: failing.generation,
        provider: Provider::Files,
        reply: Reply::Failed("unreadable".into()),
    }));
    run.settle_query();
    assert_eq!(run.announced(), [INCOMPLETE]);
    // and one that answered with nothing is
    run.type_text("x");
    let empty = held_searches().pop().expect("a search");
    run.deliver(Message::Provided(Update {
        generation: empty.generation,
        provider: Provider::Files,
        reply: Reply::Sections(Vec::new()),
    }));
    run.settle_query();
    assert_eq!(run.announced(), [NO_RESULTS]);
    assert_eq!(
        run.model().extent(),
        window_extent(PanelShape::new(
            WIDTH,
            on_shape_grid(COLLAPSED_HEIGHT + 1.0 + STATUS_HEIGHT),
            CORNER_RADIUS
        ))
    );
}

/// Typing says nothing between keys: a status is said once the query stood for its settling time, at most one per
/// settled query, never again for the same query, and a timer of an earlier query says nothing.
#[test]
fn a_typing_trace_says_at_most_one_status_per_settled_query() {
    let (_dir, index) = notes_index();
    let mut run = Run::new();
    run.deliver(Message::Indexed(index));
    run.said();
    hold_searches();
    let answer = |run: &mut Run, sections: bool| {
        let search = held_searches().pop().expect("a search");
        let update = if sections {
            search.run()
        } else {
            Update {
                generation: search.generation,
                provider: Provider::Files,
                reply: Reply::Sections(Vec::new()),
            }
        };
        run.deliver(Message::Provided(update));
    };
    // each key a new query that is still searching, then found nothing: nothing said while the keys come
    for c in ["z", "q", "x"] {
        let before = run.model().query_change();
        run.type_text(c);
        answer(&mut run, false);
        // the timer of the previous query fires late: it is not this query's
        run.deliver(Message::Settled(before));
    }
    assert_eq!(run.said(), Vec::<String>::new(), "nothing between keys");
    run.settle_query();
    run.settle_query();
    assert_eq!(run.said(), [NO_RESULTS], "once, for the settled query");
    // a key that leaves nothing found again is a new query: said again once it settles
    run.type_text("y");
    answer(&mut run, false);
    assert_eq!(run.said(), Vec::<String>::new());
    run.settle_query();
    assert_eq!(run.said(), [NO_RESULTS]);
    // the same query typed again says nothing new
    run.key("Backspace", BACKSPACE_CODE, BACKSPACE);
    answer(&mut run, false);
    run.settle_query();
    run.said();
    run.type_text("y");
    answer(&mut run, false);
    run.settle_query();
    assert_eq!(run.said(), Vec::<String>::new(), "\"zqxy\" said it already");
    // a search still running when the query settles says so, and nothing more once it answers
    run.type_text("w");
    run.settle_query();
    answer(&mut run, false);
    run.settle_query();
    assert_eq!(run.said(), [SEARCHING]);
    // results say no status
    for _ in 0..5 {
        run.key("Backspace", BACKSPACE_CODE, BACKSPACE);
    }
    run.type_text("notes");
    answer(&mut run, true);
    run.settle_query();
    assert!(!run.names(Role::ListItem).is_empty(), "results show");
    assert_eq!(run.said(), Vec::<String>::new());
}

/// The browser's "No Results" follows the search's rule: nothing between keys, said once the filter stood for its
/// settling time, at most once per settled filter, never again for the same filter, and a timer of an earlier filter
/// says nothing.
#[test]
fn a_typing_trace_in_the_browser_says_no_results_at_most_once_per_settled_filter() {
    let mut run = Run::new();
    run.browse_from_hidden();
    run.said();
    for c in ["z", "z", "z"] {
        let before = run.model().browse_change();
        run.type_text(c);
        assert!(run.names(Role::Button).is_empty(), "nothing matches");
        // the timer of the previous filter fires late: it is not this filter's
        run.deliver(Message::BrowseSettled(before));
    }
    assert_eq!(run.said(), Vec::<String>::new(), "nothing between keys");
    run.settle_browse();
    run.settle_browse();
    assert_eq!(run.said(), [NO_RESULTS], "once, for the settled filter");
    // a key that still finds nothing is a new filter: said once it settles
    run.type_text("q");
    assert_eq!(run.said(), Vec::<String>::new());
    run.settle_browse();
    assert_eq!(run.said(), [NO_RESULTS]);
    // the same filter typed again says nothing new
    run.key("Backspace", BACKSPACE_CODE, BACKSPACE);
    run.settle_browse();
    assert_eq!(run.said(), Vec::<String>::new(), "\"zzz\" said it already");
    run.type_text("q");
    run.settle_browse();
    assert_eq!(run.said(), Vec::<String>::new(), "\"zzzq\" said it already");
    // applications show again: no "No Results"
    for _ in 0..4 {
        run.key("Backspace", BACKSPACE_CODE, BACKSPACE);
    }
    run.type_text("fire");
    run.settle_browse();
    assert!(run.names(Role::Button).contains(&"Firefox".to_owned()));
    assert!(
        !run.said().contains(&NO_RESULTS.to_owned()),
        "results say no status"
    );
}

#[test]
fn space_enlarges_the_preview_which_says_when_none_came_in_time_and_escape_returns_to_the_list() {
    let (_dir, index) = notes_index();
    let mut run = Run::new();
    run.deliver(Message::Indexed(index));
    run.type_text("notes");
    run.key("Down", DOWN_CODE, DOWN);
    assert_eq!(run.model().panel.focus(), Focus::List);
    let item = run
        .model()
        .panel
        .selected_item()
        .cloned()
        .expect("a result");
    run.chord("Space", SPACE_CODE, SPACE, Some(' '), Mods::NONE);
    assert_eq!(run.model().nav.nested, Some(Nested::Preview));
    assert!(
        run.names(Role::ListItem).is_empty(),
        "the preview covers the list"
    );
    run.deliver(Message::PreviewOverdue(item.id.clone()));
    assert!(
        run.announced().contains(&PREVIEW_UNAVAILABLE.to_owned()),
        "{:?}",
        run.announced()
    );
    // a preview arriving after all still shows
    run.deliver(Message::Previewed(
        item.id.clone(),
        Arc::new(Preview {
            body: PreviewBody::Text("first line".into()),
            icon: Vec::new(),
            kind: "Plain text".into(),
            details: Vec::new(),
        }),
    ));
    assert!(!run.announced().contains(&PREVIEW_UNAVAILABLE.to_owned()));
    assert!(run
        .names(Role::StaticText)
        .contains(&"first line".to_owned()));
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    assert_eq!(run.model().nav.nested, None);
    assert_eq!(
        run.model().panel.focus(),
        Focus::List,
        "the keys go back to the list"
    );
    assert!(run.names(Role::ListItem).contains(&"notes.txt".to_owned()));
    // a preview for a result no longer selected is dropped
    run.key("Escape", ESCAPE_CODE, ESCAPE);
    run.deliver(Message::Previewed(
        "file:/elsewhere".into(),
        Arc::new(Preview {
            body: PreviewBody::Text("not this".into()),
            icon: Vec::new(),
            kind: String::new(),
            details: Vec::new(),
        }),
    ));
    assert!(!run.names(Role::StaticText).contains(&"not this".to_owned()));
}

#[test]
fn reduce_motion_puts_the_panel_where_it_heads_at_once() {
    use crate::motion::Motion;
    let mut motion = Motion::new(compact_shape());
    motion.set_reduce_motion(true);
    motion.show(browse_shape(BrowseLayout::HEIGHT), true, 0.0);
    let frame = motion.frame(0.0);
    assert_eq!(frame.scale, 1.0);
    assert_eq!(frame.shape, browse_shape(BrowseLayout::HEIGHT));
}

/// A host whose clock the test moves, and which records the window's size.
struct MovingHost(Rc<Cell<f64>>, Rc<Cell<Size>>);

impl Host for MovingHost {
    fn run(&mut self, _: CommandSpec, _: Option<String>) {}
    fn open(&mut self, _: &str) {}
    fn index(&mut self, _: FileSearchConfig) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn resize(&mut self, size: Size) {
        self.1.set(size);
    }
    fn frame(&mut self, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn icon(&mut self, _: &ResultItem) -> Vec<ImageSource> {
        Vec::new()
    }
    fn preview(&mut self, _: &ResultItem) -> Vec<Command<Message>> {
        Vec::new()
    }
    fn now(&self) -> Now {
        Now {
            mono: self.0.get(),
            unix: 1_000_000.0,
        }
    }
}

/// The browser opens, takes the output's height once it is known, folds into the search field and settles in a
/// window exactly the search field's, frame by frame on a clock that moves.
#[test]
fn on_a_moving_clock_the_window_follows_the_browser_and_settles_at_the_search_field() {
    let clock = Rc::new(Cell::new(0.0));
    let size = Rc::new(Cell::new(Size::ZERO));
    let mut model = Model::new(
        test_launcher(),
        Box::new(MovingHost(clock.clone(), size.clone())),
    );
    let run_for = |model: &mut Model, seconds: f64| {
        for _ in 0..(seconds / 0.016) as usize {
            clock.set(clock.get() + 0.016);
            let frame = model.frame;
            kit::update(model, Message::Frame(frame));
        }
    };
    kit::update(&mut model, Message::Browse);
    run_for(&mut model, 1.0);
    assert_eq!(
        size.get(),
        window_extent(browse_shape(BrowseLayout::HEIGHT))
    );
    // on a short output the browser leaves room at its foot
    kit::update(&mut model, Message::Output(Size::new(1000.0, 700.0)));
    run_for(&mut model, 1.0);
    let short = BrowseLayout::height_on(700.0, 140.0, kit::BROWSE_BOTTOM_ROOM);
    assert!(short < BrowseLayout::HEIGHT);
    assert_eq!(size.get(), window_extent(browse_shape(short)));
    kit::update(
        &mut model,
        Message::Input(Input::Key(PanelKey::Escape, PanelMods::NONE)),
    );
    assert_eq!(model.nav.view, View::Search);
    // while the shape contracts the window still holds it
    assert_eq!(size.get(), window_extent(browse_shape(short)));
    run_for(&mut model, 1.0);
    assert!(!model.motion.animating(clock.get()));
    assert_eq!(
        size.get(),
        window_extent(compact_shape()),
        "exactly the search field's window, not a point more"
    );
}

/// The panel is a layer surface, whose configures carry no activated state: it stays the active window while it has the
/// keyboard, so the field still draws its caret after the panel is configured again for its results.
#[test]
fn on_a_layer_surface_the_field_draws_its_caret_while_the_panel_has_the_keyboard() {
    let mut run = Run::on_layer_surfaces();
    assert!(run.caret_drawn(), "the empty field");
    let compact = run.extent;
    run.type_text("fire");
    assert_ne!(run.extent, compact, "the panel was configured again");
    assert!(run.caret_drawn(), "after the query");
}

impl Run {
    /// Whether the focused field paints its caret: a thin bar of the text's colour.
    fn caret_drawn(&self) -> bool {
        let snapshot = self.snapshot();
        let Some(field) = snapshot.focus else {
            return false;
        };
        // the caret is drawn in the field's overlay
        let paint = cupertino_kit::core::overlay_id(field);
        snapshot
            .display_list
            .nodes
            .iter()
            .filter(|n| n.id == paint)
            .flat_map(|n| &n.ops)
            .any(|op| {
                matches!(
                    op,
                    PaintOp::Fill {
                        shape: cupertino_kit::render::Shape::Rect(r),
                        ..
                    } if r.width() <= 2.0 && r.height() > 10.0
                )
            })
    }

    /// The colour of the fill the node `key` names in the panel paints.
    fn fill_of(&self, key: &'static str) -> Option<Color> {
        let id = self.driver.runtime().find(self.panel(), &Key::from(key))?;
        let paint = cupertino_kit::render::PaintNodeId::from(id);
        self.snapshot()
            .display_list
            .nodes
            .iter()
            .find(|n| n.id == paint)?
            .ops
            .iter()
            .find_map(|op| match op {
                PaintOp::Fill {
                    paint: Paint::Solid(c),
                    ..
                } => Some(*c),
                _ => None,
            })
    }
}

/// Over the panel's glass a separator is a faint line of the label's tone, light on dark glass and dark on light
/// glass; over the opaque fill the panel paints without glass it takes the theme's separator colour.
#[test]
fn separators_over_the_glass_are_a_faint_line_of_the_label_s_tone() {
    let white = Color::new(1.0, 1.0, 1.0, GLASS_SEPARATOR_DARK);
    let black = Color::new(0.0, 0.0, 0.0, GLASS_SEPARATOR_LIGHT);
    for (appearance, glass, expected) in [
        (Appearance::Dark, true, Some(white)),
        (Appearance::Light, true, Some(black)),
        (
            Appearance::Dark,
            false,
            theme_in(Appearance::Dark)
                .colors
                .get(ColorRole::Separator, StateVariant::NORMAL),
        ),
    ] {
        let mut run = Run::with(appearance, glass);
        run.type_text("fire");
        assert_eq!(
            run.fill_of("separator"),
            expected,
            "search, {appearance:?}, glass {glass}"
        );
        assert_eq!(
            run.fill_of("divider"),
            expected,
            "search's divider, {appearance:?}, glass {glass}"
        );
        // the browser unfolds from the search
        run.deliver(Message::Browse);
        assert_eq!(
            run.fill_of("header-separator"),
            expected,
            "browser, {appearance:?}, glass {glass}"
        );
    }
}
