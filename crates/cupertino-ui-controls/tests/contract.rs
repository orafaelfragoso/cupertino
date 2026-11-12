//! What every control shares: a disabled control never looks like one in a window that is not active, each display
//! option works alone and with the others, a held thumb's lens follows the pointer whatever the options, and glass
//! is never laid on glass.

use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Canvas, CheckBox, PopUpButton, RadioGroup, SegmentedControl,
    Slider, SliderStyle, Stack, Stepper, TextField, Toggle,
};
use cupertino_ui_core::{Application, Checked, Command, Element, Glass, Key, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{PointerEvent, WindowEvent};
use cupertino_ui_render::{Paint, PaintNodeId, PaintOp, Shape};
use cupertino_ui_text::ContentPurpose;
use cupertino_ui_theme::{
    choice_mark, control_geometry, slider_geometry, switch_geometry, Appearance, ControlFamily,
    ControlSize, ThemeTokens,
};
use std::cell::Cell;
use support::{window, Harness};

thread_local! {
    /// Whether the controls of the next window are enabled.
    static ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// The keys of the controls, one of each family; a radio group's buttons are its children, so its first one stands
/// for it.
const CONTROLS: [&str; 9] = [
    "button", "check", "radio", "switch", "slider", "segments", "popup", "field", "stepper",
];

struct Gallery;

impl Application for Gallery {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let enabled = ENABLED.with(Cell::get);
        let button = |style| Button {
            label: "Save".into(),
            role: ButtonRole::Normal,
            style,
            enabled,
            busy: false,
            on_press: Some(()),
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::new(button(ButtonStyle::Bordered)).key("button"),
                Element::new(button(ButtonStyle::Glass)).key("glass"),
                Element::new(CheckBox {
                    label: "Check".into(),
                    checked: Checked::False,
                    enabled,
                    on_toggle: Some(Box::new(|_| ())),
                })
                .key("check"),
                Element::new(RadioGroup {
                    label: "Size".into(),
                    options: vec!["Small".into(), "Large".into()],
                    selected: Some(0),
                    axis: Axis::Horizontal,
                    spacing: 8.0,
                    enabled,
                    on_select: Some(Box::new(|_| ())),
                })
                .key("radio"),
                Element::new(Toggle {
                    label: "Switch".into(),
                    on: false,
                    enabled,
                    on_toggle: Some(Box::new(|_| ())),
                })
                .key("switch"),
                Element::new(Slider {
                    label: "Volume".into(),
                    value: 0.4,
                    min: 0.0,
                    max: 1.0,
                    step: None,
                    style: SliderStyle::Regular,
                    ticks: 0,
                    enabled,
                    on_change: Some(Box::new(|_| ())),
                    on_end: None,
                })
                .width(200.0)
                .key("slider"),
                Element::new(SegmentedControl {
                    label: "View".into(),
                    segments: vec!["Icons".into(), "List".into()],
                    selected: Some(0),
                    enabled,
                    on_select: Some(Box::new(|_| ())),
                })
                .key("segments"),
                Element::new(PopUpButton {
                    label: "Sort".into(),
                    options: vec!["Name".into(), "Date".into()],
                    selected: 0,
                    open: false,
                    enabled,
                    on_open: Some(()),
                })
                .key("popup"),
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: "Name".into(),
                    value: String::new(),
                    revision: 0,
                    purpose: ContentPurpose::Normal,
                    enabled,
                    on_change: Some(Box::new(|_| ())),
                    on_submit: None,
                    selection: None,
                    initial: false,
                })
                .width(160.0)
                .key("field"),
                Element::new(Stepper {
                    label: "Copies".into(),
                    value: 2.0,
                    min: 1.0,
                    max: 9.0,
                    step: 1.0,
                    enabled,
                    on_change: Some(Box::new(|_| ())),
                })
                .key("stepper"),
            ],
        }))
    }
}

fn harness(theme: ThemeTokens, enabled: bool) -> Harness<Gallery> {
    ENABLED.with(|e| e.set(enabled));
    let mut h = Harness::with_theme(theme);
    h.settle();
    h
}

/// The fixture theme with these display options.
fn with_options(dark: bool, contrast: bool, transparency: bool, motion: bool) -> ThemeTokens {
    let mut theme = support::theme();
    theme.preferences.appearance = match (dark, contrast) {
        (false, false) => Appearance::Light,
        (true, false) => Appearance::Dark,
        (false, true) => Appearance::LightHighContrast,
        (true, true) => Appearance::DarkHighContrast,
    };
    theme.preferences.reduce_transparency = transparency;
    theme.preferences.reduce_motion = motion;
    theme.motion.reduce = motion;
    theme
}

