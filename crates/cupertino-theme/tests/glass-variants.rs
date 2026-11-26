//! The glass variants: each resolves to its recorded values in both appearances, with each display option, while
//! inactive and at the size it resolves at, and every legacy style draws as its variant.

use cupertino_theme::glass::{
    legibility_amount, GlassBase, GlassShadow, SidebarEdge, BLEED_AMOUNT_MAX, BLEED_BLUR_MAX,
    CLEAR_BLUR, DOCK_BAR_BEND_DIMENSION, DOCK_BAR_LIGHT, DOCK_BAR_LIGHT_GAIN,
    DOCK_BAR_MAX_LUMA_DARK, DOCK_BLUR_SCALE, GLASS_SIZE_MEDIUM, GLASS_SIZE_MINIMUM,
    LARGE_PANEL_BLACK_LIGHT, LARGE_PANEL_SATURATION_DARK, LARGE_PANEL_SHADOW_BLUR, MODULE_BLACK,
    MODULE_SATURATION, MODULE_WHITE, TOOLBAR_SPECULAR_AMOUNT, TOOLBAR_SPECULAR_HEIGHT,
    TRACK_CAPTURE_SCALE, WINDOW_SIDEBAR_BLACK_DARK, WINDOW_SIDEBAR_BLACK_LIGHT,
    WINDOW_SIDEBAR_SATURATION, WINDOW_SIDEBAR_VEIL_LIGHT, WINDOW_SIDEBAR_WHITE_DARK,
    WINDOW_SIDEBAR_WHITE_LIGHT,
};
use cupertino_theme::{
    GlassMode, GlassModifiers, GlassParams, GlassSize, GlassStyle, GlassVariant,
};

const EPS: f32 = 1e-5;

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < EPS
}

macro_rules! assert_close {
    ($a:expr, $b:expr) => {
        assert_close!($a, $b, "")
    };
    ($a:expr, $b:expr, $($t:tt)+) => {{
        let (a, b): (f32, f32) = ($a, $b);
        assert!(close(a, b), "{} = {a}, not {b}: {}", stringify!($a), format!($($t)+));
    }};
}

fn mode(variant: GlassVariant, dark: bool) -> GlassMode {
    GlassMode::new(dark, variant, GlassModifiers::DEFAULT)
}

fn at(variant: GlassVariant, dark: bool, side: f32) -> GlassParams {
    GlassParams::new(mode(variant, dark), side)
}

fn with(
    variant: GlassVariant,
    dark: bool,
    side: f32,
    change: impl FnOnce(&mut GlassMode),
) -> GlassParams {
    let mut m = mode(variant, dark);
    change(&mut m);
    GlassParams::new(m, side)
}

fn inactive(variant: GlassVariant, dark: bool, side: f32) -> GlassParams {
    with(variant, dark, side, |m| m.modifiers.active = false)
}

const DEG: f32 = std::f32::consts::PI / 180.0;

#[test]
fn every_legacy_style_draws_as_its_variant() {
    use GlassSize::*;
    use GlassVariant as V;
    let d = GlassModifiers::DEFAULT;
    for (style, variant, modifiers) in [
        (GlassStyle::Regular, V::Regular, d),
        (GlassStyle::Clear, V::Clear, d),
        (GlassStyle::LargePanel, V::SearchResults, d),
        (GlassStyle::Bar, V::Dock, d),
        (GlassStyle::Dense, V::CartouchePopover, d.sized(Medium)),
        (GlassStyle::Module, V::ControlCenter, d),
        (
            GlassStyle::SubduedModule,
            V::ControlCenter,
            GlassModifiers { subdued: true, ..d },
        ),
        (GlassStyle::Card, V::NotificationCenter, d.sized(Medium)),
        (GlassStyle::Snippet, V::NotificationCenter, d),
        (GlassStyle::Track, V::Slider, d),
        (GlassStyle::Popover, V::CartouchePopover, d.sized(Minimum)),
        (GlassStyle::Switcher, V::AppSwitcher, d),
        (GlassStyle::Sidebar, V::AbuttedSidebar, d),
        (GlassStyle::Toolbar, V::ToolbarItem, d),
        (
            GlassStyle::Menu,
            V::Menu,
            GlassModifiers {
                exclude_shadow: true,
                ..d
            },
        ),
        (GlassStyle::Veiled, V::Clear, d),
    ] {
        assert_eq!(style.variant(), (variant, modifiers), "{style:?}");
        for dark in [false, true] {
            let m = GlassMode::of_style(dark, style);
            assert_eq!((m.variant, m.modifiers, m.dark), (variant, modifiers, dark));
            for side in [20.0, 64.0, 200.0] {
                assert_eq!(
                    GlassParams::new(m, side),
                    GlassParams::new(GlassMode::new(dark, variant, modifiers), side),
                    "{style:?}"
                );
            }
        }
    }
}

