//! A popover grows out of what it opened from ([`Widget::grows_from_anchor`](crate::Widget::grows_from_anchor)). As it
//! opens, the window and its glass start at a tenth of their size about the point of the anchor nearest to the popover
//! (the tip of its pointer) and spring out to their full size; as it closes they shrink back into that point and fade.
//! The window keeps its size and its layout throughout: what it draws and the glass regions it asks for are scaled
//! about that point frame by frame. Under Reduce Motion a popover shows at its full size at once and goes at once.

use super::Runtime;
use crate::Application;
use cupertino_geometry::{Point, Rect};
use cupertino_ui_platform::{MaterialRegion, Timestamp, WindowSystem};
use cupertino_ui_render::{DisplayList, Transform};

/// The scale a popover starts at as it opens.
pub(crate) const OPEN_START_SCALE: f64 = 0.10;
/// The response of the spring a popover opens on: the period of its undamped swing, in seconds.
const OPEN_RESPONSE: f64 = 0.29;
/// The spring's damping ratio: a little under critical, so the popover passes its size by about 1 % and comes back.
const OPEN_DAMPING: f64 = 0.82;
/// How long the opening takes until the popover rests at its size, in nanoseconds; it is drawn whole from then on.
pub(crate) const OPEN_NS: u64 = 350_000_000;
/// How long a popover takes to shrink back into its anchor as it closes, in nanoseconds.
pub(crate) const CLOSE_NS: u64 = 220_000_000;

/// A popover growing to its size: when it began and the point it grows from, in its window's coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Grow {
    pub(crate) start: Timestamp,
    pub(crate) origin: Point,
}

impl Grow {
    /// The growth of a popover first shown at `now` from `origin`; none under Reduce Motion, where it shows at its size.
    pub(crate) fn begin(now: Timestamp, origin: Point, reduce_motion: bool) -> Option<Self> {
        (!reduce_motion).then_some(Grow { start: now, origin })
    }

    /// Its scale at `now`, or `None` once it rests at its size.
    pub(crate) fn scale(&self, now: Timestamp) -> Option<f64> {
        let elapsed = now.0.saturating_sub(self.start.0);
        (elapsed < OPEN_NS).then(|| open_scale(elapsed as f64 / 1e9))
    }
}

/// The scale of an opening popover `t` seconds in: from [`OPEN_START_SCALE`] to 1 along a damped spring.
pub(crate) fn open_scale(t: f64) -> f64 {
    OPEN_START_SCALE + (1.0 - OPEN_START_SCALE) * spring(t)
}

/// The progress of the opening spring `t` seconds in: 0 at the start, settling on 1.
fn spring(t: f64) -> f64 {
    if t <= 0.0 {
        return 0.0;
    }
    let natural = std::f64::consts::TAU / OPEN_RESPONSE;
    let decay = OPEN_DAMPING * natural;
    let damped = natural * (1.0 - OPEN_DAMPING * OPEN_DAMPING).sqrt();
    1.0 - (-decay * t).exp() * ((damped * t).cos() + decay / damped * (damped * t).sin())
}

/// The scale of a closing popover `elapsed` nanoseconds in: `(1 - t / T)²`, into its anchor's point.
pub(crate) fn close_scale(elapsed: u64) -> f64 {
    let left = 1.0 - elapsed as f64 / CLOSE_NS as f64;
    left.clamp(0.0, 1.0).powi(2)
}

/// `rect` scaled by `scale` about `origin`.
pub(crate) fn scale_rect(rect: Rect, origin: Point, scale: f64) -> Rect {
    Rect::new(
        origin.x + (rect.min_x() - origin.x) * scale,
        origin.y + (rect.min_y() - origin.y) * scale,
        rect.width() * scale,
        rect.height() * scale,
    )
}

