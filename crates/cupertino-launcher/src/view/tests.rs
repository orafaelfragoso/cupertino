//! The launcher's windows run headless on the test launcher's entries: the Inspector dump of the panel (layout,
//! semantics and display list) compared with `tests/goldens/<name>.txt`, and what a screen reader reads of it
//! compared with `tests/fixtures/screen-reader/<name>.txt`. `CUPERTINO_UPDATE_GOLDENS=1` re-records both and then
//! fails, so the variable can never pass a run.

use super::*;
use crate::engine::tests_support::test_launcher;
use crate::exec::CommandSpec;
use crate::files::FileSearchConfig;
use crate::kit::{self, Host, Message, Model};
use crate::panel::Now;
use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityNode, Application, Command, FrameSnapshot, NodeId, PlaceholderIcons,
    RuntimeOptions,
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
use std::cell::RefCell;
use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::num::NonZeroU32;
use std::sync::Arc;

const OUTPUT: Size = Size {
    width: 1000.0,
    height: 700.0,
};

thread_local! {
    /// What the test host was asked to do, in order.
    static HOST: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
    /// The height the model last gave the shown panel.
    static HEIGHT: RefCell<Option<f64>> = const { RefCell::new(None) };
    /// Messages a test sends the model as the session would (`Toggle` from the bus, `Locked` from the server).
    static INBOX: RefCell<Option<(async_channel::Sender<Message>, async_channel::Receiver<Message>)>> =
        const { RefCell::new(None) };
}

/// Waits for the next message of [`INBOX`].
fn listen() -> Command<Message> {
    let inbox = INBOX.with_borrow(|inbox| inbox.as_ref().map(|(_, r)| r.clone()));
    let inbox = inbox.expect("the test app started");
    Command::Perform(Box::pin(async move {
        inbox.recv().await.unwrap_or(Message::Unheard)
    }))
}

fn log(entry: String) {
    HOST.with_borrow_mut(|log| log.push(entry));
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

    fn resize(&mut self, height: f64) {
        HEIGHT.set(Some(height));
    }

    fn frame(&mut self, _: u64) -> Vec<Command<Message>> {
        Vec::new()
    }

    fn icon(&mut self, _: &ResultItem) -> Vec<ImageSource> {
        Vec::new()
    }

    fn now(&self) -> Now {
        Now {
            mono: 0.0,
            unix: 1_000_000.0,
        }
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
        HEIGHT.set(None);
        let mut model = Model::new(test_launcher(), Box::new(TestHost));
        model.settled = true;
        INBOX.set(Some(async_channel::unbounded()));
        let mut commands = kit::update(&mut model, Message::Toggle);
        commands.push(listen());
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let inbox = matches!(message, Message::Toggle | Message::Locked);
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
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

struct Run {
    driver: TestDriver<TestApp>,
    configured: BTreeSet<WindowId>,
    /// The height the panel window was last configured at.
    panel_height: Option<f64>,
    /// Each input and what it left, for the keyboard golden.
    trace: String,
    /// A screen reader following the windows.
    reader: AssistiveTechnology,
}

fn active() -> WindowStates {
    WindowStates {
        activated: true,
        ..WindowStates::default()
    }
}

impl Run {
    fn new() -> Run {
        let reader = AssistiveTechnology::new();
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let options = RuntimeOptions {
            theme: Arc::new(theme()),
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
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        let mut run = Run {
            driver,
            configured: BTreeSet::new(),
            panel_height: None,
            trace: String::new(),
            reader,
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
                        let height = HEIGHT.with_borrow(|h| h.unwrap_or(COLLAPSED_HEIGHT));
                        if self.configured.contains(&id) && self.panel_height == Some(height) {
                            continue;
                        }
                        self.panel_height = Some(height);
                        Some(Size::new(WIDTH, height))
                    }
                    _ if self.configured.contains(&id) => continue,
                    () if catcher => Some(OUTPUT),
                    _ => None,
                };
                self.configured.insert(id);
                self.driver
                    .window_system_mut()
                    .configure(id, size, active())
                    .unwrap();
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
    if let Some((root, _)) = snapshot.accessibility.first() {
        walk(&mut out, &nodes, *root, 0, &describe);
    }
    out
}

const BACKSPACE_CODE: u32 = 14;
const TAB_CODE: u32 = 15;
const RETURN_CODE: u32 = 28;
const ESCAPE_CODE: u32 = 1;
const UP_CODE: u32 = 103;
const DOWN_CODE: u32 = 108;

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
    assert_eq!(
        run.panel_height,
        Some(panel_height(&model.panel)),
        "the panel window takes the height of its rows"
    );
    assert!(run.panel_height.unwrap() >= MIN_EXPANDED_HEIGHT);
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
    let host = HOST.with_borrow(Clone::clone);
    assert_eq!(host.len(), 1, "{host:?}");
    assert!(
        host[0].starts_with("run Some(\"browser.desktop\") [\"firefox\"]"),
        "{host:?}"
    );
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
    let host = HOST.with_borrow(Clone::clone);
    assert!(
        host.iter().any(|h| h.contains("firewall-config")),
        "{host:?}"
    );
}

#[test]
fn a_screen_reader_s_click_on_a_result_launches_it_and_closes_the_panel() {
    let mut run = Run::new();
    run.type_text("fire");
    let tree = run.reader.tree(run.panel()).expect("the reader holds the panel");
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
    let host = HOST.with_borrow(Clone::clone);
    assert!(
        host.iter().any(|h| h.contains("firewall-config")),
        "{host:?}"
    );
}

#[test]
fn a_press_outside_the_panel_closes_it() {
    let mut run = Run::new();
    let catcher = run.window("catcher").unwrap();
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
        Rect::new(200.0, 140.0, WIDTH, 430.0)
    );
}
