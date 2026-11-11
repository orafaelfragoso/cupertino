//! The Kit views on the headless window system: what the password field publishes while a password is typed and after it
//! is submitted, how a wrong password is shown and said, the field's busy and invalid states and its length limit, and
//! the confirmation panels' keys, buttons and "Reopen windows" checkbox, through the tree assistive technology reads and
//! the Inspector dump. With `CUPERTINO_LOOK_DIR` set, the test `pictures_for_review` writes each surface in light and dark
//! at 1× and 2× there.

use super::*;
use crate::lock_notices::{from_rows, Notice};
use crate::model::{AuthOutcome, LockScreen};
use crate::panel::{Action, Decision, Kind, Panel, CANCEL_LABEL, REOPEN_LABEL};
use crate::picker::Picker;
use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityAdapter, AccessibilityNode, AccessibilityUpdate, AccessibleValue,
    Action as AtAction, ActionRequest, Application, Command, NodeId, PlaceholderIcons,
    RuntimeOptions,
};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Mods, MouseButton, PlatformEvent, PointerEvent, Timestamp, WindowEvent,
    WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, ImeBatch, ImeEvent};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_ui_test::{Inspector, ManualClock, TestDriver, LOOK_DIR};
use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::path::Path;
use std::rc::Rc;

const SECRET: &str = "hunter2";
const SCREEN: Size = Size::new(1280.0, 800.0);
const RETURN: u32 = 0xff0d;
const ESCAPE: u32 = 0xff1b;
const TAB: u32 = 0xff09;
const SPACE: u32 = 0x20;

/// What the test application shows.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Surface {
    Lock,
    Picker,
    Panel { kind: Kind, reopen: bool },
    Interrupt,
    DesktopControl(Asking),
}

/// Who asks on the consent panel of a test.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Asking {
    /// A controller that does not name its app.
    Unnamed,
    /// An app with a desktop entry, for control and capture.
    Terminal,
    /// A program without one, for capture only, whose answer cannot be remembered.
    Program,
}

/// The request `asking` makes.
fn control_request(asking: Asking) -> ControlRequest {
    let app = match asking {
        Asking::Unnamed => None,
        Asking::Terminal => Some(RequestingApp {
            name: "Terminal".into(),
            identity: "org.example.Terminal".into(),
            desktop: true,
            services: 3,
            remembered: true,
            icon: Vec::new(),
        }),
        Asking::Program => Some(RequestingApp {
            name: "agent".into(),
            identity: "/home/me/bin/agent".into(),
            desktop: false,
            services: 2,
            remembered: false,
            icon: Vec::new(),
        }),
    };
    ControlRequest {
        output: "headless-1".into(),
        capabilities: 15,
        app,
    }
}

thread_local! {
    /// The surface the next application on this thread shows.
    static SURFACE: Cell<Surface> = const { Cell::new(Surface::Lock) };
    /// The next application on this thread holds each attempt until the next activity answers it.
    static HELD: Cell<bool> = const { Cell::new(false) };
    /// The notifications the lock screen shows from the next input on, as the daemon would list them.
    static NOTICES: RefCell<Option<Vec<Notice>>> = const { RefCell::new(None) };
}

#[derive(Debug)]
struct Model {
    surface: Surface,
    screen: LockScreen,
    /// The answer to the next attempt.
    outcome: AuthOutcome,
    panel: Panel,
    frames: Frames,
    picker: Picker,
    clock: Clock,
    user: User,
    metrics: Metrics,
    /// What reached the application, by Debug: the diagnostics a person could be shown.
    handled: Vec<String>,
    /// The attempts' passwords were what was typed.
    submitted: Vec<bool>,
    /// The attempts' passwords' lengths in bytes.
    lengths: Vec<usize>,
    /// An attempt waits for its answer until the next [`Input::Activity`], as from [`HELD`].
    held: bool,
    decided: Vec<Decision>,
    now: f64,
    notices: Vec<Notice>,
}

struct Shown;

fn user(login: &str, display: &str) -> User {
    User {
        login: login.into(),
        display: display.into(),
        picture: None,
        uid: None,
        logged_in: false,
    }
}

fn text_system() -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

impl Application for Shown {
    type Model = Model;
    type Message = Input;

