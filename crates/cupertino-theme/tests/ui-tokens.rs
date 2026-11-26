//! The colour tables, the theme tokens built from them, and control geometry.

use cupertino_theme::ui::FaceTone;
use cupertino_theme::ui::{
    choice_mark, control_geometry, glass_foreground, selection_tint, slider_geometry,
    switch_geometry, vibrant_level, vibrant_secondary, ColorRole, ColorTokens, ControlFamily,
    ControlMetrics, Corners, InteractionVariant, StateVariant, Surface, ThemePreferences,
    ThemeTokens, VIBRANT_PLACEHOLDER, VIBRANT_PRIMARY,
};
use cupertino_theme::{Appearance, Color, ControlSize, Theme};

const TOKENS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/theme/tokens.json"
));
const MATERIALS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/theme/materials.json"
));
const ANIMATIONS: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../data/theme/animations.json"
));

fn theme() -> Theme {
    Theme::from_strs(TOKENS, MATERIALS, ANIMATIONS).unwrap()
}

fn prefs(appearance: Appearance) -> ThemePreferences {
    ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    }
}

fn reduced(appearance: Appearance) -> ThemePreferences {
    ThemePreferences {
        reduce_transparency: true,
        ..prefs(appearance)
    }
}

fn same(a: Color, b: Color) -> bool {
    a.to_array()
        .iter()
        .zip(b.to_array())
        .all(|(x, y)| (x - y).abs() < 1e-5)
}

const SIZES: [ControlSize; 5] = [
    ControlSize::Mini,
    ControlSize::Small,
    ControlSize::Regular,
    ControlSize::Large,
    ControlSize::ExtraLarge,
];

// colour tables

#[test]
fn every_table_has_a_colour_for_every_role_and_state() {
    let theme = theme();
    let mut tables: Vec<(String, ColorTokens)> = Appearance::ALL
        .into_iter()
        .map(|a| (format!("neutral {a:?}"), ColorTokens::neutral(a)))
        .collect();
    for a in [Appearance::Light, Appearance::Dark] {
        tables.push((
            format!("cupertino {a:?}"),
            ColorTokens::cupertino(&theme.tokens, a).unwrap(),
        ));
    }
    for (label, t) in tables {
        for role in ColorRole::ALL {
            for state in StateVariant::ALL {
                assert!(t.get(role, state).is_some(), "{label} {role:?} {state:?}");
            }
        }
    }
}

#[test]
fn high_contrast_and_reduced_transparency_use_the_neutral_table() {
    let theme = theme();
    for a in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
        assert_eq!(
            ColorTokens::cupertino(&theme.tokens, a).unwrap(),
            ColorTokens::neutral(a),
            "{a:?}"
        );
    }
    let t = ThemeTokens::cupertino(&theme, reduced(Appearance::Dark)).unwrap();
    assert_eq!(t.colors, ColorTokens::neutral(Appearance::Dark));
}

#[test]
fn light_and_dark_windows_are_light_and_dark() {
    for a in Appearance::ALL {
        let c = ColorTokens::neutral(a)
            .get(ColorRole::WindowBackground, StateVariant::NORMAL)
            .unwrap();
        let sum = c.r + c.g + c.b;
        assert!(if a.is_dark() { sum < 1.0 } else { sum > 2.0 }, "{a:?}");
    }
}

#[test]
fn a_missing_state_reads_the_normal_one_and_a_missing_role_reads_nothing() {
    let table = ColorTokens::new([(
        (ColorRole::Text, StateVariant::NORMAL),
        Color::new(1.0, 0.0, 0.0, 1.0),
    )]);
    let hover = StateVariant {
        interaction: InteractionVariant::Hover,
        selected: true,
        window_active: true,
    };
    assert_eq!(
        table.get(ColorRole::Text, hover),
        table.get(ColorRole::Text, StateVariant::NORMAL)
    );
    assert_eq!(table.get(ColorRole::Link, StateVariant::NORMAL), None);
    assert_eq!(ColorTokens::default().get(ColorRole::Text, hover), None);
}

