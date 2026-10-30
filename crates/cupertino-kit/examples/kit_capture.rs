//! The toolkit's surfaces for look checks, each a window drawn on the headless window system with the CPU renderer:
//! `kit/controls`, a sheet of the choice and value controls in their states (check boxes, radio buttons, a segmented
//! control, sliders and progress), and `kit/icons`, a grid of system symbols at the toolkit's sizes and tints.
//!
//! `ci/capture.sh kit/controls --scale 2` (the flags are `cupertino-capture`'s)
//!
//! The fonts, symbols and theme are the system's, as an application's ([`cupertino_kit::app::options`]). The clock
//! stands still, so every control shows its resting look.

use std::num::NonZeroU32;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_capture::{App, Look, Request, State};
use cupertino_kit::controls::{
    CheckBox, Icon, IconDirection, Progress, RadioGroup, SegmentedControl, Slider, SliderStyle,
    TextRole, WINDOW_CONTENT_INSET,
};
use cupertino_kit::core::{Checked, CommandRegistry};
use cupertino_kit::geometry::{Insets, Size};
use cupertino_kit::platform::{
    Capabilities, DecorationMode, SystemAppearance, Timestamp, WindowEvent, WindowStates,
};
use cupertino_kit::prelude::*;
use cupertino_kit::render::PixelFormat;
use cupertino_kit::text::{AttributedText, TextStyle};
use cupertino_kit::theme::{ColorRole, ThemeTokens};
use cupertino_theme::Theme;
use cupertino_ui_test::{ManualClock, TestDriver};
use tiny_skia::{IntSize, Pixmap};

/// Between the sheet's sections and between a section's title and its controls, an approximate design value.
const SECTION_GAP: f64 = 12.0;
/// Between the controls of a row, an approximate design value.
const ROW_GAP: f64 = 16.0;
/// The symbols of the icon grid: the toolbar, sidebar and status symbols the apps draw.
const SYMBOLS: [&str; 12] = [
    "symbol.chevron.left",
    "symbol.chevron.right",
    "symbol.magnifyingglass",
    "symbol.house",
    "symbol.desktop",
    "symbol.document",
    "symbol.download",
    "symbol.trash",
    "symbol.wifi",
    "symbol.bluetooth",
    "symbol.speaker.high",
    "symbol.battery",
];
/// The icon sizes of the grid in points: small, regular and large symbols.
const ICON_SIZES: [f64; 3] = [16.0, 20.0, 24.0];
/// The runtime turns a frame may take.
const TURNS: u32 = 10_000;
/// The frames a window may ask for before it rests.
const FRAMES: usize = 8;

#[derive(Clone, Debug, PartialEq)]
enum Message {}

fn text(content: &str, role: TextRole) -> Element<Message> {
    Element::new(Text {
        content: AttributedText::plain(content, TextStyle::new(13.0)),
        role,
        wrap: false,
    })
}

fn stack(axis: Axis, spacing: f64, children: Vec<Element<Message>>) -> Element<Message> {
    Element::new(Stack {
        axis,
        padding: Insets::ZERO,
        spacing,
        align: if axis == Axis::Vertical {
            Align::Stretch
        } else {
            Align::Center
        },
        children,
    })
}

fn section(title: &str, content: Element<Message>) -> Element<Message> {
    stack(
        Axis::Vertical,
        SECTION_GAP,
        vec![text(title, TextRole::Section), content],
    )
}

