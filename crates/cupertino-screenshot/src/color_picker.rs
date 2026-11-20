//! Picking a colour for `org.cupertino.Screenshot1.PickColor`: the outputs as they were copied when the call came,
//! frozen under an overlay; a loupe beside the pointer that magnifies the pixels around it; a click that takes the
//! colour of the pixel under the pointer; Escape that cancels. [`Picking`] is the model, without a display; [`overlay`]
//! is the window that shows it on one output.
//!
//! The pick is the consent: the app learns one colour, and only of a pixel the person clicked. A click counts only when
//! its press and its release came on overlays that had each been on screen [`ARMING`], counted from the overlay's first
//! frame the server showed: a button held from before, or a click meant for what was under the pointer as the overlay
//! appeared, picks nothing.
//!
//! Every output has an overlay for as long as the pick lasts: a pick starts only when the outputs are those it copied,
//! and any output added, gone or moved while it runs ends it ([`layout_changed`]), since a click on an output without an
//! overlay would reach the application under it.

use std::time::{Duration, Instant};

use cupertino_geometry::{Point, Rect, Size};
use cupertino_kit::core::{
    CommandRegistry, Element, Event, EventCx, Key as NodeKey, MeasureCx, PaintCx, Propagation,
    Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{DecorationMode, MouseButton, PointerEvent, WindowSpec};
use cupertino_kit::render::{
    ImageResource, Paint, PaintOp, PixelFormat, ResourceId, Sampling, Shape,
};
use cupertino_kit::theme::Color;

/// How long an overlay is on screen before a click on it picks: a click meant for what was under the pointer as the
/// overlay appeared lands in this time and picks nothing.
pub const ARMING: Duration = Duration::from_millis(500);
/// Pixels on each side of the loupe's square; odd, so one sits in the middle.
pub const LOUPE_PIXELS: u32 = 11;
/// Points each magnified pixel takes in the loupe.
pub const LOUPE_CELL: f64 = 10.0;
/// Points between the pointer and the nearest corner of the loupe.
pub const LOUPE_GAP: f64 = 18.0;
/// Width in points of the loupe's frame and of the outline of the middle pixel.
pub const LOUPE_FRAME: f64 = 2.0;
/// Arm length in points of the crosshair drawn where the hidden pointer is.
pub const CROSS_ARM: f64 = 7.0;
/// Points left open at the crosshair's centre, so the pixel picked stays visible.
pub const CROSS_GAP: f64 = 2.0;
/// Width in points of the crosshair's light stroke; its dark edge is one point wider on each side.
pub const CROSS_WIDTH: f64 = 1.0;
/// The light and dark tones the loupe and crosshair are drawn in twice, so they show on any colour.
const LIGHT: [u8; 4] = [255, 255, 255, 255];
const DARK: [u8; 4] = [0, 0, 0, 200];
/// The title of an overlay, which names it to assistive technology.
pub const TITLE: &str = "Colour Picker";

/// How a pick ended.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Picked {
    /// The colour of the pixel clicked: red, green and blue in sRGB, each in `[0, 1]`.
    Colour([f64; 3]),
    /// Escape.
    Cancelled,
}

/// An output as it was copied: where it sits in the layout, in points, and its upright picture.
#[derive(Clone, Debug, PartialEq)]
pub struct Display {
    pub rect: Rect,
    pub image: ImageResource,
    /// Names the picture to the renderer for as long as the pick lasts.
    pub id: ResourceId,
}

/// One pick across every output.
pub struct Picking {
    displays: Vec<Display>,
    /// Where the pointer is, in the layout's points; `None` before it is over the overlay.
    pointer: Option<Point>,
    /// A press came on the overlay while it was armed, and its release picks.
    pressed: bool,
    /// When each output's overlay was first on screen; `None` before it.
    shown: Vec<Option<Instant>>,
}

