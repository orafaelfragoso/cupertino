use crate::support;

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Stack};
use cupertino_ui_core::{Action, Application, Command, Element, Role, Window, WindowBackground};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{
    GlassStyle, MaterialRegion, Mods, PointerEvent, TouchEvent, WindowEvent, WindowRequest,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant, Surface};
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
            padding: Insets::ZERO,
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

/// The quiet bezel of the light test theme: a share of black.
fn bezel(share: f32) -> Color {
    Color::new(0.0, 0.0, 0.0, share)
}

const BEZEL: f32 = 0.075;
const BEZEL_PRESSED: f32 = 0.16;
const BEZEL_DISABLED: f32 = 0.035;

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
fn the_button_lays_out_on_its_chrome() {
    let h = Harness::<App>::new();
    assert_eq!(h.rect("_").size, Size::new(20.0, 20.0));
    let blank = shapes(&h, "_");
    assert_eq!((blank[0].1.size, blank[0].2), (Size::new(20.0, 20.0), 5.0));
    assert!(
        blank.iter().all(|s| s.0),
        "a bordered button is a quiet fill with no edge"
    );
    assert_eq!(shapes(&h, "delete")[0].2, 10.0, "a pill is fully rounded");
    assert_eq!(
        (shapes(&h, "round")[0].1, shapes(&h, "round")[0].2),
        (Rect::new(0.0, 0.0, 20.0, 20.0), 10.0),
        "a round icon button is a circle"
    );
}

/// Two buttons whose targets overlap in the spacing between them.
struct Pair;

impl Application for Pair {
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
        let button = |key: &'static str| {
            Element::new(Button {
                label: key.into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(key),
            })
            .key(key)
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(10.0),
            spacing: 2.0,
            align: Align::Start,
            children: vec![button("a"), button("b")],
        }))
    }
}

#[test]
fn a_stack_spaces_the_chrome_and_a_press_in_the_spacing_goes_to_the_nearer_button() {
    let mut h = Harness::<Pair>::new();
    let (a, b) = (h.rect("a"), h.rect("b"));
    assert_eq!(b.min_y() - a.max_y(), 2.0);
    assert_eq!(
        shapes(&h, "a")[0].1.size,
        a.size,
        "the chrome fills the node"
    );

    h.click(point(a.center().x, a.max_y() + 0.5));
    h.click(point(b.center().x, b.min_y() - 0.5));
    h.click(point(a.center().x, a.min_y() - 1.5));
    h.click(point(a.center().x, a.min_y() - 2.5));
    assert_eq!(h.model().pressed, ["a", "b", "a"]);
}

#[test]
fn the_label_widens_the_button_between_its_insets_but_not_a_round_icon_button() {
    let h = Harness::<App>::new();
    let width = label_width("delete");
    assert!(width > 24.0);
    assert_eq!(h.rect("delete").size, Size::new(width + 20.0, 20.0));
    assert_eq!(h.rect("round").size, Size::new(20.0, 20.0));
}