#[test]
fn regular_glass_resolves_its_recorded_values_at_each_size() {
    // (side, first level's opacity, dark luma cap, inner bend, band, bleed band)
    for (side, level, dark_luma, amount, height, bleed) in [
        (48.0, 0.4, 0.6, -24.0, 12.0, 16.8),
        (84.0, 0.4 + 0.4 * 20.0 / 96.0, 0.475, -42.0, 20.0, 29.4),
        (104.0, 0.4 + 0.4 * 40.0 / 96.0, 0.35, -52.0, 20.0, 36.4),
        (240.0, 0.8, 0.35, -60.0, 20.0, 84.0),
    ] {
        for dark in [false, true] {
            let p = at(GlassVariant::Regular, dark, side);
            assert_close!(p.blur.radius, 5.0);
            assert_close!(p.blur.level_opacity[0], level, "{side}");
            assert_close!(p.blur_sigma(), 2.0 * 5.0 * level);
            assert_close!(p.blur.fill_radius, 8.0);
            assert_close!(p.blur.fill_normal, 0.5);
            let (lighten, darken) = if dark { (0.0, 0.9) } else { (0.9, 0.0) };
            assert_close!(p.blur.fill_lighten, lighten);
            assert_close!(p.blur.fill_darken, darken);
            let f = p.face;
            if dark {
                assert_eq!((f.black, f.white, f.saturation), (0.125, 1.125, 1.3));
                assert_close!(f.max_luma, dark_luma, "{side}");
                assert_eq!(f.fill[3], 0.0);
            } else {
                assert_eq!((f.black, f.white, f.saturation), (0.4, 1.03, 1.2));
                assert_eq!((f.max_luma, f.fill), (0.94, [1.0, 1.0, 1.0, 0.2]));
            }
            assert_close!(p.refraction.amount, amount, "{side}");
            assert_close!(p.refraction.height, height, "{side}");
            assert_eq!(p.refraction.outer_opacity, 0.0);
            assert_close!(p.bleed.height, bleed, "{side}");
            assert_close!(p.bleed.amount, bleed.min(BLEED_AMOUNT_MAX));
            assert_close!(p.bleed.blur, bleed.min(BLEED_BLUR_MAX));
            assert_eq!(p.bleed.opacity, 0.2);
            assert_eq!(p.bleed.darken, !dark);
            let (bb, bw, bs) = if dark {
                (0.125, 0.5, 1.0)
            } else {
                (0.9, 1.0, 1.2)
            };
            assert_eq!(
                (p.bleed.black, p.bleed.white, p.bleed.saturation),
                (bb, bw, bs)
            );
            assert_eq!((p.highlight.amount, p.highlight.color_bias), (0.5, -0.25));
            let s = &p.shadow;
            assert_eq!(
                (s.ring_opacity, s.ring_stroke, s.ring_offset, s.ring_blur),
                (0.06, 4.0, 8.0, 5.0)
            );
            assert_eq!(s.opacity, 0.0, "no drop shadow");
            let sp = p.specular;
            assert_eq!((sp.amount, sp.height, sp.fill), (0.4, 0.75, 1.0));
            assert_close!(sp.spread, if dark { 75.0 } else { 96.0 } * DEG);
            assert_close!(sp.angle, 90.0 * DEG);
            let df = sp.diffuse;
            assert_eq!(
                (df.height_scale, df.spread_scale, df.amount_scale),
                (8.0, 0.65, 0.15)
            );
            assert_eq!(p.border.opacity, 0.0);
        }
    }
    // the edge bleed's blur stops at the blur fill's deviation; its reach follows the side
    let large = at(GlassVariant::Regular, false, 1000.0);
    assert_close!(large.bleed.blur, large.fill_sigma());
    assert_close!(large.bleed.amount, 350.0);
}

#[test]
fn the_edge_bleed_reaches_as_far_as_its_band_and_shares_a_panels_blur_fill() {
    // a 36-point toolbar capsule: band, reach and blur 0.35 of its side
    for dark in [false, true] {
        let b = at(GlassVariant::ToolbarItem, dark, 36.0).bleed;
        assert_close!(b.blur, 12.6);
        assert_close!(b.amount, 12.6);
        assert_close!(b.height, 12.6);
    }
    // a 300-point panel: band and reach 0.35 of its side, its blur the blur fill's deviation
    let b = at(GlassVariant::Regular, false, 300.0).bleed;
    assert_close!(b.height, 105.0);
    assert_close!(b.amount, 105.0);
    assert_close!(b.blur, BLEED_BLUR_MAX);
    assert!(b.amount <= BLEED_AMOUNT_MAX);
}

#[test]
fn increase_contrast_darkens_and_fills_the_regular_face_and_keeps_its_optics() {
    for dark in [false, true] {
        let plain = at(GlassVariant::Regular, dark, 100.0);
        let p = with(GlassVariant::Regular, dark, 100.0, |m| {
            m.increase_contrast = true
        });
        let f = p.face;
        assert_eq!(f.saturation, 0.8);
        if dark {
            assert_eq!((f.black, f.fill), (0.2, [0.0, 0.0, 0.0, 0.45]));
        } else {
            assert_eq!((f.black, f.fill), (0.5, [1.0, 1.0, 1.0, 0.45]));
        }
        assert_eq!(
            (f.white, f.max_luma),
            (plain.face.white, plain.face.max_luma)
        );
        assert_eq!(p.blur, plain.blur);
        assert_eq!(p.refraction, plain.refraction);
        assert_eq!(p.specular, plain.specular);
    }
}