#[test]
fn map_rewrites_every_colour_by_its_role_and_state() {
    let t = ColorTokens::neutral(Appearance::Dark);
    let marker = Color::new(0.5, 0.25, 0.125, 1.0);
    let mapped = t.map(|role, state, c| {
        if role == ColorRole::Link && state.selected {
            marker
        } else {
            c.with_alpha(0.25)
        }
    });
    for role in ColorRole::ALL {
        for state in StateVariant::ALL {
            let before = t.get(role, state).unwrap();
            let expected = if role == ColorRole::Link && state.selected {
                marker
            } else {
                before.with_alpha(0.25)
            };
            assert_eq!(
                mapped.get(role, state),
                Some(expected),
                "{role:?} {state:?}"
            );
        }
    }
}

// accent

#[test]
fn the_selection_tint_keeps_three_tenths_of_the_colour_over_a_base() {
    let base = 63.0 / 255.0;
    for (rgb, dark, expected) in [
        ([255, 0, 0], false, Color::new(1.0, 0.7, 0.7, 1.0)),
        ([0, 255, 0], true, Color::new(base, 0.3 + base, base, 1.0)),
    ] {
        assert!(
            same(selection_tint(rgb, dark), expected),
            "{rgb:?} dark={dark}"
        );
    }
}

#[test]
fn an_accent_replaces_the_selected_fills_darkens_them_when_pressed_and_leaves_the_rest() {
    let pressed = StateVariant {
        interaction: InteractionVariant::Pressed,
        ..StateVariant::NORMAL
    };
    for a in [Appearance::Light, Appearance::Dark] {
        let dark = a.is_dark();
        let t = ColorTokens::neutral(a);
        let (accent, highlight) = ([200, 40, 90], [10, 200, 30]);
        let out = t.with_accent(accent, highlight, dark);
        let fill = out
            .get(ColorRole::SelectedControlFill, StateVariant::NORMAL)
            .unwrap();
        assert!(same(fill, Color::from_rgba8([200, 40, 90, 255])), "{a:?}");
        assert_eq!(
            out.get(ColorRole::SelectedTextBackground, StateVariant::NORMAL),
            Some(selection_tint(highlight, dark)),
            "{a:?}"
        );
        let rest = out
            .get(ColorRole::DefaultButtonFill, StateVariant::NORMAL)
            .unwrap();
        let down = out.get(ColorRole::DefaultButtonFill, pressed).unwrap();
        assert!(
            same(
                down,
                Color::new(rest.r * 0.86, rest.g * 0.86, rest.b * 0.86, down.a)
            ),
            "{a:?}"
        );
        for role in [
            ColorRole::WindowBackground,
            ColorRole::Text,
            ColorRole::Separator,
        ] {
            assert_eq!(
                out.get(role, StateVariant::NORMAL),
                t.get(role, StateVariant::NORMAL),
                "{a:?} {role:?}"
            );
        }
    }
    let plain = ColorTokens::new([(
        (ColorRole::Text, StateVariant::NORMAL),
        Color::new(0.0, 0.0, 0.0, 1.0),
    )]);
    assert_eq!(plain.with_accent([1, 2, 3], [4, 5, 6], false), plain);
}

// text on glass

#[test]
fn only_text_roles_have_a_vibrant_level_and_secondary_ones_are_fainter() {
    for (role, dark, level) in [
        (ColorRole::Text, false, Some(VIBRANT_PRIMARY)),
        (ColorRole::ControlText, true, Some(VIBRANT_PRIMARY)),
        (ColorRole::Header, true, Some(VIBRANT_PRIMARY)),
        (ColorRole::PlaceholderText, false, Some(VIBRANT_PLACEHOLDER)),
        (
            ColorRole::SecondaryText,
            true,
            Some(vibrant_secondary(true)),
        ),
        (ColorRole::ControlFill, false, None),
        (ColorRole::SelectedText, false, None),
    ] {
        assert_eq!(vibrant_level(role, dark), level, "{role:?} dark={dark}");
    }
    for dark in [false, true] {
        let s = vibrant_secondary(dark);
        assert!(s > 0.0 && s < VIBRANT_PRIMARY, "dark={dark}");
    }
}

