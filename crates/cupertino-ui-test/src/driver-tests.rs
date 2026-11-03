use super::*;
use crate::{ProtectedInput, RecordedInput, SecureInputs};
use cupertino_geometry::Size;
use cupertino_ui_core::{
    AccessibleValue, Change, Command, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx,
    Phase, Propagation, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_platform::{
    ClipboardData, DecorationMode, KeyEvent, Keysym, Mods, WindowEvent, WindowSpec, WindowStates,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_text::{FontIndex, GlyphKey, GlyphRasterizer, RasterGlyph, TextError, TextSystem};
use cupertino_ui_theme::*;

struct Fields;

impl Application for Fields {
    type Model = Vec<char>;
    type Message = char;

    fn init() -> (Self::Model, Vec<Command<char>>) {
        (Vec::new(), Vec::new())
    }
    fn update(model: &mut Self::Model, message: char) -> Vec<Command<char>> {
        model.push(message);
        Vec::new()
    }
    fn view(_: &Self::Model) -> Vec<Window<char>> {
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Fields".into(),
                app_id: "test.fields".into(),
                size: Size::new(200.0, 80.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Client,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Field {
                protected: false,
                container: true,
                children: vec![
                    Element::new(Field {
                        protected: true,
                        container: false,
                        children: Vec::new(),
                    })
                    .key("secret"),
                    Element::new(Field {
                        protected: false,
                        container: false,
                        children: Vec::new(),
                    })
                    .key("plain"),
                ],
            }),
            menus: None,
        }]
    }
}

struct Field {
    protected: bool,
    container: bool,
    children: Vec<Element<char>>,
}

impl Widget<char> for Field {
    type State = ();
    fn state(&self) {}
    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(if !self.container {
            ContainerKind::Leaf
        } else {
            ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Start,
                justify: Justify::Start,
            }
        })
    }
    fn take_children(&mut self) -> Vec<Element<char>> {
        std::mem::take(&mut self.children)
    }
    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }
    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(100.0, 20.0),
            baseline: None,
        }
    }
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, char>) -> Propagation {
        if let Event::Key(key, _) = event {
            if cx.phase() == Phase::Target {
                if let Some(text) = key.text {
                    cx.emit(text);
                }
            }
        }
        Propagation::Continue
    }
    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}
    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if !self.container {
            cx.focusable();
            if self.protected {
                cx.protected();
            }
            cx.value(AccessibleValue::Text {
                value: "fixture-secret".into(),
                editing: None,
            });
        }
    }
}

struct NoGlyphs;
impl GlyphRasterizer for NoGlyphs {
    fn rasterize(&mut self, _: &GlyphKey) -> Result<Option<RasterGlyph>, TextError> {
        panic!("the field fixture paints no glyphs")
    }
}

fn options() -> RuntimeOptions {
    let role = TypeRole {
        size: 13.0,
        weight: 400,
    };
    RuntimeOptions {
        theme: Arc::new(ThemeTokens {
            preferences: ThemePreferences {
                appearance: Appearance::Light,
                reduce_transparency: false,
                reduce_motion: false,
            },
            colors: ColorTokens::default(),
            metrics: ControlMetrics {
                size: ControlSize::Regular,
                button_height: 20.0,
                button_inset: 10.0,
                button_radius: 5.0,
                field_height: 20.0,
                field_inset: 6.0,
                field_radius: 5.0,
                switch: Size::new(38.0, 22.0),
                switch_thumb: 18.0,
                switch_thumb_inset: 2.0,
                min_target: Size::new(24.0, 24.0),
            },
            type_scale: TypeScale {
                family: "fixture".into(),
                body: role,
                caption: role,
                section: role,
                title: role,
            },
            focus: FocusRing {
                width: 2.0,
                outset: 2.0,
                radius_extra: 3.0,
            },
            motion: Motion {
                reduce: false,
                caret_phase_ms: 500,
            },
        }),
        text: Arc::new(TextSystem::new(FontIndex::new(), "fixture")),
        icons: Arc::new(cupertino_ui_core::PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 32,
        renderers: Box::new(|| {
            Box::new(CpuRenderer::new(
                CpuConfig {
                    max_pixels: 200 * 80,
                    glyph_cache_bytes: 0,
                    image_cache_bytes: 0,
                    font_smoothing: true,
                },
                NoGlyphs,
            ))
        }),
        accessibility: None,
        clock: Arc::new(ManualClock::new(Timestamp(999))),
        on_window_error: None,
    }
}

fn budget() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}
fn key(keysym: u32, text: Option<char>) -> WindowEvent {
    WindowEvent::Key(
        KeyEvent {
            code: 0,
            keysym: Keysym(keysym),
            text,
            pressed: true,
            repeat: false,
            mods: Mods::default(),
        },
        Timestamp(10),
    )
}