#[test]
fn reduce_transparency_on_regular_glass_takes_the_recorded_optics_and_faces() {
    for contrast in [false, true] {
        for dark in [false, true] {
            let p = with(GlassVariant::Regular, dark, 100.0, |m| {
                m.reduce_transparency = true;
                m.increase_contrast = contrast;
            });
            assert_eq!((p.blur.radius, p.blur.level_opacity[0]), (40.625, 1.0));
            assert_close!(p.blur_sigma(), 81.25);
            assert_eq!((p.refraction.amount, p.refraction.height), (0.0, 0.0));
            assert_eq!(p.refraction.outer_opacity, 0.0);
            assert_eq!(p.shadow.ring_opacity, 0.0);
            assert_eq!(p.bleed.opacity, 0.0);
            let f = p.face;
            match (dark, contrast) {
                (false, false) => assert_eq!((f.black, f.saturation), (0.9, 0.9)),
                (false, true) => {
                    assert_eq!((f.black, f.saturation, f.fill[3]), (0.5, 0.9, 0.45))
                }
                (true, c) => {
                    assert_eq!(f.saturation, if c { 0.8 } else { 0.7 });
                    assert_eq!((f.black, f.white, f.max_luma), (0.225, 0.31, 1.0));
                    assert_eq!(f.fill[3], if c { 0.45 } else { 0.0 });
                }
            }
        }
    }
}

#[test]
fn reduce_motion_flattens_both_bends_and_widens_the_blur_four_times_for_the_centres() {
    for variant in GlassVariant::ALL {
        if variant == GlassVariant::Identity {
            continue;
        }
        for dark in [false, true] {
            let plain = at(variant, dark, 100.0);
            let p = with(variant, dark, 100.0, |m| m.reduce_motion = true);
            assert_eq!(p.refraction.height, 0.0, "{variant:?}");
            assert_eq!(p.refraction.outer_opacity, 0.0, "{variant:?}");
            assert_eq!(p.blur.level_opacity, [1.0, 1.0, 0.625, 0.625]);
            let scale = match variant {
                GlassVariant::ControlCenter | GlassVariant::NotificationCenter => 4.0,
                _ => 1.0,
            };
            assert_close!(
                p.blur.radius,
                (plain.blur.radius * scale).max(8.0),
                "{variant:?}"
            );
        }
    }
    assert_eq!(
        with(GlassVariant::Regular, false, 100.0, |m| m.reduce_motion =
            true)
        .blur
        .radius,
        8.0
    );
    // the app switcher bends what lies outside its edge, but not with Reduce Motion; the Dock bar never does
    let switcher = GlassVariant::AppSwitcher;
    assert!(at(switcher, false, 100.0).refraction.outer_opacity > 0.0);
    let reduced = with(switcher, false, 100.0, |m| m.reduce_motion = true);
    assert_eq!(reduced.refraction.outer_opacity, 0.0);
    assert_eq!(
        at(GlassVariant::Dock, false, 100.0)
            .refraction
            .outer_opacity,
        0.0
    );
}

#[test]
fn the_inactive_branch_frosts_the_regular_family_and_the_clear_family_has_none() {
    for variant in GlassVariant::ALL {
        for dark in [false, true] {
            let active = at(variant, dark, 100.0);
            let p = inactive(variant, dark, 100.0);
            if variant.base() != Some(GlassBase::Regular) {
                assert!(!variant.has_inactive_branch(), "{variant:?}");
                assert_eq!(p, active, "{variant:?}");
                continue;
            }
            assert!(variant.has_inactive_branch(), "{variant:?}");
            assert_eq!(
                (p.blur.radius, p.blur.level_opacity),
                (0.5, [1.0; 4]),
                "{variant:?}"
            );
            assert_close!(p.blur_sigma(), 1.0);
            assert_eq!(p.shadow.ring_opacity, 0.0, "{variant:?}");
            assert_eq!(p.refraction.outer_opacity, 0.0);
            assert_eq!(p.bleed.opacity, 0.0, "{variant:?}");
            assert_close!(p.specular.amount, active.specular.amount * 0.5);
            if variant != GlassVariant::Tab {
                assert_eq!(p.face, active.face, "{variant:?}");
            }
            // a glass window and a menu keep the window shadow of an inactive window
            if !matches!(variant, GlassVariant::GlassWindow | GlassVariant::Menu) {
                assert_eq!(p.shadow.opacity, 0.0, "{variant:?}");
            }
        }
    }
    assert_eq!(
        inactive(GlassVariant::Tab, false, 40.0).face.fill,
        [1.0, 1.0, 1.0, 0.3]
    );
    assert_eq!(
        inactive(GlassVariant::Tab, true, 40.0).face.fill,
        [1.0, 1.0, 1.0, 0.07]
    );
}