fn window(title: &'static str, size: Size, content: Vec<Element<Message>>) -> Vec<Window<Message>> {
    vec![Window {
        key: WindowKey(title.into()),
        spec: WindowSpec {
            title: title.into(),
            app_id: "org.cupertino.examples.Capture".into(),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::Preferred,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::Theme,
        content: Element::new(Stack {
            axis: Axis::Vertical,
            padding: WINDOW_CONTENT_INSET,
            spacing: ROW_GAP,
            align: Align::Stretch,
            children: content,
        }),
    }]
}

struct Controls;

impl Application for Controls {
    type Model = ();
    type Message = Message;

    fn init() -> ((), Vec<Command<Message>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), message: Message) -> Vec<Command<Message>> {
        match message {}
    }

    fn view(_: &()) -> Vec<Window<Message>> {
        let check = |label: &str, checked, enabled| {
            Element::new(CheckBox {
                label: label.into(),
                checked,
                enabled,
                on_toggle: None,
            })
        };
        let slider = |label: &str, value, ticks, style, enabled| {
            Element::new(Slider {
                label: label.into(),
                value,
                min: 0.0,
                max: 1.0,
                step: None,
                style,
                ticks,
                enabled,
                on_change: None,
            })
        };
        let progress = |label: &str, value| {
            Element::new(Progress {
                label: label.into(),
                value: Some(value),
            })
        };
        window(
            "Controls",
            Size::new(460.0, 560.0),
            vec![
                section(
                    "Check boxes",
                    stack(
                        Axis::Horizontal,
                        ROW_GAP,
                        vec![
                            check("Off", Checked::False, true),
                            check("On", Checked::True, true),
                            check("Mixed", Checked::Mixed, true),
                            check("Disabled", Checked::True, false),
                        ],
                    ),
                ),
                section(
                    "Radio buttons",
                    stack(
                        Axis::Horizontal,
                        ROW_GAP,
                        vec![
                            Element::new(RadioGroup {
                                label: "Size".into(),
                                options: vec!["Small".into(), "Medium".into(), "Large".into()],
                                selected: Some(1),
                                axis: Axis::Vertical,
                                spacing: 6.0,
                                enabled: true,
                                on_select: None,
                            }),
                            Element::new(RadioGroup {
                                label: "Disabled".into(),
                                options: vec!["Left".into(), "Right".into()],
                                selected: Some(0),
                                axis: Axis::Vertical,
                                spacing: 6.0,
                                enabled: false,
                                on_select: None,
                            }),
                        ],
                    ),
                ),
                section(
                    "Segmented control",
                    stack(
                        Axis::Horizontal,
                        ROW_GAP,
                        vec![
                            Element::new(SegmentedControl {
                                label: "Range".into(),
                                segments: vec!["Day".into(), "Week".into(), "Month".into()],
                                selected: Some(1),
                                enabled: true,
                                on_select: None,
                            }),
                            Element::new(SegmentedControl {
                                label: "Disabled".into(),
                                segments: vec!["On".into(), "Off".into()],
                                selected: Some(0),
                                enabled: false,
                                on_select: None,
                            }),
                        ],
                    ),
                ),
                section(
                    "Sliders",
                    stack(
                        Axis::Vertical,
                        SECTION_GAP,
                        vec![
                            slider("Regular", 0.3, 0, SliderStyle::Regular, true),
                            slider("Ticks", 0.5, 5, SliderStyle::Regular, true),
                            slider("Level", 0.6, 0, SliderStyle::Level, true),
                            slider("Disabled", 0.4, 0, SliderStyle::Regular, false),
                        ],
                    ),
                ),
                section(
                    "Progress",
                    stack(
                        Axis::Vertical,
                        SECTION_GAP,
                        vec![progress("Started", 0.1), progress("Half", 0.5)],
                    ),
                ),
            ],
        )
    }
}

struct Icons;

impl Application for Icons {
    type Model = ();
    type Message = Message;

    fn init() -> ((), Vec<Command<Message>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), message: Message) -> Vec<Command<Message>> {
        match message {}
    }

    fn view(_: &()) -> Vec<Window<Message>> {
        let icon = |name: &str, size, tint| {
            Element::new(Icon {
                name: name.into(),
                size,
                direction: IconDirection::Fixed,
                label: None,
                tint,
            })
        };
        let rows = SYMBOLS
            .iter()
            .map(|name| {
                let mut row: Vec<Element<Message>> =
                    ICON_SIZES.iter().map(|&s| icon(name, s, None)).collect();
                row.push(icon(name, 20.0, Some(ColorRole::SecondaryText)));
                row.push(icon(name, 20.0, Some(ColorRole::KeyboardFocusIndicator)));
                row.push(text(name.trim_start_matches("symbol."), TextRole::Caption));
                stack(Axis::Horizontal, ROW_GAP, row)
            })
            .collect();
        window(
            "Icons",
            Size::new(340.0, 440.0),
            vec![stack(Axis::Vertical, 8.0, rows)],
        )
    }
}