fn start() -> (TestDriver<Fields>, WindowId) {
    let mut driver = TestDriver::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(10)),
    );
    driver.run_until_idle(budget()).unwrap();
    let window = driver.runtime().windows()[0].1;
    driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    driver.run_until_idle(budget()).unwrap();
    driver
        .window_system_mut()
        .enqueue_window(window, key(0xff09, None))
        .unwrap();
    driver.run_until_idle(budget()).unwrap();
    let secret = driver.runtime().find(window, &"secret".into()).unwrap();
    assert_eq!(
        driver.runtime().snapshot(window).unwrap().focus,
        Some(secret)
    );
    (driver, window)
}

#[test]
fn tab_then_paste_uses_the_requesting_focus_even_at_the_same_timestamp() {
    let (mut driver, window) = start();
    driver.window_system_mut().set_clipboard(
        ClipboardKind::Clipboard,
        ClipboardData {
            mime: "text/plain".into(),
            bytes: b"sensitive-paste".to_vec(),
        },
    );
    driver
        .window_system_mut()
        .enqueue_window(window, key(0xff09, None))
        .unwrap();
    let token = driver
        .read_clipboard(window, ClipboardKind::Clipboard, "text/plain")
        .unwrap();
    driver.run_until_idle(budget()).unwrap();
    let plain = driver.runtime().find(window, &"plain".into()).unwrap();
    assert_eq!(
        driver.runtime().snapshot(window).unwrap().focus,
        Some(plain)
    );
    assert_eq!(
        driver.trace().events.last().unwrap().input,
        RecordedInput::Protected(ProtectedInput::Clipboard(token))
    );
    assert!(!format!("{:?}", driver.trace()).contains("sensitive-paste"));
    let index = driver.trace().events.len() - 1;
    assert_eq!(
        driver.trace().replay(&SecureInputs::default()),
        Err(TraceError::MissingSecureInput { index: index - 1 })
    );
}

#[test]
fn queued_tab_changes_the_protection_of_the_next_key() {
    let (mut driver, window) = start();
    for event in [key(0xff09, None), key(0x78, Some('x'))] {
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
    }
    driver.run_until_idle(budget()).unwrap();
    assert!(matches!(
        driver.trace().events.last().unwrap().input,
        RecordedInput::Event(_)
    ));
    assert_eq!(driver.runtime().model(), &vec!['x']);
    assert_eq!(driver.trace().events.last().unwrap().at, Timestamp(10));
}

#[test]
fn unknown_clipboard_tokens_fail_without_recording_their_data() {
    let (mut driver, _) = start();
    let count = driver.trace().events.len();
    driver
        .window_system_mut()
        .enqueue(PlatformEvent::Clipboard {
            token: ClipboardToken(900),
            data: Some(ClipboardData {
                mime: "text/plain".into(),
                bytes: b"unknown-secret".to_vec(),
            }),
        })
        .unwrap();
    assert_eq!(
        driver.run_until_idle(budget()),
        Err(DriverError::UnknownClipboardToken(ClipboardToken(900)))
    );
    assert_eq!(driver.trace().events.len(), count);
}