/// What the control `key` painted: a radio group's, its first button's.
fn ops<A: Application>(h: &Harness<A>, key: &'static str) -> Vec<PaintOp> {
    if key != "radio" {
        return h.ops(key);
    }
    let snapshot = h.snapshot();
    let group = snapshot
        .nodes
        .iter()
        .find(|n| n.key == Some(Key::from(key)))
        .expect("the radio group");
    let first = PaintNodeId::from(group.children[0]);
    snapshot
        .display_list
        .nodes
        .into_iter()
        .find(|n| n.id == first)
        .map(|n| n.ops)
        .unwrap_or_default()
}

/// The edges `ops` stroke: rounded outlines, not the strokes of chevrons and ticks.
fn strokes(ops: &[PaintOp]) -> usize {
    ops.iter()
        .filter(|op| {
            matches!(
                op,
                PaintOp::Stroke {
                    shape: Shape::RoundedRect { .. },
                    ..
                }
            )
        })
        .count()
}

/// The opaque solid fills among `ops`.
fn opaque_fills(ops: &[PaintOp]) -> usize {
    ops.iter()
        .filter(|op| matches!(op, PaintOp::Fill { paint: Paint::Solid(c), .. } if c.a >= 0.999))
        .count()
}

#[test]
fn a_disabled_control_never_looks_like_one_in_a_window_that_is_not_active() {
    for dark in [false, true] {
        let theme = || with_options(dark, false, false, false);
        let active = harness(theme(), true);
        let mut inactive = harness(theme(), true);
        inactive.send(WindowEvent::Focus(false));
        inactive.settle();
        let disabled = harness(theme(), false);
        for key in CONTROLS {
            assert_ne!(
                ops(&disabled, key),
                ops(&inactive, key),
                "{key}, dark {dark}: disabled is not the inactive window's look"
            );
            assert_ne!(
                ops(&disabled, key),
                ops(&active, key),
                "{key}, dark {dark}: disabled shows"
            );
        }
    }
}

#[test]
fn every_combination_of_contrast_transparency_and_motion_paints_every_control_its_own_way() {
    for dark in [false, true] {
        for contrast in [false, true] {
            for transparency in [false, true] {
                for motion in [false, true] {
                    let case = format!(
                        "dark {dark}, contrast {contrast}, transparency {transparency}, motion {motion}"
                    );
                    let mut h = harness(with_options(dark, contrast, transparency, motion), true);
                    for key in CONTROLS {
                        assert!(!ops(&h, key).is_empty(), "{key} paints, {case}");
                    }
                    // Increase Contrast and nothing else strokes the quiet bezels' edges
                    for key in ["button", "segments", "popup", "stepper"] {
                        assert_eq!(strokes(&h.ops(key)) > 0, contrast, "{key}'s edge, {case}");
                    }
                    // Reduce Transparency and nothing else makes the glass button's body opaque (in the light
                    // appearance an opaque copy of it casts its shade either way)
                    if dark {
                        assert_eq!(
                            opaque_fills(&h.ops("glass")) > 0,
                            transparency,
                            "the glass button's body, {case}"
                        );
                    }
                    // the held switch is a lens under every option: Reduce Motion changes no state's look
                    let rest = h.ops("switch");
                    let at = h.center("switch");
                    h.pointer(PointerEvent::Motion { position: at });
                    h.button(at, true);
                    let held = h.ops("switch");
                    h.button(at, false);
                    assert_ne!(rest, held, "the held switch's lens, {case}");
                    assert!(
                        held.iter().any(|op| matches!(op, PaintOp::PushClip(_))),
                        "the lens clips what shows through it, {case}"
                    );
                }
            }
        }
    }
}

#[test]
fn increased_contrast_strengthens_the_lens_rim_and_reduced_transparency_fills_its_body() {
    let lens = |contrast, transparency| {
        let mut h = harness(with_options(true, contrast, transparency, false), true);
        let at = h.center("switch");
        h.pointer(PointerEvent::Motion { position: at });
        h.button(at, true);
        h.ops("switch")
    };
    let rim_alpha = |ops: &[PaintOp]| {
        ops.iter()
            .filter_map(|op| match op {
                PaintOp::Stroke {
                    paint: Paint::Solid(c),
                    ..
                } => Some(c.a),
                _ => None,
            })
            .fold(0.0f32, f32::max)
    };
    let body_alpha = |ops: &[PaintOp]| {
        ops.iter()
            .filter_map(|op| match op {
                PaintOp::Fill {
                    paint: Paint::Solid(c),
                    ..
                } => Some(c.a),
                _ => None,
            })
            .fold(0.0f32, f32::max)
    };
    let plain = lens(false, false);
    assert!(rim_alpha(&lens(true, false)) > rim_alpha(&plain));
    assert!(body_alpha(&lens(false, true)) > body_alpha(&plain));
}