    fn init() -> (Model, Vec<Command<Input>>) {
        let surface = SURFACE.get();
        let mut screen = LockScreen::new(0.0, 3600.0);
        screen.settle_interface();
        let (kind, reopen) = match surface {
            Surface::Panel { kind, reopen } => (kind, reopen),
            _ => (Kind::LogOut, true),
        };
        let text = text_system();
        let model = Model {
            surface,
            screen,
            outcome: AuthOutcome::Denied,
            panel: Panel::new(kind, 0.0, reopen),
            frames: panel_frames(kind, &text),
            picker: Picker::new(vec![user("ada", "Ada"), user("grace", "Grace")], true).unwrap(),
            clock: Clock {
                date: "Monday, June 1".into(),
                time: "09:41".into(),
            },
            user: user("ada", "Ada"),
            metrics: Metrics::of(&text),
            handled: Vec::new(),
            submitted: Vec::new(),
            lengths: Vec::new(),
            held: HELD.get(),
            decided: Vec::new(),
            now: 1.0,
            notices: NOTICES.with_borrow_mut(Option::take).unwrap_or_default(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, input: Input) -> Vec<Command<Input>> {
        if let Some(notices) = NOTICES.with_borrow_mut(Option::take) {
            model.notices = notices;
        }
        model.handled.push(format!("{input:?}"));
        model.now += 0.01;
        let now = model.now;
        match input {
            Input::Edited => model.screen.edit(now),
            Input::Escape => model.screen.escape(now),
            Input::Submit(typed) => {
                if model.screen.submit(now, typed.secret()).is_some() {
                    model.submitted.push(typed.secret() == SECRET);
                    model.lengths.push(typed.secret().len());
                    if !model.held {
                        let outcome = model.outcome.clone();
                        model.screen.finish_auth(now, outcome).unwrap();
                    }
                }
            }
            // a held attempt is answered by the next input, a pointer move in the tests
            Input::Activity if model.held && model.screen.checking() => {
                let outcome = model.outcome.clone();
                model.screen.finish_auth(now, outcome).unwrap();
            }
            Input::Reopen(on) => {
                if model.panel.reopen() != on {
                    model.panel.toggle_reopen();
                }
            }
            Input::Decide(decision) => model.decided.push(decision),
            _ => {}
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Input>> {
        let key = WindowKey(Key::from("shown"));
        match model.surface {
            Surface::Lock => {
                let s = &model.screen;
                let content = screen(
                    &Screen {
                        size: SCREEN,
                        metrics: model.metrics,
                        clock: &model.clock,
                        user: Some(&model.user),
                        field: Field::Password {
                            generation: s.field(),
                            enabled: true,
                            busy: s.checking(),
                        },
                        caps_lock: false,
                        spinner: None,
                        denied: s.failure_outline(),
                        notice: s.notice(),
                        help: false,
                        opacity: 1.0,
                        shake: 0.0,
                        hidden: false,
                        power: None,
                        notices: &model.notices,
                    },
                    LOCKED_TITLE,
                    |input| input,
                );
                vec![full_screen(
                    key,
                    LOCK_APP_ID.into(),
                    LOCKED_TITLE,
                    SCREEN,
                    content,
                    None,
                )]
            }
            Surface::Picker => {
                let content = picker(
                    &PickerScreen {
                        size: SCREEN,
                        metrics: model.metrics,
                        clock: &model.clock,
                        picker: &model.picker,
                        max_per_page: 10,
                        power: None,
                    },
                    |input| input,
                );
                vec![full_screen(
                    key,
                    GREETER_APP_ID.into(),
                    GREETER_TITLE,
                    SCREEN,
                    content,
                    None,
                )]
            }
            Surface::Panel { .. } => vec![panel(
                key,
                model.panel.kind(),
                model.panel.seconds_left(0.0),
                model.panel.reopen(),
                &model.frames,
                |input| input,
            )],
            Surface::Interrupt => vec![interrupt(key, "Notes", |input| input)],
            Surface::DesktopControl(asking) => {
                vec![desktop_control(key, &control_request(asking), |input| {
                    input
                })]
            }
        }
    }
}

/// What the runtime publishes to assistive technology, and the actions a test asks for.
#[derive(Clone, Default)]
struct Assistive {
    updates: Rc<RefCell<Vec<AccessibilityUpdate>>>,
    actions: Rc<RefCell<Vec<ActionRequest>>>,
}

impl AccessibilityAdapter for Assistive {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, update: &AccessibilityUpdate) {
        self.updates.borrow_mut().push(update.clone());
    }

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.actions.borrow_mut());
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(10_000).unwrap()
}

struct Run {
    driver: TestDriver<Shown>,
    window: WindowId,
    assistive: Assistive,
}

impl Run {
    fn new(surface: Surface) -> Run {
        SURFACE.set(surface);
        let text = text_system();
        let glyphs = Arc::clone(&text);
        let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
        let preferences = ThemePreferences {
            appearance: Appearance::Light,
            reduce_transparency: false,
            reduce_motion: false,
        };
        let theme = ThemeTokens::cupertino(
            &cupertino_theme::Theme::from_dir(data).unwrap(),
            preferences,
        )
        .unwrap();
        let assistive = Assistive::default();
        let options = RuntimeOptions {
            theme: Arc::new(theme),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 1000,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 23,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 20,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(Box::new(assistive.clone())),
            clock: Arc::new(ManualClock::new(Timestamp(0))),
            on_window_error: None,
        };
        let mut driver = TestDriver::new(
            Capabilities::default(),
            options,
            ManualClock::new(Timestamp(0)),
        );
        driver.run_until_idle(budget()).unwrap();
        let window = driver.runtime().windows()[0].1;
        let active = WindowStates {
            activated: true,
            ..WindowStates::default()
        };
        driver
            .window_system_mut()
            .configure(window, None, active)
            .unwrap();
        let mut run = Run {
            driver,
            window,
            assistive,
        };
        run.settle();
        run
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open and that nothing
    /// assistive technology was ever sent reads out the password the tests type.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Login");
        let leaks: Vec<String> = self
            .assistive
            .updates
            .borrow()
            .iter()
            .flat_map(|update| cupertino_ui_test::secret_leaks(update, &[SECRET]))
            .collect();
        assert_eq!(
            leaks,
            Vec::<String>::new(),
            "assistive technology reads the password"
        );
    }

    fn settle_frames(&mut self) {
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            if !system.frame_requested(self.window).unwrap() {
                return;
            }
            system.frame_ready(self.window, Timestamp(0)).unwrap();
        }
        panic!("the window keeps asking for frames");
    }

    fn deliver(&mut self, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(self.window, event)
            .unwrap();
        self.settle();
    }

    fn key(&mut self, keysym: u32, text: Option<char>) {
        let code = match keysym {
            ESCAPE => 1,
            TAB => 15,
            RETURN => 28,
            SPACE => 57,
            _ => 0,
        };
        for pressed in [true, false] {
            self.deliver(WindowEvent::Key(
                KeyEvent {
                    code,
                    keysym: Keysym(keysym),
                    text: text.filter(|_| pressed),
                    pressed,
                    repeat: false,
                    mods: Mods::NONE,
                },
                Timestamp(0),
            ));
        }
    }

    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.key(c as u32, Some(c));
        }
    }

    fn click(&mut self, key: &'static str) {
        let runtime = self.driver.runtime();
        let id = runtime
            .find(self.window, &Key::from(key))
            .unwrap_or_else(|| panic!("no element {key}"));
        let snapshot = runtime.snapshot(self.window).unwrap();
        let rect = snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect;
        let position = Point::new(
            rect.origin.x + rect.size.width / 2.0,
            rect.origin.y + rect.size.height / 2.0,
        );
        let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
        self.deliver(pointer(PointerEvent::Motion { position }));
        for pressed in [true, false] {
            self.deliver(pointer(PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            }));
        }
    }

    /// Assistive technology asks for `action` on the first node `pick` finds.
    fn act(&mut self, action: AtAction, pick: impl Fn(&AccessibilityNode) -> bool) {
        let (id, _) = self.node(pick).expect("a node to act on");
        self.assistive.actions.borrow_mut().push(ActionRequest {
            target: id,
            action,
            data: None,
        });
        self.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        self.settle();
    }

    /// The window's tree as assistive technology holds it now: every update applied in order.
    fn tree(&self) -> Vec<(NodeId, AccessibilityNode)> {
        let mut nodes: Vec<(NodeId, AccessibilityNode)> = Vec::new();
        for update in self.assistive.updates.borrow().iter() {
            if update.full {
                nodes.clear();
            }
            nodes.retain(|(id, _)| !update.removed.contains(id));
            for (id, node) in &update.nodes {
                match nodes.iter_mut().find(|(held, _)| held == id) {
                    Some((_, held)) => *held = node.clone(),
                    None => nodes.push((*id, node.clone())),
                }
            }
        }
        nodes
    }

    fn node(
        &self,
        pick: impl Fn(&AccessibilityNode) -> bool,
    ) -> Option<(NodeId, AccessibilityNode)> {
        self.tree().into_iter().find(|(_, node)| pick(node))
    }

    fn focused(&self) -> Option<AccessibilityNode> {
        let focus = self
            .assistive
            .updates
            .borrow()
            .iter()
            .rev()
            .find_map(|u| u.focus)?;
        self.tree()
            .into_iter()
            .find(|(id, _)| *id == focus)
            .map(|(_, n)| n)
    }

    fn dump(&self) -> String {
        Inspector::default().dump(&self.driver.runtime().snapshot(self.window).unwrap())
    }

    fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    /// Everything a person could be shown of the application: the dump, the tree, every update, and the model.
    fn diagnostics(&self) -> String {
        format!(
            "{}\n{:?}\n{:?}\n{:?}",
            self.dump(),
            self.tree(),
            self.assistive.updates.borrow(),
            self.model()
        )
    }
}

