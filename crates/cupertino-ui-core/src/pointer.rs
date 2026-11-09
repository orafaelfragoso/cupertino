//! The pointer: which nodes it hovers and which node captured it. Hover changes reach each node that the pointer
//! entered or left. While a node holds the capture every pointer event goes to it alone, wherever the pointer is;
//! the capture ends when the node releases it or the pointer leaves the surface, which cancels it.
//!
//! A scroll gesture keeps the target its first event hit: its later events go along the same path even when the
//! content under the pointer moved (an inner scroll view scrolled under it). A finger's gesture ends with its stop; a
//! wheel's or a continuous scroll's after [`SCROLL_IDLE_NS`] without an event, a pointer motion or another source.
//! The kinetic motion a gesture leaves belongs to the view that took it and is not routed at all.

use crate::cx::CaptureRequest;
use crate::route::{hit_test, Geometry, Route};
use crate::{Event, NodeId};
use cupertino_geometry::Point;
use cupertino_ui_platform::{PointerEvent, ScrollSource, Timestamp};

/// A wheel or continuous scroll ends after this long without an event, in nanoseconds.
pub const SCROLL_IDLE_NS: u64 = 300_000_000;

#[derive(Default)]
pub(crate) struct Pointer {
    hover: Vec<NodeId>,
    capture: Option<NodeId>,
    scroll: Option<ScrollLatch>,
}

/// The scroll gesture in progress and the path its first event hit.
struct ScrollLatch {
    path: Vec<NodeId>,
    source: ScrollSource,
    /// The last event's time.
    last: Timestamp,
}

impl ScrollLatch {
    /// Whether an event of `source` at `time` goes on with this gesture.
    fn continues(&self, source: ScrollSource, time: Timestamp) -> bool {
        source == self.source
            && (source == ScrollSource::Finger
                || time.0.saturating_sub(self.last.0) < SCROLL_IDLE_NS)
    }
}

impl Pointer {
    pub(crate) fn capture(&self) -> Option<NodeId> {
        self.capture
    }

    pub(crate) fn handle<M: 'static>(
        &mut self,
        route: &mut Route<'_, M>,
        root: NodeId,
        geometry: &Geometry,
        event: PointerEvent,
        time: Timestamp,
    ) {
        let position = match event {
            PointerEvent::Leave => {
                self.scroll = None;
                self.hover_path(route, Vec::new(), None, time);
                self.revoke(route);
                return;
            }
            PointerEvent::Enter { position }
            | PointerEvent::Motion { position }
            | PointerEvent::Button { position, .. }
            | PointerEvent::Scroll { position, .. } => position,
        };
        let path = hit_test(route.tree, root, geometry, position).unwrap_or_default();
        self.hover_path(route, path.clone(), Some(position), time);
        if matches!(event, PointerEvent::Enter { .. }) {
            return;
        }
        let path = match event {
            PointerEvent::Scroll { source, stop, .. } => self.scroll_path(path, source, stop, time),
            PointerEvent::Motion { .. } => {
                // a moved mouse aims its wheel anew; a finger's gesture goes on to its stop
                self.scroll.take_if(|s| s.source != ScrollSource::Finger);
                path
            }
            _ => path,
        };
        route.status.capture = self.capture;
        let event = Event::Pointer(event, time);
        match self.capture {
            Some(node) => route.notify(node, &event),
            None => {
                route.dispatch(&path, &event);
            }
        }
        match route.effects.capture.take() {
            Some(CaptureRequest::Capture(node)) => self.capture = Some(node),
            Some(CaptureRequest::Release) => self.capture = None,
            None => {}
        }
    }

    /// Ends a capture without effect: the node gets [`Event::Cancel`].
    pub(crate) fn revoke<M: 'static>(&mut self, route: &mut Route<'_, M>) {
        if let Some(node) = self.capture.take() {
            route.status.capture = None;
            route.notify(node, &Event::Cancel);
        }
    }

    /// The path a scroll event of `source` at `time` goes along: the gesture's, or `hit` for an event that starts one.
    /// A finger's stop, and a continuous scroll's, ends the gesture after this event.
    fn scroll_path(
        &mut self,
        hit: Vec<NodeId>,
        source: ScrollSource,
        stop: bool,
        time: Timestamp,
    ) -> Vec<NodeId> {
        let path = match self.scroll.take() {
            Some(latch) if latch.continues(source, time) && !latch.path.is_empty() => latch.path,
            _ => hit,
        };
        let ends = stop && matches!(source, ScrollSource::Finger | ScrollSource::Continuous);
        if !ends {
            self.scroll = Some(ScrollLatch {
                path: path.clone(),
                source,
                last: time,
            });
        }
        path
    }

    /// Drops nodes that unmounted.
    pub(crate) fn retain<M: 'static>(&mut self, tree: &crate::tree::Tree<M>) {
        self.hover.retain(|&n| tree.contains(n));
        self.capture = self.capture.filter(|&n| tree.contains(n));
        if let Some(scroll) = &mut self.scroll {
            // the gesture goes on to the deepest node still mounted
            let kept = scroll
                .path
                .iter()
                .take_while(|&&n| tree.contains(n))
                .count();
            scroll.path.truncate(kept);
        }
    }

    fn hover_path<M: 'static>(
        &mut self,
        route: &mut Route<'_, M>,
        path: Vec<NodeId>,
        position: Option<Point>,
        time: Timestamp,
    ) {
        let common = self
            .hover
            .iter()
            .zip(&path)
            .take_while(|(a, b)| a == b)
            .count();
        for &left in self.hover[common..].iter().rev() {
            route.notify(left, &Event::Pointer(PointerEvent::Leave, time));
        }
        if let Some(position) = position {
            for &entered in &path[common..] {
                route.notify(
                    entered,
                    &Event::Pointer(PointerEvent::Enter { position }, time),
                );
            }
        }
        self.hover = path;
    }
}

#[cfg(test)]
mod tests;
