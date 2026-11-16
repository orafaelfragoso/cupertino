//! Whether a window whose client draws its own frame is an opaque rectangle, up to corners of its own no rounder than
//! the window's: only such a window gets the compositor's rounded corners, outline and shadow
//! ([`crate::state::WindowEntry::styled`]). A frameless transparent window (an overlay, a floating widget, a pill-shaped
//! bar) is left as it draws itself, with no box drawn around it.
//!
//! The client's opaque region says so first: browsers and toolkits that draw their own title bar declare their frame
//! opaque but for small corners. Without such a region, the pixels of a shared-memory buffer along the frame's edges,
//! just past the window's corners, say so; a buffer of a format without alpha is opaque; a GPU buffer with alpha and no
//! region covering it is taken as shaped.

use smithay::backend::allocator::format::has_alpha;
use smithay::backend::allocator::Buffer as _;
use smithay::backend::renderer::utils::with_renderer_surface_state;
use smithay::reexports::wayland_server::protocol::{
    wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface,
};
use smithay::utils::{Logical, Rectangle, Transform};
use smithay::wayland::compositor::{with_states, SurfaceAttributes};

/// The least alpha (of 255) of a pixel that counts as opaque: an edge drawn by the client, antialiasing aside.
const OPAQUE_ALPHA: u8 = 0xf0;

/// Whether the frame `geometry` (surface-local points) of `surface`'s current state is an opaque rectangle past corners
/// of `radius` points.
pub fn opaque_frame(surface: &WlSurface, geometry: Rectangle<i32, Logical>, radius: f64) -> bool {
    let region = with_states(surface, |s| {
        s.cached_state
            .get::<SurfaceAttributes>()
            .current()
            .opaque_region
            .clone()
    });
    if region.is_some_and(|r| region_covers(|x, y| r.contains((x, y)), geometry, radius)) {
        return true;
    }
    let state = with_renderer_surface_state(surface, |s| {
        (
            s.buffer().map(|b| WlBuffer::clone(b)),
            s.buffer_scale(),
            s.buffer_transform(),
            s.view(),
        )
    });
    let Some((Some(buffer), scale, transform, view)) = state else {
        return true;
    };
    if let Ok(dmabuf) = smithay::wayland::dmabuf::get_dmabuf(&buffer) {
        return !has_alpha(dmabuf.format().code);
    }
    if transform != Transform::Normal {
        return true;
    }
    // a point of the surface to a pixel of the buffer: through the viewport's crop and scale, then the buffer's scale
    let to_buffer = |x: i32, y: i32| -> (i32, i32) {
        let (mut bx, mut by) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
        if let Some(v) = view {
            if v.dst.w > 0 && v.dst.h > 0 {
                bx = v.src.loc.x + bx * v.src.size.w / f64::from(v.dst.w);
                by = v.src.loc.y + by * v.src.size.h / f64::from(v.dst.h);
            }
        }
        (
            (bx * f64::from(scale)).floor() as i32,
            (by * f64::from(scale)).floor() as i32,
        )
    };
    smithay::wayland::shm::with_buffer_contents(&buffer, |ptr, len, data| {
        // the byte of a pixel that holds its alpha, little-endian
        let alpha_byte = match data.format {
            wl_shm::Format::Argb8888 | wl_shm::Format::Abgr8888 => 3,
            format => {
                return smithay::wayland::shm::shm_format_to_fourcc(format)
                    .is_none_or(|f| !has_alpha(f));
            }
        };
        // SAFETY: smithay maps the pool for the call and `len` is its length
        let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
        edges_opaque(
            |x, y| {
                let (bx, by) = to_buffer(x, y);
                if bx < 0 || by < 0 || bx >= data.width || by >= data.height {
                    return None;
                }
                let at =
                    data.offset as usize + by as usize * data.stride as usize + bx as usize * 4;
                bytes.get(at + alpha_byte).copied()
            },
            geometry,
            radius,
        )
    })
    .unwrap_or(true)
}

/// How far from a corner of `g` the points looked at lie: past a corner of `radius` points, within the frame.
fn reach(g: Rectangle<i32, Logical>, radius: f64) -> i32 {
    let past = radius.max(0.0).ceil() as i32 + 1;
    past.min((g.size.w - 1) / 2).min((g.size.h - 1) / 2).max(0)
}

/// Whether the opaque region (`contains`, surface-local points) covers the frame `g` inset by the corner `radius`:
/// what is left out lies within a corner's reach of the edges, which the window's own rounded corners cut away. The
/// corners and the middles of that inset rectangle are looked at.
fn region_covers(
    contains: impl Fn(i32, i32) -> bool,
    g: Rectangle<i32, Logical>,
    radius: f64,
) -> bool {
    if g.size.w <= 0 || g.size.h <= 0 {
        return false;
    }
    let d = reach(g, radius);
    let (left, top) = (g.loc.x + d, g.loc.y + d);
    let (right, bottom) = (g.loc.x + g.size.w - 1 - d, g.loc.y + g.size.h - 1 - d);
    let (mid_x, mid_y) = (g.loc.x + g.size.w / 2, g.loc.y + g.size.h / 2);
    [
        (left, top),
        (right, top),
        (left, bottom),
        (right, bottom),
        (mid_x, top),
        (mid_x, bottom),
        (left, mid_y),
        (right, mid_y),
    ]
    .into_iter()
    .all(|(x, y)| contains(x, y))
}