fn is_password(node: &AccessibilityNode) -> bool {
    node.role == Role::PasswordInput
}

fn is_reopen(node: &AccessibilityNode) -> bool {
    node.role == Role::CheckBox && node.name.as_deref() == Some(REOPEN_LABEL)
}

fn checked(run: &Run) -> Option<Checked> {
    match run.node(is_reopen)?.1.value {
        Some(AccessibleValue::Toggle(checked)) => Some(checked),
        other => panic!("the checkbox's value is {other:?}"),
    }
}

#[test]
fn a_password_being_typed_is_password_text_with_a_hidden_value_and_in_no_diagnostics() {
    let mut run = Run::new(Surface::Lock);
    let field = run.focused().expect("the field has the focus");
    assert!(is_password(&field), "the focus is on {field:?}");
    run.type_text(SECRET);
    let (_, field) = run.node(is_password).expect("a password field");
    assert_eq!(
        (field.name.as_deref(), field.value.clone()),
        (Some(PASSWORD_LABEL), Some(AccessibleValue::Protected))
    );
    assert!(run.driver.runtime().protected_focus(run.window).unwrap());
    let diagnostics = run.diagnostics();
    assert!(diagnostics.contains(PASSWORD_LABEL) && diagnostics.contains("Edited"));
    assert!(!diagnostics.contains(SECRET), "{diagnostics}");
    assert!(
        !diagnostics.contains(&SECRET.chars().rev().collect::<String>()),
        "the diagnostics hold the password backwards"
    );
}

