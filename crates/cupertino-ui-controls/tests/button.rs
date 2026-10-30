mod support;

use cupertino_geometry::{Rect, Size};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Stack};
use cupertino_ui_core::{Action, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{Mods, PointerEvent, TouchEvent, WindowEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, label_width, point, window, Harness, ESCAPE, RETURN, SPACE, TAB};

/// One button per role and style; `BUSY` and `ENABLED` apply to the first.
struct Buttons<const ENABLED: bool, const BUSY: bool>;
type App = Buttons<true, false>;

#[derive(Default)]
struct Model {
    pressed: Vec<&'static str>,
}

impl<const ENABLED: bool, const BUSY: bool> Application for Buttons<ENABLED, BUSY> {
    type Model = Model;
    type Message = &'static str;

    fn init() -> (Model, Vec<Command<&'static str>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: &'static str) -> Vec<Command<&'static str>> {
        model.pressed.push(message);
        Vec::new()
    }

    fn view(_: &Model) -> Vec<Window<&'static str>> {
        let button = |key: &'static str, role, style, enabled, busy| {
            Element::new(Button {
                label: key.trim_start_matches('_').into(),
                role,
                style,
                enabled,
                busy,
                on_press: Some(key),
            })
            .key(key)
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            spacing: 0.0,
            align: Align::Start,
            children: vec![
                button(
                    "ok",
                    ButtonRole::Normal,
                    ButtonStyle::Bordered,
                    ENABLED,
                    BUSY,
                ),
                button(
                    "save",
                    ButtonRole::Default,
                    ButtonStyle::Bordered,
                    true,
                    false,
                ),
                button(
                    "delete",
                    ButtonRole::Destructive,
                    ButtonStyle::Pill,
                    true,
                    false,
                ),
                button(
                    "plain",
                    ButtonRole::Normal,
                    ButtonStyle::Borderless,
                    true,
                    false,
                ),
                button(
                    "round",
                    ButtonRole::Normal,
                    ButtonStyle::RoundIcon,
                    true,
                    false,
                ),
                button("_", ButtonRole::Normal, ButtonStyle::Bordered, true, false),
            ],
        }))
    }
}

fn shapes<A: Application>(h: &Harness<A>, key: &'static str) -> Vec<(bool, Rect, f64, Color)> {
    h.ops(key)
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(c),
            } => Some((true, rect, radius, c)),
            PaintOp::Stroke {
                shape: Shape::RoundedRect { rect, radius },
                paint: Paint::Solid(c),
                ..
            } => Some((false, rect, radius, c)),
            _ => None,
        })
        .collect()
}

fn fill<A: Application>(h: &Harness<A>, key: &'static str) -> Option<Color> {
    shapes(h, key).into_iter().find(|s| s.0).map(|s| s.3)
}

fn glyph_colors<A: Application>(h: &Harness<A>, key: &'static str) -> Vec<Color> {
    h.ops(key)
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { color, .. } => Some(color),
            _ => None,
        })
        .collect()
}

#[test]
fn the_chrome_is_centred_in_a_target_padded_to_the_minimum() {
    let h = Harness::<App>::new();
    assert_eq!(h.rect("_").size, Size::new(24.0, 24.0));
    let blank = shapes(&h, "_");
    assert_eq!((blank[0].1.size, blank[0].2), (Size::new(24.0, 20.0), 5.0));
    assert!(!blank[1].0, "a bordered button strokes its border");
    assert_eq!(shapes(&h, "delete")[0].2, 10.0, "a pill is fully rounded");
    assert_eq!(
        (shapes(&h, "round")[0].1, shapes(&h, "round")[0].2),
        (Rect::new(2.0, 2.0, 20.0, 20.0), 10.0),
        "a round icon button is a circle"
    );
}

#[test]
fn the_label_widens_the_button_between_its_insets_but_not_a_round_icon_button() {
    let h = Harness::<App>::new();
    let width = label_width("delete");
    assert!(width > 24.0);
    assert_eq!(h.rect("delete").size, Size::new(width + 20.0, 24.0));
    assert_eq!(h.rect("round").size, Size::new(24.0, 24.0));
}