/// Whether the pixels (`alpha` of a surface-local point, `None` outside the buffer) along the edges of the frame `g`,
/// just past corners of `radius` points from each corner, are opaque: a rectangle with corners no rounder than the
/// window's has them, a shape with rounder corners (a pill) or a transparent window does not.
fn edges_opaque(
    alpha: impl Fn(i32, i32) -> Option<u8>,
    g: Rectangle<i32, Logical>,
    radius: f64,
) -> bool {
    if g.size.w <= 0 || g.size.h <= 0 {
        return false;
    }
    let d = reach(g, radius);
    let (x0, y0) = (g.loc.x, g.loc.y);
    let (x1, y1) = (g.loc.x + g.size.w - 1, g.loc.y + g.size.h - 1);
    [
        (x0 + d, y0),
        (x1 - d, y0),
        (x0 + d, y1),
        (x1 - d, y1),
        (x0, y0 + d),
        (x0, y1 - d),
        (x1, y0 + d),
        (x1, y1 - d),
    ]
    .into_iter()
    .all(|(x, y)| alpha(x, y).is_some_and(|a| a >= OPAQUE_ALPHA))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RADIUS: f64 = 16.0;

    fn rect(x: i32, y: i32, w: i32, h: i32) -> Rectangle<i32, Logical> {
        Rectangle::new((x, y).into(), (w, h).into())
    }

    fn in_any(rects: &[Rectangle<i32, Logical>]) -> impl Fn(i32, i32) -> bool + '_ {
        move |x, y| rects.iter().any(|r| r.contains((x, y)))
    }

    #[test]
    fn regions_that_leave_out_small_corners_or_a_strip_cover_the_frame() {
        // a browser: its frame inside a shadow, opaque but for 8-point squares at the top corners
        let browser = [rect(24, 10, 1002, 8), rect(16, 18, 1018, 730)];
        assert!(region_covers(
            in_any(&browser),
            rect(16, 10, 1018, 738),
            RADIUS
        ));
        // an application of the same engine: 8-point squares left out at all four corners
        let app = [
            rect(24, 10, 1184, 8),
            rect(16, 18, 1200, 784),
            rect(24, 802, 1184, 8),
        ];
        assert!(region_covers(in_any(&app), rect(16, 10, 1200, 800), RADIUS));
        // a toolkit: the largest rectangle inside its rounded frame, the strip of its top corners left out
        let toolkit = [rect(14, 20, 500, 392)];
        assert!(region_covers(
            in_any(&toolkit),
            rect(14, 12, 500, 400),
            RADIUS
        ));
        // the whole frame
        assert!(region_covers(
            in_any(&[rect(0, 0, 400, 300)]),
            rect(0, 0, 400, 300),
            RADIUS
        ));
    }

    #[test]
    fn a_pill_s_region_an_empty_one_or_one_short_of_an_edge_does_not_cover_the_frame() {
        let frame = rect(0, 0, 600, 60);
        // a pill's core, between its round ends
        assert!(!region_covers(
            in_any(&[rect(30, 0, 540, 60)]),
            frame,
            RADIUS
        ));
        // a pill's cross, as a toolkit gives it
        let cross = [rect(30, 0, 540, 60), rect(0, 30, 600, 1)];
        assert!(!region_covers(in_any(&cross), frame, RADIUS));
        assert!(!region_covers(in_any(&[]), frame, RADIUS));
        // opaque but for a band along the bottom deeper than the corners
        let short = [rect(0, 0, 400, 260)];
        assert!(!region_covers(in_any(&short), rect(0, 0, 400, 300), RADIUS));
    }

    /// The alpha of a rounded rectangle `w` x `h` with corners of `r` points, opaque inside and clear outside.
    fn rounded(w: i32, h: i32, r: f64) -> impl Fn(i32, i32) -> Option<u8> {
        move |x, y| {
            if x < 0 || y < 0 || x >= w || y >= h {
                return None;
            }
            let (px, py) = (f64::from(x) + 0.5, f64::from(y) + 0.5);
            let cx = px.clamp(r, f64::from(w) - r);
            let cy = py.clamp(r, f64::from(h) - r);
            let inside = (px - cx).hypot(py - cy) <= r;
            Some(if inside { 255 } else { 0 })
        }
    }

    #[test]
    fn a_square_frame_or_one_with_corners_tighter_than_the_window_s_is_opaque_at_its_edges() {
        assert!(edges_opaque(
            rounded(400, 300, 0.0),
            rect(0, 0, 400, 300),
            RADIUS
        ));
        assert!(edges_opaque(
            rounded(400, 300, 8.0),
            rect(0, 0, 400, 300),
            RADIUS
        ));
        assert!(edges_opaque(
            rounded(400, 300, RADIUS),
            rect(0, 0, 400, 300),
            RADIUS
        ));
    }

    #[test]
    fn a_pill_a_transparent_window_or_a_frame_past_the_buffer_is_not() {
        assert!(!edges_opaque(
            rounded(600, 60, 30.0),
            rect(0, 0, 600, 60),
            RADIUS
        ));
        assert!(!edges_opaque(
            rounded(400, 300, 150.0),
            rect(0, 0, 400, 300),
            RADIUS
        ));
        assert!(!edges_opaque(|_, _| Some(0), rect(0, 0, 400, 300), RADIUS));
        // a frame said to be larger than what the client drew
        assert!(!edges_opaque(
            rounded(400, 300, 0.0),
            rect(0, 0, 500, 300),
            RADIUS
        ));
    }

    #[test]
    fn a_frame_inside_a_shadow_is_looked_at_where_it_lies() {
        // the frame at (20, 20) of a buffer whose margin is clear: its own edges are opaque
        let framed = |x: i32, y: i32| {
            let inside = (20..420).contains(&x) && (20..320).contains(&y);
            Some(if inside { 255 } else { 0 })
        };
        assert!(edges_opaque(framed, rect(20, 20, 400, 300), RADIUS));
        assert!(!edges_opaque(framed, rect(0, 0, 440, 340), RADIUS));
    }
}
