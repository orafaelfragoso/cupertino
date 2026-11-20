//! The soft shadow around the tiles: bands outside their outline and the darkening between them.

use super::*;

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
pub(super) fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let radius = radius
        .min(rect.width() / 2.0)
        .min(rect.height() / 2.0)
        .max(0.0);
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// Darkening at a distance from the modules' edge, in points: the share of black laid over the backdrop. It starts
/// at about 9 % at the edge and fades out over [`SHADOW_REACH`].
pub(super) const SHADOW_PROFILE: [(f64, f64); 5] = [
    (0.0, 0.090),
    (30.0, 0.067),
    (60.0, 0.027),
    (90.0, 0.011),
    (SHADOW_REACH, 0.0),
];
/// The shadow is drawn as bands this wide, each a ring around the modules' outline.
pub(super) const SHADOW_BAND: f64 = 5.0;

/// The darkening `distance` points from the edge.
pub(super) fn shadow_at(distance: f64) -> f64 {
    let d = distance.max(0.0);
    for pair in SHADOW_PROFILE.windows(2) {
        let ((d0, a0), (d1, a1)) = (pair[0], pair[1]);
        if d <= d1 {
            return a0 + (a1 - a0) * ((d - d0) / (d1 - d0)).clamp(0.0, 1.0);
        }
    }
    0.0
}

/// A tile's outline over the soft shadow, and how opaque its glass is: the shadow is not laid under a tile whose glass
/// shows fully, since the glass lies under everything the window paints, and is laid under a fading tile at the share
/// its glass lets through.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Cutout {
    pub(super) rect: Rect,
    pub(super) radius: f64,
    pub(super) alpha: f64,
}

/// The outline of the shadow's band `distance` points out from `rect`: as far out on the left, the right and below,
/// and `top_reach / SHADOW_REACH` as far above, so the shadow fades out within `top_reach` above the modules. Each
/// outline holds the nearer ones, so the bands between them never overlap.
pub(super) fn band_outline(rect: Rect, distance: f64, top_reach: f64) -> (Rect, f64) {
    let up = distance * (top_reach / SHADOW_REACH).clamp(0.0, 1.0);
    let outline = Rect::new(
        rect.x() - distance,
        rect.y() - up,
        rect.width() + 2.0 * distance,
        rect.height() + distance + up,
    );
    (outline, LARGE_RADIUS + distance)
}

/// A soft shadow around `rect` (rounded at [`LARGE_RADIUS`]): concentric bands outside the outline, each filled at the
/// darkening of its middle, reaching [`SHADOW_REACH`] to the sides and below and `top_reach` above.
///
/// Inside `rect` the shadow is also laid between the `cutouts`, the tiles' outlines, at the strength it has at the
/// edge, and under a fading tile at the share its glass lets through, so a tile fading out leaves the darkening the
/// gaps around it have rather than a light patch, and nothing changes the moment a motion starts or ends.
pub(super) fn shadow(
    cx: &mut PaintCx<'_>,
    rect: Rect,
    top_reach: f64,
    cutouts: &[Cutout],
    pictures: &[Bands],
) {
    let edge = shadow_at(0.0);
    let black = |alpha: f64| Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha as f32));
    let around = rounded_path(rect.outset(SHADOW_REACH + 1.0), 0.0);
    let without = |holes: &mut dyn Iterator<Item = &Cutout>| {
        let mut path = around.clone();
        for c in holes {
            path.extend(rounded_path(c.rect, c.radius));
        }
        Shape::Path {
            verbs: path,
            fill_rule: FillRule::EvenOdd,
        }
    };
    let fading = cutouts.iter().any(|c| c.alpha < 1.0);
    if fading {
        cx.push(PaintOp::PushClip(Shape::RoundedRect {
            rect,
            radius: LARGE_RADIUS,
        }));
    }
    cx.push(PaintOp::PushClip(without(&mut cutouts.iter())));
    fill(
        cx,
        rect,
        LARGE_RADIUS,
        Color::new(0.0, 0.0, 0.0, edge as f32),
    );
    cx.push(PaintOp::PopClip);
    if fading {
        // the fading tiles by their opacity, the more opaque first: each is clipped away where a more opaque one
        // lies over it
        let mut levels: Vec<f64> = cutouts
            .iter()
            .map(|c| c.alpha.clamp(0.0, 1.0))
            .filter(|a| *a < 1.0)
            .collect();
        levels.sort_by(|a, b| b.total_cmp(a));
        levels.dedup();
        for level in levels {
            let mut above = cutouts
                .iter()
                .filter(|c| c.alpha.clamp(0.0, 1.0) > level)
                .peekable();
            let clipped = above.peek().is_some();
            if clipped {
                cx.push(PaintOp::PushClip(without(&mut above)));
            }
            for c in cutouts.iter().filter(|c| c.alpha.clamp(0.0, 1.0) == level) {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect {
                        rect: c.rect,
                        radius: c.radius,
                    },
                    paint: black(edge * (1.0 - level)),
                });
            }
            if clipped {
                cx.push(PaintOp::PopClip);
            }
        }
        cx.push(PaintOp::PopClip);
    }
    lay_bands(cx, rect, top_reach, pictures);
}