#[test]
fn the_label_takes_its_role_text_colour_and_a_round_icon_button_paints_none() {
    let h = Harness::<App>::new();
    let normal = |role| color(role, InteractionVariant::Normal, false);
    let texts = [
        ("ok", ColorRole::ControlText),
        ("save", ColorRole::DefaultButtonText),
        ("delete", ColorRole::DestructiveFill),
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
    assert_eq!(fill(&h, "ok"), Some(bezel(BEZEL)));
    assert_eq!(fill(&h, "save"), Some(normal(ColorRole::DefaultButtonFill)));
    let red = normal(ColorRole::DestructiveFill);
    let tint = fill(&h, "delete").unwrap();
    assert_eq!((tint.r, tint.g, tint.b), (red.r, red.g, red.b));
    assert!(
        tint.a < 0.2,
        "a destructive button is a faint red tint under a red label"
    );
    assert_eq!(fill(&h, "plain"), None);

    let center = h.center("plain");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert_eq!(fill(&h, "plain"), Some(bezel(BEZEL_PRESSED)));
}

#[test]
fn no_button_strokes_an_edge_outside_increase_contrast_pressed_or_not() {
    let border = |h: &Harness<App>, key| shapes(h, key).into_iter().find(|s| !s.0).map(|s| s.3);
    let mut h = Harness::<App>::new();
    for key in ["ok", "save", "delete", "round", "plain"] {
        assert_eq!(border(&h, key), None, "{key}");
    }
    let center = h.center("save");
    h.pointer(PointerEvent::Motion { position: center });
    h.button(center, true);
    assert_eq!(border(&h, "save"), None);
    assert_eq!(
        fill(&h, "save"),
        Some(color(
            ColorRole::DefaultButtonFill,
            InteractionVariant::Pressed,
            false
        ))
    );
}

#[test]
fn in_a_window_that_is_not_active_the_default_button_rests_as_an_ordinary_one_and_is_not_disabled()
{
    let mut h = Harness::<App>::new();
    h.send(WindowEvent::Focus(false));
    assert_eq!(fill(&h, "save"), Some(bezel(BEZEL)));
    let label = glyph_colors(&h, "save");
    assert!(!label.is_empty());
    assert!(label
        .iter()
        .all(|&c| c != color(ColorRole::ControlText, InteractionVariant::Disabled, false)));
    let disabled = Harness::<Buttons<false, false>>::new();
    assert_ne!(
        fill(&disabled, "ok"),
        fill(&h, "ok"),
        "disabled is not the inactive window's look"
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
    assert_eq!(fill(&h, "ok"), Some(bezel(BEZEL_PRESSED)));
    h.send(WindowEvent::Focus(false));
    assert_ne!(fill(&h, "ok"), Some(bezel(BEZEL_PRESSED)));
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
            Rect::new(-3.0, -3.0, label_width("ok") + 20.0 + 6.0, 26.0),
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
fn an_inactive_window_shows_no_focus_ring_and_it_returns_with_the_window() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    let rings = |h: &Harness<App>| shapes(h, "ok").into_iter().filter(|s| !s.0).count();
    let shown = rings(&h);
    h.send(WindowEvent::Focus(false));
    assert!(h.focused("ok"), "the window keeps its focus");
    assert_eq!(rings(&h), shown - 1);
    h.send(WindowEvent::Focus(true));
    assert_eq!(rings(&h), shown);
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
    assert_eq!(fill(&h, "ok"), Some(bezel(BEZEL_DISABLED)));
    let node = h.accessible("ok");
    assert!(node.states.disabled && !node.states.focusable);
}

/// A glass button alone in a window whose background is a heads-up display's material (a material, not glass) when
/// `ON_MATERIAL`, the theme's own opaque background otherwise.
struct GlassButtonWindow<const ON_MATERIAL: bool>;

impl<const ON_MATERIAL: bool> Application for GlassButtonWindow<ON_MATERIAL> {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let mut windows = window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::uniform(20.0),
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Button {
                label: "Glass".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Glass,
                enabled: true,
                busy: false,
                on_press: Some(()),
            })
            .key("glass")],
        }));
        if ON_MATERIAL {
            windows[0].background = WindowBackground::Surface {
                surface: Surface::Hud,
                radius: 0.0,
                alpha: 1.0,
            };
        }
        windows
    }
}

/// The glass regions the window last asked for.
fn glass_regions<A: Application>(h: &Harness<A>) -> Vec<MaterialRegion> {
    h.rt.window_system()
        .requests(h.window)
        .unwrap()
        .iter()
        .rev()
        .find_map(|request| match request {
            WindowRequest::Materials(regions) => Some(regions.clone()),
            _ => None,
        })
        .unwrap_or_default()
        .into_iter()
        .filter(|r| r.glass.is_some())
        .collect()
}

/// Whether the button paints a body of its own: a fill over its whole chrome, outside any clip (the shade of a rim over
/// glass is clipped to outside the shape).
fn paints_body<A: Application>(h: &Harness<A>) -> bool {
    let rect = h.rect("glass");
    let local = Rect::new(0.0, 0.0, rect.width(), rect.height());
    let mut clips = 0;
    h.ops("glass").iter().any(|op| match op {
        PaintOp::PushClip(_) => {
            clips += 1;
            false
        }
        PaintOp::PopClip => {
            clips -= 1;
            false
        }
        PaintOp::Fill {
            shape: Shape::RoundedRect { rect: r, .. },
            ..
        } => clips == 0 && *r == local,
        _ => false,
    })
}