/// The window of `A` drawn at rest in `look` at `scale`, `size` points when given.
fn draw<A: Application>(look: Look, scale: f32, size: Option<Size>) -> Result<Pixmap, String> {
    let mut options = cupertino_kit::app::options().map_err(|e| e.to_string())?;
    let preferences = cupertino_kit::app::preferences(&SystemAppearance {
        dark: look.dark,
        increase_contrast: look.increase_contrast,
        reduce_transparency: look.reduce_transparency,
        reduce_motion: false,
    });
    let theme = Theme::load().map_err(|e| e.to_string())?;
    options.theme =
        Arc::new(ThemeTokens::cupertino(&theme, preferences).map_err(|e| e.to_string())?);
    let turns = NonZeroU32::new(TURNS).expect("a turn budget");
    let fail = |e: &dyn std::fmt::Debug| format!("the headless window system: {e:?}");
    let mut driver = TestDriver::<A>::new(
        Capabilities::default(),
        options,
        ManualClock::new(Timestamp(0)),
    );
    driver.run_until_idle(turns).map_err(|e| fail(&e))?;
    let window = driver.runtime().windows()[0].1;
    let system = driver.window_system_mut();
    system
        .enqueue_window(window, WindowEvent::Scale(f64::from(scale)))
        .map_err(|e| fail(&e))?;
    let active = WindowStates {
        activated: true,
        ..WindowStates::default()
    };
    system
        .configure(window, size, active)
        .map_err(|e| fail(&e))?;
    let mut rested = false;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns).map_err(|e| fail(&e))?;
        let system = driver.window_system_mut();
        if !system.frame_requested(window).map_err(|e| fail(&e))? {
            rested = true;
            break;
        }
        system
            .frame_ready(window, Timestamp(0))
            .map_err(|e| fail(&e))?;
    }
    if !rested {
        return Err("the window keeps asking for frames".into());
    }
    let frame = driver
        .window_system_mut()
        .frames(window)
        .map_err(|e| fail(&e))?
        .last()
        .ok_or("the window presented no frame")?;
    let mut pixels = frame.pixels.clone();
    if frame.format == PixelFormat::Rgba8 {
        for px in pixels.as_chunks_mut::<4>().0 {
            let a = u16::from(px[3]);
            for c in &mut px[..3] {
                *c = ((u16::from(*c) * a + 127) / 255) as u8;
            }
        }
    }
    let size = IntSize::from_wh(frame.width, frame.height).ok_or("the frame is empty")?;
    Pixmap::from_vec(pixels, size).ok_or_else(|| "the frame's pixels do not fill it".to_string())
}

struct Capture;

impl App for Capture {
    const NAME: &'static str = "kit";
    const STATE_FLAGS: &'static [&'static str] = &[];

    fn surfaces(&self) -> Vec<String> {
        vec!["controls".into(), "icons".into()]
    }

    fn sheet_states(&self, _: &str) -> Vec<State> {
        vec![State::new()]
    }

    fn capture(&mut self, r: &Request<'_>) -> Result<Pixmap, String> {
        let size = r.size.map(|(w, h)| Size::new(w, h));
        match r.surface {
            "controls" => draw::<Controls>(r.look, r.scale, size),
            _ => draw::<Icons>(r.look, r.scale, size),
        }
    }
}

fn main() -> ExitCode {
    cupertino_capture::run(|| cupertino_capture::assets().map(|_| Capture))
}
