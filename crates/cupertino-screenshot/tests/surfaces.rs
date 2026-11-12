#![cfg(feature = "app")]
//! The tool's windows as CupertinoKit runs them on the headless window system, driven through the real
//! [`Controller`]: the pointer and the keys on the overlay, the toolbar's buttons and their accessibility, the Options
//! menu, the thumbnail, and Escape and the close button taking every surface (and with them the keyboard) away.
//!
//! The capture thread is stood in for by a task that answers a capture with a fixed picture at once.

use std::cell::{Cell, RefCell};
use std::num::NonZeroU32;
use std::rc::Rc;
use std::sync::Arc;

use cupertino_assets::BUILTIN_FONT;
use cupertino_kit::core::{
    AccessibilityAdapter, AccessibilityUpdate, AccessibleValue, Action, ActionRequest, Application,
    Command, NodeId, PlaceholderIcons, Role, RuntimeOptions, Window,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::LayoutDirection;
use cupertino_kit::platform::{
    Capabilities, KeyEvent, Keysym, Mods, MouseButton, PlatformEvent, PointerEvent, Timestamp,
    WindowEvent, WindowId, WindowStates,
};
use cupertino_kit::render::{CpuConfig, CpuRenderer};
use cupertino_kit::text::{FontIndex, GlyphRaster, TextSystem};
use cupertino_kit::theme::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_screenshot::controller::{Controller, Effect, Phase};
use cupertino_screenshot::session::{Style, Target};
use cupertino_screenshot::settings::{Mode, Settings};
use cupertino_screenshot::view::{self, Input, CLIPBOARD_APP_ID, THUMBNAIL_APP_ID, TOOLBAR_APP_ID};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use tiny_skia::{Color, Pixmap};

const ESCAPE: u32 = 0xff1b;
const RETURN: u32 = 0xff0d;
const SPACE: u32 = 0x20;
const RIGHT: u32 = 0xff53;
const OUTPUT: Rect = Rect::new(0.0, 0.0, 800.0, 600.0);

/// How the next run starts: the overlay's style (`None` captures the whole screen at once) and the settings.
#[derive(Clone)]
struct Start {
    style: Option<Style>,
    settings: Settings,
    clipboard: bool,
}

thread_local! {
    static START: RefCell<Option<Start>> = const { RefCell::new(None) };
    /// Seconds since the run started, as the controller is told.
    static NOW: Cell<f64> = const { Cell::new(0.0) };
    /// The stand-in for the capture thread makes the clipboard offer at once.
    static OFFERS: Cell<bool> = const { Cell::new(true) };
}

fn now() -> f64 {
    NOW.get()
}

/// The picture every capture answers with: a 400 x 300 grey.
fn picture() -> Pixmap {
    let mut p = Pixmap::new(400, 300).unwrap();
    p.fill(Color::from_rgba8(120, 120, 120, 255));
    p
}

#[derive(Clone, Debug)]
enum Message {
    Input(Input),
    Shots(Vec<Pixmap>),
    Offered,
}

struct Model {
    controller: Controller,
    effects: Vec<Effect>,
}

/// The tool's application around the controller, with the capture and the clipboard answered at once.
struct Tool;

fn carry(model: &mut Model, effects: Vec<Effect>) -> Vec<Command<Message>> {
    let mut commands = Vec::new();
    for effect in &effects {
        match effect {
            Effect::Capture { .. } => commands.push(Command::Perform(Box::pin(async {
                Message::Shots(vec![picture()])
            }))),
            Effect::Offer if OFFERS.get() => {
                commands.push(Command::Perform(Box::pin(async { Message::Offered })))
            }
            _ => {}
        }
    }
    model.effects.extend(effects);
    commands
}

impl Application for Tool {
    type Model = Model;
    type Message = Message;
    const RESIDENT: bool = true;

    fn init() -> (Model, Vec<Command<Message>>) {
        let start = START.with_borrow(|s| s.clone()).expect("a start");
        let (controller, effects) =
            Controller::new(start.style, start.settings, vec![OUTPUT], start.clipboard);
        let mut model = Model {
            controller,
            effects: Vec::new(),
        };
        let commands = carry(&mut model, effects);
        (model, commands)
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        let effects = match message {
            Message::Input(input) => model.controller.input(input, now()),
            Message::Shots(shots) => model.controller.shots(Ok(shots), now()),
            Message::Offered => {
                model.controller.offered();
                Vec::new()
            }
        };
        carry(model, effects)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        model.controller.windows(now(), Message::Input)
    }
}

/// An assistive technology that is always listening and asks for what the test says.
#[derive(Clone, Default)]
struct Assistive {
    asked: Rc<RefCell<Vec<ActionRequest>>>,
}

impl AccessibilityAdapter for Assistive {
    fn is_active(&self) -> bool {
        true
    }

    fn window_focus(&mut self, _: WindowId, _: bool) {}

    fn update(&mut self, _: &AccessibilityUpdate) {}

    fn drain_actions(&mut self, out: &mut Vec<ActionRequest>) {
        out.append(&mut self.asked.borrow_mut());
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

fn tokens(appearance: Appearance) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

struct Run {
    driver: TestDriver<Tool>,
    at: Assistive,
    configured: Vec<WindowId>,
}

impl Run {
    fn start(style: Option<Style>, settings: Settings) -> Self {
        Self::start_with(style, settings, false)
    }

    fn start_with(style: Option<Style>, settings: Settings, clipboard: bool) -> Self {
        START.set(Some(Start {
            style,
            settings,
            clipboard,
        }));
        NOW.set(0.0);
        let mut fonts = FontIndex::new();
        fonts.add_font(Arc::new(BUILTIN_FONT.to_vec())).unwrap();
        let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
        let glyphs = Arc::clone(&text);
        let at = Assistive::default();
        let options = RuntimeOptions {
            theme: Arc::new(tokens(Appearance::Light)),
            text,
            icons: Arc::new(PlaceholderIcons::default()),
            direction: LayoutDirection::Ltr,
            node_limit: 400,
            renderers: Box::new(move || {
                Box::new(CpuRenderer::new(
                    CpuConfig {
                        max_pixels: 1 << 21,
                        glyph_cache_bytes: 1 << 20,
                        image_cache_bytes: 1 << 22,
                        font_smoothing: true,
                    },
                    GlyphRaster::new(Arc::clone(&glyphs)),
                ))
            }),
            accessibility: Some(Box::new(at.clone())),
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
            at,
            configured: Vec::new(),
        };
        run.settle();
        run
    }

    /// Runs until every window rests, then checks that a screen reader could read every window open.
    fn settle(&mut self) {
        self.settle_frames();
        self.driver.assert_accessible("Screenshot");
    }

    /// Run until idle, configuring each new window at its size (an overlay at its output's), and hand out the frames
    /// asked for, a bounded number of times.
    fn settle_frames(&mut self) {
        for _ in 0..16 {
            self.driver.run_until_idle(budget()).unwrap();
            let system = self.driver.window_system_mut();
            let mut busy = false;
            for window in system.windows() {
                if !self.configured.contains(&window) {
                    let spec = system.spec(window).unwrap();
                    let size = if view::overlay_index(&spec.app_id).is_some() {
                        OUTPUT.size
                    } else if spec.size.width > 0.0 {
                        spec.size
                    } else {
                        Size::new(240.0, 320.0)
                    };
                    let active = WindowStates {
                        activated: true,
                        ..WindowStates::default()
                    };
                    system.configure(window, Some(size), active).unwrap();
                    self.configured.push(window);
                    busy = true;
                } else if system.frame_requested(window).unwrap() {
                    system.frame_ready(window, Timestamp(0)).unwrap();
                    busy = true;
                }
            }
            if !busy {
                return;
            }
        }
        panic!("the windows keep asking for frames");
    }

    fn model(&self) -> &Model {
        self.driver.runtime().model()
    }

    fn effects(&self) -> &[Effect] {
        &self.model().effects
    }

    fn controller(&self) -> &Controller {
        &self.model().controller
    }

    fn app_ids(&self) -> Vec<String> {
        let system = self.driver.runtime().window_system();
        let mut ids: Vec<String> = system
            .windows()
            .into_iter()
            .map(|w| system.spec(w).unwrap().app_id.clone())
            .collect();
        ids.sort();
        ids
    }

    fn window(&self, app_id: &str) -> WindowId {
        let system = self.driver.runtime().window_system();
        system
            .windows()
            .into_iter()
            .find(|w| system.spec(*w).unwrap().app_id == app_id)
            .unwrap_or_else(|| panic!("no window {app_id}"))
    }

    fn send(&mut self, window: WindowId, event: WindowEvent) {
        self.driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
        self.settle();
    }

    fn pointer(&mut self, window: WindowId, event: PointerEvent) {
        self.send(window, WindowEvent::Pointer(event, Timestamp(0)));
    }

    fn button(&mut self, window: WindowId, position: Point, pressed: bool) {
        self.pointer(
            window,
            PointerEvent::Button {
                position,
                button: MouseButton::Left,
                pressed,
                serial: 1,
                mods: Mods::NONE,
            },
        );
    }

    fn key(&mut self, window: WindowId, keysym: u32, mods: Mods) {
        for pressed in [true, false] {
            // a press that ended the run leaves no window for the release
            if !self
                .driver
                .runtime()
                .window_system()
                .windows()
                .contains(&window)
            {
                return;
            }
            self.send(
                window,
                WindowEvent::Key(
                    KeyEvent {
                        code: 1,
                        keysym: Keysym(keysym),
                        text: None,
                        pressed,
                        repeat: false,
                        mods,
                    },
                    Timestamp(0),
                ),
            );
        }
    }

    /// The accessible node named `name` with `role` in `window`: its id and its bounds.
    fn node(&self, window: WindowId, role: Role, name: &str) -> (NodeId, Rect) {
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
        snapshot
            .accessibility
            .iter()
            .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
            .map(|(id, n)| (*id, n.bounds))
            .unwrap_or_else(|| panic!("no {role:?} named {name}"))
    }

    /// The overlay's accessible value: what a capture would take.
    fn summary(&self) -> String {
        let window = self.window(&view::overlay_app_id(0));
        let snapshot = self.driver.runtime().snapshot(window).unwrap();
        let node = snapshot
            .accessibility
            .iter()
            .find(|(_, n)| n.role == Role::Group && n.name.as_deref() == Some(view::OVERLAY_TITLE))
            .map(|(_, n)| n.clone())
            .expect("the overlay's node");
        match node.value {
            Some(AccessibleValue::Text { value, .. }) => value,
            other => panic!("the overlay's value is {other:?}"),
        }
    }

    /// Ask the node named `name` with `role` in `window` for `action`, as assistive technology does.
    fn ask(&mut self, window: WindowId, role: Role, name: &str, action: Action) {
        let (target, _) = self.node(window, role, name);
        self.at.asked.borrow_mut().push(ActionRequest {
            target,
            action,
            data: None,
        });
        self.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        self.settle();
    }

    fn captures(&self) -> Vec<Target> {
        self.effects()
            .iter()
            .filter_map(|e| match e {
                Effect::Capture { target, .. } => Some(target.clone()),
                _ => None,
            })
            .collect()
    }
}

fn toolbar_settings() -> Settings {
    Settings {
        mode: Mode::Selection,
        selection: None,
        ..Settings::default()
    }
}

#[test]
fn a_drag_on_the_overlay_spans_the_selection_and_its_release_captures_the_area() {
    let mut run = Run::start(Some(Style::Simple), Settings::default());
    assert_eq!(run.app_ids(), [view::overlay_app_id(0)]);
    let overlay = run.window(&view::overlay_app_id(0));
    run.pointer(
        overlay,
        PointerEvent::Motion {
            position: Point::new(100.0, 100.0),
        },
    );
    assert_eq!(
        run.driver
            .runtime()
            .window_system()
            .cursor(overlay)
            .unwrap(),
        None,
        "the crosshair is drawn; the system pointer is hidden"
    );
    run.button(overlay, Point::new(100.0, 100.0), true);
    run.pointer(
        overlay,
        PointerEvent::Motion {
            position: Point::new(300.0, 250.0),
        },
    );
    assert_eq!(run.summary(), "Selection 200 \u{d7} 150 at 100, 100");
    run.button(overlay, Point::new(300.0, 250.0), false);
    assert_eq!(
        run.captures(),
        [Target::Area {
            output: 0,
            rect: Rect::new(100.0, 100.0, 200.0, 150.0)
        }]
    );
    assert_eq!(
        run.driver.runtime().window_system().cursor(overlay).ok(),
        None,
        "the overlay showed the arrow and went before the capture"
    );
    // the capture answered at once: the thumbnail is up over the first output's corner, taking the pointer over the
    // picture only and never the keyboard
    assert_eq!(run.app_ids(), [THUMBNAIL_APP_ID]);
    let thumbnail = run.window(THUMBNAIL_APP_ID);
    let spec = run
        .driver
        .runtime()
        .window_system()
        .spec(thumbnail)
        .unwrap();
    let region = spec.input_region.clone().expect("an input region");
    assert_eq!(region.len(), 1);
    assert!(region[0].width() > 0.0 && region[0].width() < spec.size.width);
}

#[test]
fn the_arrow_keys_and_return_capture_a_selection_from_the_keyboard() {
    let mut run = Run::start(Some(Style::Simple), Settings::default());
    let overlay = run.window(&view::overlay_app_id(0));
    assert_eq!(run.summary(), "No selection");
    run.key(overlay, RIGHT, Mods::NONE);
    assert_eq!(run.summary(), "Selection 400 \u{d7} 300 at 200, 150");
    run.key(overlay, RIGHT, Mods::SHIFT);
    assert_eq!(run.summary(), "Selection 400 \u{d7} 300 at 210, 150");
    run.key(overlay, RETURN, Mods::NONE);
    assert_eq!(
        run.captures(),
        [Target::Area {
            output: 0,
            rect: Rect::new(210.0, 150.0, 400.0, 300.0)
        }]
    );
}

#[test]
fn escape_takes_every_surface_and_the_keyboard_away_without_a_capture() {
    for style in [Style::Simple, Style::Interactive] {
        let mut run = Run::start(Some(style), toolbar_settings());
        let keys = match style {
            Style::Simple => view::overlay_app_id(0),
            Style::Interactive => TOOLBAR_APP_ID.to_owned(),
        };
        let window = run.window(&keys);
        run.key(window, ESCAPE, Mods::NONE);
        assert!(
            run.app_ids().is_empty(),
            "{style:?}: every window is gone: {:?}",
            run.app_ids()
        );
        assert!(run.driver.runtime().windows().is_empty());
        assert_eq!(run.controller().phase(), &Phase::Done);
        assert_eq!(run.captures(), [], "{style:?}");
        assert_eq!(
            run.effects().last(),
            Some(&Effect::Exit(Ok(()))),
            "{style:?}"
        );
    }
}

#[test]
fn the_close_button_cancels_like_escape_from_assistive_technology() {
    let mut run = Run::start(Some(Style::Interactive), toolbar_settings());
    let toolbar = run.window(TOOLBAR_APP_ID);
    run.ask(toolbar, Role::Button, "Close", Action::Click);
    assert!(run.app_ids().is_empty());
    assert_eq!(run.effects().last(), Some(&Effect::Exit(Ok(()))));
}

#[test]
fn the_toolbar_names_its_controls_and_its_buttons_choose_the_mode_and_capture() {
    let mut run = Run::start(Some(Style::Interactive), toolbar_settings());
    assert_eq!(
        run.app_ids(),
        [view::overlay_app_id(0), TOOLBAR_APP_ID.to_owned()]
    );
    let toolbar = run.window(TOOLBAR_APP_ID);
    for name in ["Close", "Options", "Capture"] {
        run.node(toolbar, Role::Button, name);
    }
    run.node(toolbar, Role::Toolbar, view::TOOLBAR_TITLE);
    // the mode buttons are a group of three
    let (_, window) = run.node(toolbar, Role::RadioButton, "Window");
    run.button(toolbar, window.center(), true);
    run.button(toolbar, window.center(), false);
    assert_eq!(run.controller().session().unwrap().mode(), Mode::Window);
    assert_eq!(run.controller().settings().mode, Mode::Window);
    assert!(run.effects().contains(&Effect::StoreSettings));
    let (_, screen) = run.node(toolbar, Role::RadioButton, "Entire Screen");
    run.button(toolbar, screen.center(), true);
    run.button(toolbar, screen.center(), false);
    run.ask(toolbar, Role::Button, "Capture", Action::Click);
    assert_eq!(run.captures(), [Target::Screens]);
}

#[test]
fn space_and_return_from_the_toolbar_reach_the_overlay() {
    let mut run = Run::start(Some(Style::Interactive), toolbar_settings());
    let toolbar = run.window(TOOLBAR_APP_ID);
    run.key(toolbar, SPACE, Mods::NONE);
    assert_eq!(run.controller().session().unwrap().mode(), Mode::Window);
    run.key(toolbar, SPACE, Mods::NONE);
    run.key(toolbar, RIGHT, Mods::NONE);
    assert_eq!(run.summary(), "Selection 400 \u{d7} 300 at 200, 150");
    run.key(toolbar, RETURN, Mods::CTRL);
    assert_eq!(
        run.captures(),
        [Target::Area {
            output: 0,
            rect: Rect::new(200.0, 150.0, 400.0, 300.0)
        }]
    );
    assert!(
        run.effects().contains(&Effect::Offer),
        "Control sends the capture to the clipboard"
    );
    assert_eq!(run.controller().phase(), &Phase::Serving);
}

#[test]
fn the_options_button_opens_the_menu_under_the_toolbar() {
    let mut run = Run::start(Some(Style::Interactive), toolbar_settings());
    let toolbar = run.window(TOOLBAR_APP_ID);
    let before = run.app_ids().len();
    run.ask(toolbar, Role::Button, "Options", Action::Click);
    assert!(run.controller().menu_open());
    let ids = run.app_ids();
    assert!(ids.len() > before, "the menu opened a popup: {ids:?}");
    let popup = run
        .driver
        .runtime()
        .window_system()
        .windows()
        .into_iter()
        .find(|w| {
            run.driver
                .runtime()
                .window_system()
                .parent(*w)
                .unwrap()
                .is_some()
        })
        .expect("a popup");
    assert_eq!(
        run.driver.runtime().window_system().parent(popup).unwrap(),
        Some(toolbar)
    );
    // Escape closes the menu before it would cancel the run
    run.key(toolbar, ESCAPE, Mods::NONE);
    assert!(!run.controller().menu_open());
    assert!(run.app_ids().contains(&TOOLBAR_APP_ID.to_owned()));
}

#[test]
fn the_thumbnail_commits_once_when_clicked_and_goes_after_its_slide() {
    let mut run = Run::start(None, Settings::default());
    assert_eq!(run.captures(), [Target::Screens]);
    assert_eq!(run.app_ids(), [THUMBNAIL_APP_ID]);
    let thumbnail = run.window(THUMBNAIL_APP_ID);
    run.ask(
        thumbnail,
        Role::Button,
        view::THUMBNAIL_TITLE,
        Action::Click,
    );
    let writes = |run: &Run| {
        run.effects()
            .iter()
            .filter(|e| **e == Effect::Write)
            .count()
    };
    assert_eq!(writes(&run), 1);
    NOW.set(1.0);
    run.pointer(
        thumbnail,
        PointerEvent::Motion {
            position: Point::new(60.0, 60.0),
        },
    );
    assert_eq!(writes(&run), 1, "the files are written once");
    assert!(run.app_ids().is_empty());
    assert_eq!(run.effects().last(), Some(&Effect::Exit(Ok(()))));
}

#[test]
fn without_the_thumbnail_the_files_are_written_at_once() {
    let settings = Settings {
        show_thumbnail: false,
        ..Settings::default()
    };
    let run = Run::start(None, settings);
    assert_eq!(
        run.effects(),
        [
            Effect::Capture {
                target: Target::Screens,
                pointer: false
            },
            Effect::Write,
            Effect::Exit(Ok(()))
        ]
    );
    assert!(run.app_ids().is_empty());
}

#[test]
fn the_clipboard_pixel_holds_the_keyboard_only_until_the_offer_is_made() {
    OFFERS.set(false);
    let waiting = Run::start_with(None, Settings::default(), true);
    OFFERS.set(true);
    assert!(waiting.effects().contains(&Effect::Offer));
    assert_eq!(
        waiting.app_ids(),
        [CLIPBOARD_APP_ID],
        "the pixel waits for the offer"
    );
    let pixel = waiting.window(CLIPBOARD_APP_ID);
    let spec = waiting
        .driver
        .runtime()
        .window_system()
        .spec(pixel)
        .unwrap();
    assert_eq!(spec.size, Size::new(1.0, 1.0));
    assert_eq!(spec.input_region, Some(Vec::new()), "it takes no pointer");

    let offered = Run::start_with(None, Settings::default(), true);
    assert!(
        offered.app_ids().is_empty(),
        "once the offer is made the pixel is gone"
    );
    assert_eq!(offered.controller().phase(), &Phase::Serving);
}