impl Picking {
    /// A pick over `displays`, none of whose overlays is on screen yet.
    pub fn new(displays: Vec<Display>) -> Self {
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
            *shown = Some(at);
        }
    }

    pub fn displays(&self) -> &[Display] {
        &self.displays
    }

    pub fn pointer(&self) -> Option<Point> {
        self.pointer
    }

    /// The pointer moved to `point`, in the layout's points.
    pub fn moved(&mut self, point: Point) {
        self.pointer = Some(point);
    }

    /// The pointer left the overlay; a press it holds picks nothing on its release.
    pub fn left(&mut self) {
        self.pointer = None;
        self.pressed = false;
    }

    /// The primary button changed state at `point` at `now`. A press counts once the overlay under it is armed; its
    /// release, on an armed overlay too, picks the pixel under the pointer, if there is one.
    pub fn button(&mut self, pressed: bool, point: Point, now: Instant) -> Option<Picked> {
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

    /// The display under `point`, by index, and the pixel of its picture there.
    fn under(&self, point: Point) -> Option<(usize, (u32, u32))> {
        self.displays.iter().enumerate().find_map(|(i, d)| {
            pixel_under(d.rect, (d.image.width, d.image.height), point).map(|pixel| (i, pixel))
        })
    }

    /// The colour of the pixel under `point`: sRGB, each channel in `[0, 1]`.
    pub fn colour_at(&self, point: Point) -> Option<[f64; 3]> {
        let (display, (x, y)) = self.under(point)?;
        let [r, g, b, _] = pixel(&self.displays[display].image, x, y)?;
        Some([r, g, b].map(|c| f64::from(c) / 255.0))
    }

    /// The [`LOUPE_PIXELS`]² pixels around the one under `point`, row by row, as straight RGBA; `None` for one past
    /// its display's edge.
    pub fn loupe(&self, point: Point) -> Option<Vec<Option<[u8; 4]>>> {
        let (display, (cx, cy)) = self.under(point)?;
        let image = &self.displays[display].image;
        let half = i64::from(LOUPE_PIXELS / 2);
        let mut cells = Vec::with_capacity((LOUPE_PIXELS * LOUPE_PIXELS) as usize);
        for dy in -half..=half {
            for dx in -half..=half {
                let (x, y) = (i64::from(cx) + dx, i64::from(cy) + dy);
                cells.push(
                    u32::try_from(x)
                        .ok()
                        .zip(u32::try_from(y).ok())
                        .and_then(|(x, y)| pixel(image, x, y)),
                );
            }
        }
        Some(cells)
    }

    /// Whether a click at `point` picks at `now`: the overlay of the display under it has been on screen [`ARMING`].
    pub fn armed(&self, point: Point, now: Instant) -> bool {
        self.under(point)
            .and_then(|(display, _)| self.shown[display])
            .is_some_and(|shown| now.saturating_duration_since(shown) >= ARMING)
    }

    /// What the overlay of display `index` shows; `keys` when it takes the keyboard for the pick.
    pub fn view(&self, index: usize, keys: bool) -> Option<OverlayView> {
        let display = self.displays.get(index)?;
        let pointer = self
            .pointer
            .filter(|p| display.rect.contains(*p))
            .map(|p| Point::new(p.x - display.rect.x(), p.y - display.rect.y()));
        Some(OverlayView {
            image: display.image.clone(),
            id: display.id,
            origin: Point::new(display.rect.x(), display.rect.y()),
            pointer,
            loupe: self
                .pointer
                .filter(|_| pointer.is_some())
                .and_then(|p| self.loupe(p)),
            keys,
        })
    }
}

/// The pixel at `(x, y)` of `image` as straight RGBA; `None` past its edge.
fn pixel(image: &ImageResource, x: u32, y: u32) -> Option<[u8; 4]> {
    if x >= image.width || y >= image.height {
        return None;
    }
    let at = (y * image.stride + x * 4) as usize;
    let p: [u8; 4] = image.pixels.get(at..at + 4)?.try_into().ok()?;
    Some(match image.format {
        PixelFormat::Rgba8Premultiplied if p[3] != 0 && p[3] != 255 => {
            let a = u32::from(p[3]);
            let straight = |c: u8| ((u32::from(c) * 255 + a / 2) / a).min(255) as u8;
            [straight(p[0]), straight(p[1]), straight(p[2]), p[3]]
        }
        _ => p,
    })
}

/// The pixel of a display's upright picture of `image` pixels under `point` of the layout, for a display at `rect`;
/// `None` when the point is outside the display. A display at a scale above 1 has several pixels for each point: the
/// one whose area holds the point.
pub fn pixel_under(rect: Rect, image: (u32, u32), point: Point) -> Option<(u32, u32)> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 || image.0 == 0 || image.1 == 0 {
        return None;
    }
    let (u, v) = (
        (point.x - rect.x()) / rect.width(),
        (point.y - rect.y()) / rect.height(),
    );
    if !(0.0..1.0).contains(&u) || !(0.0..1.0).contains(&v) {
        return None;
    }
    let px = |t: f64, n: u32| ((t * f64::from(n)).floor() as u32).min(n - 1);
    Some((px(u, image.0), px(v, image.1)))
}

