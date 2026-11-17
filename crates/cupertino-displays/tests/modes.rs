//! The scaled "looks like" list of a display and the scale a new display starts at.

use cupertino_displays::{
    candidates, catalogue, default_choice, is_built_in, looks_like, scaled_choices, Candidate,
    Extent, Mode, Panel, BUILT_IN_BACKING, MAX_CHOICES, MIN_HEIGHT_POINTS, SCALE_EIGHTHS,
};

const REFRESH: u32 = 60_000;

/// A 27 inch 4K monitor.
fn monitor_4k() -> Panel {
    Panel::from_physical((3840, 2160), (597, 336), false).unwrap()
}

/// A 13 inch laptop panel.
fn laptop() -> Panel {
    Panel::from_physical((2880, 1800), (286, 179), false)
        .unwrap()
        .built_in(true)
}

fn logical(c: &Candidate) -> (u32, u32) {
    (c.mode.width, c.mode.height)
}

#[test]
fn a_panel_needs_a_size_to_derive_its_density() {
    assert_eq!(Panel::from_physical((1920, 1080), (0, 0), false), None);
    assert_eq!(Panel::from_physical((1920, 0), (600, 340), false), None);
    let p = monitor_4k();
    assert!((p.vertical_dpi - 163.3).abs() < 0.1);
    assert!(!p.built_in && !p.precise_rate);
    assert!(p.built_in(true).built_in);
}

#[test]
fn candidates_cover_every_eighth_scale_from_native_up() {
    let list = candidates((3840, 2160), REFRESH);
    assert_eq!(list.len(), SCALE_EIGHTHS.count());
    assert_eq!(list[0].scale, 1.0);
    assert!(list[0].mode.native);
    assert_eq!(logical(&list[0]), (3840, 2160));
    assert_eq!(list.iter().filter(|c| c.mode.native).count(), 1);
    assert_eq!(list.last().unwrap().scale, 3.0);
    assert_eq!(logical(list.last().unwrap()), (1280, 720));
    assert!(list.windows(2).all(|w| w[0].scale < w[1].scale));
}

#[test]
fn a_candidate_draws_into_whole_pixel_buffers() {
    let list = candidates((3840, 2160), REFRESH);
    let one_and_a_half = list.iter().find(|c| c.scale == 1.5).unwrap();
    assert_eq!(one_and_a_half.logical(), Extent::new(2560, 1440));
    assert_eq!(one_and_a_half.backing(), 2);
    assert_eq!(one_and_a_half.raster(), Extent::new(5120, 2880));
    assert_eq!(one_and_a_half.mode.refresh_mhz, REFRESH);
    let two = list.iter().find(|c| c.scale == 2.0).unwrap();
    assert_eq!(two.raster(), Extent::new(3840, 2160));
}

#[test]
fn a_4k_monitor_starts_at_one_and_a_half() {
    let start = default_choice(&monitor_4k(), REFRESH).expect("a default");
    assert_eq!(start.scale, 1.5);
    assert_eq!(logical(&start), (2560, 1440));
}

#[test]
fn the_catalogue_is_ordered_by_height_and_its_default_is_one_of_its_choices() {
    let panel = monitor_4k();
    let cat = catalogue(&panel, REFRESH);
    assert!(!cat.choices.is_empty() && cat.choices.len() <= MAX_CHOICES);
    assert!(cat
        .choices
        .windows(2)
        .all(|w| w[0].mode.height < w[1].mode.height));
    let default = cat.default_choice().expect("a default");
    assert_eq!(Some(default), default_choice(&panel, REFRESH));
    assert_eq!(cat.position(default.logical()), cat.default);
    assert_eq!(scaled_choices(&panel, REFRESH), cat.choices);
}

#[test]
fn every_choice_keeps_the_panel_shape_and_stays_above_the_floor() {
    let panel = monitor_4k();
    for c in catalogue(&panel, REFRESH).choices {
        let ratio = c.mode.width as f32 / c.mode.height as f32;
        assert!((ratio - 16.0 / 9.0).abs() < 0.02, "{:?}", logical(&c));
        assert!(c.mode.height as f32 >= MIN_HEIGHT_POINTS);
        assert!(c.mode.width <= panel.pixels_wide && c.mode.height <= panel.pixels_high);
    }
}

