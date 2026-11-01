//! What a Files window tells assistive technology: the window, named by its folder, and its sidebar. The sidebar is a
//! list with a group per section. A row is a list item named by its place: selected while its folder is shown, focused
//! while it has the keyboard ([`crate::sidebar_keys`]). A removable file system's row is described as "Removable", plus
//! ", not mounted" when it is not mounted, and is busy while a mount or an eject runs; a mounted one holds its eject
//! button, "Eject" and the name. A click on a row opens it as a pointer click does, and a click on an eject button
//! ejects. The folder's items are not in the tree yet.

use std::num::NonZeroU32;
use std::path::Path;

use cupertino_geometry::Rect as GeoRect;
use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleStates, Action, NodeId, Role,
};
use cupertino_ui_platform::WindowId;

use crate::geom::Rect;
use crate::places::{Place, PlaceKind, Volume};
use crate::window::WindowLayout;

/// The window's one AT-SPI window.
pub const WINDOW: WindowId = WindowId(1);

const ROOT: u32 = 1;
const LIST: u32 = 2;
const GROUPS: u32 = 3;
/// Row `i` is `ROWS + 2 i`, its eject button the id after it.
const ROWS: u32 = 16;

/// A sidebar node assistive technology can act on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarNode {
    Row(usize),
    Eject(usize),
}

impl SidebarNode {
    pub fn id(self) -> NodeId {
        let index = match self {
            SidebarNode::Row(i) => ROWS + 2 * i as u32,
            SidebarNode::Eject(i) => ROWS + 2 * i as u32 + 1,
        };
        id(index)
    }

    /// The node `id` names, when it is a row or an eject button.
    pub fn of(id: NodeId) -> Option<SidebarNode> {
        if id.generation() != NonZeroU32::MIN {
            return None;
        }
        let offset = id.index().checked_sub(ROWS)? as usize;
        Some(if offset.is_multiple_of(2) {
            SidebarNode::Row(offset / 2)
        } else {
            SidebarNode::Eject(offset / 2)
        })
    }
}

/// A Files window as the person sees it.
pub struct WindowView<'a> {
    pub title: &'a str,
    pub layout: &'a WindowLayout,
    pub places: &'a [Place],
    /// The folder shown.
    pub dir: &'a Path,
    /// The sidebar row with the keyboard focus.
    pub sidebar_focus: Option<usize>,
}

/// The whole tree of the window.
pub fn window_tree(view: &WindowView<'_>) -> AccessibilityUpdate {
    let layout = view.layout;
    let mut nodes = Vec::new();
    let mut groups = Vec::new();
    for (g, (group, heading)) in layout.groups.iter().enumerate() {
        let mut rows = Vec::new();
        let mut bounds = *heading;
        let in_group = view
            .places
            .iter()
            .zip(&layout.places)
            .enumerate()
            .filter(|(_, (p, _))| p.group == *group);
        for (index, (place, frame)) in in_group {
            bounds = union(bounds, *frame);
            let row = SidebarNode::Row(index).id();
            rows.push(row);
            nodes.extend(row_nodes(view, index, place, *frame));
        }
        let group_id = id(GROUPS + g as u32);
        groups.push(group_id);
        nodes.push((
            group_id,
            AccessibilityNode {
                children: rows,
                ..node(Role::Group, Some(group.title()), bounds)
            },
        ));
    }
    let list = AccessibilityNode {
        children: groups,
        item_count: Some(view.places.len().min(layout.places.len())),
        ..node(Role::List, Some("Sidebar"), layout.sidebar)
    };
    let root = AccessibilityNode {
        children: vec![id(LIST)],
        ..node(
            Role::Window,
            Some(view.title),
            Rect::new(0.0, 0.0, layout.size.w, layout.size.h),
        )
    };
    nodes.insert(0, (id(LIST), list));
    nodes.insert(0, (id(ROOT), root));
    AccessibilityUpdate {
        window: WINDOW,
        root: id(ROOT),
        focus: view
            .sidebar_focus
            .filter(|i| *i < layout.places.len())
            .map(|i| SidebarNode::Row(i).id()),
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

/// Row `index` and, for a mounted removable file system, its eject button.
fn row_nodes(
    view: &WindowView<'_>,
    index: usize,
    place: &Place,
    frame: Rect,
) -> Vec<(NodeId, AccessibilityNode)> {
    let mut nodes = Vec::new();
    let mut children = Vec::new();
    if let Some((_, target)) = view.layout.ejects.iter().find(|(i, _)| *i == index) {
        let eject = SidebarNode::Eject(index).id();
        children.push(eject);
        nodes.push((
            eject,
            AccessibilityNode {
                actions: vec![Action::Click],
                ..node(
                    Role::Button,
                    Some(&format!("Eject {}", place.name)),
                    *target,
                )
            },
        ));
    }
    let description = match place.kind {
        PlaceKind::Removable(Volume::Unmounted) => Some("Removable, not mounted"),
        PlaceKind::Removable(_) => Some("Removable"),
        _ => None,
    };
    let row = AccessibilityNode {
        description: description.map(str::to_owned),
        states: AccessibleStates {
            focusable: true,
            focused: view.sidebar_focus == Some(index),
            selected: place.path == view.dir,
            busy: place.kind == PlaceKind::Removable(Volume::Busy),
            ..AccessibleStates::default()
        },
        actions: vec![Action::Click, Action::Focus],
        children,
        item_index: Some(index),
        ..node(Role::ListItem, Some(&place.name), frame)
    };
    nodes.insert(0, (SidebarNode::Row(index).id(), row));
    nodes
}

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::MIN)
}

fn union(a: Rect, b: Rect) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(
        x,
        y,
        (a.x + a.w).max(b.x + b.w) - x,
        (a.y + a.h).max(b.y + b.h) - y,
    )
}

fn node(role: Role, name: Option<&str>, bounds: Rect) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: name.map(str::to_owned),
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds: GeoRect::new(bounds.x, bounds.y, bounds.w, bounds.h),
        actions: Vec::new(),
        relations: Vec::new(),
        children: Vec::new(),
        live: None,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_and_eject_ids_name_their_node_and_no_other() {
        for node in [
            SidebarNode::Row(0),
            SidebarNode::Eject(0),
            SidebarNode::Row(7),
            SidebarNode::Eject(7),
        ] {
            assert_eq!(SidebarNode::of(node.id()), Some(node));
        }
        assert_eq!(SidebarNode::of(id(ROOT)), None);
        assert_eq!(SidebarNode::of(id(LIST)), None);
        assert_eq!(SidebarNode::of(id(GROUPS + 1)), None);
    }
}