#[test]
fn return_submits_the_password_once_and_a_new_empty_field_takes_the_focus() {
    let mut run = Run::new(Surface::Lock);
    run.key(RETURN, None);
    assert!(
        run.model().submitted.is_empty(),
        "an empty field submits nothing"
    );
    run.type_text(SECRET);
    let before = run.model().screen.field();
    run.key(RETURN, None);
    assert_eq!(run.model().submitted, [true]);
    assert_eq!(run.model().screen.field(), before + 1);
    let field = run.focused().expect("the new field has the focus");
    assert!(is_password(&field), "the focus is on {field:?}");
    // the new field is empty: one character and Return submit only that character
    run.type_text("x");
    run.key(RETURN, None);
    assert_eq!(run.model().submitted, [true, false]);
    assert!(!run.diagnostics().contains(SECRET));
}

#[test]
fn escape_empties_the_field() {
    let mut run = Run::new(Surface::Lock);
    run.type_text(SECRET);
    run.key(ESCAPE, None);
    assert_eq!(run.model().screen.field(), 1);
    run.type_text("x");
    run.key(RETURN, None);
    assert_eq!(
        run.model().submitted,
        [false],
        "only what was typed after Escape"
    );
}

#[test]
fn a_wrong_password_is_said_in_words_assertively_and_outlined_until_the_next_edit() {
    let mut run = Run::new(Surface::Lock);
    run.type_text(SECRET);
    run.key(RETURN, None);
    let (_, said) = run
        .node(|n| n.name.as_deref() == Some(DENIED))
        .expect("the denial is in the tree");
    assert_eq!(said.live, Some(Live::Assertive));
    assert!(run.model().screen.failure_outline());
    run.type_text("x");
    assert!(run.node(|n| n.name.as_deref() == Some(DENIED)).is_none());
    assert!(!run.model().screen.failure_outline());
}

/// The password field's busy and invalid states as assistive technology holds them now.
fn field_states(run: &Run) -> (bool, bool) {
    let (_, field) = run.node(is_password).expect("a password field");
    (field.states.busy, field.states.invalid)
}

/// How many times assistive technology was told of a live node named `text`: added with that name or renamed to it.
fn said(run: &Run, text: &str) -> usize {
    let mut names: Vec<(NodeId, Option<String>)> = Vec::new();
    let mut count = 0;
    for update in run.assistive.updates.borrow().iter() {
        if update.full {
            names.clear();
        }
        names.retain(|(id, _)| !update.removed.contains(id));
        for (id, node) in &update.nodes {
            let held = names.iter_mut().find(|(held, _)| held == id);
            let before = held.as_ref().and_then(|(_, name)| name.clone());
            if node.live.is_some()
                && node.name.as_deref() == Some(text)
                && before.as_deref() != Some(text)
            {
                count += 1;
            }
            match held {
                Some((_, name)) => *name = node.name.clone(),
                None => names.push((*id, node.name.clone())),
            }
        }
    }
    count
}