#[test]
fn public_paste_stays_public_when_focus_moves_to_the_protected_field() {
    let (mut driver, window) = start();
    driver
        .window_system_mut()
        .enqueue_window(window, key(0xff09, None))
        .unwrap();
    driver.run_until_idle(budget()).unwrap();
    driver.window_system_mut().set_clipboard(
        ClipboardKind::Clipboard,
        ClipboardData {
            mime: "text/plain".into(),
            bytes: b"public-paste".to_vec(),
        },
    );
    driver
        .window_system_mut()
        .enqueue_window(window, key(0xff09, None))
        .unwrap();
    let token = driver
        .read_clipboard(window, ClipboardKind::Clipboard, "text/plain")
        .unwrap();
    driver.run_until_idle(budget()).unwrap();
    assert_eq!(driver.runtime().protected_focus(window), Some(true));
    assert_eq!(
        driver.trace().events.last().unwrap().input,
        RecordedInput::Event(PlatformEvent::Clipboard {
            token,
            data: Some(ClipboardData {
                mime: "text/plain".into(),
                bytes: b"public-paste".to_vec()
            }),
        })
    );
}

#[test]
fn the_turn_budget_keeps_input_queued_and_time_changes_only_explicitly() {
    let (mut driver, window) = start();
    for event in [key(0xff09, None), key(0x78, Some('x'))] {
        driver
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
    }
    assert_eq!(
        driver.run_until_idle(NonZeroU32::new(1).unwrap()),
        Ok(StepStatus::BudgetExhausted(Pending {
            messages: 0,
            tasks: 0,
            frames: 1,
            animating: false,
            next_wake: None,
        }))
    );
    assert_eq!(driver.window_system_mut().queued(), 1);
    assert_eq!(driver.runtime().model(), &Vec::<char>::new());
    driver.advance_to(Timestamp(20)).unwrap();
    driver.run_until_idle(budget()).unwrap();
    assert_eq!(driver.runtime().model(), &vec!['x']);
    assert_eq!(driver.trace().events.last().unwrap().at, Timestamp(20));
    assert_eq!(
        driver.advance_to(Timestamp(19)),
        Err(DriverError::Time(TimeWentBackwards {
            now: Timestamp(20),
            requested: Timestamp(19),
        }))
    );
    driver
        .window_system_mut()
        .enqueue(PlatformEvent::Wake)
        .unwrap();
    driver.run_until_idle(budget()).unwrap();
    assert_eq!(driver.trace().events.last().unwrap().at, Timestamp(20));
    let frames = driver.runtime().window_system().frames(window).unwrap();
    assert_eq!(frames.len(), 1);
    assert_eq!((frames[0].width, frames[0].height), (200, 80));
    assert_eq!(
        frames[0].acknowledged,
        Some(cupertino_ui_platform::ConfigureId(1))
    );
}

#[test]
fn runtime_trace_replay_reproduces_focus_model_and_snapshot() {
    let (mut first, window) = start();
    for event in [key(0xff09, None), key(0x78, Some('x'))] {
        first
            .window_system_mut()
            .enqueue_window(window, event)
            .unwrap();
    }
    first.run_until_idle(budget()).unwrap();
    let mut secure = SecureInputs::default();
    secure.insert(
        2,
        PlatformEvent::Window {
            window,
            event: key(0xff09, None),
        },
    );
    let events = first.trace().replay(&secure).unwrap();
    let mut replay = TestDriver::<Fields>::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(0)),
    );
    replay.run_until_idle(budget()).unwrap();
    for (at, event) in events {
        replay.advance_to(at).unwrap();
        if let PlatformEvent::Window {
            window,
            event: WindowEvent::Configure(configure),
        } = event
        {
            let id = replay
                .window_system_mut()
                .configure(window, configure.size, configure.states)
                .unwrap();
            assert_eq!(id, configure.id);
        } else {
            replay.window_system_mut().enqueue(event).unwrap();
        }
        replay.run_until_idle(budget()).unwrap();
    }
    assert_eq!(first.trace(), replay.trace());
    assert_eq!(replay.runtime().model(), &vec!['x']);
    assert_eq!(
        first.runtime().snapshot(window),
        replay.runtime().snapshot(window)
    );
    assert_eq!(
        replay
            .runtime()
            .window_system()
            .frames(window)
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        first.runtime().window_system().frames(window),
        replay.runtime().window_system().frames(window)
    );
}

#[test]
fn clipboard_read_rejects_a_window_that_does_not_exist() {
    let (mut driver, _) = start();
    assert_eq!(
        driver.read_clipboard(WindowId(999), ClipboardKind::Clipboard, "text/plain"),
        Err(DriverError::UnknownWindow(WindowId(999)))
    );
}