#[test]
fn glass_foreground_is_opaque_and_follows_the_measured_face_then_the_appearance() {
    // (dark appearance, measured tone, light text)
    for (dark, tone, light_text) in [
        (false, Some(FaceTone::Dark), true),
        (true, Some(FaceTone::Light), false),
        (true, None, true),
        (false, None, false),
    ] {
        let c = glass_foreground(dark, tone);
        assert_eq!(c.a, 1.0);
        assert_eq!(c.r > 0.9, light_text, "dark={dark} {tone:?}");
        assert_eq!(c.r < 0.1, !light_text, "dark={dark} {tone:?}");
    }
}

#[test]
fn on_glass_redraws_resting_labels_in_the_glass_foreground_and_keeps_everything_else() {
    for a in [Appearance::Light, Appearance::Dark] {
        let tokens = ThemeTokens::neutral(prefs(a)).unwrap();
        for tone in [None, Some(FaceTone::Light), Some(FaceTone::Dark)] {
            let glass = tokens.on_glass(tone);
            let fg = glass_foreground(a.is_dark(), tone);
            let text = glass
                .colors
                .get(ColorRole::Text, StateVariant::NORMAL)
                .unwrap();
            assert_eq!(text.to_array()[..3], fg.to_array()[..3], "{a:?} {tone:?}");
            assert!(text.a >= VIBRANT_PRIMARY - 1e-6);
            let selected = StateVariant {
                selected: true,
                ..StateVariant::NORMAL
            };
            for (role, state) in [
                (ColorRole::ControlFill, StateVariant::NORMAL),
                (ColorRole::WindowBackground, StateVariant::NORMAL),
                (ColorRole::Text, selected),
            ] {
                assert_eq!(
                    glass.colors.get(role, state),
                    tokens.colors.get(role, state),
                    "{a:?} {tone:?} {role:?} {state:?}"
                );
            }
            assert_eq!(glass.metrics, tokens.metrics);
            assert_eq!(glass.preferences, tokens.preferences);
        }
    }
}

// theme tokens

#[test]
fn metrics_at_a_size_read_the_control_geometry_and_keep_the_minimum_target() {
    let regular = ControlMetrics::regular();
    assert_eq!(regular.size, ControlSize::Regular);
    for (size, compact) in [
        (ControlSize::Small, false),
        (ControlSize::Regular, true),
        (ControlSize::Large, false),
    ] {
        let m = regular.at_size(size, compact);
        let push = control_geometry(ControlFamily::PushButton, size, compact);
        assert_eq!(
            m.geometry(ControlFamily::PushButton),
            push,
            "{size:?} {compact}"
        );
        assert_eq!(m.button_height, push.height, "{size:?} {compact}");
        assert_eq!(m.min_target, regular.min_target, "{size:?} {compact}");
    }
    let compact = regular.at_size(ControlSize::Regular, true);
    assert!(compact.switch.width < regular.switch.width);
    assert!(compact.switch_thumb < regular.switch_thumb);
}

#[test]
fn the_cupertino_theme_takes_its_type_from_the_data_and_motion_follows_the_preference() {
    let theme = theme();
    let t = ThemeTokens::cupertino(&theme, prefs(Appearance::Light)).unwrap();
    let typography = theme.tokens.typography();
    assert_eq!(f64::from(t.type_scale.body.size), typography.sizes.regular);
    assert_eq!(f64::from(t.type_scale.caption.size), typography.sizes.small);
    assert_eq!(t.type_scale.family, typography.family);
    assert!(!t.motion.reduce);
    let still = ThemeTokens::neutral(ThemePreferences {
        reduce_motion: true,
        ..prefs(Appearance::Light)
    })
    .unwrap();
    assert!(still.motion.reduce);
}