/// Draws the window's list and its glass scaled by `scale` about `origin`.
pub(crate) fn apply(
    list: &mut DisplayList,
    materials: &mut [MaterialRegion],
    origin: Point,
    scale: f64,
) {
    if scale == 1.0 {
        return;
    }
    for node in &mut list.nodes {
        let [a, b, c, d, e, f] = node.transform.0;
        node.transform = Transform([
            a * scale,
            b * scale,
            c * scale,
            d * scale,
            origin.x + (e - origin.x) * scale,
            origin.y + (f - origin.y) * scale,
        ]);
        node.bounds = scale_rect(node.bounds, origin, scale);
        node.hit = scale_rect(node.hit, origin, scale);
    }
    for region in materials {
        region.rect = scale_rect(region.rect, origin, scale);
        region.radius *= scale;
        if let Some(glass) = &mut region.glass {
            glass.spacing *= scale;
        }
    }
}

/// The glass of a closing popover fades with what it draws.
pub(crate) fn fade_materials(materials: &mut [MaterialRegion], opacity: f32) {
    for region in materials {
        region.alpha *= f64::from(opacity);
    }
}

/// The point a popup placed at `placed` grows from: the point of it nearest to the middle of `anchor`, both in the
/// parent's coordinates; given in the popup's.
pub(crate) fn origin(anchor: Rect, placed: Rect) -> Point {
    let middle = Point::new(
        anchor.min_x() + anchor.width() / 2.0,
        anchor.min_y() + anchor.height() / 2.0,
    );
    Point::new(
        middle.x.clamp(placed.min_x(), placed.max_x()) - placed.min_x(),
        middle.y.clamp(placed.min_y(), placed.max_y()) - placed.min_y(),
    )
}

impl<A: Application, W: WindowSystem> Runtime<A, W> {
    /// The popup at `i` draws its first frame at `now`: a popover starts growing out of its anchor.
    pub(super) fn begin_grow(&mut self, i: usize, now: Timestamp) {
        let w = &self.windows[i];
        if w.committed || w.grows_from.is_some() || w.fade.is_some() {
            return;
        }
        let (Some(popup), Some(root)) = (&w.popup, w.root) else {
            return;
        };
        if !self
            .tree
            .get(root)
            .is_some_and(|n| n.widget.grows_from_anchor())
        {
            return;
        }
        let placed = self
            .windows
            .iter()
            .find(|p| p.key == popup.parent)
            .and_then(|p| p.popups.iter().find(|(k, _)| *k == w.key))
            .map(|(_, rect)| *rect);
        // until the compositor says where it put the popover, the middle of its top edge, under its anchor
        let at = placed.map_or(Point::new(w.size.width / 2.0, 0.0), |placed| {
            origin(popup.placed.anchor_rect, placed)
        });
        let reduce_motion = w.env.theme.preferences.reduce_motion;
        let w = &mut self.windows[i];
        w.grows_from = Some(at);
        w.grow = Grow::begin(now, at, reduce_motion);
    }