/// Whether the server's outputs changed since the overlays went up, so the pick ends: one added, gone or moved. Each
/// output is the id the server knows it by and where it sits, in any order.
pub fn layout_changed<Id: PartialEq>(shown: &[(Id, Rect)], now: &[(Id, Rect)]) -> bool {
    shown.len() != now.len() || now.iter().any(|output| !shown.contains(output))
}

/// The side of the loupe in points.
pub fn loupe_side() -> f64 {
    f64::from(LOUPE_PIXELS) * LOUPE_CELL
}

/// The loupe's square in points on a display `size` points large, with the pointer at `pointer`: below and right of
/// it, moved to the other side of the pointer on an axis it would leave the display by.
pub fn loupe_frame(pointer: Point, size: Size) -> Rect {
    let side = loupe_side();
    let along = |at: f64, room: f64| {
        if at + LOUPE_GAP + side <= room {
            at + LOUPE_GAP
        } else {
            (at - LOUPE_GAP - side).max(0.0)
        }
    };
    Rect::new(
        along(pointer.x, size.width),
        along(pointer.y, size.height),
        side,
        side,
    )
}

/// What the overlay of one output shows.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayView {
    pub image: ImageResource,
    pub id: ResourceId,
    /// Where the output sits in the layout, in points.
    pub origin: Point,
    /// The pointer on this output, in the window's points.
    pub pointer: Option<Point>,
    /// The loupe's cells around the pointer ([`Picking::loupe`]).
    pub loupe: Option<Vec<Option<[u8; 4]>>>,
    /// The overlay takes the keyboard, for Escape.
    pub keys: bool,
}

/// What the person did on an overlay, in the layout's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PickInput {
    Moved(Point),
    Button { pressed: bool, at: Point },
    Left,
    Escape,
}

/// The overlay window of output `index`; it takes the size of its output.
pub fn overlay<M: Clone + 'static>(
    key: WindowKey,
    view: OverlayView,
    to: fn(PickInput) -> M,
) -> Window<M> {
    Window {
        key,
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: "cupertino-screenshot.color-picker".to_owned(),
            size: Size::ZERO,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content: Element::new(Overlay { view, to }),
        popup: None,
        menus: None,
    }
}

struct Overlay<M> {
    view: OverlayView,
    to: fn(PickInput) -> M,
}

/// The escape key's keysym.
const ESCAPE: u32 = 0xff1b;

fn color(rgba: [u8; 4]) -> Color {
    let c = |v: u8| f32::from(v) / 255.0;
    Color::new(c(rgba[0]), c(rgba[1]), c(rgba[2]), c(rgba[3]))
}

fn fill(cx: &mut PaintCx<'_>, rect: Rect, rgba: [u8; 4]) {
    cx.push(PaintOp::Fill {
        shape: Shape::Rect(rect),
        paint: Paint::Solid(color(rgba)),
    });
}

/// `rect` filled, its sides moved to whole device pixels at `scale` (a half pixel goes to the next), so it stays sharp
/// at any scale.
fn sharp(cx: &mut PaintCx<'_>, rect: Rect, scale: f64, rgba: [u8; 4]) {
    let snap = |v: f64| (v * scale + 0.5).floor() / scale;
    let (x0, y0) = (snap(rect.x()), snap(rect.y()));
    let (x1, y1) = (
        snap(rect.x() + rect.width()),
        snap(rect.y() + rect.height()),
    );
    if x1 > x0 && y1 > y0 {
        fill(cx, Rect::new(x0, y0, x1 - x0, y1 - y0), rgba);
    }
}

/// A crosshair at `at` (points), a light stroke over a dark one, around the pixel's centre.
fn cross(cx: &mut PaintCx<'_>, at: Point) {
    let scale = cx.env().scale.max(1.0);
    let (x, y) = (at.x.floor() + 0.5, at.y.floor() + 0.5);
    for (rgba, width) in [(DARK, CROSS_WIDTH + 2.0), (LIGHT, CROSS_WIDTH)] {
        let half = width / 2.0;
        for sign in [1.0, -1.0] {
            let (near, far) = (CROSS_GAP * sign, (CROSS_GAP + CROSS_ARM) * sign);
            let (from, to) = (near.min(far), near.max(far));
            sharp(
                cx,
                Rect::new(x + from, y - half, to - from, width),
                scale,
                rgba,
            );
            sharp(
                cx,
                Rect::new(x - half, y + from, width, to - from),
                scale,
                rgba,
            );
        }
    }
}