#[test]
fn surfaces_pick_their_material_from_the_appearance_and_options() {
    use Appearance::*;
    // (preferences, surface, material)
    for (p, surface, material) in [
        (prefs(Light), Surface::Sidebar, Some("SidebarGlass")),
        (reduced(Light), Surface::Sidebar, None),
        (prefs(Light), Surface::Menu, Some("MenuGlass")),
        (reduced(Light), Surface::Menu, Some("MenuGlass")),
        (prefs(Light), Surface::Dock, Some("Glass")),
        (reduced(Light), Surface::Dock, Some("Dock")),
        (prefs(Light), Surface::DockBar, Some("BarGlass")),
        (reduced(Light), Surface::DockBar, Some("Dock")),
        (prefs(Light), Surface::ToolbarBand, Some("ToolbarBand")),
        (reduced(Light), Surface::ToolbarBand, None),
        (prefs(Light), Surface::MenuBar, Some("MenuBar")),
        (reduced(Dark), Surface::MenuBar, Some("MenuBar")),
        (prefs(DarkHighContrast), Surface::MenuBar, Some("MenuBar")),
        (prefs(Dark), Surface::OverviewBand, Some("OverviewBand")),
        (reduced(Light), Surface::OverviewBand, Some("OverviewBand")),
        (
            prefs(LightHighContrast),
            Surface::OverviewBand,
            Some("OverviewBand"),
        ),
        (
            prefs(Light),
            Surface::OverviewBandSelection,
            Some("OverviewBandSelection"),
        ),
        (
            prefs(LightHighContrast),
            Surface::Sidebar,
            Some("SidebarGlass"),
        ),
        (
            prefs(LightHighContrast),
            Surface::Hud,
            Some("LoginWindowControl"),
        ),
        (reduced(LightHighContrast), Surface::Sidebar, None),
        (
            prefs(Dark),
            Surface::DockIndicator,
            Some("RunningIndicator"),
        ),
        (
            prefs(Light),
            Surface::DockIndicator,
            Some("RunningIndicator"),
        ),
        (prefs(Light), Surface::DockSeparator, Some("DockSeparator")),
        (prefs(DarkHighContrast), Surface::DockSeparator, None),
        (prefs(LightHighContrast), Surface::DockIndicator, None),
    ] {
        let t = ThemeTokens::neutral(p).unwrap();
        assert_eq!(
            t.surface(surface).material,
            material,
            "{:?} reduce={} {surface:?}",
            p.appearance,
            p.reduce_transparency
        );
    }
    let reduced = ThemeTokens::neutral(reduced(Light)).unwrap();
    assert_eq!(
        reduced.surface(Surface::DockBar),
        reduced.surface(Surface::Dock)
    );
}

#[test]
fn window_and_text_backgrounds_fill_with_their_own_roles() {
    let t = ThemeTokens::neutral(prefs(Appearance::Light)).unwrap();
    for (surface, role) in [
        (Surface::WindowBackground, ColorRole::WindowBackground),
        (Surface::TextBackground, ColorRole::TextBackground),
    ] {
        assert_eq!(
            t.surface(surface).fill,
            t.colors.get(role, StateVariant::NORMAL),
            "{surface:?}"
        );
    }
}

// control geometry

#[test]
fn every_family_at_every_size_has_a_positive_geometry() {
    for family in ControlFamily::ALL {
        for size in ControlSize::ALL {
            for compact in [false, true] {
                let g = control_geometry(family, size, compact);
                assert!(g.height > 0.0 && g.label_size > 0.0, "{family:?} {size:?}");
                assert!(g.width.is_none_or(|w| w > 0.0), "{family:?} {size:?}");
                assert!(g.inset >= 0.0, "{family:?} {size:?}");
            }
        }
    }
}

