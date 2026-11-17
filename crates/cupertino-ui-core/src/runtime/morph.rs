//! A popover grows out of what it opened from ([`Widget::grows_from_anchor`](crate::Widget::grows_from_anchor)). As it
//! opens, the window and its glass start at a tenth of their size about the point of the anchor nearest to the popover
//! (the tip of its pointer) and spring out to their full size; as it closes they shrink back into that point and fade.
//! The window keeps its size and its layout throughout: what it draws and the glass regions it asks for are scaled
//! about that point frame by frame. Under Reduce Motion a popover shows at its full size at once and goes at once.

use super::Runtime;
use crate::platform::{MaterialRegion, Timestamp, WindowRequest, WindowSystem};
use crate::{Application, PopupAnchor};
use cupertino_geometry::{Point, Rect};
use cupertino_ui_render::{DisplayList, Layer, Transform};

/// The scale a popover starts at as it opens.
pub(crate) const OPEN_START_SCALE: f64 = 0.10;
/// The response of the spring a popover opens on: the period of its undamped swing, in seconds.
pub(crate) const OPEN_RESPONSE: f64 = 0.29;
/// The spring's damping ratio: a little under critical, so the popover passes its size by about 1 % and comes back.
pub(crate) const OPEN_DAMPING: f64 = 0.82;
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
        // a root's subtree is drawn at a transform that changes from frame to frame
        if node.parent.is_none() && !matches!(node.layer, Some(Layer::Scroll { .. })) {
            node.layer = Some(Layer::Transform);
        }
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
        let parent = self.windows.iter().find(|p| p.key == popup.parent);
        let placed = parent
            .and_then(|p| p.popups.iter().find(|(k, _)| *k == w.key))
            .map(|(_, rect)| *rect);
        // until the compositor says where it put the popover, the middle of its top edge, under its anchor
        let at = match (parent, placed) {
            (Some(parent), Some(placed)) => origin(
                super::grows_out_of(&self.tree, parent, &popup.anchor, &popup.placed),
                placed,
            ),
            _ => Point::new(w.size.width / 2.0, 0.0),
        };
        let reduce_motion = w.env.theme.preferences.reduce_motion;
        let (id, size) = (w.id, w.size);
        // a compositor that presents popups grows it on its clock, on the same spring: no frame is drawn for it
        let handed = !reduce_motion
            && self.system.capabilities().present_popups
            && self
                .system
                .request(id, WindowRequest::Present(super::handover::grow(at, size)))
                .is_ok();
        let w = &mut self.windows[i];
        w.grows_from = Some(at);
        w.grow = if handed {
            None
        } else {
            Grow::begin(now, at, reduce_motion)
        };
    }

    /// A popover that grows out of a node of its parent other than its anchor ([`PopupAnchor::Panel`]) follows that
    /// node, so it shrinks back into wherever the node is when it closes. The node is looked for in the parent's tree
    /// only after the parent was laid out again, or the popover placed or asked to be elsewhere.
    pub(super) fn follow_grow_origins(&mut self) {
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            let (Some(popup), Some(_)) = (&w.popup, w.grows_from) else {
                continue;
            };
            if w.fade.is_some() || !matches!(popup.anchor, PopupAnchor::Panel { .. }) {
                continue;
            }
            let Some(parent) = self.windows.iter().find(|p| p.key == popup.parent) else {
                continue;
            };
            let Some(placed) = parent
                .popups
                .iter()
                .find(|(k, _)| *k == w.key)
                .map(|(_, rect)| *rect)
            else {
                continue;
            };
            let followed = (parent.layouts, placed, popup.placed.clone());
            if w.grow_followed.as_ref() == Some(&followed) {
                continue;
            }
            let at = origin(
                super::grows_out_of(&self.tree, parent, &popup.anchor, &popup.placed),
                placed,
            );
            let w = &mut self.windows[i];
            w.grows_from = Some(at);
            w.grow_followed = Some(followed);
        }
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