#[test]
fn a_glass_window_casts_the_recorded_window_shadow_unless_its_surface_draws_one() {
    // (active, dark, blur, offset, opacity)
    for (active, dark, blur, offset, opacity) in [
        (true, false, 24.0, 10.0, 0.7),
        (true, true, 24.0, 10.0, 0.8),
        (false, false, 16.0, 4.0, 0.55),
        (false, true, 16.0, 4.0, 0.65),
    ] {
        for variant in [GlassVariant::GlassWindow, GlassVariant::Menu] {
            let p = with(variant, dark, 300.0, |m| m.modifiers.active = active);
            let s = p.shadow;
            assert_eq!(
                (s.height, s.offset, s.fill[3]),
                (blur, [0.0, offset], opacity)
            );
            assert_eq!((s.amount, s.opacity), (100.0, 1.0));
            assert!(p.shadow_reach() > 0.0);
            let bare = with(variant, dark, 300.0, |m| {
                m.modifiers.active = active;
                m.modifiers.exclude_shadow = true;
            });
            assert_eq!(bare.shadow.opacity, 0.0);
            assert_eq!(bare.shadow_reach(), 0.0);
        }
    }
    let base = at(GlassVariant::Regular, false, 200.0).shadow;
    for (active, dark, blur, opacity) in [
        (true, false, 24.0, 0.6),
        (true, true, 24.0, 0.7),
        (false, false, 16.0, 0.45),
        (false, true, 16.0, 0.55),
    ] {
        let s: GlassShadow = base.sheet(active, dark);
        assert_eq!((s.height, s.fill[3]), (blur, opacity));
    }
    // a glass window's face is the regular one
    assert_eq!(
        at(GlassVariant::GlassWindow, true, 300.0).face,
        at(GlassVariant::Regular, true, 300.0).face
    );
}

#[test]
fn a_menu_is_regular_glass_at_its_own_size() {
    for dark in [false, true] {
        for side in [60.0, 200.0] {
            let menu = with(GlassVariant::Menu, dark, side, |m| {
                m.modifiers.exclude_shadow = true
            });
            let regular = at(GlassVariant::Regular, dark, side);
            assert_eq!(menu.face, regular.face);
            assert_eq!(menu.blur, regular.blur);
            assert_eq!(menu.specular, regular.specular);
            assert_eq!(menu.refraction, regular.refraction);
        }
    }
}

#[test]
fn a_tab_lies_under_a_white_fill_and_the_light_one_is_lowered() {
    let light = at(GlassVariant::Tab, false, 30.0).face;
    assert_close!(light.black, 0.35);
    assert_close!(light.white, 0.98);
    assert_eq!(light.fill, [1.0, 1.0, 1.0, 0.15]);
    let dark = at(GlassVariant::Tab, true, 30.0).face;
    assert_eq!(
        (dark.black, dark.white, dark.fill),
        (0.125, 1.125, [1.0, 1.0, 1.0, 0.15])
    );
    let contrast = with(GlassVariant::Tab, false, 30.0, |m| {
        m.increase_contrast = true
    })
    .face;
    assert_close!(contrast.black, 0.45);
    assert_eq!(contrast.fill, [1.0, 1.0, 1.0, 0.15]);
}

#[test]
fn the_window_sidebars_are_planar_dense_panes_and_the_floating_one_keeps_the_light() {
    for dark in [false, true] {
        let regular = at(GlassVariant::Regular, dark, 200.0);
        for variant in [GlassVariant::AbuttedSidebar, GlassVariant::Inspector] {
            let p = at(variant, dark, 200.0);
            assert_eq!(p.specular.amount, 0.0, "{variant:?}");
            assert_eq!(p.highlight.amount, 0.0);
            assert_eq!(p.shadow.ring_opacity, 0.0);
            assert_eq!(p.shadow.edge_line, 0.0);
            assert_eq!(p.refraction.outer_opacity, 0.0);
            assert_eq!(
                p.bleed.opacity, 0.0,
                "nothing outside the window in its edge"
            );
            assert_eq!((p.blur.radius, p.blur.fill_radius), (5.0, 8.0));
            assert_eq!(p.blur.level_opacity, [1.0; 4]);
            assert_eq!((p.refraction.amount, p.refraction.height), (-24.0, 12.0));
            // a dense pane with a faint tint of what lies behind the window: black and white come out close together
            let f = p.face;
            assert_eq!((f.saturation, f.max_luma), (WINDOW_SIDEBAR_SATURATION, 1.0));
            if dark {
                assert_eq!(
                    (f.black, f.white, f.fill),
                    (
                        WINDOW_SIDEBAR_BLACK_DARK,
                        WINDOW_SIDEBAR_WHITE_DARK,
                        [0.0; 4]
                    )
                );
            } else {
                assert_eq!(
                    (f.black, f.white, f.fill),
                    (
                        WINDOW_SIDEBAR_BLACK_LIGHT,
                        WINDOW_SIDEBAR_WHITE_LIGHT,
                        WINDOW_SIDEBAR_VEIL_LIGHT
                    )
                );
            }
            assert!(f.white - f.black < 0.05);
            // Increase Contrast keeps the pane's face
            let c = with(variant, dark, 200.0, |m| m.increase_contrast = true);
            assert_eq!(c.face, f);
        }
        assert_eq!(
            at(GlassVariant::Inspector, dark, 200.0),
            at(GlassVariant::AbuttedSidebar, dark, 200.0)
        );
        let floating = at(GlassVariant::Sidebar, dark, 200.0);
        assert_eq!(floating.specular, regular.specular);
        assert_eq!(
            floating.face.max_luma,
            if dark { regular.face.max_luma } else { 0.85 }
        );
    }
    assert_eq!(
        GlassVariant::Sidebar.sidebar_edge(),
        Some(SidebarEdge::Leading)
    );
    assert_eq!(
        GlassVariant::AbuttedSidebar.sidebar_edge(),
        Some(SidebarEdge::Leading)
    );
    assert_eq!(
        GlassVariant::Inspector.sidebar_edge(),
        Some(SidebarEdge::Trailing)
    );
    assert_eq!(GlassVariant::Regular.sidebar_edge(), None);
}

