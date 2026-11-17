//! The colour tables, the theme tokens built from them, and control geometry.

use cupertino_theme::ui::FaceTone;
use cupertino_theme::ui::{
    choice_mark, control_geometry, glass_foreground, selection_tint, slider_geometry,
    switch_geometry, vibrant_level, vibrant_secondary, ColorRole, ColorTokens, ControlFamily,
    ControlMetrics, Corners, InteractionVariant, StateVariant, Surface, ThemePreferences,
    ThemeTokens, CARET_PHASE_MS, VIBRANT_PLACEHOLDER, VIBRANT_PRIMARY,
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

fn prefs(appearance: Appearance) -> ThemePreferences {
    ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    }
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-5
}

fn same(a: Color, b: Color) -> bool {
    a.to_array()
        .iter()
        .zip(b.to_array())
        .all(|(x, y)| close(*x, y))
}

// state variants and roles

#[test]
fn there_are_forty_four_roles_and_sixteen_states() {
    assert_eq!(ColorRole::ALL.len(), 44);
    assert_eq!(StateVariant::ALL.len(), 16);
}

#[test]
fn the_states_are_every_combination_once() {
    let mut seen = std::collections::HashSet::new();
    for s in StateVariant::ALL {
        assert!(seen.insert(s));
    }
    for interaction in [
        InteractionVariant::Normal,
        InteractionVariant::Hover,
        InteractionVariant::Pressed,
        InteractionVariant::Disabled,
    ] {
        for selected in [false, true] {
            for window_active in [false, true] {
                assert!(seen.contains(&StateVariant {
                    interaction,
                    selected,
                    window_active
                }));
            }
        }
    }
}

#[test]
fn the_normal_state_is_the_resting_selected_nothing_active_window() {
    assert_eq!(
        StateVariant::NORMAL,
        StateVariant {
            interaction: InteractionVariant::Normal,
            selected: false,
            window_active: true
        }
    );
    assert!(StateVariant::ALL.contains(&StateVariant::NORMAL));
}

// colour tables

#[test]
fn every_appearance_has_a_colour_for_every_role_and_state() {
    for a in Appearance::ALL {
        let t = ColorTokens::neutral(a);
        for role in ColorRole::ALL {
            for state in StateVariant::ALL {
                assert!(t.get(role, state).is_some(), "{a:?} {role:?} {state:?}");
            }
        }
    }
}

#[test]
fn the_cupertino_table_covers_every_role_and_state_in_both_appearances() {
    let theme = Theme::from_strs(TOKENS, MATERIALS, ANIMATIONS).unwrap();
    for a in [Appearance::Light, Appearance::Dark] {
        let t = ColorTokens::cupertino(&theme.tokens, a).unwrap();
        for role in ColorRole::ALL {
            for state in StateVariant::ALL {
                assert!(t.get(role, state).is_some(), "{a:?} {role:?} {state:?}");
            }
        }
    }
}

#[test]
fn high_contrast_uses_the_neutral_table() {
    let theme = Theme::from_strs(TOKENS, MATERIALS, ANIMATIONS).unwrap();
    for a in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
        assert_eq!(
            ColorTokens::cupertino(&theme.tokens, a).unwrap(),
            ColorTokens::neutral(a)
        );
    }
}

#[test]
fn light_and_dark_windows_are_light_and_dark() {
    let lum = |a| {
        let c = ColorTokens::neutral(a)
            .get(ColorRole::WindowBackground, StateVariant::NORMAL)
            .unwrap();
        c.r + c.g + c.b
    };
    assert!(lum(Appearance::Light) > 2.0);
    assert!(lum(Appearance::Dark) < 1.0);
    assert!(lum(Appearance::LightHighContrast) > 2.0);
    assert!(lum(Appearance::DarkHighContrast) < 1.0);
}

