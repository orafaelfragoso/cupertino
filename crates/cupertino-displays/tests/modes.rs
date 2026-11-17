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
    assert!((monitor_4k().vertical_dpi - 163.3).abs() < 0.1);
}

#[test]
fn candidates_cover_every_eighth_scale_from_native_up_in_whole_pixel_buffers() {
    let list = candidates((3840, 2160), REFRESH);
    assert_eq!(list.len(), SCALE_EIGHTHS.count());
    assert!(list.windows(2).all(|w| w[0].scale < w[1].scale));
    assert_eq!(list.iter().filter(|c| c.mode.native).count(), 1);
    assert!(list[0].mode.native);
    assert!(list.iter().all(|c| c.mode.refresh_mhz == REFRESH));
    for (scale, logical, backing, raster) in [
        (1.0, Extent::new(3840, 2160), 1, Extent::new(3840, 2160)),
        (1.5, Extent::new(2560, 1440), 2, Extent::new(5120, 2880)),
        (2.0, Extent::new(1920, 1080), 2, Extent::new(3840, 2160)),
        (3.0, Extent::new(1280, 720), 3, Extent::new(3840, 2160)),
    ] {
        let c = list.iter().find(|c| c.scale == scale).unwrap();
        assert_eq!(
            (c.logical(), c.backing(), c.raster()),
            (logical, backing, raster),
            "{scale}"
        );
    }
}

#[test]
fn a_new_display_starts_at_the_size_its_density_suits() {
    let low_density_laptop = Panel::from_physical((1366, 768), (290, 163), false)
        .unwrap()
        .built_in(true);
    for (name, panel, size, scale) in [
        ("a 4K monitor", monitor_4k(), (2560, 1440), 1.5),
        ("a laptop", laptop(), (1440, 900), 2.0),
        ("a low density laptop", low_density_laptop, (1366, 768), 1.0),
    ] {
        let start = default_choice(&panel, REFRESH).expect(name);
        assert_eq!((logical(&start), start.scale), (size, scale), "{name}");
        let cat = catalogue(&panel, REFRESH);
        assert_eq!(cat.default_choice(), Some(start), "{name}");
        assert_eq!(cat.position(start.logical()), cat.default, "{name}");
    }
}

#[test]
fn the_catalogue_keeps_the_panel_shape_above_the_floor_ordered_by_height() {
    let portrait = Panel::from_physical((2160, 3840), (336, 597), true).unwrap();
    for (name, panel) in [("landscape", monitor_4k()), ("portrait", portrait)] {
        let cat = catalogue(&panel, REFRESH);
        assert!(
            !cat.choices.is_empty() && cat.choices.len() <= MAX_CHOICES,
            "{name}"
        );
        assert_eq!(scaled_choices(&panel, REFRESH), cat.choices, "{name}");
        assert!(
            cat.choices
                .windows(2)
                .all(|w| w[0].mode.height < w[1].mode.height),
            "{name}"
        );
        let shape = panel.pixels_wide as f32 / panel.pixels_high as f32;
        // A rotated panel's floor is lowered by its aspect ratio.
        let floor = MIN_HEIGHT_POINTS * shape.min(1.0);
        for c in &cat.choices {
            let ratio = c.mode.width as f32 / c.mode.height as f32;
            assert!((ratio - shape).abs() < 0.02, "{name} {:?}", logical(c));
            assert!(c.mode.height as f32 >= floor, "{name} {:?}", logical(c));
            assert!(c.mode.width <= panel.pixels_wide && c.mode.height <= panel.pixels_high);
        }
    }
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
fn looks_like_picks_modes_of_the_panel_shape_and_sorts_them_by_height() {
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
    assert!(list.choices.iter().all(|&i| i == 2));

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
fn a_laptop_panel_offers_a_ladder_of_sizes_drawn_at_twice_their_points() {
    let cat = catalogue(&laptop(), REFRESH);
    assert!(cat.choices.len() >= 4);
    assert!(cat
        .choices
        .windows(2)
        .all(|w| w[0].mode.width < w[1].mode.width));
    // Except a size the panel shows pixel for pixel.
    for c in &cat.choices {
        let expected = if c.mode.native { 1 } else { BUILT_IN_BACKING };
        assert_eq!(c.backing(), expected);
        assert_eq!(c.raster().width, c.mode.width * expected);
    }
}

#[test]
fn connector_names_decide_which_displays_are_built_in() {
    for (name, built_in) in [
        ("eDP-1", true),
        ("LVDS-1", true),
        ("DSI-1", true),
        ("eDP", true),
        ("DSI", true),
        ("HDMI-A-1", false),
        ("DP-1", false),
        ("eDPX-1", false),
        ("edp-1", false),
        ("", false),
        ("Virtual-1", false),
        ("XeDP-1", false),
    ] {
        assert_eq!(is_built_in(name), built_in, "{name}");
    }
}
