//! Pointer constraint geometry and state helpers (`zwp_pointer_constraints_v1`).

use smithay::reexports::wayland_server::backend::ObjectId;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Point, Rectangle};
use smithay::wayland::compositor::{
    get_parent, with_states, RectangleKind, RegionAttributes, SubsurfaceCachedState,
};

/// The surfaces that have a constraint, as their clients made them.
#[derive(Default)]
pub struct PointerConstraints {
    pub surfaces: Vec<WlSurface>,
    /// The window with the keyboard focus that has shown its notice for the pointer: once per focus or release, not
    /// again when Smithay ends the constraint as the pointer crosses into one of its subsurfaces and it is taken again.
    pub noticed: Option<ObjectId>,
}

/// The origin of `surface` when `under`, a surface at its origin, is `surface` or one of its subsurfaces.
pub fn origin_of(
    surface: &WlSurface,
    under: &(WlSurface, Point<f64, Logical>),
) -> Option<Point<f64, Logical>> {
    let (mut at, mut origin) = under.clone();
    while at != *surface {
        let parent = get_parent(&at)?;
        let offset = with_states(&at, |s| {
            s.cached_state
                .get::<SubsurfaceCachedState>()
                .current()
                .location
        });
        origin -= offset.to_f64();
        at = parent;
    }
    Some(origin)
}

/// What an active constraint does to a motion of the pointer.
pub enum Constrained {
    /// The pointer stays where it is.
    Locked,
    /// The pointer goes here instead.
    Confined(Point<f64, Logical>),
}

/// The point nearest `to` that `allowed` accepts among `to` itself and `to` clamped into each of `rects`, else
/// `from`: the pointer slides along the edge of the region instead of stopping short of it.
pub fn confine(
    from: Point<f64, Logical>,
    to: Point<f64, Logical>,
    rects: &[Rectangle<i32, Logical>],
    allowed: impl Fn(Point<f64, Logical>) -> bool,
) -> Point<f64, Logical> {
    if allowed(to) {
        return to;
    }
    rects
        .iter()
        .filter(|r| r.size.w > 0 && r.size.h > 0)
        .map(|r| {
            let (x0, y0) = (f64::from(r.loc.x), f64::from(r.loc.y));
            Point::from((
                to.x.clamp(x0, (x0 + f64::from(r.size.w) - 1.0).max(x0)),
                to.y.clamp(y0, (y0 + f64::from(r.size.h) - 1.0).max(y0)),
            ))
        })
        .filter(|p| allowed(*p))
        .min_by(|a, b| {
            let d = |p: &Point<f64, Logical>| (p.x - to.x).powi(2) + (p.y - to.y).powi(2);
            d(a).total_cmp(&d(b))
        })
        .unwrap_or(from)
}

/// Whether the surface-local point `p` is in `region`; no region is the whole surface.
pub fn in_region(region: Option<&RegionAttributes>, p: Point<f64, Logical>) -> bool {
    region.is_none_or(|r| r.contains(p.to_i32_floor()))
}

/// The rectangles a region adds, or `whole` for no region.
pub fn added(
    region: Option<&RegionAttributes>,
    whole: Rectangle<i32, Logical>,
) -> Vec<Rectangle<i32, Logical>> {
    match region {
        None => vec![whole],
        Some(r) => r
            .rects
            .iter()
            .filter(|(kind, _)| matches!(kind, RectangleKind::Add))
            .map(|(_, rect)| *rect)
            .collect(),
    }
}