/// The shadow's bands around `rect`: rings that never overlap, each filled at the darkening of its middle.
pub(super) fn bands(rect: Rect, top_reach: f64) -> Vec<PaintOp> {
    let black = |alpha: f64| Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha as f32));
    let count = (SHADOW_REACH / SHADOW_BAND).ceil() as usize;
    let mut ops = Vec::with_capacity(count);
    for i in 0..count {
        let from = i as f64 * SHADOW_BAND;
        let alpha = shadow_at(from + SHADOW_BAND / 2.0);
        if alpha <= 0.0 {
            continue;
        }
        let (outer, outer_radius) = band_outline(rect, from + SHADOW_BAND, top_reach);
        let (inner, inner_radius) = band_outline(rect, from, top_reach);
        let mut path = rounded_path(outer, outer_radius);
        path.extend(rounded_path(inner, inner_radius));
        ops.push(PaintOp::Fill {
            shape: Shape::Path {
                verbs: path,
                fill_rule: FillRule::EvenOdd,
            },
            paint: black(alpha),
        });
    }
    ops
}

/// How many pictures of the shadow's bands the panel keeps: the panel at rest with no view and with a view open. Each
/// is the window's size in device pixels, some 2.5 MiB at scale 1.5.
const BAND_PICTURES: usize = 2;

/// The outline the soft shadow goes around, from the window's corner, and how far above it the shadow reaches: it
/// fades out by the menu bar's edge above the modules and never reaches into the bar. `shadow` is the outline from the
/// grid's corner.
pub(super) fn outline(shadow: Rect) -> (Rect, f64) {
    let rect = shadow.translate(GRID_ORIGIN.x, GRID_ORIGIN.y);
    (rect, rect.y() - bar_edge())
}

/// Where the menu bar's edge lies in the window: the shadow is clipped there.
pub(super) fn bar_edge() -> f64 {
    SHADOW_TOP - f64::from(BAR_MARGIN)
}

/// What a picture of the bands is made for: the outline's corners, the top reach and the scale, as bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct BandKey([u64; 6]);

impl BandKey {
    fn of(rect: Rect, top_reach: f64, scale: f64) -> BandKey {
        BandKey([
            rect.min_x().to_bits(),
            rect.min_y().to_bits(),
            rect.max_x().to_bits(),
            rect.max_y().to_bits(),
            top_reach.to_bits(),
            scale.to_bits(),
        ])
    }
}

/// A picture of the shadow's bands around one outline, at the device pixels of one scale, one texel per pixel, and
/// where it goes in the window. Laying it over gives the pixels the bands filled in place have, to within rounding: a
/// frame of the panel then lays one picture instead of filling some 26 rings each as large as the window, which cost a
/// frame most of its time.
#[derive(Clone, Debug)]
pub struct Bands {
    key: BandKey,
    id: ResourceId,
    image: ImageResource,
    dst: Rect,
}

