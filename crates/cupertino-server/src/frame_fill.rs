//! Whether a window whose client draws its own frame is an opaque rectangle, up to corners of its own no rounder than
//! the window's: only such a window gets the compositor's rounded corners, outline and shadow
//! ([`crate::state::WindowEntry::styled`]). A frameless transparent window (an overlay, a floating widget, a pill-shaped
//! bar) is left as it draws itself, with no box drawn around it.
//!
//! The client's opaque region says so first: browsers and toolkits that draw their own title bar declare their frame
//! opaque but for small corners. Without such a region, the pixels of a shared-memory buffer along the frame's edges,
//! just past the window's corners, say so; a buffer of a format without alpha is opaque. A GPU buffer with alpha and no
//! region covering it is framed when the client draws a margin of its own around its frame (a shadow): its window
//! geometry lies inside its surface, short of it on some side, as a toolkit's frame does and a frameless window, whose
//! geometry is its whole surface, does not. Once seen, that margin keeps the window framed when its geometry later fills
//! its surface (maximised or tiled, the client drops its margin), until a shared-memory buffer's edges show it
//! transparent; otherwise such a buffer is taken as shaped. A region may leave out corners of up to [`REGION_CORNER`]
//! points however small the window's own corner radius is.

use smithay::backend::allocator::format::has_alpha;
use smithay::backend::allocator::{Buffer as _, Fourcc};
use smithay::backend::renderer::utils::{with_renderer_surface_state, SurfaceView};
use smithay::reexports::wayland_server::protocol::{
    wl_buffer::WlBuffer, wl_shm, wl_surface::WlSurface,
};
use smithay::utils::{Logical, Rectangle, Size, Transform};
use smithay::wayland::compositor::{with_states, SurfaceAttributes};

/// The least alpha (of 255) of a pixel that counts as opaque: an edge drawn by the client, antialiasing aside.
const OPAQUE_ALPHA: u8 = 0xf0;

/// The corners, in points, a client's opaque region may leave out and still cover its frame, however small the
/// window's own corner radius is: browsers leave out squares of their own corners whatever the server rounds.
const REGION_CORNER: f64 = 16.0;

/// What a client-drawn frame is, as of a commit ([`opaque_frame`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Frame {
    /// The frame is an opaque rectangle past the window's corners: the window is styled.
    pub opaque: bool,
    /// The client has drawn a margin of its own around its frame (its geometry inside its surface) since its edges were
    /// last seen transparent: the window stays framed when its geometry fills its surface.
    pub margined: bool,
}

/// What the frame `geometry` (surface-local points) of `surface`'s current state is, past corners of `radius` points,
/// for a window `margined` as of its last commit ([`Frame::margined`]).
pub fn opaque_frame(
    surface: &WlSurface,
    geometry: Rectangle<i32, Logical>,
    radius: f64,
    margined: bool,
) -> Frame {
    let region = with_states(surface, |s| {
        s.cached_state
            .get::<SurfaceAttributes>()
            .current()
            .opaque_region
            .clone()
    });
    let covered =
        region.is_some_and(|r| region_covers(|x, y| r.contains((x, y)), geometry, radius));
    let state = with_renderer_surface_state(surface, |s| {
        (
            s.buffer().map(|b| WlBuffer::clone(b)),
            s.buffer_scale(),
            s.buffer_transform(),
            s.view(),
            s.surface_size(),
        )
    });
    let Some((Some(buffer), scale, transform, view, size)) = state else {
        return Frame {
            opaque: true,
            margined,
        };
    };
    let gpu = smithay::wayland::dmabuf::get_dmabuf(&buffer)
        .ok()
        .map(|dmabuf| dmabuf.format().code);
    let seen = Seen {
        covered,
        gpu,
        margin: size.is_some_and(|size| margin_around(geometry, size)),
        margined,
    };
    frame_is_opaque(seen, || {
        shared_edges_opaque(&buffer, scale, transform, view, geometry, radius)
    })
}

/// What a commit shows of a client-drawn frame: the inputs of [`frame_is_opaque`].
#[derive(Clone, Copy, Debug)]
struct Seen {
    /// The client's opaque region covers the frame.
    covered: bool,
    /// The format of the GPU buffer, `None` for a shared-memory one.
    gpu: Option<Fourcc>,
    /// The window geometry lies inside the surface, short of it on some side ([`margin_around`]).
    margin: bool,
    /// [`Frame::margined`] as of the last commit.
    margined: bool,
}