#[test]
fn the_field_is_busy_while_an_attempt_is_checked_and_invalid_after_a_wrong_password_until_the_next_edit(
) {
    HELD.set(true);
    let mut run = Run::new(Surface::Lock);
    assert_eq!(field_states(&run), (false, false));
    run.type_text(SECRET);
    run.key(RETURN, None);
    assert!(run.model().screen.checking());
    assert_eq!(
        field_states(&run),
        (true, false),
        "busy while it is checked"
    );
    assert_eq!(said(&run, DENIED), 0, "nothing is said before the answer");

    // the answer
    run.deliver(WindowEvent::Pointer(
        PointerEvent::Motion {
            position: Point::new(10.0, 10.0),
        },
        Timestamp(0),
    ));
    assert!(!run.model().screen.checking());
    assert_eq!(field_states(&run), (false, true), "invalid once refused");
    // more frames, a pointer move and the end of the shake change nothing that is said
    run.deliver(WindowEvent::Pointer(
        PointerEvent::Motion {
            position: Point::new(20.0, 20.0),
        },
        Timestamp(0),
    ));
    run.settle();
    assert_eq!(field_states(&run), (false, true));
    assert_eq!(said(&run, DENIED), 1, "the refusal is said once");

    run.type_text("x");
    assert_eq!(
        field_states(&run),
        (false, false),
        "the next edit clears it"
    );
    assert_eq!(said(&run, DENIED), 1);
    assert!(!run.diagnostics().contains(SECRET));
}

#[test]
fn the_field_refuses_what_the_authenticator_would_not_take_and_says_nothing_of_it() {
    let mut run = Run::new(Surface::Lock);
    run.type_text("a");
    // an input method's text past the limit is cut at the last character that fits
    let long = "b".repeat(MAX_PASSWORD_BYTES);
    run.deliver(WindowEvent::Ime(ImeEvent::Done(ImeBatch {
        preedit: None,
        commit: Some(long),
        delete: (0, 0),
        current: true,
    })));
    let edits = |run: &Run| {
        run.model()
            .handled
            .iter()
            .filter(|h| *h == "Edited")
            .count()
    };
    assert_eq!(edits(&run), 2);
    let before = run.assistive.updates.borrow().len();
    // the field is full: a key is no edit, and assistive technology hears nothing new of the field
    run.type_text("cé");
    assert_eq!(edits(&run), 2, "a refused key is no edit");
    let field = run.node(is_password).expect("a password field").0;
    let told: Vec<_> = run.assistive.updates.borrow()[before..]
        .iter()
        .flat_map(|u| u.nodes.iter())
        .filter(|(id, _)| *id == field)
        .map(|(_, node)| node.clone())
        .collect();
    assert!(
        told.iter()
            .all(|n| n.value == Some(AccessibleValue::Protected)
                && n.name.as_deref() == Some(PASSWORD_LABEL)
                && n.description.is_none()),
        "{told:?}"
    );
    run.key(RETURN, None);
    assert_eq!(run.model().lengths, [MAX_PASSWORD_BYTES]);
}

#[test]
fn the_lock_screen_says_it_is_locked_and_for_whom_and_its_only_control_is_the_field() {
    let run = Run::new(Surface::Lock);
    let tree = run.tree();
    let controls: Vec<Role> = tree
        .iter()
        .map(|(_, n)| n.role)
        .filter(|role| {
            matches!(
                role,
                Role::Button | Role::PasswordInput | Role::TextInput | Role::CheckBox
            )
        })
        .collect();
    assert_eq!(controls, [Role::PasswordInput]);
    assert!(
        tree.iter()
            .any(|(_, n)| n.name.as_deref() == Some(LOCKED_TITLE)
                && n.description.as_deref() == Some("Ada")),
        "{tree:?}"
    );
}

#[test]
fn the_reopen_checkbox_starts_at_the_saved_choice_and_a_click_space_or_action_flips_it() {
    for reopen in [true, false] {
        let run = Run::new(Surface::Panel {
            kind: Kind::LogOut,
            reopen,
        });
        let expected = if reopen {
            Checked::True
        } else {
            Checked::False
        };
        assert_eq!(checked(&run), Some(expected));
    }
    let mut run = Run::new(Surface::Panel {
        kind: Kind::Restart,
        reopen: true,
    });
    run.click("reopen");
    assert_eq!(
        (run.model().panel.reopen(), checked(&run)),
        (false, Some(Checked::False))
    );
    run.key(SPACE, Some(' '));
    assert_eq!(
        (run.model().panel.reopen(), checked(&run)),
        (true, Some(Checked::True))
    );
    run.act(AtAction::Click, is_reopen);
    assert_eq!(
        (run.model().panel.reopen(), checked(&run)),
        (false, Some(Checked::False))
    );
    assert!(
        run.model().decided.is_empty(),
        "the checkbox decides nothing"
    );
    run.key(RETURN, None);
    assert_eq!(run.model().decided, [Decision::Do(Action::Restart)]);
}

