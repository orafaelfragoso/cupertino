use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleValue, Checked, NodeId, Relation, Role,
};
use cupertino_ui_platform::WindowId;
use std::collections::{HashMap, HashSet};

/// Roles assistive technology announces by their contents, so a node of one needs no name of its own.
pub(crate) const NAMED_BY_CONTENTS: [Role; 8] = [
    Role::Group,
    Role::Toolbar,
    Role::ScrollView,
    Role::ScrollBar,
    Role::Separator,
    Role::Splitter,
    Role::Row,
    Role::TabPanel,
];

/// What a screen reader could not announce in a whole tree, one line per problem in reading order: a node without a
/// name (unless its role is named by its contents, another node labels it, or it is a list item, tree item, cell or
/// tab holding a named child) and a child the tree does not hold. A line
/// names the node by its path from the root, each step its role and name or, unnamed, its place among its siblings:
/// `Window "Downloads" > List "Sidebar" > ListItem #2: no name`. Every node has a role by type.
pub fn semantic_gaps(update: &AccessibilityUpdate) -> Vec<String> {
    let nodes: HashMap<NodeId, &AccessibilityNode> =
        update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let mut gaps = Vec::new();
    match nodes.get(&update.root) {
        Some(root) => walk(&nodes, root, step(root, 0), &mut gaps),
        None => gaps.push(format!("root {:?}: not in the tree", update.root)),
    }
    gaps
}

fn walk(
    nodes: &HashMap<NodeId, &AccessibilityNode>,
    node: &AccessibilityNode,
    path: String,
    gaps: &mut Vec<String>,
) {
    if !named(nodes, node) && !NAMED_BY_CONTENTS.contains(&node.role) {
        gaps.push(format!("{path}: no name"));
    }
    for (index, id) in node.children.iter().enumerate() {
        match nodes.get(id) {
            Some(child) => walk(
                nodes,
                child,
                format!("{path} > {}", step(child, index)),
                gaps,
            ),
            None => gaps.push(format!("{path} > #{index}: child {id:?} not in the tree")),
        }
    }
}

/// Items a screen reader names by the text they hold when they have no name of their own, as the AT-SPI bridge
/// labels them by their children.
const NAMED_BY_CHILDREN: [Role; 4] = [Role::ListItem, Role::TreeItem, Role::Cell, Role::Tab];

/// Whether a screen reader has a name to read for `node`: its own, another node that labels it, or for an item of
/// [`NAMED_BY_CHILDREN`] a child of its that has a name.
pub(crate) fn named(nodes: &HashMap<NodeId, &AccessibilityNode>, node: &AccessibilityNode) -> bool {
    has_name(node)
        || labelled(node)
        || (node.name.is_none()
            && NAMED_BY_CHILDREN.contains(&node.role)
            && node
                .children
                .iter()
                .any(|c| nodes.get(c).is_some_and(|c| has_name(c))))
}

/// Whether another node of the tree labels this one.
pub(crate) fn labelled(node: &AccessibilityNode) -> bool {
    node.relations
        .iter()
        .any(|r| matches!(r, Relation::LabelledBy(_)))
}

pub(crate) fn has_name(node: &AccessibilityNode) -> bool {
    node.name.as_deref().is_some_and(|n| !n.trim().is_empty())
}

pub(crate) fn step(node: &AccessibilityNode, index: usize) -> String {
    match &node.name {
        Some(name) if has_name(node) => format!("{:?} {name:?}", node.role),
        _ => format!("{:?} #{index}", node.role),
    }
}

/// The whole tree of `window` from its `nodes` in any order, as a full update: the root is the node no other node lists
/// as a child (the first such node, if the nodes hold more than one tree). A runtime's snapshot hands its nodes this
/// way.
pub fn whole_tree(
    window: WindowId,
    nodes: Vec<(NodeId, AccessibilityNode)>,
    focus: Option<NodeId>,
) -> AccessibilityUpdate {
    let children: HashSet<NodeId> = nodes
        .iter()
        .flat_map(|(_, n)| n.children.iter().copied())
        .collect();
    let root = nodes
        .iter()
        .map(|(id, _)| *id)
        .find(|id| !children.contains(id))
        .or_else(|| nodes.first().map(|(id, _)| *id))
        .expect("a window's tree has a node");
    AccessibilityUpdate {
        window,
        root,
        focus,
        nodes,
        removed: Vec::new(),
        full: true,
    }
}