#[test]
fn a_cartouche_bends_a_fixed_five_points_at_the_size_it_resolves_at() {
    for size in [GlassSize::Minimum, GlassSize::Medium] {
        for dark in [false, true] {
            let mode = GlassMode::new(
                dark,
                GlassVariant::CartouchePopover,
                GlassModifiers::DEFAULT.sized(size),
            );
            let small = GlassParams::new(mode, 20.0);
            assert_eq!(
                small,
                GlassParams::new(mode, 300.0),
                "{size:?}: resolved at its size"
            );
            assert_eq!(
                (small.refraction.amount, small.refraction.height),
                (-5.0, 5.0)
            );
            assert_eq!(small.blur.radius, 5.0);
            assert_eq!(small.shadow.ring_opacity, 0.06);
            let side = size.side(0.0);
            let regular = at(GlassVariant::Regular, dark, side);
            assert_eq!(small.specular, regular.specular);
            assert_eq!(small.blur, regular.blur);
            if dark {
                assert_eq!(small.face, regular.face);
            } else {
                assert_eq!(small.face.fill, [1.0, 1.0, 1.0, 0.1]);
            }
        }
    }
    assert_eq!(GlassSize::Minimum.side(300.0), GLASS_SIZE_MINIMUM);
    assert_eq!(GlassSize::Medium.side(10.0), GLASS_SIZE_MEDIUM);
    assert_eq!(GlassSize::Dynamic.side(37.0), 37.0);
}

#[test]
fn a_toolbar_item_blurs_a_fifth_bends_a_fixed_band_lights_a_deeper_rim_and_leaves_its_outline_to_the_toolkit(
) {
    for dark in [false, true] {
        for side in [28.0, 36.0] {
            let p = at(GlassVariant::ToolbarItem, dark, side);
            assert_eq!(&p.blur.level_opacity[..2], &[0.2, 0.2]);
            assert_close!(p.blur_sigma(), 2.0);
            assert_eq!((p.refraction.amount, p.refraction.height), (-18.0, 9.0));
            assert_eq!(
                p.border.opacity, 0.0,
                "the system's control border is clear without a border setting"
            );
            assert_eq!(p.shadow.edge_line, 0.0);
            // a deeper, softer line of light than the regular glass's, its glow as deep and as strong
            let (sp, regular) = (p.specular, at(GlassVariant::Regular, dark, side).specular);
            assert_eq!(
                (sp.amount, sp.height),
                (TOOLBAR_SPECULAR_AMOUNT, TOOLBAR_SPECULAR_HEIGHT)
            );
            assert!(sp.height > regular.height && sp.amount < regular.amount);
            assert_close!(
                sp.diffuse.height_scale * sp.height,
                regular.diffuse.height_scale * regular.height
            );
            assert_close!(
                sp.diffuse.amount_scale * sp.amount,
                regular.diffuse.amount_scale * regular.amount
            );
            assert_eq!(sp.diffuse.spread_scale, regular.diffuse.spread_scale);
        }
        let contrast = with(GlassVariant::ToolbarItem, dark, 36.0, |m| {
            m.increase_contrast = true
        });
        assert_eq!(contrast.border.opacity, 0.0);
    }
}