/// A glass button on a panel of glass and one beside it.
struct Nested;

impl Application for Nested {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let button = || {
            Element::new(Button {
                label: "Done".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Glass,
                enabled: true,
                busy: false,
                on_press: Some(()),
            })
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                Element::new(Canvas {
                    size: Some(Size::new(160.0, 60.0)),
                    label: "Panel".into(),
                    paint: Box::new(|cx| {
                        cx.glass(Glass::regular().radius(12.0));
                    }),
                    children: vec![button().key("on-glass")],
                }),
                button().key("on-window"),
            ],
        }))
    }
}

#[test]
fn a_glass_button_on_glass_lays_no_glass_of_its_own() {
    let mut h = Harness::<Nested>::with_effects(with_options(true, false, false, false));
    h.settle();
    let fills = |ops: &[PaintOp]| {
        ops.iter()
            .filter(|op| matches!(op, PaintOp::Fill { .. }))
            .count()
    };
    assert!(
        fills(&h.ops("on-window")) > 0,
        "on the window, its own glass"
    );
    assert_eq!(fills(&h.ops("on-glass")), 0, "on glass, its label alone");
}

/// The rounded fills among `ops`, as their sizes.
fn fill_sizes(ops: &[PaintOp]) -> Vec<(f64, f64)> {
    ops.iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                ..
            } => Some((rect.width(), rect.height())),
            _ => None,
        })
        .collect()
}

#[test]
fn each_control_takes_its_size_from_the_table_at_every_size_normal_and_compact() {
    let near = |a: (f64, f64), b: (f64, f64)| (a.0 - b.0).abs() < 1e-6 && (a.1 - b.1).abs() < 1e-6;
    for size in ControlSize::ASCENDING {
        for compact in [false, true] {
            let case = format!("{size:?}, compact {compact}");
            let mut theme = with_options(true, false, false, false);
            theme.metrics = theme.metrics.at_size(size, compact);
            let h = harness(theme, true);
            let button = control_geometry(ControlFamily::PushButton, size, compact);
            let bezel = fill_sizes(&h.ops("button"));
            assert!(
                bezel.iter().any(|s| (s.1 - button.height).abs() < 1e-6),
                "the button's bezel is {} high, {case}: {bezel:?}",
                button.height
            );
            let mark = choice_mark(size, compact);
            assert!(
                fill_sizes(&h.ops("check")).contains(&(mark, mark)),
                "the check box is {mark} square, {case}"
            );
            let switch = switch_geometry(compact);
            let parts = fill_sizes(&h.ops("switch"));
            assert!(
                near(parts[0], switch.track),
                "the switch's track, {case}: {parts:?}"
            );
            assert!(
                (parts[1].1 - (switch.track.1 - 2.0 * switch.inset)).abs() < 1e-6,
                "the thumb fills the track's height inside its inset, {case}: {parts:?}"
            );
            let knob = slider_geometry(size).knob;
            assert!(
                fill_sizes(&h.ops("slider")).iter().any(|&s| near(s, knob)),
                "the slider's knob is {knob:?}, {case}"
            );
            let popup = control_geometry(ControlFamily::PopUp, size, compact);
            assert!(
                fill_sizes(&h.ops("popup"))
                    .iter()
                    .any(|s| (s.1 - popup.height).abs() < 1e-6),
                "the pop-up button is {} high, {case}",
                popup.height
            );
            let stepper = control_geometry(ControlFamily::Stepper, size, compact);
            let stepper = (stepper.width.unwrap(), stepper.height);
            assert!(
                fill_sizes(&h.ops("stepper"))
                    .iter()
                    .any(|&s| near(s, stepper)),
                "the stepper is {stepper:?}, {case}"
            );
            let segments = control_geometry(ControlFamily::Segmented, size, compact);
            assert!(
                fill_sizes(&h.ops("segments"))
                    .iter()
                    .any(|s| (s.1 - segments.height).abs() < 1e-6),
                "the segmented control is {} high, {case}",
                segments.height
            );
        }
    }
}
