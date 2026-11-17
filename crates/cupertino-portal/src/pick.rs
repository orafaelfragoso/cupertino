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
