//! The decorations a compositor draws: the title bar strip and its buttons, the title's placement, the rim and the
//! edges.

use cupertino_assets::Assets;
use cupertino_geometry::Size;
use cupertino_server_render::decorations::edge::{edge, EdgeKind, Hairline};
use cupertino_server_render::decorations::{
    ButtonKind, DecorationConfig, DecorationState, Decorations, Draw, TitlebarStyle,
};
use cupertino_theme::Appearance;

fn decorations(appearance: Appearance, style: TitlebarStyle) -> Decorations {
    let config = DecorationConfig::builtin(appearance, style);
    Decorations::new(config, &Assets::discover()).expect("the fonts of the asset directory load")
}

fn alpha_sum(pm: &tiny_skia::Pixmap) -> u64 {
    pm.pixels().iter().map(|p| p.alpha() as u64).sum()
}

#[test]
fn the_title_bar_strip_is_as_wide_and_tall_as_the_layout_at_every_scale() {
    let mut dec = decorations(Appearance::Light, TitlebarStyle::Standard);
    let state = DecorationState::default();
    let height = dec.layout(Size::new(400.0, 300.0)).titlebar.height();
    for scale in [1.0, 1.5, 2.0] {
        let strip = dec.render_titlebar(400.0, scale, &state);
        assert_eq!(strip.width(), (400.0 * scale).round() as u32);
        assert_eq!(strip.height(), (height * scale).round() as u32);
        assert!(alpha_sum(&strip) > 0);
    }
}

#[test]
fn the_strip_shows_the_window_state() {
    let mut dec = decorations(Appearance::Light, TitlebarStyle::Standard);
    let base = DecorationState {
        title: "Notes".into(),
        ..DecorationState::default()
    };
    let render = |dec: &mut Decorations, state: &DecorationState| {
        dec.render_titlebar(400.0, 2.0, state).data().to_vec()
    };
    let reference = render(&mut dec, &base);
    let variants: [(&str, DecorationState); 6] = [
        (
            "inactive",
            DecorationState {
                active: false,
                ..base.clone()
            },
        ),
        (
            "hovered group",
            DecorationState {
                group_hovered: true,
                ..base.clone()
            },
        ),
        (
            "pressed close",
            DecorationState {
                pressed: Some(ButtonKind::Close),
                ..base.clone()
            },
        ),
        (
            "disabled zoom",
            DecorationState {
                enabled: [true, true, false],
                ..base.clone()
            },
        ),
        (
            "other title",
            DecorationState {
                title: "Mail".into(),
                ..base.clone()
            },
        ),
        (
            "no title",
            DecorationState {
                title: String::new(),
                ..base.clone()
            },
        ),
    ];
    for (what, state) in variants {
        assert_ne!(render(&mut dec, &state), reference, "{what}");
    }
    assert_eq!(
        render(&mut dec, &base),
        reference,
        "drawing is deterministic"
    );
}

#[test]
fn the_appearance_changes_the_colours_and_a_flush_strip_has_square_corners() {
    let state = DecorationState::default();
    let mut light = decorations(Appearance::Light, TitlebarStyle::Standard);
    let mut dark = decorations(Appearance::Dark, TitlebarStyle::Standard);
    assert_ne!(
        light.render_titlebar(300.0, 1.0, &state).data(),
        dark.render_titlebar(300.0, 1.0, &state).data()
    );
    let rounded = light.render_titlebar(300.0, 1.0, &state);
    let flush = light.render_flush_titlebar(300.0, 1.0, &state);
    assert!(
        rounded.pixels()[0].alpha() < 255,
        "the window's corner is cut away"
    );
    assert_eq!(
        flush.pixels()[0].alpha(),
        255,
        "the flush strip reaches the corner"
    );
}

#[test]
fn a_client_drawn_row_gets_the_buttons_alone() {
    let state = DecorationState {
        title: "Ignored".into(),
        ..DecorationState::default()
    };
    for style in [
        TitlebarStyle::Unified,
        TitlebarStyle::UnifiedCompact,
        TitlebarStyle::Sidebar,
    ] {
        let mut dec = decorations(Appearance::Light, style);
        assert_eq!(
            dec.title_placement(400.0, "Ignored", 1.0),
            None,
            "{style:?}"
        );
        let draws = dec.titlebar_draws(400.0, 1.0, &state, false);
        let layout = dec.layout(Size::new(400.0, 300.0));
        let strip = draws.paint();
        let group = layout.button_group;
        for (i, p) in strip.pixels().iter().enumerate() {
            let (x, y) = (
                (i as u32 % strip.width()) as f64,
                (i as u32 / strip.width()) as f64,
            );
            if !group
                .outset(1.0)
                .contains(cupertino_geometry::Point::new(x, y))
            {
                assert_eq!(p.alpha(), 0, "{style:?}: ({x}, {y}) is the client's");
            }
        }
        assert!(alpha_sum(&strip) > 0, "{style:?}");
    }
}

