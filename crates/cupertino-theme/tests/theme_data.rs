//! Loading a theme from its three documents, and resolving materials from it.

use cupertino_theme::recipe::{
    app_grid_background, dock_reduced, opaque_background, running_indicator, switcher_panel,
    window_background, APP_GRID_MATERIAL, DOCK_MATERIAL, RUNNING_INDICATOR_MATERIAL,
    SWITCHER_MATERIAL, TEXT_BACKGROUND_MATERIAL, WINDOW_BACKGROUND_MATERIAL,
};
use cupertino_theme::{
    kept_when_inactive, Appearance, Backdrop, Blend, Catalog, Color, ColorOp, CornerCurve, Error,
    Interaction, MaterialLibrary, MaterialOptions, Theme, WindowState, KEPT_WHEN_INACTIVE,
};
use cupertino_theme::{AccentColor, SemanticColor};

const ANIMATIONS: &str = r#"{
    "timing_functions": { "default": [0.25, 0.1, 0.25, 1.0] },
    "cosine_ease": true,
    "springs": {},
    "settling_epsilon": 0.001,
    "scrolling": { "momentum_deceleration_per_ms": 0.998, "rubber_band_coefficient": 0.55 },
    "dock": { "fish_speed": 1.0, "ramp_while_dragging_ms": 1.0, "drop_speed": 1.0,
              "autohide": { "delay_s": 0.1, "duration_s": 0.1, "curve": "default" },
              "magnification_span": 3.0 },
    "minimize_effects": []
}"#;

fn catalog(colors: &str) -> String {
    format!("{{ {colors} }}")
}

fn tokens() -> String {
    let light = catalog(
        r#""window-background": [236, 236, 236, 255], "text-background": [255, 255, 255, 255],
           "label": [0, 0, 0, 217], "system-blue": [0, 122, 255, 255]"#,
    );
    let dark = catalog(
        r#""window-background": [30, 30, 30, 255], "text-background": [30, 30, 30, 255],
           "label": [255, 255, 255, 217], "system-blue": [10, 132, 255, 255]"#,
    );
    let light_hc = catalog(r#""label": [0, 0, 0, 255]"#);
    let dark_hc = catalog(r#""label": [255, 255, 255, 255]"#);
    format!(
        r#"{{
        "appearances": {{ "light": {light}, "dark": {dark},
                          "light-high-contrast": {light_hc}, "dark-high-contrast": {dark_hc} }},
        "typography": {{
            "sizes": {{ "regular": 14, "small": 12, "mini": 10, "label": 11 }},
            "system_font": {{ "units_per_em": 1000, "ascender": 900, "descender": -200,
                              "line_gap": 100, "cap_height": 700, "x_height": 500,
                              "tracking": {{ "0": {{ "10": 20, "20": -20 }} }} }}
        }}
    }}"#
    )
}

fn recipe(radius: &str, curve: &str, gray: f32, variant: &str) -> String {
    format!(
        r#"{{ "backdrop": {{ "scale": 0.5, "sigma": 8.0,
                  "ops": [ {{ "saturate": 1.5 }}, {{ "luminance_curve": [0.0, 0.3, 0.6, 1.0], "amount": 0.5 }},
                           {{ "monochrome": [0.1, 0.2, 0.3], "amount": 0.25 }} ] }},
           "fills": [ {{ "rgba": [{gray}, {gray}, {gray}, 0.5], "blend": "soft-light", "tint": true }} ],
           "corner_radius": {radius}, "corner_curve": "{curve}", "notes": ["a note"], "variant": "{variant}" }}"#
    )
}

fn material(a: f32, b: f32, c: f32) -> String {
    format!(
        r#"{{ "variants": {{ "active.normal": 0, "active.rollover": 1, "active.pressed": 2,
                             "inactive.normal": 3, "inactive.rollover": 3, "inactive.pressed": 3 }},
              "recipes": [ {}, {}, {}, {} ] }}"#,
        recipe("10.0", "continuous", a, "active.normal"),
        recipe("null", "circular", b, "active.rollover"),
        recipe("10.0", "circular", c, "active.pressed"),
        r#"{ "backdrop": null, "fills": [], "corner_radius": null, "corner_curve": "circular",
             "notes": [], "variant": "inactive.normal" }"#,
    )
}

