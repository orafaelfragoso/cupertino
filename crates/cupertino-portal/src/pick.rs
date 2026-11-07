//! Picking a colour for `PickColor`: the displays as they were copied when the call came, frozen under an overlay; a
//! loupe beside the pointer that magnifies the pixels around it; a click that takes the colour of the pixel under the
//! pointer; Escape that cancels. It is the model and the painting of the overlay, without a display, so it is tested
//! as it is; the surfaces that show it are [`crate::pick_overlay`].
//!
//! The pick is the consent: the app learns one colour, and only of a pixel the person clicked. A click counts only when
//! its press and its release came on overlays that had each been on screen [`ARMING`](crate::consent::ARMING), counted
//! from the overlay's first frame the server showed: a button held from before, or a click meant for what was under the
//! pointer as the overlay appeared, picks nothing.
//!
//! Every display has an overlay for as long as the pick lasts. A display the server has that was not copied, such as
//! one plugged in between the copy and the overlays, ends the pick before it starts ([`cover`]), and any display
//! added, gone or moved while it runs ends it ([`layout_changed`]): a click on a display without an overlay would
//! reach the application under it.

use std::time::Instant;

use tiny_skia::{
    Color, FilterQuality, Paint, PathBuilder, Pixmap, PixmapPaint, Rect, Stroke, Transform,
};

use crate::consent::Arming;
use crate::geometry::{pixel_under, Placement};
use crate::screenshot::Output;

/// Pixels on each side of the loupe's square; odd, so one sits in the middle.
pub const LOUPE_PIXELS: u32 = 11;
/// Points each magnified pixel takes in the loupe.
pub const LOUPE_CELL: f32 = 10.0;
/// Points between the pointer and the nearest corner of the loupe.
pub const LOUPE_GAP: f32 = 18.0;
/// Width in points of the loupe's frame and of the outline of the middle pixel.
pub const LOUPE_FRAME: f32 = 2.0;
/// Arm length in points of the crosshair drawn where the hidden pointer is.
pub const CROSS_ARM: f32 = 7.0;
/// Points left open at the crosshair's centre, so the pixel picked stays visible.
pub const CROSS_GAP: f32 = 2.0;
/// Width in points of the crosshair's light stroke; its dark edge is one point wider on each side.
pub const CROSS_WIDTH: f32 = 1.0;
/// The light and dark tones the loupe and crosshair are drawn in twice, so they show on any colour.
const LIGHT: [u8; 4] = [255, 255, 255, 255];
const DARK: [u8; 4] = [0, 0, 0, 200];

/// How a pick ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Picked {
    /// The colour of the pixel clicked: red, green and blue in sRGB, each in `[0, 1]`.
    Colour([f64; 3]),
    /// Escape.
    Cancelled,
}

/// One pick across every display.
pub struct Picking {
    displays: Vec<Output>,
    /// Where the pointer is, in the compositor's space; `None` before it is over the overlay.
    pointer: Option<(f64, f64)>,
    /// A press came on the overlay while it was armed, and its release picks.
    pressed: bool,
    /// The arming of each display's overlay, from its first frame on screen; `None` before it.
    shown: Vec<Option<Arming>>,
}

impl Picking {
    /// A pick over `displays` (as [`crate::screenshot::Shoot`] copied them), none of whose overlays is on screen yet.
    pub fn new(displays: Vec<Output>) -> Self {
        Self {
            shown: vec![None; displays.len()],
            displays,
            pointer: None,
            pressed: false,
        }
    }

    /// The overlay of display `index` was first on screen at `at`: its arming delay starts. A later frame changes
    /// nothing.
    pub fn shown(&mut self, index: usize, at: Instant) {
        if let Some(shown @ None) = self.shown.get_mut(index) {
            *shown = Some(Arming::new(at));
        }
    }

    pub fn displays(&self) -> &[Output] {
        &self.displays
    }

    pub fn pointer(&self) -> Option<(f64, f64)> {
        self.pointer
    }

    /// The pointer moved to `point`, in the compositor's space.
    pub fn moved(&mut self, point: (f64, f64)) {
        self.pointer = Some(point);
    }

    /// The pointer left the overlay; a press it holds picks nothing on its release.
    pub fn left(&mut self) {
        self.pointer = None;
        self.pressed = false;
    }

    /// The primary button changed state at `point` at `now`. A press counts once the overlay under it is armed; its
    /// release, on an armed overlay too, picks the pixel under the pointer, if there is one.
    pub fn button(&mut self, pressed: bool, point: (f64, f64), now: Instant) -> Option<Picked> {
        self.pointer = Some(point);
        if pressed {
            self.pressed = self.armed(point, now);
            return None;
        }
        if !std::mem::take(&mut self.pressed) || !self.armed(point, now) {
            return None;
        }
        self.colour_at(point).map(Picked::Colour)
    }

