//! The Kit views on the headless window system: what the password field publishes while a password is typed and after it
//! is submitted, how a wrong password is shown and said, and the confirmation panels' keys, buttons and "Reopen windows"
//! checkbox, through the tree assistive technology reads and the Inspector dump. With `CUPERTINO_LOOK_DIR` set, the test
//! `pictures_for_review` writes each surface in light and dark at 1× and 2× there.

use super::*;
use crate::model::{AuthOutcome, LockScreen};
use crate::panel::{Action, Decision, Kind, Panel, REOPEN_LABEL};
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
use cupertino_kit::text::{FontIndex, GlyphRaster};
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
}

thread_local! {
    /// The surface the next application on this thread shows.
    static SURFACE: Cell<Surface> = const { Cell::new(Surface::Lock) };
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
    decided: Vec<Decision>,
    now: f64,
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
            decided: Vec::new(),
            now: 1.0,
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, input: Input) -> Vec<Command<Input>> {
        model.handled.push(format!("{input:?}"));
        model.now += 0.01;
        let now = model.now;
        match input {
            Input::Edited => model.screen.edit(now),
            Input::Escape => model.screen.escape(now),
            Input::Submit(typed) => {
                if model.screen.submit(now, typed.secret()).is_some() {
                    model.submitted.push(typed.secret() == SECRET);
                    let outcome = model.outcome.clone();
                    model.screen.finish_auth(now, outcome).unwrap();
                }
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

    fn settle(&mut self) {
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
    // Space flips no choice here: it presses the focused button, the default Shut Down, as Return does
    run.key(SPACE, Some(' '));
    assert!(run.model().panel.reopen());
    assert_eq!(
        run.model().decided,
        [
            Decision::Do(Action::Sleep),
            Decision::Do(Action::Restart),
            Decision::Do(Action::ShutDown)
        ]
    );
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
    look(&mut Run::new(Surface::Picker), "greeter-picker");
}