#[test]
fn the_default_button_has_the_focus_return_confirms_and_escape_cancels() {
    let mut run = Run::new(Surface::Panel {
        kind: Kind::ShutDown,
        reopen: true,
    });
    let focused = run.focused().expect("a focused button");
    assert_eq!(
        (focused.role, focused.name.as_deref()),
        (Role::Button, Some("Shut Down"))
    );
    let dialog = run
        .node(|n| n.role == Role::AlertDialog)
        .expect("an alert dialog")
        .1;
    assert!(dialog.states.modal);
    run.key(ESCAPE, None);
    run.key(RETURN, None);
    run.click("Cancel");
    run.act(AtAction::Click, |n| {
        n.role == Role::Button && n.name.as_deref() == Some("Shut Down")
    });
    assert_eq!(
        run.model().decided,
        [
            Decision::Cancel,
            Decision::Do(Action::ShutDown),
            Decision::Cancel,
            Decision::Do(Action::ShutDown)
        ]
    );
}

#[test]
fn tab_moves_the_focus_through_the_checkbox_and_the_buttons() {
    let mut run = Run::new(Surface::Panel {
        kind: Kind::LogOut,
        reopen: true,
    });
    let mut seen = Vec::new();
    for _ in 0..3 {
        run.key(TAB, None);
        let focused = run.focused().expect("a focused control");
        seen.push((focused.role, focused.name.unwrap_or_default()));
    }
    for (role, name) in [
        (Role::CheckBox, REOPEN_LABEL),
        (Role::Button, "Cancel"),
        (Role::Button, "Log Out"),
    ] {
        assert!(
            seen.contains(&(role, name.to_owned())),
            "Tab never reached {name}: {seen:?}"
        );
    }
}

#[test]
fn the_power_button_panel_has_no_checkbox_and_its_letters_choose() {
    let mut run = Run::new(Surface::Panel {
        kind: Kind::PowerButton,
        reopen: true,
    });
    assert!(run.node(is_reopen).is_none());
    run.key('s' as u32, Some('s'));
    run.key('r' as u32, Some('r'));
    // Shut Down has no countdown here, so Cancel has the focus: Space flips no choice and cancels, Return still shuts down
    let focused = run.focused().expect("a focused button");
    assert_eq!(
        (focused.role, focused.name.as_deref()),
        (Role::Button, Some(CANCEL_LABEL))
    );
    run.key(SPACE, Some(' '));
    assert!(run.model().panel.reopen());
    run.key(RETURN, None);
    assert_eq!(
        run.model().decided,
        [
            Decision::Do(Action::Sleep),
            Decision::Do(Action::Restart),
            Decision::Cancel,
            Decision::Do(Action::ShutDown)
        ]
    );
}

#[test]
fn native_control_requires_an_explicit_allow_and_tells_a_dismissal_from_a_refusal() {
    let mut run = Run::new(Surface::DesktopControl(Asking::Terminal));
    assert_eq!(run.focused().unwrap().name.as_deref(), Some("Don't Allow"));
    // Return is no default button: at most it presses the focused Don't Allow
    run.key(RETURN, None);
    assert!(!run
        .model()
        .handled
        .iter()
        .any(|m| m == "DesktopControl(Allow)"));
    run.key(SPACE, Some(' '));
    assert!(run
        .model()
        .handled
        .iter()
        .any(|m| m == "DesktopControl(DontAllow)"));
    run.key(TAB, None);
    assert_eq!(run.focused().unwrap().name.as_deref(), Some("Allow"));
    run.key(SPACE, Some(' '));
    assert!(run
        .model()
        .handled
        .iter()
        .any(|m| m == "DesktopControl(Allow)"));
    let mut run = Run::new(Surface::DesktopControl(Asking::Terminal));
    run.key(ESCAPE, None);
    assert_eq!(
        run.model().handled.last().map(String::as_str),
        Some("DesktopControl(Dismiss)"),
        "Escape closes the panel without a choice to remember"
    );
}

#[test]
fn the_consent_panel_names_the_app_and_what_it_asks() {
    let run = Run::new(Surface::DesktopControl(Asking::Terminal));
    let names: Vec<String> = run.tree().into_iter().filter_map(|(_, n)| n.name).collect();
    let title = "Allow \u{201c}Terminal\u{201d} to control and capture the desktop?";
    assert!(names.iter().any(|n| n == title), "{names:?}");
    let (_, info) = control_text(&control_request(Asking::Terminal));
    for part in [
        "inspect the display",
        "capture the display",
        "use the pointer",
        "use the keyboard",
        "on headless-1",
        "remembered for this app",
        "Settings \u{203a} Privacy",
        "Ctrl+Alt+Escape",
    ] {
        assert!(info.contains(part), "{part:?} in {info:?}");
    }
    let (title, info) = control_text(&control_request(Asking::Program));
    assert_eq!(title, "Allow \u{201c}agent\u{201d} to capture the screen?");
    assert!(info.contains("capture the display"), "{info}");
    assert!(
        !info.contains("pointer"),
        "only the rights of the services asked: {info}"
    );
    assert!(
        info.contains("The program is /home/me/bin/agent."),
        "{info}"
    );
    assert!(info.contains("this request only"), "{info}");
    let (title, info) = control_text(&control_request(Asking::Unnamed));
    assert_eq!(title, "Allow desktop control?");
    assert!(info.contains("this request only"), "{info}");
}