#[test]
fn a_title_is_placed_between_the_buttons_and_truncated_with_an_ellipsis() {
    let mut dec = decorations(Appearance::Light, TitlebarStyle::Standard);
    let reach = dec.layout(Size::new(400.0, 0.0)).group_reach();

    assert_eq!(dec.title_placement(400.0, "", 1.0), None);
    let (left, width, text) = dec.title_placement(400.0, "Notes", 1.0).unwrap();
    assert_eq!(text, "Notes");
    assert!(
        (left + width / 2.0 - 200.0).abs() < 0.5,
        "a short title is centred"
    );

    let long = "A very long document title that cannot fit between the window buttons";
    let (left, width, text) = dec.title_placement(400.0, long, 1.0).unwrap();
    assert!(text.ends_with('…') && text.len() < long.len());
    assert!(left >= reach && left + width <= 400.0 - reach + 0.5);

    assert_eq!(
        dec.title_placement(2.0 * reach, long, 1.0),
        None,
        "no room, no title"
    );
}

#[test]
fn the_rim_is_a_transparent_ring_inside_the_frame() {
    let dec = decorations(Appearance::Dark, TitlebarStyle::Standard);
    let size = Size::new(200.0, 120.0);
    let rim = dec.render_rim(size, 2.0);
    assert_eq!((rim.width(), rim.height()), (400, 240));
    let at = |x: u32, y: u32| rim.pixels()[(y * rim.width() + x) as usize].alpha();
    assert!(at(200, 0) > 0, "the top edge has the rim");
    assert!(at(0, 120) > 0, "the left edge has the rim");
    assert_eq!(at(200, 120), 0, "the inside is clear");
}

#[test]
fn edges_are_strongest_for_the_active_window_and_use_the_contrast_rim_when_asked() {
    let rim = [0.2, 0.4, 0.6, 1.0];
    let active = edge(EdgeKind::Window, false, true, false, rim);
    let inactive = edge(EdgeKind::Window, false, false, false, rim);
    assert!(active.outline.color[3] > inactive.outline.color[3]);

    let contrast = edge(EdgeKind::Window, false, true, true, rim);
    assert_eq!(contrast.outline.color, rim);
    assert!(contrast.outline.width_pt > active.outline.width_pt);

    let menu = edge(EdgeKind::Menu, true, true, false, rim);
    assert!(menu.inner.is_some());
    assert_eq!(
        edge(EdgeKind::Menu, true, true, true, rim).inner,
        None,
        "the contrast border stands alone"
    );
}

#[test]
fn an_inactive_window_keeps_most_of_its_outline_and_a_dark_window_rim_is_two_device_pixels_at_2x() {
    let rim = [0.2, 0.4, 0.6, 1.0];
    let alpha = |dark, active| {
        edge(EdgeKind::Window, dark, active, false, rim)
            .outline
            .color[3]
    };
    // the dark outline: 0.93 active, about 0.83 inactive
    assert!(
        (alpha(true, false) - 0.83).abs() < 0.01,
        "{}",
        alpha(true, false)
    );
    for dark in [false, true] {
        let share = alpha(dark, false) / alpha(dark, true);
        assert!((0.85..1.0).contains(&share), "dark {dark}: {share}");
    }
    let inner = |dark| {
        edge(EdgeKind::Window, dark, true, false, rim)
            .inner
            .expect("a window has an inner rim")
            .device(2.0)
            .0
    };
    assert_eq!((inner(true), inner(false)), (2, 1), "device pixels at 2x");
    // a menu's side outline is darker in the dark appearance
    let menu = |dark| edge(EdgeKind::Menu, dark, true, false, rim).outline.color[3];
    assert_eq!((menu(true), menu(false)), (0.7, 0.5));
    // along the top a light menu keeps a quarter of its outline, a dark one none
    let top = |dark| {
        edge(EdgeKind::Menu, dark, true, false, rim)
            .falloff
            .weight((0.0, -1.0))
    };
    assert_eq!((top(true), top(false)), (0.0, 0.25));
}

#[test]
fn a_hairline_is_whole_device_pixels_with_the_opacity_its_coverage_leaves() {
    let line = Hairline {
        color: [0.0, 0.0, 0.0, 0.8],
        width_pt: 0.5,
    };
    let cases = [(1.0, 1, 0.4), (2.0, 1, 0.8), (3.0, 2, 0.8 * 0.75)];
    for (scale, pixels, alpha) in cases {
        let (px, color) = line.device(scale);
        assert_eq!(px, pixels, "scale {scale}");
        assert!(
            (color[3] - alpha).abs() < 1e-6,
            "scale {scale}: {}",
            color[3]
        );
    }
}

#[test]
fn the_title_bar_is_a_draw_list_the_gpu_can_replay() {
    let mut dec = decorations(Appearance::Light, TitlebarStyle::Standard);
    let state = DecorationState {
        title: "Notes".into(),
        ..DecorationState::default()
    };
    let list = dec.titlebar_draws(400.0, 2.0, &state, false);
    assert_eq!((list.width, list.height), (800, 64));
    assert!(list.draws.iter().any(|d| matches!(d, Draw::Shape { .. })));
    assert_eq!(
        list.paint().data(),
        dec.render_titlebar(400.0, 2.0, &state).data(),
        "painting the list is the strip"
    );
}