/// What a screen reader announces of each node of a whole tree, one line a node in reading order, indented two spaces
/// a level below the root: the node's [`describe`]d role, name, value and states. A child the tree does not hold is a
/// line of its own, `? NodeId(..) not in the tree`. A test compares the lines it cares about, trimmed, and prints the
/// whole outline when one is missing.
pub fn semantic_outline(update: &AccessibilityUpdate) -> Vec<String> {
    let nodes: HashMap<NodeId, &AccessibilityNode> =
        update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let mut lines = Vec::new();
    let mut stack = vec![(update.root, 0)];
    while let Some((id, depth)) = stack.pop() {
        let indent = "  ".repeat(depth);
        match nodes.get(&id) {
            Some(node) => {
                lines.push(format!("{indent}{}", describe(node)));
                stack.extend(node.children.iter().rev().map(|c| (*c, depth + 1)));
            }
            None => lines.push(format!("{indent}? {id:?} not in the tree")),
        }
    }
    lines
}

/// The `expected` [`describe`]d lines no node of the tree reads as, in their order: what a screen reader would miss.
/// Each line is compared with the outline's lines trimmed, so it names the node, not where it lies.
pub fn unannounced<'a>(update: &AccessibilityUpdate, expected: &[&'a str]) -> Vec<&'a str> {
    let outline = semantic_outline(update);
    expected
        .iter()
        .copied()
        .filter(|line| !outline.iter().any(|l| l.trim_start() == *line))
        .collect()
}