    /// Escape was pressed.
    pub fn escape(&self) -> Picked {
        Picked::Cancelled
    }

    /// The display under `point`, by index, and the pixel of its image there.
    fn under(&self, point: (f64, f64)) -> Option<(usize, (u32, u32))> {
        self.displays.iter().enumerate().find_map(|(i, d)| {
            let image = (d.image.width(), d.image.height());
            pixel_under(placement(d), image, point).map(|pixel| (i, pixel))
        })
    }

    /// The colour of the pixel under `point`: sRGB, each channel in `[0, 1]`.
    pub fn colour_at(&self, point: (f64, f64)) -> Option<[f64; 3]> {
        let (display, (x, y)) = self.under(point)?;
        let pixel = self.displays[display].image.pixel(x, y)?.demultiply();
        Some([pixel.red(), pixel.green(), pixel.blue()].map(|c| f64::from(c) / 255.0))
    }

    /// The [`LOUPE_PIXELS`]² pixels around the one under `point`, row by row, as straight RGBA; `None` for one past
    /// its display's edge.
    pub fn loupe(&self, point: (f64, f64)) -> Option<Vec<Option<[u8; 4]>>> {
        let (display, (cx, cy)) = self.under(point)?;
        let display = &self.displays[display];
        let half = (LOUPE_PIXELS / 2) as i64;
        let mut cells = Vec::with_capacity((LOUPE_PIXELS * LOUPE_PIXELS) as usize);
        for dy in -half..=half {
            for dx in -half..=half {
                let (x, y) = (i64::from(cx) + dx, i64::from(cy) + dy);
                let pixel = u32::try_from(x)
                    .ok()
                    .zip(u32::try_from(y).ok())
                    .and_then(|(x, y)| display.image.pixel(x, y))
                    .map(|p| {
                        let p = p.demultiply();
                        [p.red(), p.green(), p.blue(), p.alpha()]
                    });
                cells.push(pixel);
            }
        }
        Some(cells)
    }

    /// Whether a click at `point` picks at `now`: the overlay of the display under it has been on screen
    /// [`ARMING`](crate::consent::ARMING).
    pub fn armed(&self, point: (f64, f64), now: Instant) -> bool {
        self.under(point)
            .and_then(|(display, _)| self.shown[display])
            .is_some_and(|arming| arming.armed(now))
    }

    /// Paint the overlay of display `index` into `pixmap` at `scale` device pixels per point: the display as it was
    /// copied, and where the pointer is, the crosshair and the loupe.
    pub fn paint(&self, index: usize, pixmap: &mut Pixmap, scale: f32) {
        let Some(display) = self.displays.get(index) else {
            return;
        };
        pixmap.fill(Color::BLACK);
        let sx = pixmap.width() as f32 / display.image.width().max(1) as f32;
        let sy = pixmap.height() as f32 / display.image.height().max(1) as f32;
        pixmap.draw_pixmap(
            0,
            0,
            display.image.as_ref(),
            &PixmapPaint {
                quality: FilterQuality::Nearest,
                ..PixmapPaint::default()
            },
            Transform::from_scale(sx, sy),
            None,
        );
        let Some(point) = self.pointer else {
            return;
        };
        let Placement { position, size } = placement(display);
        let local = (
            (point.0 - f64::from(position.0)) as f32,
            (point.1 - f64::from(position.1)) as f32,
        );
        if local.0 < 0.0 || local.1 < 0.0 || local.0 >= size.0 as f32 || local.1 >= size.1 as f32 {
            return;
        }
        let to_device = Transform::from_scale(scale, scale);
        cross(pixmap, local, to_device);
        if let Some(cells) = self.loupe(point) {
            let frame = loupe_frame(local, (size.0 as f32, size.1 as f32));
            draw_loupe(pixmap, frame, &cells, to_device);
        }
    }
}

/// Where a display's image sits in the compositor's space.
fn placement(display: &Output) -> Placement {
    Placement {
        position: display.position,
        size: display.size,
    }
}

/// Which of the copied `displays` the overlay on each of the server's displays now (`outputs`, where each sits) shows:
/// the one copied where it is. `Err` when one of them has no copy, as a display plugged in since the copy: it would
/// get no overlay, and a click there would reach the application under it, so the pick does not start.
pub fn cover(outputs: &[Placement], displays: &[Output]) -> Result<Vec<usize>, String> {
    outputs
        .iter()
        .map(|at| {
            displays
                .iter()
                .position(|d| placement(d) == *at)
                .ok_or_else(|| {
                    format!(
                        "the display at {:?} was not copied, so it would have no overlay",
                        at.position
                    )
                })
        })
        .collect()
}