    /// The growing popovers draw again at each frame, whole once they rest at their size.
    pub(super) fn advance_grows(&mut self) {
        let now = self.options.clock.now();
        for w in &mut self.windows {
            let Some(grow) = w.grow else { continue };
            if grow.scale(now).is_none() {
                w.grow = None;
            }
            w.dirty = true;
            w.invalidate_damage();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// A popover 240 by 300 points opened below a 28-point button whose middle is 120 points from its left edge.
    fn popover() -> (Rect, Rect, Rect) {
        let button = Rect::new(106.0, 0.0, 28.0, 28.0);
        let placed = Rect::new(0.0, 34.0, 240.0, 300.0);
        let full = Rect::new(0.0, 0.0, 240.0, 300.0);
        (button, placed, full)
    }

    #[test]
    fn the_shape_runs_from_a_tenth_at_the_anchor_to_the_whole_popover() {
        let (button, placed, full) = popover();
        let at = origin(button, placed);
        assert_eq!(
            at,
            Point::new(120.0, 0.0),
            "the pointer's tip, under the button"
        );
        let grow = Grow::begin(Timestamp(0), at, false).expect("it grows");
        let first = scale_rect(full, at, grow.scale(Timestamp(0)).unwrap());
        assert_eq!(
            first,
            Rect::new(108.0, 0.0, 24.0, 30.0),
            "a tenth, at the tip"
        );
        // no larger than the button it comes out of
        assert!(first.width() <= button.width() && first.height() <= button.height() + 2.0);
        let mut last = first;
        let mut widest: f64 = 0.0;
        for ms in (16..350).step_by(16) {
            let rect = scale_rect(full, at, grow.scale(Timestamp(ms * MS)).unwrap());
            // always about the tip: the middle stays under the button and the top at the tip
            assert!((rect.min_x() + rect.width() / 2.0 - 120.0).abs() < 1e-9);
            assert_eq!(rect.min_y(), 0.0);
            if ms <= 160 {
                assert!(rect.width() > last.width(), "it grows at {ms} ms");
            }
            widest = widest.max(rect.width());
            last = rect;
        }
        // the spring passes the size by about 1 % and settles back
        assert!(widest > 240.0 && widest < 240.0 * 1.015, "{widest}");
        assert!(
            (last.width() - 240.0).abs() < 1.0,
            "close to its size near the end: {last:?}"
        );
        assert_eq!(grow.scale(Timestamp(OPEN_NS)), None, "whole from then on");
    }

    #[test]
    fn under_reduce_motion_it_shows_at_its_size_at_once() {
        assert_eq!(
            Grow::begin(Timestamp(5), Point::new(120.0, 0.0), true),
            None
        );
    }

    #[test]
    fn closing_it_shrinks_into_the_tip() {
        assert_eq!(close_scale(0), 1.0);
        assert!((close_scale(CLOSE_NS / 2) - 0.25).abs() < 1e-12);
        assert_eq!(close_scale(CLOSE_NS), 0.0);
        let (_, _, full) = popover();
        let tip = Point::new(120.0, 0.0);
        let gone = scale_rect(full, tip, close_scale(CLOSE_NS));
        assert_eq!(
            (gone.min_x(), gone.min_y(), gone.width()),
            (120.0, 0.0, 0.0)
        );
    }

    #[test]
    fn a_popover_flipped_above_its_anchor_grows_from_its_bottom_edge() {
        let button = Rect::new(106.0, 400.0, 28.0, 28.0);
        let placed = Rect::new(10.0, 94.0, 240.0, 300.0);
        assert_eq!(origin(button, placed), Point::new(110.0, 300.0));
    }

    #[test]
    fn the_glass_and_what_the_window_draws_scale_together() {
        use cupertino_ui_platform::{GlassRegion, GlassStyle};
        use cupertino_ui_render::{PaintNode, PaintNodeId, PaintRevision};
        let at = Point::new(120.0, 0.0);
        let mut list = DisplayList::default();
        list.nodes.push(PaintNode {
            id: PaintNodeId(1),
            parent: None,
            revision: PaintRevision(1),
            transform: Transform([1.0, 0.0, 0.0, 1.0, 20.0, 40.0]),
            clip: None,
            group: None,
            bounds: Rect::new(20.0, 40.0, 100.0, 20.0),
            hit: Rect::new(20.0, 40.0, 100.0, 20.0),
            ops: Vec::new(),
        });
        let mut glass = vec![MaterialRegion {
            material: "DenseGlass".into(),
            rect: Rect::new(0.0, 10.0, 240.0, 290.0),
            radius: 16.0,
            alpha: 1.0,
            glass: Some(GlassRegion {
                style: GlassStyle::Dense,
                container: 1,
                spacing: 8.0,
            }),
            raised: false,
        }];
        apply(&mut list, &mut glass, at, 0.5);
        assert_eq!(list.nodes[0].transform.0, [0.5, 0.0, 0.0, 0.5, 70.0, 20.0]);
        assert_eq!(list.nodes[0].bounds, Rect::new(70.0, 20.0, 50.0, 10.0));
        assert_eq!(glass[0].rect, Rect::new(60.0, 5.0, 120.0, 145.0));
        assert_eq!(glass[0].radius, 8.0);
        assert_eq!(glass[0].glass.unwrap().spacing, 4.0);
        fade_materials(&mut glass, 0.25);
        assert_eq!(glass[0].alpha, 0.25);
    }
}