#[test]
fn the_label_takes_its_role_text_colour_and_a_round_icon_button_paints_none() {
    let h = Harness::<App>::new();
    let normal = |role| color(role, InteractionVariant::Normal, false);
    let texts = [
        ("ok", ColorRole::ControlText),
        ("save", ColorRole::DefaultButtonText),
        ("delete", ColorRole::DestructiveText),
        ("plain", ColorRole::ControlText),
    ];
    for (key, role) in texts {
        let colors = glyph_colors(&h, key);
        assert!(!colors.is_empty(), "{key} paints its label");
        assert!(colors.iter().all(|&c| c == normal(role)), "{key}");
    }
    assert!(glyph_colors(&h, "round").is_empty());
    assert!(glyph_colors(&h, "_").is_empty());

    let h = Harness::<Buttons<false, false>>::new();
    let disabled = color(ColorRole::ControlText, InteractionVariant::Disabled, false);
    assert!(glyph_colors(&h, "ok").iter().all(|&c| c == disabled));
}

#[test]
fn the_label_is_centred_with_its_baseline_below_the_chrome_centre() {
    let h = Harness::<App>::new();
    let size = h.rect("delete").size;
    let origins: Vec<_> = h
        .ops("delete")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { origin, .. } => Some(origin),
            _ => None,
        })
        .collect();
    assert_eq!(origins.len(), 1);
    assert!((origins[0].x - 10.0).abs() < 1e-6);
    assert!((origins[0].y - (size.height / 2.0 + 13.0 * 0.36)).abs() < 1e-4);
}

#[test]
fn roles_pick_the_fill_and_a_borderless_button_fills_only_when_pressed() {
    let mut h = Harness::<App>::new();
    let normal = |role| color(role, InteractionVariant::Normal, false);
    assert_eq!(fill(&h, "ok"), Some(normal(ColorRole::ControlFill)));
    assert_eq!(fill(&h, "save"), Some(normal(ColorRole::DefaultButtonFill)));
    assert_eq!(fill(&h, "delete"), Some(normal(ColorRole::DestructiveFill)));
    assert_eq!(fill(&h, "plain"), None);

    let center = h.center("plain");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert_eq!(
        fill(&h, "plain"),
        Some(color(
            ColorRole::ControlFill,
            InteractionVariant::Pressed,
            false
        ))
    );
}

#[test]
fn every_style_but_borderless_strokes_the_control_border_in_its_state() {
    let border = |h: &Harness<App>, key| shapes(h, key).into_iter().find(|s| !s.0).map(|s| s.3);
    let mut h = Harness::<App>::new();
    let normal = Some(color(
        ColorRole::ControlBorder,
        InteractionVariant::Normal,
        false,
    ));
    for key in ["ok", "save", "delete", "round"] {
        assert_eq!(border(&h, key), normal, "{key}");
    }
    assert_eq!(border(&h, "plain"), None);

    let center = h.center("save");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert_eq!(
        border(&h, "save"),
        Some(color(
            ColorRole::ControlBorder,
            InteractionVariant::Pressed,
            false
        ))
    );
}

#[test]
fn a_click_activates_once_and_a_release_elsewhere_or_a_cancel_does_nothing() {
    let mut h = Harness::<App>::new();
    h.click(h.center("ok"));
    assert_eq!(h.model().pressed, ["ok"]);

    let center = h.center("ok");
    h.button(center, true);
    h.pointer(PointerEvent::Motion {
        position: point(150.0, 90.0),
    });
    h.button(point(150.0, 90.0), false);

    h.button(center, true);
    h.pointer(PointerEvent::Leave);
    h.button(center, false);

    h.touch(TouchEvent::Down {
        id: 1,
        position: center,
    });
    h.touch(TouchEvent::Cancel);
    assert_eq!(h.model().pressed, ["ok"]);

    h.touch(TouchEvent::Down {
        id: 2,
        position: center,
    });
    h.touch(TouchEvent::Up { id: 2 });
    assert_eq!(h.model().pressed, ["ok", "ok"]);
}