/// The loupe in `frame` (points): every cell filled with its pixel (a cell past the display's edge left dark), a frame
/// round it and the middle cell outlined.
fn loupe(cx: &mut PaintCx<'_>, frame: Rect, cells: &[Option<[u8; 4]>]) {
    let n = LOUPE_PIXELS as usize;
    for (i, cell) in cells.iter().enumerate() {
        let (col, row) = ((i % n) as f64, (i / n) as f64);
        let rect = Rect::new(
            frame.x() + col * LOUPE_CELL,
            frame.y() + row * LOUPE_CELL,
            LOUPE_CELL,
            LOUPE_CELL,
        );
        fill(cx, rect, cell.map_or(DARK, |c| [c[0], c[1], c[2], 255]));
    }
    let middle = f64::from(LOUPE_PIXELS / 2) * LOUPE_CELL;
    let centre = Rect::new(
        frame.x() + middle,
        frame.y() + middle,
        LOUPE_CELL,
        LOUPE_CELL,
    );
    let scale = cx.env().scale.max(1.0);
    for rect in [frame, centre] {
        for (rgba, width) in [(DARK, LOUPE_FRAME), (LIGHT, LOUPE_FRAME / 2.0)] {
            ring(cx, rect, width, scale, rgba);
        }
    }
}

/// A line `width` points wide centred on the edge of `rect`, sharp at `scale` ([`sharp`]).
fn ring(cx: &mut PaintCx<'_>, rect: Rect, width: f64, scale: f64, rgba: [u8; 4]) {
    let half = width / 2.0;
    let (l, t) = (rect.x() - half, rect.y() - half);
    let (w, h) = (rect.width() + width, rect.height() + width);
    for side in [
        Rect::new(l, t, w, width),
        Rect::new(l, t + h - width, w, width),
        Rect::new(l, t + width, width, h - 2.0 * width),
        Rect::new(l + w - width, t + width, width, h - 2.0 * width),
    ] {
        sharp(cx, side, scale, rgba);
    }
}

impl<M: 'static> Overlay<M> {
    fn global(&self, p: Point) -> Point {
        Point::new(p.x + self.view.origin.x, p.y + self.view.origin.y)
    }
}

impl<M: 'static> Widget<M> for Overlay<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> cupertino_kit::core::Change {
        if self.view == old.view {
            cupertino_kit::core::Change::None
        } else {
            cupertino_kit::core::Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let to = self.to;
        match event {
            Event::Pointer(
                PointerEvent::Enter { position } | PointerEvent::Motion { position },
                _,
            ) => {
                // the overlay draws its own crosshair where the pointer is
                cx.set_cursor(None);
                cx.emit(to(PickInput::Moved(self.global(*position))));
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                cx.emit(to(PickInput::Left));
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed,
                    ..
                },
                _,
            ) => {
                if *pressed {
                    cx.capture_pointer();
                } else {
                    cx.release_pointer();
                }
                cx.emit(to(PickInput::Button {
                    pressed: *pressed,
                    at: self.global(*position),
                }));
                Propagation::Stop
            }
            Event::Key(key, _) if key.pressed && key.keysym.0 == ESCAPE => {
                cx.emit(to(PickInput::Escape));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
        });
        let full = Rect::new(0.0, 0.0, size.width, size.height);
        cx.image(self.view.id, &self.view.image, full, Sampling::Nearest);
        let Some(pointer) = self.view.pointer else {
            return;
        };
        cross(cx, pointer);
        if let Some(cells) = &self.view.loupe {
            loupe(cx, loupe_frame(pointer, size), cells);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(TITLE);
        cx.description("Click a pixel to pick its colour. Escape cancels.");
        if self.view.keys {
            cx.focusable();
            cx.initial_focus();
        }
    }
}

/// The key of the overlay of output `index`.
pub fn overlay_key(index: usize) -> WindowKey {
    WindowKey(NodeKey::from(format!("pick/{index}")))
}

/// The output whose overlay `key` names, by its place among the pick's; `None` for another window.
pub fn overlay_index(key: &WindowKey) -> Option<usize> {
    let NodeKey::Str(name) = &key.0 else {
        return None;
    };
    name.strip_prefix("pick/")?.parse().ok()
}