/// Whether the server's displays changed since the overlays went up, so the pick ends: one added, gone or moved. Each
/// display is the id the server knows it by and where it sits, in any order.
pub fn layout_changed<Id: PartialEq>(shown: &[(Id, Placement)], now: &[(Id, Placement)]) -> bool {
    shown.len() != now.len() || now.iter().any(|display| !shown.contains(display))
}

/// The side of the loupe in points.
pub fn loupe_side() -> f32 {
    LOUPE_PIXELS as f32 * LOUPE_CELL
}

/// The loupe's square in points on a display `size` points large, with the pointer at `pointer`: below and right of
/// it, moved to the other side of the pointer on an axis it would leave the display by.
pub fn loupe_frame(pointer: (f32, f32), size: (f32, f32)) -> Rect {
    let side = loupe_side();
    let along = |at: f32, room: f32| {
        if at + LOUPE_GAP + side <= room {
            at + LOUPE_GAP
        } else {
            (at - LOUPE_GAP - side).max(0.0)
        }
    };
    let (x, y) = (along(pointer.0, size.0), along(pointer.1, size.1));
    Rect::from_xywh(x, y, side, side)
        .unwrap_or_else(|| Rect::from_xywh(0.0, 0.0, 1.0, 1.0).unwrap())
}

fn paint_of(rgba: [u8; 4]) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]);
    paint.anti_alias = false;
    paint
}

/// A crosshair at `at` (points), a light stroke over a dark one.
fn cross(pixmap: &mut Pixmap, at: (f32, f32), to_device: Transform) {
    let mut path = PathBuilder::new();
    let (x, y) = (at.0.floor() + 0.5, at.1.floor() + 0.5);
    for (dx, dy) in [(1.0, 0.0), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)] {
        path.move_to(x + dx * CROSS_GAP, y + dy * CROSS_GAP);
        path.line_to(
            x + dx * (CROSS_GAP + CROSS_ARM),
            y + dy * (CROSS_GAP + CROSS_ARM),
        );
    }
    let Some(path) = path.finish() else {
        return;
    };
    for (rgba, width) in [(DARK, CROSS_WIDTH + 2.0), (LIGHT, CROSS_WIDTH)] {
        let stroke = Stroke {
            width,
            ..Stroke::default()
        };
        pixmap.stroke_path(&path, &paint_of(rgba), &stroke, to_device, None);
    }
}