/// What a frame is: opaque when the client's opaque region covers it, else by its buffer: a GPU buffer by whether its
/// format has alpha, and one with alpha by a margin drawn around the frame now or since its edges were last seen
/// transparent; a shared-memory buffer by the pixels at its edges (`shared`), which also end a margin seen before when
/// they are transparent.
fn frame_is_opaque(seen: Seen, shared: impl FnOnce() -> bool) -> Frame {
    let margined = seen.margined || seen.margin;
    if seen.covered {
        return Frame {
            opaque: true,
            margined,
        };
    }
    match seen.gpu {
        Some(format) => Frame {
            opaque: !has_alpha(format) || margined,
            margined,
        },
        None => {
            let opaque = shared();
            Frame {
                opaque,
                margined: margined && opaque,
            }
        }
    }
}

/// Whether the frame `geometry` lies inside a surface of `size` (surface-local points) and short of it on some side:
/// the client draws a margin of its own (a shadow) around its frame.
fn margin_around(geometry: Rectangle<i32, Logical>, size: Size<i32, Logical>) -> bool {
    let surface = Rectangle::from_size(size);
    geometry.size.w > 0
        && geometry.size.h > 0
        && surface.contains_rect(geometry)
        && geometry != surface
}

/// Whether the shared-memory `buffer`'s pixels along the edges of the frame `geometry` are opaque ([`edges_opaque`]).
fn shared_edges_opaque(
    buffer: &WlBuffer,
    scale: i32,
    transform: Transform,
    view: Option<SurfaceView>,
    geometry: Rectangle<i32, Logical>,
    radius: f64,
) -> bool {
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
    smithay::wayland::shm::with_buffer_contents(buffer, |ptr, len, data| {
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

/// Whether the opaque region (`contains`, surface-local points) covers the frame `g` inset by the corner `radius`, or
/// by [`REGION_CORNER`] when that is larger: what is left out lies within a corner's reach of the edges, the client's
/// own small corners or what the window's rounded corners cut away. The corners and the middles of that inset
/// rectangle are looked at.
fn region_covers(
    contains: impl Fn(i32, i32) -> bool,
    g: Rectangle<i32, Logical>,
    radius: f64,
) -> bool {
    if g.size.w <= 0 || g.size.h <= 0 {
        return false;
    }
    let d = reach(g, radius.max(REGION_CORNER));
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

    /// The window corner radii the region rules hold for: square, smaller than a browser's corners, the default.
    const RADII: [f64; 3] = [0.0, 4.0, RADIUS];

    #[test]
    fn regions_that_leave_out_small_corners_or_a_strip_cover_the_frame() {
        for radius in RADII {
            // a browser: its frame inside a shadow, opaque but for 8-point squares at the top corners
            let browser = [rect(24, 10, 1002, 8), rect(16, 18, 1018, 730)];
            assert!(
                region_covers(in_any(&browser), rect(16, 10, 1018, 738), radius),
                "a browser's region at a corner radius of {radius}"
            );
            // an application of the same engine: 8-point squares left out at all four corners
            let app = [
                rect(24, 10, 1184, 8),
                rect(16, 18, 1200, 784),
                rect(24, 802, 1184, 8),
            ];
            assert!(
                region_covers(in_any(&app), rect(16, 10, 1200, 800), radius),
                "an application's region at a corner radius of {radius}"
            );
            // a toolkit: the largest rectangle inside its rounded frame, the strip of its top corners left out
            let toolkit = [rect(14, 20, 500, 392)];
            assert!(
                region_covers(in_any(&toolkit), rect(14, 12, 500, 400), radius),
                "a toolkit's region at a corner radius of {radius}"
            );
            // the whole frame
            assert!(region_covers(
                in_any(&[rect(0, 0, 400, 300)]),
                rect(0, 0, 400, 300),
                radius
            ));
        }
    }

    #[test]
    fn a_pill_s_region_an_empty_one_or_one_short_of_an_edge_does_not_cover_the_frame() {
        for radius in RADII {
            let frame = rect(0, 0, 600, 60);
            // a pill's core, between its round ends
            assert!(!region_covers(
                in_any(&[rect(30, 0, 540, 60)]),
                frame,
                radius
            ));
            // a pill's cross, as a toolkit gives it
            let cross = [rect(30, 0, 540, 60), rect(0, 30, 600, 1)];
            assert!(!region_covers(in_any(&cross), frame, radius));
            assert!(!region_covers(in_any(&[]), frame, radius));
            // opaque but for a band along the bottom deeper than the corners
            let short = [rect(0, 0, 400, 260)];
            assert!(!region_covers(in_any(&short), rect(0, 0, 400, 300), radius));
        }
    }

    /// What a commit shows: a buffer of `gpu` (`None` for shared memory), `covered` by the opaque region, with a
    /// `margin` around its frame now and `margined` before.
    fn seen(covered: bool, gpu: Option<Fourcc>, margin: bool, margined: bool) -> Seen {
        Seen {
            covered,
            gpu,
            margin,
            margined,
        }
    }

    fn unread() -> bool {
        panic!("a GPU buffer's pixels are not read")
    }

    #[test]
    fn a_gpu_buffer_with_alpha_is_opaque_by_a_covering_region_or_a_margin_of_its_own() {
        let argb = Some(Fourcc::Argb8888);
        assert!(
            frame_is_opaque(seen(true, argb, false, false), unread).opaque,
            "an ARGB GPU buffer with a covering opaque region"
        );
        assert!(
            !frame_is_opaque(seen(false, argb, false, false), unread).opaque,
            "an ARGB GPU buffer without one or a margin is shaped"
        );
        assert!(
            frame_is_opaque(seen(false, Some(Fourcc::Xrgb8888), false, false), unread).opaque,
            "a GPU buffer without alpha is opaque"
        );
        let framed = frame_is_opaque(seen(false, argb, true, false), unread);
        assert!(
            framed.opaque && framed.margined,
            "an ARGB GPU buffer drawing a margin around its frame, with no region"
        );
        // a shared-memory buffer's pixels are read only without a covering region
        assert!(frame_is_opaque(seen(true, None, false, false), || panic!("read")).opaque);
        assert!(!frame_is_opaque(seen(false, None, false, false), || false).opaque);
        assert!(frame_is_opaque(seen(false, None, false, false), || true).opaque);
    }

    #[test]
    fn a_margin_once_drawn_keeps_the_frame_until_its_edges_show_transparent() {
        let argb = Some(Fourcc::Argb8888);
        // drawn with a margin, then maximised: the geometry fills the surface
        let first = frame_is_opaque(seen(false, argb, true, false), unread);
        let maximised = frame_is_opaque(seen(false, argb, false, first.margined), unread);
        assert_eq!(
            maximised,
            Frame {
                opaque: true,
                margined: true
            }
        );
        // a covering region or a buffer without alpha keeps it too
        assert!(frame_is_opaque(seen(true, argb, false, true), unread).margined);
        assert!(frame_is_opaque(seen(false, Some(Fourcc::Xrgb8888), false, true), unread).margined);
        // a shared-memory buffer whose edges are opaque keeps it, one whose edges are transparent ends it
        assert!(frame_is_opaque(seen(false, None, false, true), || true).margined);
        let cleared = frame_is_opaque(seen(false, None, false, true), || false);
        assert_eq!(cleared, Frame::default());
        assert!(!frame_is_opaque(seen(false, argb, false, cleared.margined), unread).opaque);
    }

    #[test]
    fn a_frame_is_margined_when_its_geometry_lies_inside_its_surface_and_short_of_it() {
        let surface = Size::from((1707, 960));
        // a browser drawing its own toolkit frame inside its shadow
        assert!(margin_around(rect(34, 18, 1639, 892), surface));
        // short of the surface on one side alone (tiled)
        assert!(margin_around(rect(0, 0, 1707, 940), surface));
        // a frameless window: its geometry is its whole surface
        assert!(!margin_around(rect(0, 0, 1707, 960), surface));
        // past the surface (a subsurface out of it), or empty
        assert!(!margin_around(rect(-10, 0, 1707, 960), surface));
        assert!(!margin_around(rect(0, 0, 1800, 960), surface));
        assert!(!margin_around(rect(10, 10, 0, 0), surface));
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
