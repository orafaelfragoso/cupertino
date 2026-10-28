//! The pointer: which nodes it hovers and which node captured it. Hover changes reach each node that the pointer
//! entered or left. While a node holds the capture every pointer event goes to it alone, wherever the pointer is;
//! the capture ends when the node releases it or the pointer leaves the surface, which cancels it.

use crate::cx::CaptureRequest;
use crate::route::{hit_test, Geometry, Route};
use crate::{Event, NodeId};
use cupertino_geometry::Point;
use cupertino_ui_platform::{PointerEvent, Timestamp};

#[derive(Default)]
pub(crate) struct Pointer {
    hover: Vec<NodeId>,
    capture: Option<NodeId>,
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

    /// Drops nodes that unmounted.
    pub(crate) fn retain<M: 'static>(&mut self, tree: &crate::tree::Tree<M>) {
        self.hover.retain(|&n| tree.contains(n));
        self.capture = self.capture.filter(|&n| tree.contains(n));
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