impl PartialEq for Bands {
    fn eq(&self, other: &Bands) -> bool {
        self.id == other.id
    }
}

/// The bands of a panel at rest, to be drawn into a picture ([`BandsWanted::draw`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BandsWanted {
    key: BandKey,
    rect: Rect,
    top_reach: f64,
    scale: f64,
}

impl BandsWanted {
    /// The bands around `rect`, reaching `top_reach` above it, at `scale`; `None` for a scale no display has.
    pub(super) fn new(rect: Rect, top_reach: f64, scale: f64) -> Option<BandsWanted> {
        (scale.is_finite() && scale > 0.0).then(|| BandsWanted {
            key: BandKey::of(rect, top_reach, scale),
            rect,
            top_reach,
            scale,
        })
    }

    /// Draws the bands into a picture of the device pixels they reach, on the CPU pool, and gives it to `done` on the
    /// UI thread; `None` when the picture could not be drawn.
    pub fn draw<M: Send + 'static>(
        &self,
        env: &Environment,
        done: impl FnOnce(Option<Bands>) -> M + Send + 'static,
    ) -> Command<M> {
        let BandsWanted {
            key,
            rect,
            top_reach,
            scale,
        } = *self;
        let (outer, _) = band_outline(rect, SHADOW_REACH, top_reach);
        let (x0, y0) = (
            (outer.min_x() * scale).floor(),
            (outer.min_y() * scale).floor(),
        );
        let (x1, y1) = (
            (outer.max_x() * scale).ceil(),
            (outer.max_y() * scale).ceil(),
        );
        // the picture's first pixel is the window's pixel (x0, y0): the bands move by whole device pixels into it
        let at = Point::new(x0 / scale, y0 / scale);
        let size = Size::new((x1 - x0) / scale, (y1 - y0) / scale);
        let ops = bands(rect.translate(-at.x, -at.y), top_reach);
        render::picture(env, ops, size, scale, move |image| {
            done(image.map(|image| Bands {
                key,
                id: next_resource_id(),
                dst: Rect::new(
                    at.x,
                    at.y,
                    f64::from(image.width) / scale,
                    f64::from(image.height) / scale,
                ),
                image,
            }))
        })
    }
}

/// The pictures of the bands the panel keeps, the latest used last.
#[derive(Clone, Debug, Default)]
pub struct BandPictures {
    kept: Vec<Bands>,
}

impl BandPictures {
    /// Whether `wanted` has no picture yet; a picture kept for it becomes the latest used.
    pub fn wants(&mut self, wanted: &BandsWanted) -> bool {
        match self.kept.iter().position(|b| b.key == wanted.key) {
            Some(i) => {
                let found = self.kept.remove(i);
                self.kept.push(found);
                false
            }
            None => true,
        }
    }

    /// Keeps `bands`, letting the picture used longest ago go past [`BAND_PICTURES`].
    pub fn keep(&mut self, bands: Bands) {
        self.kept.retain(|b| b.key != bands.key);
        self.kept.push(bands);
        if self.kept.len() > BAND_PICTURES {
            self.kept.remove(0);
        }
    }

    /// The pictures kept, for the board to find the one of its outline and scale.
    pub(super) fn all(&self) -> Vec<Bands> {
        self.kept.clone()
    }
}

/// Lays the bands around `rect`: the picture of them at the window's scale when `pictures` has one, else filled in
/// place.
fn lay_bands(cx: &mut PaintCx<'_>, rect: Rect, top_reach: f64, pictures: &[Bands]) {
    let key = BandKey::of(rect, top_reach, cx.env().scale);
    match pictures.iter().find(|b| b.key == key) {
        Some(b) => cx.image(b.id, &b.image, b.dst, Sampling::Nearest),
        None => {
            for op in bands(rect, top_reach) {
                cx.push(op);
            }
        }
    }
}