#[test]
fn a_missing_state_reads_the_normal_one() {
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
fn map_rewrites_every_colour_and_keeps_the_keys() {
    let t = ColorTokens::neutral(Appearance::Light);
    let mapped = t.map(|_, _, c| c.with_alpha(0.25));
    for role in ColorRole::ALL {
        for state in StateVariant::ALL {
            let before = t.get(role, state).unwrap();
            let after = mapped.get(role, state).unwrap();
            assert_eq!(after, before.with_alpha(0.25));
        }
    }
}

#[test]
fn map_hands_each_colour_its_role_and_state() {
    let t = ColorTokens::neutral(Appearance::Dark);
    let seen = std::cell::Cell::new(0);
    let marker = Color::new(0.5, 0.25, 0.125, 1.0);
    let mapped = t.map(|role, state, c| {
        seen.set(seen.get() + 1);
        if role == ColorRole::Link && state.selected {
            marker
        } else {
            c
        }
    });
    assert!(seen.get() >= ColorRole::ALL.len());
    let selected = StateVariant {
        selected: true,
        ..StateVariant::NORMAL
    };
    if t.get(ColorRole::Link, selected) != t.get(ColorRole::Link, StateVariant::NORMAL) {
        assert_eq!(mapped.get(ColorRole::Link, selected), Some(marker));
    }
    assert_eq!(
        mapped.get(ColorRole::Text, StateVariant::NORMAL),
        t.get(ColorRole::Text, StateVariant::NORMAL)
    );
}

// accent

#[test]
fn the_selection_tint_keeps_three_tenths_of_the_colour_over_a_base() {
    let light = selection_tint([255, 0, 0], false);
    assert!(same(light, Color::new(0.3 + 0.7, 0.7, 0.7, 1.0)));
    let dark = selection_tint([0, 255, 0], true);
    let base = 63.0 / 255.0;
    assert!(same(dark, Color::new(base, 0.3 + base, base, 1.0)));
}

#[test]
fn an_accent_replaces_the_selected_control_fill() {
    for dark in [false, true] {
        let a = if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        let t = ColorTokens::neutral(a);
        let accent = [200, 40, 90];
        let highlight = [10, 200, 30];
        let out = t.with_accent(accent, highlight, dark);
        let want = Color::from_rgba8([200, 40, 90, 255]);
        assert!(same(
            out.get(ColorRole::SelectedControlFill, StateVariant::NORMAL)
                .unwrap(),
            want
        ));
        assert_eq!(
            out.get(ColorRole::SelectedTextBackground, StateVariant::NORMAL),
            Some(selection_tint(highlight, dark))
        );
    }
}

#[test]
fn an_accent_leaves_other_colours_alone() {
    let t = ColorTokens::neutral(Appearance::Light);
    let out = t.with_accent([200, 40, 90], [10, 200, 30], false);
    for role in [
        ColorRole::WindowBackground,
        ColorRole::Text,
        ColorRole::Separator,
    ] {
        assert_eq!(
            out.get(role, StateVariant::NORMAL),
            t.get(role, StateVariant::NORMAL),
            "{role:?}"
        );
    }
}

#[test]
fn a_pressed_accent_is_darker_than_the_resting_one() {
    let t = ColorTokens::neutral(Appearance::Light);
    let out = t.with_accent([200, 100, 50], [0, 0, 0], false);
    let pressed = StateVariant {
        interaction: InteractionVariant::Pressed,
        ..StateVariant::NORMAL
    };
    let rest = out
        .get(ColorRole::DefaultButtonFill, StateVariant::NORMAL)
        .unwrap();
    let down = out.get(ColorRole::DefaultButtonFill, pressed).unwrap();
    assert!(down.r < rest.r && down.g < rest.g && down.b < rest.b);
    assert!(same(
        down,
        Color::new(rest.r * 0.86, rest.g * 0.86, rest.b * 0.86, down.a)
    ));
}

#[test]
fn a_table_without_the_marked_states_is_unchanged_by_an_accent() {
    let t = ColorTokens::new([(
        (ColorRole::Text, StateVariant::NORMAL),
        Color::new(0.0, 0.0, 0.0, 1.0),
    )]);
    assert_eq!(t.with_accent([1, 2, 3], [4, 5, 6], false), t);
}

// text on glass

#[test]
fn only_text_roles_have_a_vibrant_level() {
    assert_eq!(vibrant_level(ColorRole::Text, false), Some(VIBRANT_PRIMARY));
    assert_eq!(
        vibrant_level(ColorRole::ControlText, true),
        Some(VIBRANT_PRIMARY)
    );
    assert_eq!(
        vibrant_level(ColorRole::Header, true),
        Some(VIBRANT_PRIMARY)
    );
    assert_eq!(
        vibrant_level(ColorRole::PlaceholderText, false),
        Some(VIBRANT_PLACEHOLDER)
    );
    assert_eq!(
        vibrant_level(ColorRole::SecondaryText, true),
        Some(vibrant_secondary(true))
    );
    assert_eq!(vibrant_level(ColorRole::ControlFill, false), None);
    assert_eq!(vibrant_level(ColorRole::SelectedText, false), None);
}

#[test]
fn the_secondary_labels_are_fainter_than_the_primary_ones() {
    for dark in [false, true] {
        let s = vibrant_secondary(dark);
        assert!(s > 0.0 && s < VIBRANT_PRIMARY);
    }
    const { assert!(VIBRANT_PLACEHOLDER < VIBRANT_PRIMARY) };
}

#[test]
fn glass_foreground_is_opaque_and_follows_the_face() {
    let on_dark_face = glass_foreground(false, Some(FaceTone::Dark));
    let on_light_face = glass_foreground(true, Some(FaceTone::Light));
    assert_eq!(on_dark_face.a, 1.0);
    assert!(on_dark_face.r > 0.9, "light text on a dark face");
    assert!(on_light_face.r < 0.1, "dark text on a light face");
    assert_eq!(
        glass_foreground(true, None).to_array()[..3],
        glass_foreground(true, Some(FaceTone::Dark)).to_array()[..3]
    );
    assert_eq!(
        glass_foreground(false, None).to_array()[..3],
        glass_foreground(false, Some(FaceTone::Light)).to_array()[..3]
    );
}

#[test]
fn on_glass_redraws_labels_in_the_glass_foreground_and_keeps_fills() {
    for a in [Appearance::Light, Appearance::Dark] {
        let tokens = ThemeTokens::neutral(prefs(a)).unwrap();
        let glass = tokens.on_glass(None);
        let fg = glass_foreground(a.is_dark(), None);
        let text = glass
            .colors
            .get(ColorRole::Text, StateVariant::NORMAL)
            .unwrap();
        assert_eq!(text.to_array()[..3], fg.to_array()[..3]);
        assert!(text.a >= VIBRANT_PRIMARY - 1e-6);
        for role in [ColorRole::ControlFill, ColorRole::WindowBackground] {
            assert_eq!(
                glass.colors.get(role, StateVariant::NORMAL),
                tokens.colors.get(role, StateVariant::NORMAL),
                "{role:?}"
            );
        }
        assert_eq!(glass.metrics, tokens.metrics);
        assert_eq!(glass.preferences, tokens.preferences);
    }
}

#[test]
fn on_glass_follows_the_measured_face_over_the_appearance() {
    let tokens = ThemeTokens::neutral(prefs(Appearance::Dark)).unwrap();
    let on_light_face = tokens.on_glass(Some(FaceTone::Light));
    let text = on_light_face
        .colors
        .get(ColorRole::Text, StateVariant::NORMAL)
        .unwrap();
    assert!(text.r < 0.1, "dark text over a light face");
}

#[test]
fn on_glass_leaves_selected_and_disabled_labels_their_strength() {
    let tokens = ThemeTokens::neutral(prefs(Appearance::Light)).unwrap();
    let glass = tokens.on_glass(None);
    let selected = StateVariant {
        selected: true,
        ..StateVariant::NORMAL
    };
    assert_eq!(
        glass.colors.get(ColorRole::Text, selected),
        tokens.colors.get(ColorRole::Text, selected)
    );
    let disabled = StateVariant {
        interaction: InteractionVariant::Disabled,
        ..StateVariant::NORMAL
    };
    let before = tokens.colors.get(ColorRole::ControlText, disabled).unwrap();
    let after = glass.colors.get(ColorRole::ControlText, disabled).unwrap();
    assert!(after.a <= before.a + 1e-6 || before.a >= 0.99);
}

// theme tokens

#[test]
fn the_regular_metrics_are_the_regular_control_geometry() {
    let m = ControlMetrics::regular();
    assert_eq!(m.size, ControlSize::Regular);
    assert!(!m.compact);
    assert_eq!(m.button_height, 24.0);
    assert_eq!(m.field_height, 24.0);
    assert_eq!(m.field_inset, 6.0);
    assert_eq!(m.field_radius, 6.0);
    assert_eq!(m.button_radius, 6.0);
    assert_eq!((m.switch.width, m.switch.height), (54.0, 24.0));
    assert_eq!(m.switch_thumb, 20.0);
    assert_eq!(m.switch_thumb_inset, 2.0);
    assert_eq!((m.min_target.width, m.min_target.height), (24.0, 24.0));
}

#[test]
fn metrics_change_with_the_size_and_compactness() {
    let small = ControlMetrics::regular().at_size(ControlSize::Small, false);
    assert_eq!(small.button_height, 20.0);
    let compact = ControlMetrics::regular().at_size(ControlSize::Regular, true);
    assert_eq!(compact.button_height, 20.0);
    assert_eq!((compact.switch.width, compact.switch.height), (38.0, 20.0));
    assert_eq!(compact.switch_thumb, 16.0);
    assert_eq!(compact.min_target, ControlMetrics::regular().min_target);
    assert_eq!(
        small.geometry(ControlFamily::PushButton),
        control_geometry(ControlFamily::PushButton, ControlSize::Small, false)
    );
}

#[test]
fn the_neutral_theme_has_a_thirteen_and_eleven_point_type_scale() {
    let t = ThemeTokens::neutral(prefs(Appearance::Light)).unwrap();
    assert_eq!(t.type_scale.body.size, 13.0);
    assert_eq!(t.type_scale.caption.size, 11.0);
    assert_eq!(t.type_scale.section.size, 12.0);
    assert_eq!(t.type_scale.title.size, 15.0);
    assert_eq!(t.type_scale.body.weight, 400);
    assert_eq!(t.type_scale.title.weight, 600);
    assert_eq!(t.type_scale.family, "");
}

#[test]
fn the_cupertino_theme_takes_its_type_from_the_data() {
    let theme = Theme::from_strs(TOKENS, MATERIALS, ANIMATIONS).unwrap();
    let t = ThemeTokens::cupertino(&theme, prefs(Appearance::Light)).unwrap();
    let sizes = theme.tokens.typography().sizes;
    assert_eq!(f64::from(t.type_scale.body.size), sizes.regular);
    assert_eq!(f64::from(t.type_scale.caption.size), sizes.small);
    assert_eq!(t.type_scale.family, theme.tokens.typography().family);
}

#[test]
fn reduce_transparency_swaps_the_cupertino_colours_for_the_neutral_ones() {
    let theme = Theme::from_strs(TOKENS, MATERIALS, ANIMATIONS).unwrap();
    let p = ThemePreferences {
        reduce_transparency: true,
        ..prefs(Appearance::Dark)
    };
    let t = ThemeTokens::cupertino(&theme, p).unwrap();
    assert_eq!(t.colors, ColorTokens::neutral(Appearance::Dark));
}

#[test]
fn the_focus_ring_and_caret_are_fixed_design_values() {
    let t = ThemeTokens::neutral(prefs(Appearance::Light)).unwrap();
    assert_eq!(
        (t.focus.width, t.focus.outset, t.focus.radius_extra),
        (2.0, 2.0, 3.0)
    );
    assert_eq!(t.motion.caret_phase_ms, 500);
    assert_eq!(CARET_PHASE_MS, 500);
    assert!(!t.motion.reduce);
    let reduced = ThemeTokens::neutral(ThemePreferences {
        reduce_motion: true,
        ..prefs(Appearance::Light)
    })
    .unwrap();
    assert!(reduced.motion.reduce);
}

#[test]
fn surfaces_use_glass_unless_transparency_is_reduced() {
    let tokens = |reduce, a| {
        ThemeTokens::neutral(ThemePreferences {
            reduce_transparency: reduce,
            ..prefs(a)
        })
        .unwrap()
    };
    let normal = tokens(false, Appearance::Light);
    let reduced = tokens(true, Appearance::Light);
    assert_eq!(
        normal.surface(Surface::Sidebar).material,
        Some("SidebarGlass")
    );
    assert_eq!(reduced.surface(Surface::Sidebar).material, None);
    assert_eq!(normal.surface(Surface::Menu).material, Some("MenuGlass"));
    assert_eq!(reduced.surface(Surface::Menu).material, Some("MenuGlass"));
    assert_eq!(normal.surface(Surface::Dock).material, Some("Glass"));
    assert_eq!(reduced.surface(Surface::Dock).material, Some("Dock"));
    assert_eq!(normal.surface(Surface::DockBar).material, Some("BarGlass"));
    assert_eq!(
        reduced.surface(Surface::DockBar),
        reduced.surface(Surface::Dock)
    );
}

#[test]
fn high_contrast_keeps_the_sidebar_material_and_drops_the_hud_one_only_with_both_options() {
    let hc = ThemeTokens::neutral(prefs(Appearance::LightHighContrast)).unwrap();
    assert_eq!(hc.surface(Surface::Sidebar).material, Some("SidebarGlass"));
    assert_eq!(
        hc.surface(Surface::Hud).material,
        Some("LoginWindowControl")
    );
    let both = ThemeTokens::neutral(ThemePreferences {
        reduce_transparency: true,
        ..prefs(Appearance::LightHighContrast)
    })
    .unwrap();
    assert_eq!(both.surface(Surface::Sidebar).material, None);
}

#[test]
fn the_dock_indicator_has_a_material_only_in_the_dark_appearance() {
    let dark = ThemeTokens::neutral(prefs(Appearance::Dark)).unwrap();
    let light = ThemeTokens::neutral(prefs(Appearance::Light)).unwrap();
    assert_eq!(
        dark.surface(Surface::DockIndicator).material,
        Some("RunningIndicator")
    );
    assert_eq!(light.surface(Surface::DockIndicator).material, None);
}

#[test]
fn window_and_text_backgrounds_fill_with_their_own_roles_and_the_toolbar_band_goes_when_opaque() {
    let t = ThemeTokens::neutral(prefs(Appearance::Light)).unwrap();
    assert_eq!(
        t.surface(Surface::WindowBackground).fill,
        t.colors
            .get(ColorRole::WindowBackground, StateVariant::NORMAL)
    );
    assert_eq!(
        t.surface(Surface::TextBackground).fill,
        t.colors
            .get(ColorRole::TextBackground, StateVariant::NORMAL)
    );
    assert_eq!(
        t.surface(Surface::ToolbarBand).material,
        Some("ToolbarBand")
    );
    let reduced = ThemeTokens::neutral(ThemePreferences {
        reduce_transparency: true,
        ..prefs(Appearance::Light)
    })
    .unwrap();
    assert_eq!(reduced.surface(Surface::ToolbarBand).material, None);
}

// control geometry

#[test]
fn a_regular_push_button_is_twenty_four_points_with_rounded_corners() {
    let g = control_geometry(ControlFamily::PushButton, ControlSize::Regular, false);
    assert_eq!(g.height, 24.0);
    assert_eq!(g.width, None);
    assert_eq!(g.corners, Corners::Radius(6.0));
    assert_eq!(g.label_size, 13.0);
    assert_eq!(g.inset, 13.5);
}

#[test]
fn button_heights_grow_with_the_size() {
    let heights: Vec<f64> = [
        ControlSize::Mini,
        ControlSize::Small,
        ControlSize::Regular,
        ControlSize::Large,
        ControlSize::ExtraLarge,
    ]
    .iter()
    .map(|s| control_geometry(ControlFamily::PushButton, *s, false).height)
    .collect();
    assert_eq!(heights, vec![16.0, 20.0, 24.0, 28.0, 36.0]);
}

#[test]
fn large_buttons_are_capsules() {
    for size in [ControlSize::Large, ControlSize::ExtraLarge] {
        let g = control_geometry(ControlFamily::PushButton, size, false);
        assert_eq!(g.corners, Corners::Capsule);
        assert_eq!(g.corners.radius(g.height), g.height / 2.0);
    }
}

#[test]
fn compact_metrics_shrink_the_small_controls_only() {
    let normal = control_geometry(ControlFamily::PushButton, ControlSize::Regular, false);
    let compact = control_geometry(ControlFamily::PushButton, ControlSize::Regular, true);
    assert_eq!((normal.height, compact.height), (24.0, 20.0));
    assert!(compact.inset < normal.inset);
    let large = control_geometry(ControlFamily::PushButton, ControlSize::Large, true);
    assert_eq!(large.height, 28.0);
}

#[test]
fn a_corner_radius_never_exceeds_half_the_height() {
    assert_eq!(Corners::Radius(6.0).radius(24.0), 6.0);
    assert_eq!(Corners::Radius(6.0).radius(8.0), 4.0);
    assert_eq!(Corners::Capsule.radius(20.0), 10.0);
}

#[test]
fn glass_buttons_have_their_own_heights() {
    assert_eq!(
        control_geometry(ControlFamily::GlassButton, ControlSize::Small, false).height,
        20.0
    );
    assert_eq!(
        control_geometry(ControlFamily::GlassButton, ControlSize::Small, true).height,
        18.0
    );
    assert_eq!(
        control_geometry(ControlFamily::GlassButton, ControlSize::Mini, false).height,
        13.0
    );
}

#[test]
fn a_switch_is_a_capsule_of_a_fixed_size() {
    let g = control_geometry(ControlFamily::Switch, ControlSize::Regular, false);
    assert_eq!((g.width, g.height), (Some(54.0), 24.0));
    assert_eq!(g.corners, Corners::Capsule);
    let c = control_geometry(ControlFamily::Switch, ControlSize::Regular, true);
    assert_eq!((c.width, c.height), (Some(38.0), 20.0));
}

#[test]
fn the_switch_parts_fit_inside_the_track() {
    for compact in [false, true] {
        let s = switch_geometry(compact);
        assert!(s.thumb.0 + 2.0 * s.inset <= s.track.0);
        assert!(s.thumb.1 + 2.0 * s.inset <= s.track.1);
    }
    assert_eq!(switch_geometry(false).thumb, (32.0, 20.0));
    assert_eq!(switch_geometry(true).thumb, (22.0, 16.0));
}

#[test]
fn text_fields_are_one_height_at_every_size() {
    for size in ControlSize::ALL {
        let g = control_geometry(ControlFamily::TextField, size, false);
        assert_eq!(g.height, 24.0);
        assert_eq!(g.corners, Corners::Radius(6.0));
        assert_eq!(g.inset, 6.0);
        assert_eq!(control_geometry(ControlFamily::SecureField, size, false), g);
    }
}

#[test]
fn search_fields_are_capsules_and_combo_boxes_are_not() {
    let search = control_geometry(ControlFamily::SearchField, ControlSize::Regular, false);
    let combo = control_geometry(ControlFamily::ComboBox, ControlSize::Regular, false);
    assert_eq!(search.corners, Corners::Capsule);
    assert_eq!(combo.corners, Corners::Radius(6.0));
    assert_eq!(search.inset, 6.0);
    assert_eq!(search.height, combo.height);
}

#[test]
fn check_boxes_are_square_and_radios_round() {
    let check = control_geometry(ControlFamily::CheckBox, ControlSize::Regular, false);
    let radio = control_geometry(ControlFamily::Radio, ControlSize::Regular, false);
    assert_eq!(check.height, 16.0);
    assert_eq!(check.corners, Corners::Radius(4.0));
    assert_eq!(radio.corners, Corners::Capsule);
    assert_eq!(choice_mark(ControlSize::Regular, false), 16.0);
    assert_eq!(choice_mark(ControlSize::Mini, false), 12.0);
    assert_eq!(choice_mark(ControlSize::Mini, true), 11.0);
}

#[test]
fn sliders_have_a_knob_a_quarter_wider_than_tall() {
    let g = slider_geometry(ControlSize::Regular);
    assert_eq!(g.track, 6.0);
    assert_eq!(g.knob, (20.0, 16.0));
    assert_eq!(
        control_geometry(ControlFamily::Slider, ControlSize::Regular, false).height,
        16.0
    );
    assert!(slider_geometry(ControlSize::Mini).track < g.track);
}

#[test]
fn steppers_and_date_pickers_and_disclosure_triangles_have_a_width() {
    let stepper = control_geometry(ControlFamily::Stepper, ControlSize::Regular, false);
    assert_eq!(stepper.width, Some(20.0));
    assert_eq!(stepper.inset, 0.0);
    let compact = control_geometry(ControlFamily::Stepper, ControlSize::Regular, true);
    assert_eq!(compact.width, Some(13.0));
    let date = control_geometry(ControlFamily::DatePicker, ControlSize::Regular, false);
    assert_eq!((date.width, date.height), (Some(162.0), 24.0));
    let disclosure = control_geometry(ControlFamily::Disclosure, ControlSize::Regular, false);
    assert_eq!((disclosure.width, disclosure.height), (Some(20.0), 20.0));
}

#[test]
fn label_sizes_follow_the_size_class() {
    let label = |s| control_geometry(ControlFamily::PushButton, s, false).label_size;
    assert_eq!(label(ControlSize::Mini), 9.0);
    assert_eq!(label(ControlSize::Small), 11.0);
    assert_eq!(label(ControlSize::Regular), 13.0);
    assert_eq!(label(ControlSize::ExtraLarge), 15.0);
}

#[test]
fn every_family_at_every_size_has_a_positive_geometry() {
    for family in ControlFamily::ALL {
        for size in ControlSize::ALL {
            for compact in [false, true] {
                let g = control_geometry(family, size, compact);
                assert!(g.height > 0.0 && g.label_size > 0.0, "{family:?} {size:?}");
                assert!(g.width.is_none_or(|w| w > 0.0));
                assert!(g.inset >= 0.0);
            }
        }
    }
}