/// One node as a screen reader announces it: its role, its name in quotes when it has one, its value, then each state
/// that is set, in a fixed order: `Switch "Wi-Fi" on focusable`, `Slider "Volume" = 0.5 of 0..1`,
/// `ListItem "Home" focusable selected`, `List "Sidebar" 4 items`. A toggle's value reads `on`, `off` or `mixed`; a
/// protected value `= [protected]`, never the secret.
pub fn describe(node: &AccessibilityNode) -> String {
    let mut line = format!("{:?}", node.role);
    if let Some(name) = node.name.as_deref().filter(|_| has_name(node)) {
        line.push_str(&format!(" {name:?}"));
    }
    match &node.value {
        None => {}
        Some(AccessibleValue::Text { value, .. }) => line.push_str(&format!(" = {value:?}")),
        Some(AccessibleValue::Number {
            value, min, max, ..
        }) => line.push_str(&format!(" = {value} of {min}..{max}")),
        Some(AccessibleValue::Toggle(checked)) => line.push_str(match checked {
            Checked::True => " on",
            Checked::False => " off",
            Checked::Mixed => " mixed",
        }),
        Some(AccessibleValue::Protected) => line.push_str(" = [protected]"),
    }
    if let Some(count) = node.item_count {
        line.push_str(&format!(" {count} items"));
    }
    let s = &node.states;
    let states = [
        (s.disabled, "disabled"),
        (s.focusable, "focusable"),
        (s.focused, "focused"),
        (s.selected, "selected"),
        (s.expanded == Some(true), "expanded"),
        (s.expanded == Some(false), "collapsed"),
        (s.busy, "busy"),
        (s.read_only, "read-only"),
        (s.required, "required"),
        (s.invalid, "invalid"),
        (s.modal, "modal"),
        (s.multiselectable, "multiselectable"),
    ];
    for (_, state) in states.iter().filter(|(set, _)| *set) {
        line.push(' ');
        line.push_str(state);
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Rect;
    use cupertino_ui_core::AccessibleStates;

    fn id(n: u32) -> NodeId {
        NodeId::new(n, std::num::NonZeroU32::MIN)
    }

    fn node(role: Role, name: Option<&str>, children: &[u32]) -> AccessibilityNode {
        AccessibilityNode {
            role,
            name: name.map(str::to_owned),
            description: None,
            value: None,
            states: AccessibleStates::default(),
            bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
            actions: Vec::new(),
            relations: Vec::new(),
            children: children.iter().copied().map(id).collect(),
            live: None,
            item_count: None,
            level: None,
            item_index: None,
            column_index: None,
            sort: None,
        }
    }

    fn tree(nodes: Vec<(u32, AccessibilityNode)>) -> AccessibilityUpdate {
        AccessibilityUpdate {
            window: WindowId(1),
            root: id(1),
            focus: None,
            nodes: nodes.into_iter().map(|(n, node)| (id(n), node)).collect(),
            removed: Vec::new(),
            full: true,
        }
    }

    #[test]
    fn a_tree_whose_nodes_are_all_named_has_no_gaps() {
        let t = tree(vec![
            (1, node(Role::Window, Some("Downloads"), &[2])),
            (2, node(Role::Group, None, &[3])),
            (3, node(Role::Button, Some("Eject Backup"), &[])),
        ]);
        assert_eq!(semantic_gaps(&t), Vec::<String>::new());
    }

    #[test]
    fn an_unnamed_or_blank_control_and_a_missing_child_are_listed_by_their_path() {
        let t = tree(vec![
            (1, node(Role::Window, Some("Downloads"), &[2, 4])),
            (2, node(Role::List, Some("Sidebar"), &[3, 5, 9])),
            (3, node(Role::ListItem, Some("Home"), &[])),
            (4, node(Role::Button, Some("  "), &[])),
            (5, node(Role::ListItem, None, &[])),
        ]);
        assert_eq!(
            semantic_gaps(&t),
            [
                "Window \"Downloads\" > List \"Sidebar\" > ListItem #1: no name",
                &format!(
                    "Window \"Downloads\" > List \"Sidebar\" > #2: child {:?} not in the tree",
                    id(9)
                ),
                "Window \"Downloads\" > Button #1: no name",
            ]
        );
    }

    #[test]
    fn a_node_another_node_labels_needs_no_name_of_its_own() {
        let mut field = node(Role::TextInput, None, &[]);
        field.relations.push(Relation::LabelledBy(id(2)));
        let t = tree(vec![
            (1, node(Role::Window, Some("Wi-Fi"), &[2, 3])),
            (2, node(Role::StaticText, Some("Password"), &[])),
            (3, field),
        ]);
        assert_eq!(semantic_gaps(&t), Vec::<String>::new());
    }

    #[test]
    fn the_outline_reads_roles_names_values_and_states_in_reading_order() {
        let mut list = node(Role::List, Some("Sidebar"), &[3, 9]);
        list.item_count = Some(2);
        let mut home = node(Role::ListItem, Some("Home"), &[]);
        home.states.focusable = true;
        home.states.selected = true;
        let mut wifi = node(Role::Switch, Some("Wi-Fi"), &[]);
        wifi.value = Some(AccessibleValue::Toggle(Checked::True));
        wifi.states.disabled = true;
        let mut volume = node(Role::Slider, Some("Volume"), &[]);
        volume.value = Some(AccessibleValue::Number {
            value: 0.5,
            min: 0.0,
            max: 1.0,
            step: None,
        });
        let mut secure_field = node(Role::PasswordInput, Some("Password"), &[]);
        secure_field.value = Some(AccessibleValue::Protected);
        secure_field.states.focused = true;
        let t = tree(vec![
            (1, node(Role::Window, Some("Downloads"), &[2, 4, 5, 6])),
            (2, list),
            (3, home),
            (4, wifi),
            (5, volume),
            (6, secure_field),
        ]);
        assert_eq!(
            semantic_outline(&t),
            [
                "Window \"Downloads\"".to_owned(),
                "  List \"Sidebar\" 2 items".to_owned(),
                "    ListItem \"Home\" focusable selected".to_owned(),
                format!("    ? {:?} not in the tree", id(9)),
                "  Switch \"Wi-Fi\" on disabled".to_owned(),
                "  Slider \"Volume\" = 0.5 of 0..1".to_owned(),
                "  PasswordInput \"Password\" = [protected] focused".to_owned(),
            ]
        );
    }

    #[test]
    fn unannounced_lists_the_expected_lines_no_node_reads_as() {
        let t = tree(vec![
            (1, node(Role::Window, Some("Downloads"), &[2])),
            (2, node(Role::Button, Some("Eject"), &[])),
        ]);
        assert_eq!(
            unannounced(
                &t,
                &["Button \"Eject\"", "Button \"Eject\" focusable", "Window"]
            ),
            ["Button \"Eject\" focusable", "Window"]
        );
    }

    #[test]
    fn the_whole_tree_is_rooted_at_the_node_no_other_node_holds() {
        let nodes = vec![
            (id(3), node(Role::Button, Some("Eject"), &[])),
            (id(1), node(Role::Window, Some("Downloads"), &[2])),
            (id(2), node(Role::Group, None, &[3])),
        ];
        let t = whole_tree(WindowId(1), nodes, Some(id(3)));
        assert_eq!((t.root, t.focus, t.full), (id(1), Some(id(3)), true));
    }
}
