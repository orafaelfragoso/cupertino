//! The colour picker's model (`cupertino_screenshot::color_picker`): which pixel a point is over at any scale, the
//! colour a click picks once the overlay is armed and not before, the loupe around the pointer and where it goes, and
//! when the outputs changing ends a pick.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_kit::core::next_resource_id;
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_screenshot::color_picker::{
    layout_changed, loupe_frame, loupe_side, overlay_index, overlay_key, pixel_under, Display,
    Picked, Picking, ARMING, LOUPE_GAP, LOUPE_PIXELS,
};

/// A picture `width` x `height` whose pixel at (x, y) is straight `colour(x, y)`, stored premultiplied.
fn picture(width: u32, height: u32, colour: impl Fn(u32, u32) -> [u8; 4]) -> ImageResource {
    let mut pixels = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let [r, g, b, a] = colour(x, y);
            let pre = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
            pixels.extend_from_slice(&[pre(r), pre(g), pre(b), a]);
        }
    }
    ImageResource {
        revision: 1,
        width,
        height,
        stride: width * 4,
        format: PixelFormat::Rgba8Premultiplied,
        pixels: Arc::from(pixels),
    }
}

/// A colour of its own for every pixel, opaque.
fn gradient(x: u32, y: u32) -> [u8; 4] {
    [x as u8, y as u8, 200, 255]
}

/// Two outputs side by side: 100 x 50 points at scale 1, and 100 x 50 points at scale 2 right of it.
fn two_outputs() -> Picking {
    Picking::new(vec![
        Display {
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
            image: picture(100, 50, gradient),
            id: next_resource_id(),
        },
        Display {
            rect: Rect::new(100.0, 0.0, 100.0, 50.0),
            image: picture(200, 100, gradient),
            id: next_resource_id(),
        },
    ])
}

/// A click: the press and the release at `point` at `now`.
fn click(picking: &mut Picking, point: Point, now: Instant) -> Option<Picked> {
    assert_eq!(
        picking.button(true, point, now),
        None,
        "a press picks nothing"
    );
    picking.button(false, point, now)
}

#[test]
fn a_point_is_over_the_pixel_whose_area_holds_it_at_any_scale() {
    let rect = Rect::new(100.0, 0.0, 100.0, 50.0);
    assert_eq!(
        pixel_under(rect, (100, 50), Point::new(100.0, 0.0)),
        Some((0, 0))
    );
    assert_eq!(
        pixel_under(rect, (200, 100), Point::new(110.25, 5.75)),
        Some((20, 11))
    );
    assert_eq!(
        pixel_under(rect, (200, 100), Point::new(199.99, 49.99)),
        Some((199, 99))
    );
    assert_eq!(
        pixel_under(rect, (200, 100), Point::new(200.0, 10.0)),
        None,
        "the right edge is the next output's"
    );
    assert_eq!(pixel_under(rect, (200, 100), Point::new(99.9, 10.0)), None);
    assert_eq!(
        pixel_under(rect, (0, 0), Point::new(150.0, 10.0)),
        None,
        "an empty picture has no pixel"
    );
}

#[test]
fn a_click_picks_the_colour_under_it_once_its_overlay_has_been_shown_arming() {
    let mut picking = two_outputs();
    let shown = Instant::now();
    picking.shown(0, shown);
    picking.shown(1, shown);
    let at = Point::new(30.5, 20.5);
    assert_eq!(
        click(&mut picking, at, shown),
        None,
        "not armed when it first shows"
    );
    assert!(!picking.armed(at, shown + ARMING - Duration::from_millis(1)));
    let armed = shown + ARMING;
    assert_eq!(
        click(&mut picking, at, armed),
        Some(Picked::Colour([30.0 / 255.0, 20.0 / 255.0, 200.0 / 255.0]))
    );
    // on the output at scale 2, the pixel of the point's area
    assert_eq!(
        click(&mut picking, Point::new(110.25, 5.75), armed),
        Some(Picked::Colour([20.0 / 255.0, 11.0 / 255.0, 200.0 / 255.0]))
    );
}

#[test]
fn each_overlay_arms_from_its_own_first_frame() {
    let mut picking = two_outputs();
    let first = Instant::now();
    picking.shown(0, first);
    let later = first + Duration::from_secs(1);
    picking.shown(1, later);
    // a later frame of the first changes nothing
    picking.shown(0, later);
    let now = first + ARMING;
    assert!(picking.armed(Point::new(10.0, 10.0), now));
    assert!(
        !picking.armed(Point::new(150.0, 10.0), now),
        "the second has not been shown ARMING"
    );
    assert!(picking.armed(Point::new(150.0, 10.0), later + ARMING));
}