#[test]
fn the_consent_panel_shows_untrusted_names_plainly_and_bounded() {
    let mut request = control_request(Asking::Program);
    let app = request.app.as_mut().unwrap();
    app.name = "evil\u{202e}gnp.exe\nAllow".into();
    app.identity = format!("/{}agent", "deep/".repeat(60));
    let (title, info) = control_text(&request);
    assert!(
        !title.contains('\u{202e}') && !title.contains('\n'),
        "{title:?}"
    );
    assert!(info.contains("\u{2026}"), "a long path is cut: {info}");
    assert!(
        info.contains("deep/agent."),
        "the cut keeps the program's name: {info}"
    );
    request.app.as_mut().unwrap().name = "N".repeat(200);
    let (title, _) = control_text(&request);
    assert!(title.chars().count() < 80, "{title}");
    assert!(title.contains('\u{2026}'), "{title}");
}

#[test]
fn the_interrupt_panel_focuses_cancel_and_return_does_not_force_quit() {
    let mut run = Run::new(Surface::Interrupt);
    let focused = run.focused().expect("a focused button");
    assert_eq!(focused.name.as_deref(), Some("Cancel"));
    run.key(RETURN, None);
    assert!(
        !run.model().handled.iter().any(|m| m.contains("ForceQuit")),
        "{:?}",
        run.model().handled
    );
    run.key(ESCAPE, None);
    assert!(run
        .model()
        .handled
        .iter()
        .any(|m| m.contains("Interrupt(Cancel)")));
}

#[test]
fn the_picker_offers_each_account_and_other_and_the_arrows_step() {
    let mut run = Run::new(Surface::Picker);
    let tiles: Vec<String> = run
        .tree()
        .into_iter()
        .filter(|(_, n)| n.role == Role::Button)
        .filter_map(|(_, n)| n.name)
        .collect();
    for name in ["Ada", "Grace", OTHER_LABEL] {
        assert!(tiles.iter().any(|t| t == name), "no tile {name}: {tiles:?}");
    }
    run.key(0xff53, None);
    run.key(RETURN, None);
    let handled = &run.model().handled;
    assert!(
        handled.iter().any(|m| m.contains("Step(Right)")),
        "{handled:?}"
    );
    assert!(handled.iter().any(|m| m.contains("Choose(")), "{handled:?}");
}

/// The surfaces in light and dark at 1× and 2×, for a person to look at; nothing without [`LOOK_DIR`].
#[test]
fn pictures_for_review() {
    let Some(dir) = std::env::var_os(LOOK_DIR) else {
        return;
    };
    let dir = Path::new(&dir);
    let appearances = [Appearance::Light, Appearance::Dark];
    let look = |run: &mut Run, name: &str| {
        for scale in [1.0, 2.0] {
            run.driver
                .look_into(dir, run.window, name, scale, &appearances)
                .unwrap();
        }
    };
    let mut lock = Run::new(Surface::Lock);
    look(&mut lock, "lock");
    NOTICES.set(Some(mixed_notices()));
    look(&mut Run::new(Surface::Lock), "lock-notifications");
    lock.type_text(SECRET);
    look(&mut lock, "lock-typed");
    lock.key(RETURN, None);
    look(&mut lock, "lock-denied");
    for (kind, name) in [
        (Kind::LogOut, "panel-log-out"),
        (Kind::Restart, "panel-restart"),
        (Kind::ShutDown, "panel-shut-down"),
        (Kind::PowerButton, "panel-power-button"),
    ] {
        look(&mut Run::new(Surface::Panel { kind, reopen: true }), name);
    }
    look(
        &mut Run::new(Surface::Panel {
            kind: Kind::LogOut,
            reopen: false,
        }),
        "panel-log-out-unchecked",
    );
    look(&mut Run::new(Surface::Interrupt), "interrupt");
    for (asking, name) in [
        (Asking::Terminal, "control-consent"),
        (Asking::Program, "control-consent-program"),
        (Asking::Unnamed, "control-consent-unnamed"),
    ] {
        look(&mut Run::new(Surface::DesktopControl(asking)), name);
    }
    look(&mut Run::new(Surface::Picker), "greeter-picker");
}

