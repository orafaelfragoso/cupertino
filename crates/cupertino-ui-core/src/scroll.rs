//! What scroll containers hear after layout: their geometry when it changed, and the offset that shows a node that
//! keyboard or programmatic focus moved to. The offset lives in the application's model, so a container passes both
//! on as a message and the next view moves the content.

use crate::route::Geometry;
use crate::tree::Tree;
use crate::{Event, NodeId};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_layout::{ContainerKind, LayoutDirection};
use cupertino_ui_platform::ScrollBars;
use std::cell::Cell;
use std::collections::HashMap;

thread_local! {
    static SESSION_SCROLL_BARS: Cell<Option<ScrollBars>> = const { Cell::new(None) };
}

/// How the session wants scroll views to show their bars, as the runtime on this thread last heard from its window
/// system; `None` until it does, and with a window system that can't tell, when each view keeps its own style. A
/// view built from the model reads it, and the runtime builds every window again when it changes.
pub fn session_scroll_bars() -> Option<ScrollBars> {
    SESSION_SCROLL_BARS.get()
}

/// Whether `bars` differs from what [`session_scroll_bars`] said before.
pub(crate) fn set_session_scroll_bars(bars: Option<ScrollBars>) -> bool {
    SESSION_SCROLL_BARS.replace(bars) != bars
}

/// A scroll container as laid out.
struct Scroller {
    horizontal: bool,
    vertical: bool,
    /// In window coordinates.
    viewport: Rect,
    /// Zero along an axis the container does not scroll.
    offset: Point,
    content: Size,
}

impl Scroller {
    fn max_offset(&self) -> Point {
        Point::new(
            (self.content.width - self.viewport.size.width).max(0.0),
            (self.content.height - self.viewport.size.height).max(0.0),
        )
    }
}

fn scroller<M: 'static>(
    tree: &Tree<M>,
    geometry: &Geometry,
    id: NodeId,
    direction: LayoutDirection,
) -> Option<Scroller> {
    let node = tree.get(id)?;
    let ContainerKind::Scroll {
        horizontal,
        vertical,
    } = node.spec.kind
    else {
        return None;
    };
    let viewport = geometry.get(&id)?.rect;
    let scrolled = node.widget.scroll_offset(&*node.state);
    let offset = Point::new(
        if horizontal { scrolled.x } else { 0.0 },
        if vertical { scrolled.y } else { 0.0 },
    );
    // the content's start corner, where an offset of zero puts it
    let start_y = viewport.origin.y - offset.y;
    let mut content = Size::new(0.0, 0.0);
    for child in &node.children {
        let Some(rect) = geometry.get(child).map(|p| p.rect) else {
            continue;
        };
        let width = match direction {
            LayoutDirection::Ltr => rect.max_x() - (viewport.origin.x - offset.x),
            LayoutDirection::Rtl => viewport.max_x() + offset.x - rect.origin.x,
        };
        content = Size::new(
            content.width.max(width),
            content.height.max(rect.max_y() - start_y),
        );
    }
    Some(Scroller {
        horizontal,
        vertical,
        viewport,
        offset,
        content,
    })
}

/// Whether `id` is a scroll container whose content passes its viewport along an axis it scrolls.
pub(crate) fn can_scroll<M: 'static>(
    tree: &Tree<M>,
    geometry: &Geometry,
    id: NodeId,
    direction: LayoutDirection,
) -> bool {
    scroller(tree, geometry, id, direction).is_some_and(|s| s.max_offset() != Point::ZERO)
}

/// [`Event::ScrollGeometry`] for each scroll container under `root` whose viewport or content changed since `heard`,
/// or that `heard` has not seen; `heard` is brought up to date and forgets containers that are gone.
pub(crate) fn geometry_events<M: 'static>(
    tree: &Tree<M>,
    root: NodeId,
    geometry: &Geometry,
    direction: LayoutDirection,
    heard: &mut HashMap<NodeId, (Size, Size)>,
) -> Vec<(NodeId, Event)> {
    let mut events = Vec::new();
    let mut seen = HashMap::with_capacity(heard.len());
    for id in tree.preorder(root) {
        let Some(s) = scroller(tree, geometry, id, direction) else {
            continue;
        };
        let now = (s.viewport.size, s.content);
        if heard.get(&id) != Some(&now) {
            events.push((
                id,
                Event::ScrollGeometry {
                    viewport: now.0,
                    content: now.1,
                },
            ));
        }
        seen.insert(id, now);
    }
    *heard = seen;
    events
}

/// [`Event::Reveal`] for each scroll container around `target` that has to move to show it, or `part` of it (in its
/// coordinates), innermost first. Each outer container shows what of the target its inner one will show once moved.
/// What the window's `raised` nodes cover of a viewport does not show it: the container shows it below them.
pub(crate) fn reveal_events<M: 'static>(
    tree: &Tree<M>,
    geometry: &Geometry,
    raised: &[(NodeId, Rect)],
    direction: LayoutDirection,
    target: NodeId,
    part: Option<Rect>,
) -> Vec<(NodeId, Event)> {
    let mut events = Vec::new();
    let Some(bounds) = geometry.get(&target).map(|p| p.rect) else {
        return events;
    };
    let mut rect = part.map_or(bounds, |part| {
        part.translate(bounds.origin.x, bounds.origin.y)
    });
    for id in tree.path(target).into_iter().rev().skip(1) {
        let Some(mut s) = scroller(tree, geometry, id, direction) else {
            continue;
        };
        let max = s.max_offset();
        // a container inside a raised node lies over what is raised, not under it
        if !raised.iter().any(|(r, _)| tree.path(id).contains(r)) {
            s.viewport = crate::paint::unobscured(s.viewport, raised);
        }
        let mut next = s.offset;
        // how far the content moves on screen
        let mut moved = Point::new(0.0, 0.0);
        if s.vertical {
            let d = reveal_delta(
                rect.origin.y,
                rect.size.height,
                s.viewport.origin.y,
                s.viewport.size.height,
            );
            next.y = (s.offset.y + d).clamp(0.0, max.y);
            moved.y = s.offset.y - next.y;
        }
        if s.horizontal {
            let d = reveal_delta(
                rect.origin.x,
                rect.size.width,
                s.viewport.origin.x,
                s.viewport.size.width,
            );
            match direction {
                LayoutDirection::Ltr => {
                    next.x = (s.offset.x + d).clamp(0.0, max.x);
                    moved.x = s.offset.x - next.x;
                }
                LayoutDirection::Rtl => {
                    next.x = (s.offset.x - d).clamp(0.0, max.x);
                    moved.x = next.x - s.offset.x;
                }
            }
        }
        if next != s.offset {
            events.push((id, Event::Reveal(next)));
        }
        match rect.translate(moved.x, moved.y).intersection(&s.viewport) {
            Some(shown) => rect = shown,
            None => break,
        }
    }
    events
}

/// How far the content has to move toward its end along one axis so `start..start + len` shows in the viewport: its
/// start edge first when it does not fit.
fn reveal_delta(start: f64, len: f64, view_start: f64, view_len: f64) -> f64 {
    if start < view_start || len > view_len {
        start - view_start
    } else {
        (start + len - (view_start + view_len)).max(0.0)
    }
}
