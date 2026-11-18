//! What the harness records of a run and gives back: input to a protected field never enters a trace, a trace that
//! cannot be replayed whole is refused, and time moves only when the test moves it.

use cupertino_geometry::Size;
use cupertino_theme::ui::{Appearance, ThemePreferences, ThemeTokens};
use cupertino_ui_core::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
};
use cupertino_ui_core::platform::{
    Capabilities, ClipboardData, ClipboardKind, DecorationMode, KeyEvent, Keysym, Mods,
    PlatformEvent, Timestamp, WindowEvent, WindowId, WindowSpec, WindowStates,
};
use cupertino_ui_core::{
    Application, Change, Clock, Command, CommandRegistry, Element, Event, EventCx, MeasureCx,
    PaintCx, PlaceholderIcons, Propagation, Role, RuntimeOptions, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_ui_render::{CpuConfig, CpuRenderer};
use cupertino_ui_test::{
    DriverError, ManualClock, ProtectedInput, RecordedInput, SecureInputs, TestDriver, Trace,
    TraceError, TraceEvent, TRACE_VERSION,
};
use cupertino_ui_text::{FontIndex, GlyphRaster, TextSystem};
use std::num::NonZeroU32;
use std::sync::Arc;

const TAB: Keysym = Keysym(0xff09);

/// A window of a field that takes anything typed and a password field that does.
struct Form;

struct Field {
    protected: bool,
}

struct Column(Option<Vec<Element<()>>>);

impl Widget<()> for Column {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Start,
            justify: Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<()>> {
        self.0.take().unwrap_or_default()
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::Layout
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, ()>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl Widget<()> for Field {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(80.0, 20.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, ()>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(if self.protected {
            Role::PasswordInput
        } else {
            Role::TextInput
        });
        cx.name(if self.protected { "Password" } else { "Name" });
        cx.focusable();
        if self.protected {
            cx.protected();
        }
    }
}

impl Application for Form {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey("form".into()),
            spec: WindowSpec {
                title: "Form".into(),
                app_id: "org.cupertino.tests.Form".into(),
                size: Size::new(200.0, 100.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            popup: None,
            content: Element::new(Column(Some(vec![
                Element::new(Field { protected: false }),
                Element::new(Field { protected: true }),
            ]))),
            menus: None,
        }]
    }
}

fn options() -> RuntimeOptions {
    let theme = ThemeTokens::neutral(ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: true,
    })
    .unwrap();
    let mut fonts = FontIndex::new();
    fonts
        .add_font(Arc::new(cupertino_assets::BUILTIN_FONT.to_vec()))
        .unwrap();
    let text = Arc::new(TextSystem::new(fonts, "Inter Variable"));
    let glyphs = Arc::clone(&text);
    RuntimeOptions {
        theme: Arc::new(theme),
        text,
        icons: Arc::new(PlaceholderIcons::default()),
        direction: LayoutDirection::Ltr,
        node_limit: 100,
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
        accessibility: None,
        clock: Arc::new(ManualClock::new(Timestamp(0))),
        on_window_error: None,
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(1000).unwrap()
}

/// The form on the headless window system, configured and active.
fn open() -> (TestDriver<Form>, WindowId) {
    let mut driver = TestDriver::new(
        Capabilities::default(),
        options(),
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(turns()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    driver
        .window_system_mut()
        .configure(window, None, active)
        .unwrap();
    driver.run_until_idle(turns()).unwrap();
    (driver, window)
}

fn key(window: WindowId, keysym: Keysym, at: u64) -> PlatformEvent {
    let event = KeyEvent {
        code: 0,
        keysym,
        text: None,
        pressed: true,
        repeat: false,
        mods: Mods::NONE,
    };
    PlatformEvent::Window {
        window,
        event: WindowEvent::Key(event, Timestamp(at)),
    }
}

#[test]
fn input_to_a_protected_field_is_recorded_without_its_content_and_replays_only_with_the_tests_own()
{
    let (mut driver, window) = open();
    let mut at = 1000;
    let mut send = |driver: &mut TestDriver<Form>, keysym| {
        at += 1000;
        driver.advance_to(Timestamp(at)).unwrap();
        driver
            .window_system_mut()
            .enqueue(key(window, keysym, at))
            .unwrap();
        driver.run_until_idle(turns()).unwrap();
        at
    };
    send(&mut driver, TAB); // the name field
    send(&mut driver, Keysym(0x61));
    send(&mut driver, TAB); // the password field
    let typed_at = send(&mut driver, Keysym(0x73));
    let trace = driver.trace().clone();
    let keys: Vec<_> = trace
        .events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| match &e.input {
            RecordedInput::Event(PlatformEvent::Window {
                event: WindowEvent::Key(k, _),
                ..
            }) => Some((i, Some(k.keysym))),
            RecordedInput::Protected(ProtectedInput::Key(_)) => Some((i, None)),
            _ => None,
        })
        .collect();
    let secret_at = keys.last().unwrap().0;
    assert_eq!(
        keys.iter().map(|(_, k)| *k).collect::<Vec<_>>(),
        [Some(TAB), Some(Keysym(0x61)), Some(TAB), None],
        "the key that went to the password is the only one kept without its content"
    );
    assert_eq!(
        trace.events[secret_at].input,
        RecordedInput::Protected(ProtectedInput::Key(window))
    );
    assert!(
        !format!("{trace:?}").contains(&format!("{:?}", Keysym(0x73))),
        "nothing of the key is kept"
    );

    assert_eq!(
        trace.replay(&SecureInputs::default()),
        Err(TraceError::MissingSecureInput { index: secret_at })
    );
    let mut wrong = SecureInputs::default();
    wrong.insert(secret_at, PlatformEvent::Wake);
    assert_eq!(
        trace.replay(&wrong),
        Err(TraceError::SecureInputMismatch { index: secret_at })
    );
    let mut own = SecureInputs::default();
    own.insert(secret_at, key(window, Keysym(0x73), typed_at));
    let replayed = trace.replay(&own).unwrap();
    assert_eq!(replayed.len(), trace.events.len());
    assert_eq!(
        replayed[secret_at],
        (Timestamp(typed_at), key(window, Keysym(0x73), typed_at))
    );
    assert!(
        replayed.windows(2).all(|w| w[0].0 <= w[1].0),
        "in the order they happened"
    );
}

#[test]
fn what_a_field_pastes_is_recorded_as_protected_as_that_field_and_an_unknown_reply_is_refused() {
    let (mut driver, window) = open();
    let mut at = 0;
    let mut tab = |driver: &mut TestDriver<Form>| {
        at += 1000;
        driver.advance_to(Timestamp(at)).unwrap();
        driver
            .window_system_mut()
            .enqueue(key(window, TAB, at))
            .unwrap();
        driver.run_until_idle(turns()).unwrap();
    };
    let offer = |driver: &mut TestDriver<Form>, text: &str| {
        let data = ClipboardData {
            mime: "text/plain".into(),
            bytes: text.as_bytes().to_vec(),
        };
        driver
            .window_system_mut()
            .offer_clipboard(ClipboardKind::Clipboard, Some(data));
    };
    tab(&mut driver); // the name field
    offer(&mut driver, "plain text");
    driver
        .read_clipboard(window, ClipboardKind::Clipboard, "text/plain")
        .unwrap();
    driver.run_until_idle(turns()).unwrap();
    tab(&mut driver); // the password field
    offer(&mut driver, "s3cret");
    let token = driver
        .read_clipboard(window, ClipboardKind::Clipboard, "text/plain")
        .unwrap();
    driver.run_until_idle(turns()).unwrap();

    let replies: Vec<_> = driver
        .trace()
        .events
        .iter()
        .filter(|e| {
            matches!(
                &e.input,
                RecordedInput::Event(PlatformEvent::Clipboard { .. })
                    | RecordedInput::Protected(ProtectedInput::Clipboard(_))
            )
        })
        .map(|e| &e.input)
        .collect();
    assert_eq!(replies.len(), 2);
    let RecordedInput::Event(PlatformEvent::Clipboard {
        data: Some(public), ..
    }) = replies[0]
    else {
        panic!(
            "a read for a public field keeps what it read: {:?}",
            replies[0]
        );
    };
    assert_eq!(public.bytes, b"plain text");
    assert_eq!(
        replies[1],
        &RecordedInput::Protected(ProtectedInput::Clipboard(token)),
        "a read for the password field"
    );
    let printed = format!("{:?}", driver.trace());
    assert!(!printed.contains("s3cret") && !printed.contains(&format!("{:?}", b"s3cret")));

    let stray = PlatformEvent::Clipboard {
        token: cupertino_ui_core::platform::ClipboardToken(999),
        data: None,
    };
    driver.window_system_mut().enqueue(stray).unwrap();
    let failed = driver.run_until_idle(turns());
    assert!(
        matches!(failed, Err(DriverError::UnknownClipboardToken(_))),
        "{failed:?}"
    );
}

#[test]
fn a_trace_that_cannot_be_replayed_whole_is_refused_before_anything_is_returned() {
    let wake = |at: u64| TraceEvent {
        at: Timestamp(at),
        input: RecordedInput::Event(PlatformEvent::Wake),
    };
    let good = Trace {
        version: TRACE_VERSION,
        events: vec![wake(1), wake(1), wake(5)],
    };
    assert_eq!(
        good.replay(&SecureInputs::default()).unwrap().len(),
        3,
        "equal times are in order"
    );
    let rows = [
        (
            "a version this harness does not know",
            Trace {
                version: TRACE_VERSION + 1,
                ..good.clone()
            },
            TraceError::UnsupportedVersion(TRACE_VERSION + 1),
        ),
        (
            "an event before the one before it",
            Trace {
                events: vec![wake(5), wake(2)],
                ..good.clone()
            },
            TraceError::TimeWentBackwards { index: 1 },
        ),
    ];
    for (what, trace, error) in rows {
        assert_eq!(trace.replay(&SecureInputs::default()), Err(error), "{what}");
    }
}

#[test]
fn the_clock_moves_only_when_the_test_moves_it_or_ticks_for_a_limited_number_of_reads() {
    let clock = ManualClock::new(Timestamp(100));
    let shared = clock.clone();
    assert_eq!(clock.now(), Timestamp(100));
    assert_eq!(clock.now(), Timestamp(100), "reading does not move it");
    clock.advance_by(50);
    assert_eq!(shared.now(), Timestamp(150), "clones share the time");
    clock.advance_to(Timestamp(150)).unwrap();
    let back = clock.advance_to(Timestamp(149)).unwrap_err();
    assert_eq!((back.now, back.requested), (Timestamp(150), Timestamp(149)));
    assert_eq!(
        clock.now(),
        Timestamp(150),
        "a refused move changes nothing"
    );

    clock.tick_on_read(10, 3);
    assert_eq!(
        (clock.now(), clock.now(), clock.now()),
        (Timestamp(150), Timestamp(160), Timestamp(170))
    );
    let past = std::panic::catch_unwind(|| clock.now());
    assert!(
        past.is_err(),
        "the read past the limit fails instead of looping"
    );
    clock.tick_on_read(0, 0);
    assert_eq!(
        clock.now(),
        Timestamp(180),
        "a step of zero stops the ticking, at the time it reached"
    );
}