#[test]
fn the_dock_takes_its_own_recipe_and_its_own_accessibility_stages() {
    for side in [64.0, 90.0] {
        for dark in [false, true] {
            let p = at(GlassVariant::Dock, dark, side);
            assert_close!(p.blur.radius, DOCK_BLUR_SCALE * CLEAR_BLUR);
            // a Gaussian of about 2 points
            assert!((p.blur_sigma() - 2.0).abs() < 0.01, "{}", p.blur_sigma());
            assert_eq!(p.blur.level_opacity[0], 1.0);
            assert!(!p.fill_shows());
            let f = p.face;
            assert_eq!(f.saturation, 1.25);
            if dark {
                assert_eq!(
                    (f.black, f.white, f.fill, f.max_luma),
                    (0.05, 0.92, [0.0, 0.0, 0.0, 0.02], DOCK_BAR_MAX_LUMA_DARK)
                );
                assert_eq!(f.max_luma, 0.75);
            } else {
                assert_eq!(
                    (f.black, f.white, f.fill[3], f.max_luma),
                    (0.15, 0.9, 0.0, 1.0)
                );
            }
            // a thin bend inside the edge, the same at any size, and nothing bent outside it
            let r = p.refraction;
            assert_close!(r.amount, -0.18 * DOCK_BAR_BEND_DIMENSION);
            assert_close!(r.height, 0.08 * DOCK_BAR_BEND_DIMENSION);
            assert_close!(r.amount, -8.64);
            assert_close!(r.height, 3.84);
            assert_close!(r.outer_amount, 0.0);
            assert_close!(r.outer_height, 0.0);
            assert_close!(r.outer_opacity, 0.0);
            // the light along the top and the bottom, from above and as much from below, in the colour of the face
            let s = p.specular;
            assert_eq!(
                (s.amount, s.fill, s.matrix_share),
                (DOCK_BAR_LIGHT, 1.0, 1.0)
            );
            assert_close!(s.angle, std::f32::consts::FRAC_PI_2);
            assert_close!(s.spread, 60.0_f32.to_radians());
            assert_close!(s.height, 1.5);
            assert_close!(s.height * s.diffuse.height_scale, 8.0);
            assert_eq!(
                (s.diffuse.spread_scale, s.diffuse.amount_scale),
                (0.65, 0.15)
            );
            // the clear glass's rim, laid just outside the ends instead of inside the corners
            let h = p.highlight;
            assert!(h.outside);
            assert_eq!(
                (h.amount, h.color_bias, h.height, h.offset),
                (0.4, -0.25, 0.5, -0.5)
            );
            assert_eq!(h.angle, 0.0);
            assert_close!(h.spread, 120.0_f32.to_radians());
            // the light's colour: the matrix of its appearance, brightened by its gain
            let lift = if dark { 0.25 } else { 0.2 };
            assert_close!(h.matrix[4], lift * DOCK_BAR_LIGHT_GAIN);
            assert_eq!(h.matrix[18], 1.0);
            assert_eq!(h.glow_amount, 0.0);
            // no line but the rim's outside the edge, and no ring
            let sh = p.shadow;
            assert_eq!(sh.edge_line, 0.0);
            assert_eq!(sh.ring_opacity, 0.0);

            let c = with(GlassVariant::Dock, dark, side, |m| {
                m.increase_contrast = true
            });
            if dark {
                assert_eq!(
                    (c.face.black, c.face.white, c.face.fill),
                    (0.1, 0.65, [0.0, 0.0, 0.0, 0.15])
                );
            } else {
                assert_eq!(
                    (c.face.black, c.face.white, c.face.fill),
                    (0.125, 0.7, [1.0, 1.0, 1.0, 0.12])
                );
            }
            assert_close!(c.specular.amount, 0.4 * 0.75);
            assert_eq!(c.specular.matrix_share, 0.0, "the regular key light, white");
            assert_eq!((c.blur, c.refraction), (p.blur, p.refraction));

            for contrast in [false, true] {
                let r = with(GlassVariant::Dock, dark, side, |m| {
                    m.reduce_transparency = true;
                    m.increase_contrast = contrast;
                });
                assert_eq!(
                    (r.blur, r.refraction),
                    (p.blur, p.refraction),
                    "keeps its optics"
                );
                if dark {
                    assert_eq!(r.face, if contrast { c.face } else { p.face });
                } else {
                    assert_eq!(
                        (r.face.black, r.face.white, r.face.fill),
                        (0.4, 0.92, [1.0, 1.0, 1.0, 0.1])
                    );
                }
            }
        }
    }
}

#[test]
fn the_app_switcher_is_the_dock_with_a_wider_blur() {
    for (dark, scale) in [(false, 2.0), (true, 3.5)] {
        let p = at(GlassVariant::AppSwitcher, dark, 120.0);
        let dock = at(GlassVariant::Dock, dark, 120.0);
        assert_close!(p.blur.radius, CLEAR_BLUR * scale);
        // the Dock's face with the recipe's own bends, which the Dock bar's thin lens replaces
        assert_eq!(
            (p.face.black, p.face.white, p.face.fill),
            (dock.face.black, dock.face.white, dock.face.fill)
        );
        assert_eq!(p.face.max_luma, 1.0);
        assert_close!(p.refraction.amount, -0.18 * 120.0);
        assert_close!(p.refraction.height, 0.08 * 120.0);
        assert_close!(p.refraction.outer_amount, 0.42 * 120.0);
        assert_close!(p.refraction.outer_height, 0.13 * 120.0);
        assert_eq!(p.refraction.outer_opacity, 0.2);
        assert_eq!(p.highlight.glow_amount, 0.0);
        // the clear glass's rim inside its corners, and its own edge line
        assert!(!p.highlight.outside);
        assert!(p.shadow.edge_line > 0.0);
        let r = with(GlassVariant::AppSwitcher, dark, 120.0, |m| {
            m.reduce_transparency = true
        });
        let dock_r = with(GlassVariant::Dock, dark, 120.0, |m| {
            m.reduce_transparency = true
        });
        assert_eq!(
            (r.face.black, r.face.white, r.face.fill),
            (dock_r.face.black, dock_r.face.white, dock_r.face.fill),
            "the Dock's Reduce Transparency"
        );
    }
}

#[test]
fn clear_glass_is_the_same_in_both_appearances_and_blurs_widely_without_transparency() {
    for side in [30.0, 60.0] {
        let light = at(GlassVariant::Clear, false, side);
        assert_eq!(light, at(GlassVariant::Clear, true, side));
        assert_eq!(light.blur.radius, CLEAR_BLUR);
        assert_close!(light.blur_sigma(), 2.0 * CLEAR_BLUR);
        let f = light.face;
        assert_eq!(
            (f.black, f.white, f.saturation, f.max_luma, f.fill[3]),
            (0.125, 1.08, 1.06, 1.0, 0.0)
        );
        assert_eq!(light.shadow.ring_opacity, 0.0);
        assert_eq!(light.specular.amount, 0.0);
        assert_eq!(at(GlassVariant::AppIcons, false, side), light);
    }
    let p = at(GlassVariant::Clear, false, 60.0);
    assert_eq!((p.refraction.amount, p.refraction.height), (-39.0, 20.0));
    for dark in [false, true] {
        let r = with(GlassVariant::Clear, dark, 60.0, |m| {
            m.reduce_transparency = true
        });
        assert_eq!(
            (r.blur.radius, r.refraction.amount, r.refraction.height),
            (200.0, 0.0, 0.0)
        );
        assert_eq!(r.face.black, 0.125);
        assert_eq!(r.face.saturation, if dark { 0.7 } else { 1.06 });
    }
}

