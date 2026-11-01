use cupertino_ui_core::{AccessibilityNode, AccessibilityUpdate, NodeId, Relation, Role};
use std::collections::HashMap;

/// Roles assistive technology announces by their contents, so a node of one needs no name of its own.
const NAMED_BY_CONTENTS: [Role; 7] = [
    Role::Group,
    Role::ScrollView,
    Role::ScrollBar,
    Role::Separator,
    Role::Splitter,
    Role::Row,
    Role::TabPanel,
];

/// What a screen reader could not announce in a whole tree, one line per problem in reading order: a node without a
/// name (unless its role is named by its contents or another node labels it) and a child the tree does not hold. A line
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
    let labelled = node
        .relations
        .iter()
        .any(|r| matches!(r, Relation::LabelledBy(_)));
    if !has_name(node) && !labelled && !NAMED_BY_CONTENTS.contains(&node.role) {
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

fn has_name(node: &AccessibilityNode) -> bool {
    node.name.as_deref().is_some_and(|n| !n.trim().is_empty())
}

fn step(node: &AccessibilityNode, index: usize) -> String {
    match &node.name {
        Some(name) if has_name(node) => format!("{:?} {name:?}", node.role),
        _ => format!("{:?} #{index}", node.role),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_geometry::Rect;
    use cupertino_ui_core::AccessibleStates;
    use cupertino_ui_platform::WindowId;

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
}