#[test]
fn a_rotated_panel_has_a_lower_floor_by_its_aspect_ratio() {
    let portrait = Panel::from_physical((2160, 3840), (336, 597), true).unwrap();
    let cat = catalogue(&portrait, REFRESH);
    assert!(!cat.choices.is_empty());
    let floor = MIN_HEIGHT_POINTS * portrait.pixels_wide as f32 / portrait.pixels_high as f32;
    assert!(cat.choices.iter().all(|c| c.mode.height as f32 >= floor));
}

#[test]
fn a_panel_no_taller_than_the_floor_offers_nothing() {
    let small = Panel::from_physical((1024, 600), (222, 130), false).unwrap();
    let modes: Vec<Mode> = candidates((1024, 600), REFRESH)
        .iter()
        .map(|c| c.mode)
        .collect();
    let list = looks_like(&small, &modes);
    assert!(list.choices.is_empty());
    assert_eq!(list.anchor, None);
    assert_eq!(default_choice(&small, REFRESH), None);
    assert!(catalogue(&small, REFRESH).choices.is_empty());
}

#[test]
fn looks_like_ignores_modes_of_another_shape_and_larger_than_the_panel() {
    let panel = monitor_4k();
    let native = Mode {
        width: 3840,
        height: 2160,
        scale: 1.0,
        pixels_high: 2160,
        refresh_mhz: REFRESH,
        native: true,
    };
    let wide = Mode {
        width: 2560,
        height: 1080,
        pixels_high: 1080,
        native: false,
        ..native
    };
    let too_big = Mode {
        width: 7680,
        height: 4320,
        pixels_high: 4320,
        native: false,
        ..native
    };
    let list = looks_like(&panel, &[wide, too_big, native]);
    assert!(!list.choices.contains(&0) && !list.choices.contains(&1));
    assert!(list.choices.iter().all(|&i| i == 2));
}

#[test]
fn by_height_sorts_the_choices_not_the_mode_list() {
    let panel = monitor_4k();
    let modes: Vec<Mode> = candidates((3840, 2160), REFRESH)
        .iter()
        .map(|c| c.mode)
        .collect();
    let list = looks_like(&panel, &modes);
    let sorted = list.by_height(&modes);
    assert_eq!(sorted.len(), list.choices.len());
    assert!(sorted
        .windows(2)
        .all(|w| modes[w[0]].height <= modes[w[1]].height));
}

#[test]
fn a_laptop_panel_offers_its_ladder_of_sizes() {
    let cat = catalogue(&laptop(), REFRESH);
    let default = cat.default_choice().expect("a default");
    assert_eq!(logical(&default), (1440, 900));
    assert_eq!(default.scale, 2.0);
    assert_eq!(default.backing(), BUILT_IN_BACKING);
    assert!(cat.choices.len() >= 4);
    assert!(cat
        .choices
        .windows(2)
        .all(|w| w[0].mode.width < w[1].mode.width));
    // Every size is drawn from a buffer of twice its points, except one the panel shows pixel for pixel.
    for c in &cat.choices {
        let expected = if c.mode.native { 1 } else { BUILT_IN_BACKING };
        assert_eq!(c.backing(), expected);
        assert_eq!(c.raster().width, c.mode.width * expected);
    }
}

#[test]
fn a_low_density_laptop_panel_starts_at_native_size() {
    let panel = Panel::from_physical((1366, 768), (290, 163), false)
        .unwrap()
        .built_in(true);
    let start = default_choice(&panel, REFRESH).expect("a default");
    assert_eq!(logical(&start), (1366, 768));
    assert!(start.mode.native);
}

#[test]
fn connector_names_decide_which_displays_are_built_in() {
    for name in ["eDP-1", "LVDS-1", "DSI-1", "eDP", "DSI"] {
        assert!(is_built_in(name), "{name}");
    }
    for name in [
        "HDMI-A-1",
        "DP-1",
        "eDPX-1",
        "edp-1",
        "",
        "Virtual-1",
        "XeDP-1",
    ] {
        assert!(!is_built_in(name), "{name}");
    }
}