#[test]
fn the_control_centre_keeps_its_fitted_module_face_and_shifts_it_by_the_recorded_amounts() {
    let rest = at(GlassVariant::ControlCenter, false, 100.0);
    assert_eq!(
        rest,
        at(GlassVariant::ControlCenter, true, 100.0),
        "the same in both appearances"
    );
    let f = rest.face;
    assert_eq!(
        (f.saturation, f.white, f.black),
        (MODULE_SATURATION, MODULE_WHITE, MODULE_BLACK)
    );
    for dark in [false, true] {
        let c = with(GlassVariant::ControlCenter, dark, 100.0, |m| {
            m.increase_contrast = true
        })
        .face;
        if dark {
            assert_close!(c.white, MODULE_WHITE + 0.55);
            assert_eq!(c.black, MODULE_BLACK);
        } else {
            assert_close!(c.black, MODULE_BLACK - 0.2);
            assert_eq!(c.white, MODULE_WHITE);
        }
        let r = with(GlassVariant::ControlCenter, dark, 100.0, |m| {
            m.reduce_transparency = true
        });
        assert_eq!(
            (r.blur.radius, r.refraction.amount, r.refraction.height),
            (10.0, 0.0, 0.0)
        );
        if dark {
            assert_close!(r.face.black, MODULE_BLACK + 0.05);
            assert_close!(r.face.white, MODULE_WHITE + 0.10);
            assert_eq!(r.face.saturation, 0.7);
        } else {
            assert_close!(r.face.black, MODULE_BLACK - 0.25);
            assert_close!(r.face.white, MODULE_WHITE - 0.11);
        }
    }
    let subdued = with(GlassVariant::ControlCenter, false, 100.0, |m| {
        m.modifiers.subdued = true
    });
    assert_close!(subdued.blur.radius, rest.blur.radius * 2.0);
}

#[test]
fn the_controls_lenses_take_the_recorded_control_recipes() {
    // (variant, ring opacity light / dark, ring offset, ring deviation, bend and band per point of short side)
    for (variant, ring, offset, deviation, amount, height) in [
        (GlassVariant::Control, (0.1, 0.1), 8.0, 3.0, -0.15, 0.1),
        (GlassVariant::Slider, (0.15, 0.4), 8.0, 3.0, -0.27, 0.18),
        (GlassVariant::Loupe, (0.07, 0.07), 12.0, 4.0, -0.3, 0.095),
    ] {
        for dark in [false, true] {
            let p = at(variant, dark, 31.0);
            assert_eq!(p.blur_sigma(), 0.0, "{variant:?}");
            assert!(!p.fill_shows());
            let f = p.face;
            let (black, white) = if dark { (0.2, 0.6) } else { (0.5, 1.03) };
            assert_eq!(
                (f.black, f.white, f.saturation, f.max_luma),
                (black, white, 1.0, 1.0)
            );
            let s = p.shadow;
            assert_eq!(
                s.ring_opacity,
                if dark { ring.1 } else { ring.0 },
                "{variant:?}"
            );
            assert_eq!(
                (s.ring_offset, s.ring_blur, s.ring_stroke),
                (offset, deviation, 4.0)
            );
            assert_close!(p.refraction.amount, amount * 31.0);
            assert_close!(p.refraction.height, height * 31.0);
            assert_eq!(p.capture_scale, TRACK_CAPTURE_SCALE);
            // no accessibility values are recorded
            let r = with(variant, dark, 31.0, |m| {
                m.reduce_transparency = true;
                m.increase_contrast = true;
            });
            assert_eq!(r, p, "{variant:?}");
        }
    }
}

#[test]
fn the_notification_centre_is_regular_glass_at_the_medium_size() {
    for dark in [false, true] {
        let card = GlassParams::new(GlassMode::of_style(dark, GlassStyle::Card), 70.0);
        let regular = at(GlassVariant::Regular, dark, GLASS_SIZE_MEDIUM);
        assert_eq!(
            (card.face, card.blur, card.refraction),
            (regular.face, regular.blur, regular.refraction)
        );
        assert_eq!(card.specular, regular.specular);
        assert_eq!(card.shadow.ring_opacity, 0.06);
        assert!(card.shadow.edge_line > 0.0);
    }
}

#[test]
fn the_launcher_panel_takes_its_face_over_the_regular_optics() {
    let light = at(GlassVariant::SearchResults, false, 400.0);
    assert_eq!(light.face.black, LARGE_PANEL_BLACK_LIGHT);
    assert_eq!(light.blur.radius, 10.0);
    assert_eq!(light.shadow.height, LARGE_PANEL_SHADOW_BLUR);
    let dark = at(GlassVariant::SearchResults, true, 400.0);
    assert_eq!(dark.face.saturation, LARGE_PANEL_SATURATION_DARK);
    // the regular glass's bend, edge bleed, rim, ring and key and fill light, as large as the panel
    for (p, dark) in [(light, false), (dark, true)] {
        let regular = at(GlassVariant::Regular, dark, 400.0);
        assert_eq!(p.refraction, regular.refraction);
        assert_eq!((p.refraction.amount, p.refraction.height), (-60.0, 20.0));
        assert_eq!(p.bleed, regular.bleed);
        assert_eq!(p.highlight, regular.highlight);
        assert_eq!(p.specular, regular.specular);
        assert_eq!(p.shadow.ring_opacity, regular.shadow.ring_opacity);
    }
    let reduced = with(GlassVariant::SearchResults, true, 400.0, |m| {
        m.reduce_transparency = true
    });
    assert_eq!(reduced.face.saturation, 0.3);
}