#[test]
fn a_press_before_arming_or_one_that_left_the_overlay_picks_nothing() {
    let mut picking = two_outputs();
    let shown = Instant::now();
    picking.shown(0, shown);
    let at = Point::new(10.0, 10.0);
    assert_eq!(picking.button(true, at, shown), None);
    assert_eq!(
        picking.button(false, at, shown + ARMING),
        None,
        "the press came before arming"
    );

    let armed = shown + ARMING;
    assert_eq!(picking.button(true, at, armed), None);
    picking.left();
    assert_eq!(picking.pointer(), None);
    assert_eq!(
        picking.button(false, at, armed),
        None,
        "the pointer left with the button held"
    );
    assert!(
        click(&mut picking, at, armed).is_some(),
        "a whole click picks"
    );
}

#[test]
fn a_translucent_pixel_is_picked_straight() {
    let mut picking = Picking::new(vec![Display {
        rect: Rect::new(0.0, 0.0, 4.0, 4.0),
        image: picture(4, 4, |_, _| [200, 100, 50, 128]),
        id: next_resource_id(),
    }]);
    let shown = Instant::now();
    picking.shown(0, shown);
    let Some(Picked::Colour(colour)) = click(&mut picking, Point::new(1.0, 1.0), shown + ARMING)
    else {
        panic!("a colour");
    };
    let straight = [200.0, 100.0, 50.0].map(|c: f64| c / 255.0);
    assert!(
        colour
            .iter()
            .zip(straight)
            .all(|(c, s)| (c - s).abs() <= 1.0 / 255.0),
        "{colour:?} is {straight:?} within the premultiplied rounding"
    );
}

#[test]
fn the_loupe_shows_the_pixels_around_the_pointer_and_none_past_the_edge() {
    let picking = two_outputs();
    let half = LOUPE_PIXELS / 2;
    let cells = picking
        .loupe(Point::new(50.5, 25.5))
        .expect("over an output");
    assert_eq!(cells.len(), (LOUPE_PIXELS * LOUPE_PIXELS) as usize);
    let centre = (half * LOUPE_PIXELS + half) as usize;
    assert_eq!(cells[centre], Some(gradient(50, 25)));
    assert_eq!(cells[0], Some(gradient(50 - half, 25 - half)));

    let corner = picking.loupe(Point::new(0.5, 0.5)).expect("over an output");
    assert_eq!(corner[0], None, "past the output's edge");
    assert_eq!(corner[centre], Some(gradient(0, 0)));
    assert_eq!(
        picking.loupe(Point::new(300.0, 10.0)),
        None,
        "over no output"
    );
}

#[test]
fn the_overlay_of_an_output_shows_the_pointer_over_it_alone() {
    let mut picking = two_outputs();
    picking.moved(Point::new(150.0, 20.0));
    let first = picking.view(0, true).expect("the first output");
    assert_eq!(first.pointer, None);
    assert_eq!(first.loupe, None);
    assert!(first.keys);
    let second = picking.view(1, false).expect("the second output");
    assert_eq!(
        second.pointer,
        Some(Point::new(50.0, 20.0)),
        "in the output's own points"
    );
    assert_eq!(second.origin, Point::new(100.0, 0.0));
    assert!(second.loupe.is_some());
    assert_eq!(picking.view(2, false), None);
}

#[test]
fn the_loupe_sits_below_right_of_the_pointer_and_flips_at_an_edge() {
    let size = Size::new(800.0, 600.0);
    let side = loupe_side();
    let frame = loupe_frame(Point::new(100.0, 100.0), size);
    assert_eq!(
        frame,
        Rect::new(100.0 + LOUPE_GAP, 100.0 + LOUPE_GAP, side, side)
    );
    let flipped = loupe_frame(Point::new(790.0, 590.0), size);
    assert_eq!(
        flipped,
        Rect::new(
            790.0 - LOUPE_GAP - side,
            590.0 - LOUPE_GAP - side,
            side,
            side
        )
    );
}

#[test]
fn an_output_added_gone_or_moved_ends_the_pick() {
    let a = (1, Rect::new(0.0, 0.0, 100.0, 50.0));
    let b = (2, Rect::new(100.0, 0.0, 100.0, 50.0));
    assert!(
        !layout_changed(&[a, b], &[b, a]),
        "the same outputs in another order"
    );
    assert!(layout_changed(&[a, b], &[a]));
    assert!(layout_changed(&[a], &[a, b]));
    assert!(layout_changed(
        &[a, b],
        &[a, (2, Rect::new(0.0, 50.0, 100.0, 50.0))]
    ));
}

#[test]
fn an_overlays_key_names_its_output() {
    for index in [0, 1, 7] {
        assert_eq!(overlay_index(&overlay_key(index)), Some(index));
    }
}