/// What the daemon lists for the lock screen: Mail without its preview, Chat with it, and more than the screen shows.
fn mixed_notices() -> Vec<Notice> {
    from_rows(vec![
        (4, "Mail".into(), "Notification".into(), String::new()),
        (
            3,
            "Chat".into(),
            "Ada".into(),
            "See you at six, the table is booked under my name.".into(),
        ),
        (2, "Calendar".into(), "Notification".into(), String::new()),
        (1, "Reminders".into(), "Notification".into(), String::new()),
    ])
}

fn redacted(id: u32, app: &str) -> Notice {
    from_rows(vec![(id, app.into(), "Notification".into(), String::new())]).remove(0)
}

/// How many glyphs the window draws.
fn glyphs(run: &Run) -> usize {
    run.dump()
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Glyphs "))
        .filter_map(|line| {
            line.split(" count=")
                .nth(1)?
                .split(' ')
                .next()?
                .parse::<usize>()
                .ok()
        })
        .sum()
}

#[test]
fn the_lock_screen_shows_and_says_only_what_the_daemon_listed_and_the_field_keeps_the_focus() {
    let bare = glyphs(&Run::new(Surface::Lock));
    NOTICES.set(Some(vec![redacted(7, "Mail")]));
    let run = Run::new(Surface::Lock);
    // drawn: the application and "Notification", nothing else
    assert_eq!(glyphs(&run) - bare, "Mail".len() + "Notification".len());
    let tree = run.tree();
    let cards: Vec<&AccessibilityNode> = tree
        .iter()
        .map(|(_, n)| n)
        .filter(|n| n.live == Some(Live::Polite))
        .collect();
    assert_eq!(cards.len(), 1, "{tree:?}");
    assert_eq!(cards[0].name.as_deref(), Some("Mail: Notification"));
    assert_eq!(cards[0].role, Role::StaticText);
    assert!(!cards[0].states.focusable, "a card is no Tab stop");
    let controls: Vec<Role> = tree
        .iter()
        .map(|(_, n)| n.role)
        .filter(|role| matches!(role, Role::Button | Role::PasswordInput | Role::TextInput))
        .collect();
    assert_eq!(
        controls,
        [Role::PasswordInput],
        "the field is still the only control"
    );
    assert!(is_password(&run.focused().expect("a focus")));
}

#[test]
fn a_notice_arriving_on_the_lock_screen_is_said_once_and_the_rest_are_counted() {
    let mut run = Run::new(Surface::Lock);
    let said = |run: &Run| -> Vec<String> {
        run.tree()
            .into_iter()
            .filter(|(_, n)| n.live == Some(Live::Polite))
            .filter_map(|(_, n)| n.name)
            .collect()
    };
    assert!(said(&run).is_empty());
    NOTICES.set(Some(vec![redacted(1, "Mail")]));
    run.deliver(WindowEvent::Pointer(
        PointerEvent::Motion {
            position: Point::new(5.0, 5.0),
        },
        Timestamp(0),
    ));
    assert_eq!(said(&run), ["Mail: Notification"]);
    assert!(
        is_password(&run.focused().expect("a focus")),
        "announcing takes no focus"
    );
    NOTICES.set(Some(mixed_notices()));
    run.deliver(WindowEvent::Pointer(
        PointerEvent::Motion {
            position: Point::new(6.0, 6.0),
        },
        Timestamp(0),
    ));
    assert_eq!(
        said(&run),
        [
            "Mail: Notification",
            "Chat: Ada, See you at six, the table is booked under my name.",
            "Calendar: Notification"
        ]
    );
    assert!(
        run.tree()
            .iter()
            .any(|(_, n)| n.name.as_deref() == Some("1 more notification")),
        "the fourth is counted"
    );
    // a column at the top right, each card as tall as its text with the gap below it
    let rect = |key: &str| {
        let runtime = run.driver.runtime();
        let id = runtime
            .find(run.window, &Key::from(key.to_owned()))
            .unwrap();
        let snapshot = runtime.snapshot(run.window).unwrap();
        snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
    };
    let (mail, chat, calendar) = (rect("notice-4"), rect("notice-3"), rect("notice-2"));
    let (ascent, descent) = (run.model().metrics.ascent, run.model().metrics.descent);
    use crate::lock_notices::{card_height, CARD_GAP, CARD_MARGIN, CARD_TOP};
    assert_eq!(mail.max_x(), SCREEN.width - CARD_MARGIN);
    assert_eq!(mail.y(), CARD_TOP);
    assert_eq!(mail.height(), card_height(0, ascent, descent) + CARD_GAP);
    assert_eq!(chat.y(), mail.max_y());
    assert_eq!(
        chat.height(),
        card_height(1, ascent, descent) + CARD_GAP,
        "one line of body"
    );
    assert_eq!(calendar.y(), chat.max_y());
}