#[test]
fn an_entry_field_does_not_blur_and_a_focus_border_is_regular_glass() {
    for dark in [false, true] {
        let field = at(GlassVariant::EntryField, dark, 22.0);
        assert_eq!(field.blur_sigma(), 0.0);
        assert!(!field.fill_shows());
        assert_eq!(field.face, at(GlassVariant::Regular, dark, 22.0).face);
        assert_eq!(
            at(GlassVariant::FocusBorder, dark, 22.0),
            at(GlassVariant::Regular, dark, 22.0)
        );
    }
}

#[test]
fn identity_glass_leaves_the_backdrop_as_it_is_with_every_option() {
    assert!(!GlassVariant::Identity.draws_glass());
    assert!(
        GlassVariant::ALL
            .iter()
            .filter(|v| !v.draws_glass())
            .count()
            == 1
    );
    for bits in 0..32u8 {
        let p = with(GlassVariant::Identity, bits & 1 != 0, 80.0, |m| {
            m.reduce_transparency = bits & 2 != 0;
            m.increase_contrast = bits & 4 != 0;
            m.reduce_motion = bits & 8 != 0;
            m.density = if bits & 16 != 0 { 100 } else { 0 };
            m.modifiers.active = bits & 1 == 0;
        });
        assert_eq!(p, GlassParams::identity());
        assert_eq!(p.blur_sigma(), 0.0);
        assert!(!p.fill_shows());
        let f = p.face;
        assert_eq!(
            (f.saturation, f.white, f.black, f.max_luma, f.fill[3]),
            (1.0, 1.0, 0.0, 1.0, 0.0)
        );
        assert_eq!(
            (
                p.refraction.amount,
                p.refraction.outer_opacity,
                p.bleed.opacity
            ),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            (
                p.highlight.amount,
                p.highlight.glow_amount,
                p.specular.amount
            ),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            (p.shadow.ring_opacity, p.shadow.opacity, p.shadow.edge_line),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            (p.border.opacity, p.tint[3], p.veil),
            (0.0, 0.0, [0.0, 0.0])
        );
        assert_eq!(p.capture_scale, TRACK_CAPTURE_SCALE);
    }
}

#[test]
fn density_zero_leaves_every_recipe_as_it_is_and_any_density_changes_only_the_blur_fill() {
    assert_eq!(legibility_amount(0), 0.5, "the neutral amount");
    for variant in GlassVariant::ALL {
        for bits in 0..32u8 {
            let m = GlassMode {
                reduce_transparency: bits & 2 != 0,
                increase_contrast: bits & 4 != 0,
                reduce_motion: bits & 8 != 0,
                modifiers: GlassModifiers {
                    active: bits & 16 == 0,
                    ..GlassModifiers::DEFAULT
                },
                ..mode(variant, bits & 1 != 0)
            };
            let plain = GlassParams::new(m, 90.0);
            let mut dense = GlassParams::new(GlassMode { density: 100, ..m }, 90.0);
            if m.reduce_transparency || variant == GlassVariant::Identity {
                assert_eq!(dense, plain, "{variant:?}: nothing to thicken");
            }
            dense.blur.fill_radius = plain.blur.fill_radius;
            dense.blur.fill_normal = plain.blur.fill_normal;
            dense.blur.fill_lighten = plain.blur.fill_lighten;
            dense.blur.fill_darken = plain.blur.fill_darken;
            assert_eq!(dense, plain, "{variant:?} {bits}");
        }
    }
}

#[test]
fn the_legibility_amount_fills_regular_glass_towards_opaque_and_gives_clear_glass_a_fill() {
    assert_eq!(
        (
            legibility_amount(50),
            legibility_amount(100),
            legibility_amount(200)
        ),
        (0.75, 1.0, 1.0)
    );
    for dark in [false, true] {
        let full = with(GlassVariant::Regular, dark, 100.0, |m| m.density = 100).blur;
        assert_eq!(full.fill_normal, 1.0);
        assert_close!(
            if dark {
                full.fill_darken
            } else {
                full.fill_lighten
            },
            0.9
        );
        let half = with(GlassVariant::Regular, dark, 100.0, |m| m.density = 50).blur;
        assert_close!(half.fill_normal, 0.75);
        for variant in [GlassVariant::Clear, GlassVariant::Dock] {
            let clear = with(variant, dark, 100.0, |m| m.density = 50).blur;
            assert_eq!(
                (clear.fill_radius, clear.fill_lighten, clear.fill_darken),
                (8.0, 0.0, 0.0)
            );
            assert_close!(clear.fill_normal, 0.5);
            assert!(with(variant, dark, 100.0, |m| m.density = 100).fill_shows());
        }
    }
}