fn materials() -> String {
    let full = format!(
        r#"{{ "Panel": {}, "Panel-Opaque": {}, "Sidebar": {}, "Sidebar-Opaque": {} }}"#,
        material(0.1, 0.2, 0.3),
        material(0.4, 0.5, 0.6),
        material(0.7, 0.7, 0.7),
        material(0.8, 0.8, 0.8),
    );
    let hc = format!(r#"{{ "Panel-Opaque": {} }}"#, material(0.9, 0.9, 0.9));
    format!(
        r#"{{ "catalogs": {{ "light": {full}, "dark": {full},
                              "light-high-contrast": {hc}, "dark-high-contrast": {hc} }} }}"#
    )
}

fn theme() -> Theme {
    Theme::from_strs(&tokens(), &materials(), ANIMATIONS).expect("a valid theme")
}

fn opts() -> MaterialOptions {
    MaterialOptions::default()
}

fn light() -> Appearance {
    Appearance::Light
}

// loading

#[test]
fn a_theme_loads_from_its_three_documents() {
    let t = theme();
    assert_eq!(t.tokens.typography().sizes.regular, 14.0);
    assert!(t
        .materials
        .has_material(Catalog::Appearance(light()), "Panel"));
    assert_eq!(t.animation.timing_names().count(), 1);
}

#[test]
fn each_document_that_fails_to_parse_fails_the_theme() {
    assert!(Theme::from_strs("{", &materials(), ANIMATIONS).is_err());
    assert!(Theme::from_strs(&tokens(), "{", ANIMATIONS).is_err());
    assert!(Theme::from_strs(&tokens(), &materials(), "{").is_err());
}

#[test]
fn tokens_need_all_four_appearances() {
    let json = r#"{ "appearances": { "light": {}, "dark": {} },
        "typography": { "sizes": {}, "system_font": { "units_per_em": 1, "ascender": 1,
        "descender": 0, "line_gap": 0, "cap_height": 1, "x_height": 1, "tracking": {} } } }"#;
    let e = Theme::from_strs(json, &materials(), ANIMATIONS).unwrap_err();
    assert!(
        matches!(&e, Error::NotFound(m) if m.contains("light-high-contrast")),
        "{e}"
    );
}

#[test]
fn materials_need_at_least_one_catalog() {
    let e = MaterialLibrary::from_str(r#"{ "catalogs": {} }"#).unwrap_err();
    assert!(matches!(&e, Error::NotFound(m) if m == "catalogs"), "{e}");
}

#[test]
fn a_material_needs_all_six_variants() {
    let json = r#"{ "catalogs": { "light": { "M": { "variants": { "active.normal": 0 },
        "recipes": [ { "backdrop": null, "fills": [], "corner_radius": null,
        "corner_curve": "circular", "notes": [], "variant": "active.normal" } ] } } } }"#;
    let e = MaterialLibrary::from_str(json).unwrap_err();
    assert!(
        matches!(&e, Error::NotFound(m) if m.contains("variant") && m.contains("light/M")),
        "{e}"
    );
}