/// The loupe in `frame` (points): every cell filled with its pixel (a cell past the display's edge left dark), a
/// frame round it and the middle cell outlined.
fn draw_loupe(pixmap: &mut Pixmap, frame: Rect, cells: &[Option<[u8; 4]>], to_device: Transform) {
    let n = LOUPE_PIXELS as usize;
    for (i, cell) in cells.iter().enumerate() {
        let (col, row) = ((i % n) as f32, (i / n) as f32);
        let Some(rect) = Rect::from_xywh(
            frame.x() + col * LOUPE_CELL,
            frame.y() + row * LOUPE_CELL,
            LOUPE_CELL,
            LOUPE_CELL,
        ) else {
            continue;
        };
        let rgba = cell.map(|c| [c[0], c[1], c[2], 255]).unwrap_or(DARK);
        pixmap.fill_rect(rect, &paint_of(rgba), to_device, None);
    }
    let middle = (LOUPE_PIXELS / 2) as f32 * LOUPE_CELL;
    let centre = Rect::from_xywh(
        frame.x() + middle,
        frame.y() + middle,
        LOUPE_CELL,
        LOUPE_CELL,
    );
    for rect in [Some(frame), centre].into_iter().flatten() {
        let path = PathBuilder::from_rect(rect);
        for (rgba, width) in [(DARK, LOUPE_FRAME), (LIGHT, LOUPE_FRAME / 2.0)] {
            let stroke = Stroke {
                width,
                ..Stroke::default()
            };
            pixmap.stroke_path(&path, &paint_of(rgba), &stroke, to_device, None);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::consent::ARMING;

    fn display(position: (i32, i32), size: (i32, i32), scale: u32, rgba: [u8; 4]) -> Output {
        let mut image = Pixmap::new(size.0 as u32 * scale, size.1 as u32 * scale).unwrap();
        image.fill(Color::from_rgba8(rgba[0], rgba[1], rgba[2], rgba[3]));
        Output {
            position,
            size,
            image,
        }
    }

    fn two(opened: Instant) -> Picking {
        let mut picking = Picking::new(displays());
        picking.shown(0, opened);
        picking.shown(1, opened);
        picking
    }

    /// A display at scale 1 and one at scale 2 to its right.
    fn displays() -> Vec<Output> {
        let mut right = display((400, 0), (200, 150), 2, [0, 0, 255, 255]);
        // one pixel of its own, at logical (410.25, 5.25) on the scale-2 display
        right.image.pixels_mut()[10 * 400 + 20] =
            tiny_skia::ColorU8::from_rgba(255, 128, 0, 255).premultiply();
        vec![display((0, 0), (400, 300), 1, [255, 0, 0, 255]), right]
    }

    #[test]
    fn a_click_once_armed_picks_the_colour_of_the_pixel_under_the_pointer() {
        let opened = Instant::now();
        let mut picking = two(opened);
        let armed = opened + ARMING;
        assert_eq!(picking.button(true, (410.2, 5.3), armed), None);
        assert_eq!(
            picking.button(false, (410.2, 5.3), armed),
            Some(Picked::Colour([1.0, 128.0 / 255.0, 0.0]))
        );
        assert_eq!(picking.button(true, (10.0, 10.0), armed), None);
        assert_eq!(
            picking.button(false, (12.0, 10.0), armed),
            Some(Picked::Colour([1.0, 0.0, 0.0])),
            "the release picks where the pointer is then"
        );
    }

    #[test]
    fn a_press_before_arming_or_from_off_the_overlay_picks_nothing() {
        let opened = Instant::now();
        let mut picking = two(opened);
        assert_eq!(picking.button(true, (10.0, 10.0), opened), None);
        assert_eq!(
            picking.button(false, (10.0, 10.0), opened + ARMING * 2),
            None,
            "pressed before arming"
        );
        assert_eq!(
            picking.button(false, (10.0, 10.0), opened + ARMING * 2),
            None,
            "a release without its press"
        );
        assert_eq!(picking.button(true, (10.0, 10.0), opened + ARMING), None);
        picking.left();
        assert_eq!(
            picking.button(false, (10.0, 10.0), opened + ARMING),
            None,
            "the pointer left between press and release"
        );
        assert_eq!(picking.button(true, (500.0, 200.0), opened + ARMING), None);
        assert_eq!(
            picking.button(false, (500.0, 200.0), opened + ARMING),
            None,
            "a point on no display"
        );
        assert!(!picking.armed((10.0, 10.0), opened + ARMING - Duration::from_millis(1)));
        assert_eq!(picking.escape(), Picked::Cancelled);
    }

    /// Each overlay arms from its own first frame on screen, however late that comes after the pick began.
    #[test]
    fn an_overlay_arms_only_once_it_has_been_on_screen_for_the_delay() {
        let began = Instant::now();
        let mut picking = Picking::new(displays());
        let late = began + ARMING * 3;
        assert!(
            !picking.armed((10.0, 10.0), late),
            "no overlay is on screen yet, however long the pick has waited"
        );
        assert_eq!(picking.button(true, (10.0, 10.0), late), None);
        assert_eq!(picking.button(false, (10.0, 10.0), late), None);

        picking.shown(0, late);
        picking.shown(0, began);
        assert!(!picking.armed((10.0, 10.0), late), "the first frame counts");
        assert!(picking.armed((10.0, 10.0), late + ARMING));
        assert!(
            !picking.armed((410.0, 5.0), late + ARMING * 2),
            "the other display's overlay has not been shown"
        );
        assert_eq!(picking.button(true, (10.0, 10.0), late + ARMING), None);
        assert_eq!(
            picking.button(false, (410.0, 5.0), late + ARMING),
            None,
            "released over an overlay not yet armed"
        );
        picking.shown(1, late + ARMING);
        assert_eq!(picking.button(true, (10.0, 10.0), late + ARMING * 2), None);
        assert_eq!(
            picking.button(false, (12.0, 10.0), late + ARMING * 2),
            Some(Picked::Colour([1.0, 0.0, 0.0]))
        );
    }

    #[test]
    fn every_display_now_needs_a_copy_and_any_change_of_the_displays_ends_the_pick() {
        let copied = displays();
        let left = Placement {
            position: (0, 0),
            size: (400, 300),
        };
        let right = Placement {
            position: (400, 0),
            size: (200, 150),
        };
        assert_eq!(cover(&[right, left], &copied), Ok(vec![1, 0]));
        assert_eq!(cover(&[left], &copied), Ok(vec![0]), "one unplugged");
        let plugged = Placement {
            position: (600, 0),
            size: (300, 200),
        };
        assert!(
            cover(&[left, right, plugged], &copied).is_err(),
            "a display plugged in since the copy would have no overlay"
        );

        let shown = [(1, left), (2, right)];
        assert!(!layout_changed(&shown, &[(2, right), (1, left)]));
        assert!(
            layout_changed(&shown, &[(1, left), (2, right), (3, plugged)]),
            "a display plugged in"
        );
        assert!(layout_changed(&shown, &[(1, left)]), "a display unplugged");
        assert!(
            layout_changed(&shown, &[(1, left), (3, right)]),
            "another display where one was: it has no overlay"
        );
        let moved = Placement {
            position: (-200, 0),
            ..right
        };
        assert!(layout_changed(&shown, &[(1, left), (2, moved)]));
    }

    #[test]
    fn the_loupe_holds_the_pixels_round_the_one_picked_and_none_past_the_edge() {
        let picking = two(Instant::now());
        let cells = picking.loupe((410.2, 5.3)).unwrap();
        assert_eq!(cells.len(), (LOUPE_PIXELS * LOUPE_PIXELS) as usize);
        let middle = (LOUPE_PIXELS * LOUPE_PIXELS / 2) as usize;
        assert_eq!(cells[middle], Some([255, 128, 0, 255]));
        assert_eq!(cells[middle + 1], Some([0, 0, 255, 255]));
        let corner = picking.loupe((0.5, 0.5)).unwrap();
        assert_eq!(corner[0], None, "above and left of the first pixel");
        assert_eq!(corner[middle], Some([255, 0, 0, 255]));
        assert_eq!(picking.loupe((500.0, 200.0)), None);
    }

    #[test]
    fn the_loupe_sits_below_right_of_the_pointer_and_turns_back_at_the_edges() {
        let side = loupe_side();
        let size = (400.0, 300.0);
        let frame = loupe_frame((10.0, 20.0), size);
        assert_eq!((frame.x(), frame.y()), (10.0 + LOUPE_GAP, 20.0 + LOUPE_GAP));
        let frame = loupe_frame((390.0, 290.0), size);
        assert_eq!(
            (frame.x(), frame.y()),
            (390.0 - LOUPE_GAP - side, 290.0 - LOUPE_GAP - side)
        );
        assert!(frame.right() <= size.0 && frame.bottom() <= size.1);
    }

    #[test]
    fn the_overlay_shows_the_frozen_display_and_the_loupe_magnifies_where_the_pointer_is() {
        let opened = Instant::now();
        let mut picking = two(opened);
        // the scale-2 display, drawn at scale 2
        let mut pixmap = Pixmap::new(400, 300).unwrap();
        picking.paint(1, &mut pixmap, 2.0);
        let at = |pm: &Pixmap, x: f32, y: f32| {
            let p = pm.pixel(x as u32, y as u32).unwrap().demultiply();
            [p.red(), p.green(), p.blue()]
        };
        assert_eq!(
            at(&pixmap, 300.0, 200.0),
            [0, 0, 255],
            "the display as copied"
        );
        assert_eq!(
            at(&pixmap, 20.0, 10.0),
            [255, 128, 0],
            "its own pixel where it was"
        );

        picking.moved((410.2, 5.3));
        picking.paint(1, &mut pixmap, 2.0);
        let frame = loupe_frame((10.2, 5.3), (200.0, 150.0));
        let cell = |col: f32, row: f32| {
            (
                (frame.x() + (col + 0.5) * LOUPE_CELL) * 2.0,
                (frame.y() + (row + 0.5) * LOUPE_CELL) * 2.0,
            )
        };
        let half = (LOUPE_PIXELS / 2) as f32;
        let (x, y) = cell(half, half);
        assert_eq!(
            at(&pixmap, x, y),
            [255, 128, 0],
            "the middle cell, magnified"
        );
        let (x, y) = cell(half + 2.0, half);
        assert_eq!(at(&pixmap, x, y), [0, 0, 255], "a neighbour");
        assert_eq!(at(&pixmap, 300.0, 200.0), [0, 0, 255], "the rest as copied");

        // on the other display the pointer draws nothing
        let mut left = Pixmap::new(400, 300).unwrap();
        picking.paint(0, &mut left, 1.0);
        assert_eq!(at(&left, 30.0, 30.0), [255, 0, 0]);
        assert_eq!(at(&left, 399.0, 10.0), [255, 0, 0]);
    }
}