#[test]
fn a_glass_button_on_a_material_is_the_compositor_s_regular_glass_and_paints_no_body() {
    for dark in [false, true] {
        let mut theme = support::theme();
        if dark {
            theme.preferences.appearance = cupertino_ui_theme::Appearance::Dark;
        }
        let h = Harness::<GlassButtonWindow<true>>::with_effects(theme);
        let regions = glass_regions(&h);
        let rect = h.rect("glass");
        let button = regions
            .iter()
            .find(|r| r.rect == rect)
            .unwrap_or_else(|| panic!("glass under the button (dark {dark}): {regions:?}"));
        let glass = button.glass.unwrap();
        assert_eq!(
            (glass.style, glass.container),
            (GlassStyle::Regular, 0),
            "regular glass of its own"
        );
        assert!(button.radius > 0.0, "with the button's corners");
        assert!(!paints_body(&h), "the glass is its body (dark {dark})");
    }
}

#[test]
fn a_glass_button_over_content_rises_into_the_window_s_layer_as_glass_of_its_own() {
    for dark in [false, true] {
        let mut theme = support::theme();
        if dark {
            theme.preferences.appearance = cupertino_ui_theme::Appearance::Dark;
        }
        let h = Harness::<GlassButtonWindow<false>>::with_layer(theme);
        let rect = h.rect("glass");
        let regions = glass_regions(&h);
        let button = regions
            .iter()
            .find(|r| r.rect == rect)
            .unwrap_or_else(|| panic!("glass under the button (dark {dark}): {regions:?}"));
        assert!(button.raised, "laid in the layer over the content");
        assert_eq!(
            button.glass.unwrap().style,
            GlassStyle::Regular,
            "regular glass"
        );
        assert!(!paints_body(&h), "the glass is its body (dark {dark})");
    }
}

#[test]
fn a_glass_button_paints_its_body_without_a_layer_with_reduce_transparency_and_without_the_desktop_s_glass(
) {
    // over the window's own background, with no layer to lift it over the content
    let over_content = Harness::<GlassButtonWindow<false>>::with_effects(support::theme());
    assert!(glass_regions(&over_content).is_empty());
    assert!(paints_body(&over_content));
    // with Reduce Transparency, on a material and in the layer
    let mut reduced = support::theme();
    reduced.preferences.reduce_transparency = true;
    let on_material = Harness::<GlassButtonWindow<true>>::with_effects(reduced.clone());
    let rect = on_material.rect("glass");
    assert!(glass_regions(&on_material).iter().all(|r| r.rect != rect));
    assert!(paints_body(&on_material));
    let layered = Harness::<GlassButtonWindow<false>>::with_layer(reduced);
    assert!(glass_regions(&layered).is_empty());
    assert!(paints_body(&layered));
    // on a desktop that shows no glass, with or without the layer
    let plain = Harness::<GlassButtonWindow<true>>::with_theme(support::theme());
    assert!(glass_regions(&plain).is_empty());
    assert!(paints_body(&plain));
    let layer_only = Harness::<GlassButtonWindow<false>>::with_layer_only(support::theme());
    assert!(glass_regions(&layer_only).is_empty());
    assert!(paints_body(&layer_only));
}

#[test]
fn a_glass_button_on_glass_lays_a_rim_over_the_glass_only_with_increase_contrast() {
    use cupertino_ui_theme::Appearance;
    for appearance in [
        Appearance::Light,
        Appearance::LightHighContrast,
        Appearance::Dark,
    ] {
        let mut theme = support::theme();
        theme.preferences.appearance = appearance;
        let h = Harness::<GlassButtonWindow<true>>::with_effects(theme);
        let rect = h.rect("glass");
        assert!(
            glass_regions(&h).iter().any(|r| r.rect == rect),
            "the glass stays ({appearance:?})"
        );
        let rim = h
            .ops("glass")
            .iter()
            .any(|op| matches!(op, PaintOp::Stroke { .. }));
        assert_eq!(
            rim,
            appearance == Appearance::LightHighContrast,
            "the compositor draws the glass's edge; the toolkit adds a rim only with Increase Contrast ({appearance:?})"
        );
        assert!(!paints_body(&h));
    }
}