#[test]
fn a_variant_must_name_an_existing_recipe() {
    let json = material(0.1, 0.1, 0.1).replace("\"inactive.normal\": 3", "\"inactive.normal\": 9");
    let json = format!(r#"{{ "catalogs": {{ "light": {{ "M": {json} }} }} }}"#);
    let e = MaterialLibrary::from_str(&json).unwrap_err();
    assert!(
        matches!(&e, Error::Invalid(m) if m.contains("recipe 9")),
        "{e}"
    );
}

#[test]
fn an_unknown_corner_curve_is_invalid() {
    let json = material(0.1, 0.1, 0.1).replace("\"circular\"", "\"squircle\"");
    let json = format!(r#"{{ "catalogs": {{ "light": {{ "M": {json} }} }} }}"#);
    let e = MaterialLibrary::from_str(&json).unwrap_err();
    assert!(
        matches!(&e, Error::Invalid(m) if m.contains("squircle")),
        "{e}"
    );
}

#[test]
fn an_unknown_blend_is_an_error() {
    let json = material(0.1, 0.1, 0.1).replace("soft-light", "burn");
    let json = format!(r#"{{ "catalogs": {{ "light": {{ "M": {json} }} }} }}"#);
    assert!(MaterialLibrary::from_str(&json).is_err());
}

#[test]
fn every_blend_name_is_read() {
    let names = [
        ("normal", Blend::Normal),
        ("plus-lighter", Blend::PlusLighter),
        ("plus-darker", Blend::PlusDarker),
        ("darken", Blend::Darken),
        ("lighten", Blend::Lighten),
        ("soft-light", Blend::SoftLight),
        ("copy", Blend::Copy),
    ];
    for (name, blend) in names {
        let json = material(0.1, 0.1, 0.1).replace("soft-light", name);
        let json = format!(r#"{{ "catalogs": {{ "light": {{ "M": {json} }} }} }}"#);
        let lib = MaterialLibrary::from_str(&json).unwrap();
        let r = lib.recipe(light(), "M", &opts()).unwrap();
        assert_eq!(r.fills[0].blend, blend, "{name}");
    }
}

// the library

#[test]
fn the_library_lists_the_materials_of_a_catalog() {
    let t = theme();
    let mut m = t.materials.materials(Catalog::Appearance(light()));
    m.sort_unstable();
    assert_eq!(
        m,
        vec!["Panel", "Panel-Opaque", "Sidebar", "Sidebar-Opaque"]
    );
    assert_eq!(
        t.materials
            .materials(Catalog::Appearance(Appearance::DarkHighContrast)),
        vec!["Panel-Opaque"]
    );
}

#[test]
fn a_recipe_carries_its_backdrop_fills_and_corner() {
    let r = theme().recipe(light(), "Panel", &opts()).unwrap();
    assert_eq!(r.name, "light/Panel");
    assert_eq!(r.material, "Panel");
    assert!(!r.opaque);
    assert_eq!(r.corner_radius, Some(10.0));
    assert_eq!(r.corner_curve, CornerCurve::Continuous);
    assert_eq!(r.notes, vec!["a note".to_string()]);
    assert_eq!(
        r.backdrop,
        Some(Backdrop {
            scale: 0.5,
            sigma_pt: 8.0,
            ops: vec![
                ColorOp::Saturate(1.5),
                ColorOp::LumCurve([0.0, 0.3, 0.6, 1.0], 0.5),
                ColorOp::Monochrome {
                    color: [0.1, 0.2, 0.3],
                    amount: 0.25
                },
            ],
        })
    );
    assert!(r.has_backdrop() && r.uses_tint_layer());
    assert_eq!(r.fills[0].rgba, [0.1, 0.1, 0.1, 0.5]);
    assert_eq!(r.fills[0].blend, Blend::SoftLight);
}

#[test]
fn the_window_and_interaction_pick_the_variant() {
    let t = theme();
    let pick = |window, interaction| {
        let o = MaterialOptions {
            window,
            interaction,
            ..opts()
        };
        t.recipe(light(), "Panel", &o).unwrap()
    };
    let normal = pick(WindowState::Active, Interaction::Normal);
    assert_eq!(normal.rendition_key, "active.normal");
    assert_eq!(normal.fills[0].rgba[0], 0.1);
    assert_eq!(
        pick(WindowState::Active, Interaction::Rollover).corner_radius,
        None
    );
    assert_eq!(
        pick(WindowState::Active, Interaction::Pressed).fills[0].rgba[0],
        0.3
    );
    let inactive = pick(WindowState::Inactive, Interaction::Normal);
    assert!(!inactive.has_backdrop() && inactive.fills.is_empty());
}

#[test]
fn an_explicit_rendition_key_wins_over_the_window_state() {
    let o = MaterialOptions {
        window: WindowState::Inactive,
        rendition_key: Some("active.pressed".into()),
        ..opts()
    };
    let r = theme().recipe(light(), "Panel", &o).unwrap();
    assert_eq!(r.fills[0].rgba[0], 0.3);
    let o = MaterialOptions {
        rendition_key: Some("nope".into()),
        ..opts()
    };
    assert!(matches!(
        theme().recipe(light(), "Panel", &o),
        Err(Error::NotFound(_))
    ));
}

#[test]
fn reduce_transparency_selects_the_opaque_material() {
    let o = MaterialOptions {
        reduce_transparency: true,
        ..opts()
    };
    let r = theme().recipe(light(), "Panel", &o).unwrap();
    assert!(r.opaque);
    assert_eq!(r.material, "Panel-Opaque");
    assert_eq!(r.fills[0].rgba[0], 0.4);
    let again = theme().recipe(light(), "Panel-Opaque", &o).unwrap();
    assert_eq!(again.material, "Panel-Opaque");
}

#[test]
fn a_material_without_an_opaque_form_keeps_its_own_under_reduce_transparency() {
    let json = format!(
        r#"{{ "catalogs": {{ "light": {{ "Only": {} }} }} }}"#,
        material(0.1, 0.1, 0.1)
    );
    let lib = MaterialLibrary::from_str(&json).unwrap();
    let o = MaterialOptions {
        reduce_transparency: true,
        ..opts()
    };
    let r = lib.recipe(light(), "Only", &o).unwrap();
    assert_eq!(r.material, "Only");
    assert!(!r.opaque);
}

#[test]
fn high_contrast_prefers_its_own_catalog_then_the_base_one() {
    let t = theme();
    let r = t
        .recipe(Appearance::LightHighContrast, "Panel", &opts())
        .unwrap();
    assert_eq!(r.name, "light-high-contrast/Panel-Opaque");
    assert_eq!(r.fills[0].rgba[0], 0.9);
    let r = t
        .recipe(Appearance::DarkHighContrast, "Sidebar", &opts())
        .unwrap();
    assert_eq!(r.name, "dark/Sidebar-Opaque");
}

#[test]
fn an_unknown_material_is_not_found() {
    let e = theme().recipe(light(), "Nothing", &opts()).unwrap_err();
    assert!(
        matches!(&e, Error::NotFound(m) if m.contains("Nothing") && m.contains("light")),
        "{e}"
    );
}

#[test]
fn recipe_in_reads_exactly_the_named_catalog_and_material() {
    let t = theme();
    let r = t
        .materials
        .recipe_in(Catalog::Appearance(Appearance::Dark), "Sidebar", &opts())
        .unwrap();
    assert_eq!(r.name, "dark/Sidebar");
    assert!(t
        .materials
        .recipe_in(
            Catalog::Appearance(Appearance::LightHighContrast),
            "Panel",
            &opts()
        )
        .is_err());
}

#[test]
fn a_window_body_material_keeps_its_active_look_when_inactive() {
    let t = theme();
    let o = MaterialOptions {
        window: WindowState::Inactive,
        ..opts()
    };
    let sidebar = t.recipe(light(), "Sidebar", &o).unwrap();
    // "Sidebar" itself is in the list but only its opaque form is kept
    assert!(!kept_when_inactive("Sidebar"));
    assert!(sidebar.fills.is_empty());
    let opaque = MaterialOptions {
        reduce_transparency: true,
        ..o
    };
    let r = t.recipe(light(), "Sidebar", &opaque).unwrap();
    assert_eq!(r.fills[0].rgba[0], 0.8, "the active variant");
}

#[test]
fn only_the_opaque_forms_of_window_body_materials_are_kept_when_inactive() {
    assert_eq!(
        KEPT_WHEN_INACTIVE,
        [
            "Sidebar",
            "InlineSidebar",
            "Titlebar",
            "SeamlessWindowTitlebar",
            "Header"
        ]
    );
    for m in KEPT_WHEN_INACTIVE {
        assert!(kept_when_inactive(&format!("{m}-Opaque")), "{m}");
        assert!(!kept_when_inactive(m), "{m}");
    }
    assert!(!kept_when_inactive("Panel-Opaque"));
    assert!(!kept_when_inactive("-Opaque"));
}

// the materials the theme draws from code

#[test]
fn the_switcher_is_a_saturated_blur_with_a_grey_fill_and_a_tint() {
    let t = theme();
    for (a, sat, gray, opacity) in [
        (Appearance::Light, 1.8, 0.9646, 0.36),
        (Appearance::Dark, 1.6, 0.1569, 0.40),
    ] {
        let r = t.recipe(a, SWITCHER_MATERIAL, &opts()).unwrap();
        assert_eq!(r, switcher_panel(a, false));
        let b = r.backdrop.as_ref().unwrap();
        assert_eq!((b.scale, b.sigma_pt), (0.125, 30.0));
        assert_eq!(b.ops, vec![ColorOp::Saturate(sat)]);
        assert_eq!(r.fills[0].rgba, [gray, gray, gray, opacity]);
        assert_eq!(r.fills[1].rgba, [0.0, 0.0, 0.0, 0.05]);
        assert!(r.fills[1].tint_layer);
        assert_eq!(r.corner_radius, Some(24.0));
    }
}

#[test]
fn the_switcher_turns_solid_for_reduce_transparency_and_high_contrast() {
    let t = theme();
    let o = MaterialOptions {
        reduce_transparency: true,
        ..opts()
    };
    let r = t.recipe(Appearance::Light, SWITCHER_MATERIAL, &o).unwrap();
    assert!(r.opaque && !r.has_backdrop());
    assert_eq!(r.fills[0].rgba, [0.8548, 0.8548, 0.8548, 1.0]);
    let r = t.recipe(Appearance::Dark, SWITCHER_MATERIAL, &o).unwrap();
    assert_eq!(r.fills[0].rgba[0], 0.1255);
    let r = t
        .recipe(Appearance::LightHighContrast, SWITCHER_MATERIAL, &opts())
        .unwrap();
    assert_eq!(r.fills[0].rgba[0], 0.7843);
    let r = t
        .recipe(Appearance::DarkHighContrast, SWITCHER_MATERIAL, &o)
        .unwrap();
    assert_eq!(
        r.fills[0].rgba[0], 0.05882,
        "contrast wins over reduced transparency"
    );
}

#[test]
fn the_app_grid_blurs_the_desktop_and_depends_on_the_appearance() {
    let t = theme();
    for a in Appearance::ALL {
        for reduce in [false, true] {
            let o = MaterialOptions {
                reduce_transparency: reduce,
                ..opts()
            };
            assert_eq!(
                t.recipe(a, APP_GRID_MATERIAL, &o).unwrap(),
                app_grid_background(a, reduce)
            );
        }
    }
    let r = app_grid_background(Appearance::Dark, false);
    assert!(r.has_backdrop());
    assert_eq!(r.backdrop.as_ref().unwrap().scale, 0.125);
    assert_ne!(r.fills, app_grid_background(Appearance::Light, false).fills);
}

#[test]
fn the_dock_with_reduce_transparency_covers_a_saturated_blur() {
    let r = theme()
        .recipe(Appearance::Dark, DOCK_MATERIAL, &opts())
        .unwrap();
    assert_eq!(r, dock_reduced(Appearance::Dark));
    let b = r.backdrop.as_ref().unwrap();
    assert_eq!((b.scale, b.sigma_pt), (0.125, 40.0));
    assert_eq!(b.ops, vec![ColorOp::Saturate(3.2)]);
    assert_eq!(r.fills[0].rgba, [0.0, 0.0, 0.0, 0.87]);
    assert_eq!(
        dock_reduced(Appearance::Light).fills[0].rgba,
        [1.0, 1.0, 1.0, 0.87]
    );
    assert!(r.opaque);
}

#[test]
fn the_running_indicator_adds_light_in_the_dark_and_takes_it_away_in_the_light() {
    let t = theme();
    let dark = t
        .recipe(Appearance::Dark, RUNNING_INDICATOR_MATERIAL, &opts())
        .unwrap();
    assert_eq!(dark.fills[0].blend, Blend::PlusLighter);
    assert_eq!(dark.fills[0].rgba, [0.49, 0.49, 0.49, 1.0]);
    let light = t
        .recipe(Appearance::Light, RUNNING_INDICATOR_MATERIAL, &opts())
        .unwrap();
    assert_eq!(light.fills[0].blend, Blend::PlusDarker);
    assert!((light.fills[0].rgba[0] - 0.51).abs() < 1e-6);
    assert!(!dark.has_backdrop());
    assert_eq!(dark, running_indicator(Appearance::Dark));
}

#[test]
fn the_window_and_text_backgrounds_are_their_opaque_token_colours() {
    let t = theme();
    let r = t
        .recipe(light(), WINDOW_BACKGROUND_MATERIAL, &opts())
        .unwrap();
    let c = Color::from_rgba8([236, 236, 236, 255]);
    assert_eq!(r, window_background(c));
    assert_eq!(r.fills[0].rgba, [c.r, c.g, c.b, 1.0]);
    assert_eq!(r.fills[0].blend, Blend::Copy);
    assert!(r.opaque && !r.has_backdrop());
    let text = t
        .recipe(Appearance::Dark, TEXT_BACKGROUND_MATERIAL, &opts())
        .unwrap();
    assert_eq!(
        text,
        opaque_background(
            TEXT_BACKGROUND_MATERIAL,
            Color::from_rgba8([30, 30, 30, 255])
        )
    );
}

#[test]
fn the_backgrounds_follow_the_high_contrast_catalog_with_its_base_fallback() {
    let t = theme();
    let r = t
        .recipe(
            Appearance::DarkHighContrast,
            WINDOW_BACKGROUND_MATERIAL,
            &opts(),
        )
        .unwrap();
    assert_eq!(r.fills[0].rgba[0], 30.0 / 255.0);
}

// tokens

#[test]
fn colours_are_looked_up_by_name_and_by_semantic_name() {
    let t = theme();
    let blue = Color::from_rgba8([0, 122, 255, 255]);
    assert_eq!(t.tokens.color(light(), "system-blue"), Some(blue));
    assert_eq!(
        t.tokens.semantic(light(), SemanticColor::SystemBlue),
        Some(blue)
    );
    assert_eq!(t.tokens.color(light(), "no-such-colour"), None);
    assert_eq!(t.tokens.accent(light(), AccentColor::ALL[0]), None);
}

#[test]
fn high_contrast_catalogs_fall_back_to_their_base_for_missing_names() {
    let t = theme();
    let hc = Appearance::DarkHighContrast;
    assert_eq!(
        t.tokens.color(hc, "label"),
        Some(Color::from_rgba8([255, 255, 255, 255]))
    );
    assert_eq!(
        t.tokens.color(hc, "system-blue"),
        t.tokens.color(Appearance::Dark, "system-blue")
    );
    assert_eq!(t.tokens.table(hc).get("system-blue"), None);
    assert_eq!(t.tokens.table(hc).len(), 1);
}

#[test]
fn a_normal_catalog_does_not_fall_back() {
    let t = theme();
    assert_eq!(t.tokens.color(Appearance::Light, "only-in-hc"), None);
    let table = t.tokens.table(Appearance::Light);
    assert!(!table.is_empty());
    let mut names: Vec<&str> = table.names().collect();
    names.sort_unstable();
    assert_eq!(
        names,
        vec![
            "label",
            "system-blue",
            "text-background",
            "window-background"
        ]
    );
}

#[test]
fn the_typography_comes_from_the_tokens() {
    let ty = theme().tokens.typography().clone();
    assert_eq!(
        (
            ty.sizes.regular,
            ty.sizes.small,
            ty.sizes.mini,
            ty.sizes.label
        ),
        (14.0, 12.0, 10.0, 11.0)
    );
    assert!((ty.line_height(10.0) - 10.0 * 1.2).abs() < 1e-9);
    assert!((ty.tracking.units(15.0, 0.0)).abs() < 1e-9);
    assert_eq!(ty.tracking.units(10.0, 0.0), 20.0);
}

#[test]
fn the_names_of_semantic_colours_are_kebab_case_and_unique() {
    let mut seen = std::collections::HashSet::new();
    for c in SemanticColor::ALL {
        let n = c.name();
        assert!(
            n.chars()
                .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'),
            "{n}"
        );
        assert!(seen.insert(n), "{n}");
    }
    assert_eq!(SemanticColor::Label.name(), "label");
    assert_eq!(SemanticColor::WindowBackground.name(), "window-background");
}