#[test]
fn push_buttons_grow_with_the_size_and_turn_into_capsules_when_large() {
    let at = |s| control_geometry(ControlFamily::PushButton, s, false);
    for pair in SIZES.windows(2) {
        let (smaller, larger) = (at(pair[0]), at(pair[1]));
        assert!(smaller.height < larger.height, "{pair:?}");
        assert!(smaller.label_size <= larger.label_size, "{pair:?}");
    }
    for size in SIZES {
        let capsule = matches!(size, ControlSize::Large | ControlSize::ExtraLarge);
        assert_eq!(at(size).corners == Corners::Capsule, capsule, "{size:?}");
    }
}

#[test]
fn compact_metrics_shrink_the_regular_and_smaller_controls_only() {
    for (family, size, shrinks) in [
        (ControlFamily::PushButton, ControlSize::Regular, true),
        (ControlFamily::PushButton, ControlSize::Large, false),
        (ControlFamily::GlassButton, ControlSize::Small, true),
        (ControlFamily::Switch, ControlSize::Regular, true),
        (ControlFamily::Stepper, ControlSize::Regular, true),
    ] {
        let normal = control_geometry(family, size, false);
        let compact = control_geometry(family, size, true);
        let smaller = compact.height < normal.height
            || compact.width.zip(normal.width).is_some_and(|(c, n)| c < n);
        assert_eq!(smaller, shrinks, "{family:?} {size:?}");
        if !shrinks {
            assert_eq!(compact.height, normal.height, "{family:?} {size:?}");
        }
    }
    assert!(choice_mark(ControlSize::Mini, true) < choice_mark(ControlSize::Mini, false));
}

#[test]
fn a_corner_radius_never_exceeds_half_the_height() {
    for (corners, height, radius) in [
        (Corners::Radius(6.0), 24.0, 6.0),
        (Corners::Radius(6.0), 8.0, 4.0),
        (Corners::Capsule, 20.0, 10.0),
    ] {
        assert_eq!(corners.radius(height), radius, "{corners:?} at {height}");
    }
}

#[test]
fn each_family_has_its_shape() {
    for (family, capsule) in [
        (ControlFamily::Switch, true),
        (ControlFamily::SearchField, true),
        (ControlFamily::Radio, true),
        (ControlFamily::ComboBox, false),
        (ControlFamily::CheckBox, false),
        (ControlFamily::TextField, false),
    ] {
        let g = control_geometry(family, ControlSize::Regular, false);
        assert_eq!(g.corners == Corners::Capsule, capsule, "{family:?}");
    }
    let field = control_geometry(ControlFamily::TextField, ControlSize::Regular, false);
    assert_eq!(
        control_geometry(ControlFamily::SearchField, ControlSize::Regular, false).height,
        control_geometry(ControlFamily::ComboBox, ControlSize::Regular, false).height
    );
    for size in ControlSize::ALL {
        let g = control_geometry(ControlFamily::TextField, size, false);
        assert_eq!(g, field, "text fields are one geometry at {size:?}");
        assert_eq!(control_geometry(ControlFamily::SecureField, size, false), g);
    }
    let check = control_geometry(ControlFamily::CheckBox, ControlSize::Regular, false);
    assert_eq!(check.height, choice_mark(ControlSize::Regular, false));
    for family in [
        ControlFamily::Switch,
        ControlFamily::Stepper,
        ControlFamily::DatePicker,
        ControlFamily::Disclosure,
    ] {
        assert!(
            control_geometry(family, ControlSize::Regular, false)
                .width
                .is_some(),
            "{family:?}"
        );
    }
}

#[test]
fn switch_and_slider_parts_fit_their_track() {
    for compact in [false, true] {
        let s = switch_geometry(compact);
        assert!(s.thumb.0 + 2.0 * s.inset <= s.track.0, "compact={compact}");
        assert!(s.thumb.1 + 2.0 * s.inset <= s.track.1, "compact={compact}");
    }
    let regular = slider_geometry(ControlSize::Regular);
    assert!(
        regular.knob.0 > regular.knob.1,
        "the knob is wider than tall"
    );
    assert_eq!(
        control_geometry(ControlFamily::Slider, ControlSize::Regular, false).height,
        regular.knob.1
    );
    assert!(slider_geometry(ControlSize::Mini).track < regular.track);
}
