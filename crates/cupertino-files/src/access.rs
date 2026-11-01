//! What a Files window tells assistive technology: the window, named by its folder, and its sidebar. The sidebar is a
//! list with a group per section. A row is a list item named by its place: selected while its folder is shown, focused
//! while it has the keyboard ([`crate::sidebar_keys`]). A removable file system's row is described as "Removable", plus
//! ", not mounted" when it is not mounted, and is busy while a mount or an eject runs; a mounted one holds its eject
//! button, "Eject" and the name. A click on a row opens it as a pointer click does, and a click on an eject button
//! ejects. The folder's items are not in the tree yet.
//!
//! A row's objects are named by its place's key ([`PlaceKeys`]), not by its position, so they stay the same objects
//! while rows come and go around them, and a request to a row that went away finds nothing.

use std::collections::HashMap;
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
/// The row of the place with key `k` is `ROWS + 2 k`, its eject button the id after it.
const ROWS: u32 = 16;

/// The keys of the sidebar's places. A place keeps the key of its identity for as long as the window lives (a folder by
/// its kind and path, a removable file system by its udisks object), and a key is never given to another identity.
#[derive(Debug, Default)]
pub struct PlaceKeys {
    known: HashMap<String, u32>,
    next: u32,
}

impl PlaceKeys {
    pub fn key(&mut self, identity: &str) -> u32 {
        if let Some(key) = self.known.get(identity) {
            return *key;
        }
        let key = self.next;
        self.next += 1;
        self.known.insert(identity.to_owned(), key);
        key
    }
}

/// A sidebar node assistive technology can act on, by its row's index in the places now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidebarNode {
    Row(usize),
    Eject(usize),
}

fn row_id(key: u32) -> NodeId {
    id(ROWS + 2 * key)
}

fn eject_id(key: u32) -> NodeId {
    id(ROWS + 2 * key + 1)
}

/// The row or eject button `id` names among the places whose keys are `keys`; `None` for any other node and for a row
/// no longer listed.
pub fn sidebar_node(keys: &[u32], id: NodeId) -> Option<SidebarNode> {
    if id.generation() != NonZeroU32::MIN {
        return None;
    }
    let offset = id.index().checked_sub(ROWS)?;
    let row = keys.iter().position(|k| *k == offset / 2)?;
    Some(if offset.is_multiple_of(2) {
        SidebarNode::Row(row)
    } else {
        SidebarNode::Eject(row)
    })
}

/// A Files window as the person sees it.
pub struct WindowView<'a> {
    pub title: &'a str,
    pub layout: &'a WindowLayout,
    pub places: &'a [Place],
    /// The key of each place ([`PlaceKeys`]).
    pub keys: &'a [u32],
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
            let row = row_id(view.keys[index]);
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
            .map(|i| row_id(view.keys[i])),
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
        let eject = eject_id(view.keys[index]);
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
    nodes.insert(0, (row_id(view.keys[index]), row));
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
    use crate::geom::Size;
    use crate::places::PlaceGroup;
    use crate::window::Bars;
    use std::path::PathBuf;

    fn home() -> Place {
        Place {
            name: "rafael".into(),
            path: PathBuf::from("/home/rafael"),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Home,
        }
    }

    fn disk(name: &str) -> Place {
        Place::removable(
            name,
            Some(Path::new("/run/media/rafael").join(name).as_path()),
            false,
        )
    }

    /// The node named `name` in the tree of `places` with `keys`.
    fn named(places: &[Place], keys: &[u32], name: &str) -> NodeId {
        let layout = WindowLayout::new(Size { w: 920.0, h: 436.0 }, places, Bars::HIDDEN);
        let tree = window_tree(&WindowView {
            title: "rafael",
            layout: &layout,
            places,
            keys,
            dir: Path::new("/home/rafael"),
            sidebar_focus: None,
        });
        tree.nodes
            .iter()
            .find(|(_, n)| n.name.as_deref() == Some(name))
            .map(|(id, _)| *id)
            .unwrap()
    }

    #[test]
    fn an_eject_button_stays_with_its_disk_when_another_disk_lists_above_it_and_names_nothing_once_it_is_gone(
    ) {
        let mut keys = PlaceKeys::default();
        let mut key_of =
            |identities: &[&str]| -> Vec<u32> { identities.iter().map(|i| keys.key(i)).collect() };
        let before = [home(), disk("Backup Disk")];
        let before_keys = key_of(&["home", "/org/freedesktop/UDisks2/block_devices/sdb1"]);
        let eject = named(&before, &before_keys, "Eject Backup Disk");
        let row = named(&before, &before_keys, "Backup Disk");
        assert_eq!(
            sidebar_node(&before_keys, eject),
            Some(SidebarNode::Eject(1))
        );

        let after = [home(), disk("Camera Card"), disk("Backup Disk")];
        let after_keys = key_of(&[
            "home",
            "/org/freedesktop/UDisks2/block_devices/sdc1",
            "/org/freedesktop/UDisks2/block_devices/sdb1",
        ]);
        assert_eq!(
            sidebar_node(&after_keys, eject),
            Some(SidebarNode::Eject(2))
        );
        assert_eq!(sidebar_node(&after_keys, row), Some(SidebarNode::Row(2)));
        assert_eq!(named(&after, &after_keys, "Eject Backup Disk"), eject);

        let gone_keys = key_of(&["home", "/org/freedesktop/UDisks2/block_devices/sdc1"]);
        assert_eq!(sidebar_node(&gone_keys, eject), None);
        assert_eq!(sidebar_node(&gone_keys, row), None);
        assert_eq!(sidebar_node(&gone_keys, id(ROOT)), None);
        assert_eq!(sidebar_node(&gone_keys, id(LIST)), None);
        assert_eq!(sidebar_node(&gone_keys, id(GROUPS + 1)), None);
    }
}