#[test]
fn deactivating_the_window_while_space_is_held_ends_the_press() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    h.key(SPACE, true);
    h.send(WindowEvent::Focus(false));
    h.send(WindowEvent::Focus(true));
    h.key(SPACE, false);
    assert!(
        h.model().pressed.is_empty(),
        "the release of the cancelled press does nothing"
    );
    h.key(SPACE, true);
    h.key(SPACE, false);
    assert_eq!(
        h.model().pressed,
        ["ok"],
        "the next press and release activate once"
    );
}

#[test]
fn space_activates_on_release_return_on_press_and_a_repeat_or_escape_never() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    assert!(h.focused("ok"));

    h.key(SPACE, true);
    h.key_event(SPACE, true, true, Mods::NONE);
    assert!(h.model().pressed.is_empty());
    h.key(SPACE, false);
    assert_eq!(h.model().pressed, ["ok"]);

    h.key(RETURN, true);
    h.key_event(RETURN, true, true, Mods::NONE);
    h.key(RETURN, false);
    assert_eq!(h.model().pressed, ["ok", "ok"]);

    h.key(SPACE, true);
    h.tap(ESCAPE);
    h.key(SPACE, false);
    assert_eq!(h.model().pressed, ["ok", "ok"], "Escape disarms Space");
}

#[test]
fn deactivating_the_window_mid_press_ends_the_press_without_activating() {
    let mut h = Harness::<App>::new();
    let center = h.center("ok");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert_eq!(
        fill(&h, "ok"),
        Some(color(
            ColorRole::ControlFill,
            InteractionVariant::Pressed,
            false
        ))
    );
    h.send(WindowEvent::Focus(false));
    assert_ne!(
        fill(&h, "ok"),
        Some(color(
            ColorRole::ControlFill,
            InteractionVariant::Pressed,
            false
        ))
    );
    h.send(WindowEvent::Focus(true));
    h.button(center, false);
    assert!(h.model().pressed.is_empty());
}

#[test]
fn a_space_repeat_that_arrives_with_focus_neither_arms_nor_activates() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    h.key_event(SPACE, true, true, Mods::NONE);
    h.key(SPACE, false);
    assert!(h.model().pressed.is_empty());
}

#[test]
fn a_keyboard_focus_rings_the_chrome() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    let ring = shapes(&h, "ok").into_iter().last().unwrap();
    assert_eq!(
        ring,
        (
            false,
            Rect::new(-3.0, -1.0, label_width("ok") + 20.0 + 6.0, 26.0),
            8.0,
            color(
                ColorRole::KeyboardFocusIndicator,
                InteractionVariant::Normal,
                false
            )
        )
    );
}

#[test]
fn it_is_a_button_named_by_its_label_that_can_be_clicked() {
    let h = Harness::<App>::new();
    let node = h.accessible("ok");
    assert_eq!(node.role, Role::Button);
    assert_eq!(node.name.as_deref(), Some("ok"));
    assert_eq!(node.actions, [Action::Click]);
    assert!(node.states.focusable && !node.states.busy && !node.states.disabled);
}

#[test]
fn a_busy_button_says_so_and_ignores_activation() {
    let mut h = Harness::<Buttons<true, true>>::new();
    h.click(h.center("ok"));
    h.tap(TAB);
    h.tap(SPACE);
    h.tap(RETURN);
    assert!(h.model().pressed.is_empty());
    let node = h.accessible("ok");
    assert!(node.states.busy && node.states.focusable);
    assert!(node.actions.is_empty());
}

#[test]
fn a_disabled_button_looks_disabled_skips_tab_and_ignores_activation() {
    let mut h = Harness::<Buttons<false, false>>::new();
    h.click(h.center("ok"));
    h.tap(TAB);
    assert!(!h.focused("ok"));
    assert!(h.model().pressed.is_empty());
    assert_eq!(
        fill(&h, "ok"),
        Some(color(
            ColorRole::ControlFill,
            InteractionVariant::Disabled,
            false
        ))
    );
    let node = h.accessible("ok");
    assert!(node.states.disabled && !node.states.focusable);
}
