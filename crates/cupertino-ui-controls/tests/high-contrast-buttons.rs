use crate::support;

use cupertino_geometry::Insets;
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Stack};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{
    Appearance, Color, ColorRole, ColorTokens, StateVariant, ThemePreferences, ThemeTokens,
};
use support::{window, Harness};

const ROLES: [ButtonRole; 3] = [
    ButtonRole::Normal,
    ButtonRole::Default,
    ButtonRole::Destructive,
];
const STYLES: [ButtonStyle; 4] = [
    ButtonStyle::Bordered,
    ButtonStyle::Borderless,
    ButtonStyle::Pill,
    ButtonStyle::RoundIcon,
];
const KEYS: [&str; 12] = [
    "n0", "n1", "n2", "n3", "d0", "d1", "d2", "d3", "x0", "x1", "x2", "x3",
];

/// One button per role and style, keyed by role letter and style index.
struct Buttons;

impl Application for Buttons {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let mut children = Vec::new();
        for (r, role) in ROLES.into_iter().enumerate() {
            for (s, style) in STYLES.into_iter().enumerate() {
                children.push(
                    Element::new(Button {
                        label: "B".into(),
                        role,
                        style,
                        enabled: true,
                        busy: false,
                        on_press: Some(()),
                    })
                    .key(KEYS[r * 4 + s]),
                );
            }
        }
        window(Element::new(Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Start,
            padding: Insets::ZERO,
            children,
        }))
    }
}

fn theme(appearance: Appearance) -> ThemeTokens {
    ThemeTokens {
        preferences: ThemePreferences {
            appearance,
            reduce_transparency: false,
            reduce_motion: false,
        },
        colors: ColorTokens::neutral(appearance),
        ..support::theme()
    }
}

fn luminance(c: Color) -> f32 {
    let lin = |v: f32| {
        if v <= 0.04045 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
}

fn contrast(a: Color, b: Color) -> f32 {
    let (x, y) = (luminance(a), luminance(b));
    (x.max(y) + 0.05) / (x.min(y) + 0.05)
}

/// Whether the button fills its chrome, and the colours of the borders it strokes.
fn chrome(h: &Harness<Buttons>, key: &'static str) -> (bool, Vec<Color>) {
    let mut filled = false;
    let mut borders = Vec::new();
    for op in h.ops(key) {
        match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { .. },
                ..
            } => filled = true,
            PaintOp::Stroke {
                shape: Shape::RoundedRect { .. },
                paint: Paint::Solid(c),
                ..
            } => borders.push(c),
            _ => {}
        }
    }
    (filled, borders)
}

#[test]
fn in_high_contrast_every_filled_button_strokes_a_border_that_stands_out_from_the_window() {
    for appearance in [Appearance::DarkHighContrast, Appearance::LightHighContrast] {
        let theme = theme(appearance);
        let window_background = theme
            .colors
            .get(ColorRole::WindowBackground, StateVariant::NORMAL)
            .unwrap();
        for key in KEYS {
            for pressed in [false, true] {
                let mut h = Harness::<Buttons>::with_theme(theme.clone());
                if pressed {
                    h.button(h.center(key), true);
                }
                let (filled, borders) = chrome(&h, key);
                if !filled {
                    continue;
                }
                assert!(
                    !borders.is_empty(),
                    "{appearance:?} {key} pressed={pressed}: a fill without a border"
                );
                for border in borders {
                    let ratio = contrast(border, window_background);
                    assert!(
                        ratio >= 3.0,
                        "{appearance:?} {key} pressed={pressed}: border {ratio:.2} < 3"
                    );
                }
            }
        }
    }
}

#[test]
fn outside_high_contrast_a_borderless_button_still_fills_without_a_border_when_pressed() {
    let mut h = Harness::<Buttons>::with_theme(theme(Appearance::Dark));
    h.button(h.center("d1"), true);
    let (filled, borders) = chrome(&h, "d1");
    assert!(filled);
    assert!(borders.is_empty());
}
