use crate::semantics::{has_name, labelled, step, NAMED_BY_CONTENTS};
use cupertino_geometry::Rect;
use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleValue, Action, NodeId, Relation, Role,
};
use std::collections::{HashMap, HashSet};

/// The characters a secure field paints in place of each character of its secret: a run of them in the tree would
/// tell a listener the secret's length.
const MASKS: [char; 3] = ['•', '●', '∙'];

/// The shortest run of mask characters the audit takes for a masked secret rather than a separator.
const MASK_RUN: usize = 2;

/// Actions that make a node a control a person operates, not merely something focus can land on.
const OPERATIONS: [Action; 7] = [
    Action::Click,
    Action::Increment,
    Action::Decrement,
    Action::Expand,
    Action::Collapse,
    Action::ShowMenu,
    Action::SetValue,
];

/// Roles that only hold or show other things: a node of one that a person can click reads as no control at all.
const NOT_CONTROLS: [Role; 13] = [
    Role::Window,
    Role::Dialog,
    Role::AlertDialog,
    Role::StaticText,
    Role::Image,
    Role::Separator,
    Role::Tooltip,
    Role::List,
    Role::Table,
    Role::Tree,
    Role::TabList,
    Role::MenuBar,
    Role::RadioGroup,
];

/// Roles whose value is on, off or mixed.
const TOGGLES: [Role; 5] = [
    Role::Switch,
    Role::CheckBox,
    Role::RadioButton,
    Role::MenuItemCheckBox,
    Role::MenuItemRadio,
];

/// Items whose place in their list a screen reader reads out ("3 of 7"), and the containers that hold them.
const ITEMS: [(Role, &[Role]); 6] = [
    (Role::ListItem, &[Role::List]),
    (Role::MenuItem, &[Role::Menu, Role::MenuBar]),
    (Role::MenuItemCheckBox, &[Role::Menu, Role::MenuBar]),
    (Role::MenuItemRadio, &[Role::Menu, Role::MenuBar]),
    (Role::TreeItem, &[Role::Tree, Role::TreeItem]),
    (Role::Tab, &[Role::TabList]),
];

/// Everything in a whole tree that a screen reader would announce wrongly or not at all, one line a problem in reading
/// order, each naming its node by path as [`semantic_gaps`](crate::semantic_gaps) does. Beyond the gaps it lists:
///
/// - a control (a node focus can reach or that takes a click, a step, a menu or a value) without a name, and one whose
///   role says it is no control (static text, an image, a container);
/// - a toggle without an on, off or mixed value, a slider or spin button without a number in its range, a progress
///   indicator whose number is out of its range, and a pop-up button that does not say whether its menu is open;
/// - a password field whose value is its text, and any name, description or value holding a run of mask
///   characters, which tells the secret's length;
/// - a focus the tree does not hold or that cannot take focus, and a node that says it is focused when it is not;
/// - a relation to a node the tree does not hold;
/// - a list, menu, tree or tab item outside a container of its kind, without its place, or whose container does not
///   say how many items it holds; and a place at or beyond that count, or held by two items of one container;
/// - a control with no area on screen, which a screen reader cannot show or click.
pub fn accessibility_audit(update: &AccessibilityUpdate) -> Vec<String> {
    let nodes: HashMap<NodeId, &AccessibilityNode> =
        update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let mut problems = Vec::new();
    let Some(root) = nodes.get(&update.root) else {
        return crate::semantic_gaps(update);
    };
    let mut visited = HashSet::new();
    let mut stack = vec![Visit {
        id: update.root,
        node: root,
        path: step(root, 0),
        ancestors: Vec::new(),
    }];
    let mut places: HashMap<NodeId, Vec<(usize, String)>> = HashMap::new();
    while let Some(visit) = stack.pop() {
        if !visited.insert(visit.id) {
            problems.push(format!("{}: reached twice", visit.path));
            continue;
        }
        node_problems(&nodes, update, &visit, &mut places, &mut problems);
        let mut ancestors = visit.ancestors.clone();
        ancestors.push(visit.id);
        for (index, child) in visit.node.children.iter().enumerate().rev() {
            match nodes.get(child) {
                Some(node) => stack.push(Visit {
                    id: *child,
                    node,
                    path: format!("{} > {}", visit.path, step(node, index)),
                    ancestors: ancestors.clone(),
                }),
                None => problems.push(format!(
                    "{} > #{index}: child {child:?} not in the tree",
                    visit.path
                )),
            }
        }
    }
    let mut containers: Vec<_> = places.into_iter().collect();
    containers.sort_by_key(|(id, _)| *id);
    for (_, items) in containers {
        let mut seen: HashMap<usize, &str> = HashMap::new();
        for (place, path) in &items {
            if let Some(first) = seen.insert(*place, path) {
                problems.push(format!("{path}: has place {place}, as {first} does"));
            }
        }
    }
    match update.focus {
        None => {}
        Some(focus) => match nodes.get(&focus) {
            None => problems.push(format!("focus {focus:?}: not in the tree")),
            Some(node) if !node.states.focusable && !is_window(node.role) => problems.push(
                format!("focus {}: cannot take focus", crate::describe(node)),
            ),
            Some(_) => {}
        },
    }
    problems
}

/// Every node of the tree that reads out one of `secrets` in its name, description or value: what a screen reader
/// would say of a password or passphrase it should never learn. An empty secret is skipped.
pub fn secret_leaks(update: &AccessibilityUpdate, secrets: &[&str]) -> Vec<String> {
    let secrets: Vec<&str> = secrets.iter().copied().filter(|s| !s.is_empty()).collect();
    update
        .nodes
        .iter()
        .filter(|(_, node)| {
            readable(node)
                .iter()
                .any(|text| secrets.iter().any(|s| text.contains(s)))
        })
        .map(|(_, node)| format!("{}: reads a secret", crate::describe(node)))
        .collect()
}

/// The steps of a focus sequence, as Tab visits the nodes of one window, that go backwards in reading order: to a
/// node on the same row before the last one (left of it in a left-to-right layout), or to one above it in the same
/// column. A step to the top of the next column on the right is reading order; the first node after the last is not
/// a step. Each line names the two nodes.
pub fn reading_order_breaks(update: &AccessibilityUpdate, sequence: &[NodeId]) -> Vec<String> {
    let nodes: HashMap<NodeId, &AccessibilityNode> =
        update.nodes.iter().map(|(id, n)| (*id, n)).collect();
    let mut breaks = Vec::new();
    for pair in sequence.windows(2) {
        let (Some(from), Some(to)) = (nodes.get(&pair[0]), nodes.get(&pair[1])) else {
            breaks.push(format!("{:?} -> {:?}: not in the tree", pair[0], pair[1]));
            continue;
        };
        if goes_back(from.bounds, to.bounds) {
            breaks.push(format!(
                "{} -> {}",
                crate::describe(from),
                crate::describe(to)
            ));
        }
    }
    breaks
}

fn goes_back(from: Rect, to: Rect) -> bool {
    let same_row = to.min_y() < from.max_y() && from.min_y() < to.max_y();
    let left_of = to.max_x() <= from.min_x();
    let above = to.max_y() <= from.min_y();
    let right_of = to.min_x() >= from.max_x();
    (same_row && left_of) || (above && !right_of)
}

struct Visit<'a> {
    id: NodeId,
    node: &'a AccessibilityNode,
    path: String,
    ancestors: Vec<NodeId>,
}

fn is_window(role: Role) -> bool {
    matches!(role, Role::Window | Role::Dialog | Role::AlertDialog)
}

fn operated(node: &AccessibilityNode) -> bool {
    node.actions.iter().any(|a| OPERATIONS.contains(a))
}

fn readable(node: &AccessibilityNode) -> Vec<&str> {
    let mut texts: Vec<&str> = node
        .name
        .iter()
        .chain(node.description.iter())
        .map(String::as_str)
        .collect();
    if let Some(AccessibleValue::Text { value, .. }) = &node.value {
        texts.push(value);
    }
    texts
}

fn masked(text: &str) -> bool {
    let mut run = 0;
    for c in text.chars() {
        run = if MASKS.contains(&c) { run + 1 } else { 0 };
        if run >= MASK_RUN {
            return true;
        }
    }
    false
}

fn node_problems(
    nodes: &HashMap<NodeId, &AccessibilityNode>,
    update: &AccessibilityUpdate,
    visit: &Visit,
    places: &mut HashMap<NodeId, Vec<(usize, String)>>,
    problems: &mut Vec<String>,
) {
    let (node, path) = (visit.node, &visit.path);
    let named = has_name(node) || labelled(node);
    let control = node.states.focusable || operated(node);
    if !named && !NAMED_BY_CONTENTS.contains(&node.role) {
        problems.push(format!("{path}: no name"));
    } else if !named && control && !matches!(node.role, Role::Row | Role::ScrollView) {
        problems.push(format!("{path}: a control without a name"));
    }
    if operated(node) && NOT_CONTROLS.contains(&node.role) {
        problems.push(format!(
            "{path}: takes {:?} but its role is no control",
            node.actions
                .iter()
                .filter(|a| OPERATIONS.contains(a))
                .collect::<Vec<_>>()
        ));
    }
    value_problems(node, path, problems);
    if readable(node).into_iter().any(masked) {
        problems.push(format!(
            "{path}: reads a mask, which tells a secret's length"
        ));
    }
    let focused = update.focus == Some(visit.id);
    if node.states.focused && !focused {
        problems.push(format!(
            "{path}: says it is focused, but focus is elsewhere"
        ));
    }
    if focused && !node.states.focused && !is_window(node.role) {
        problems.push(format!("{path}: has the focus but does not say so"));
    }
    for relation in &node.relations {
        let (Relation::LabelledBy(target)
        | Relation::DescribedBy(target)
        | Relation::Controls(target)
        | Relation::FlowsTo(target)) = relation;
        if !nodes.contains_key(target) {
            problems.push(format!("{path}: {relation:?} names a node not in the tree"));
        }
    }
    if control && (node.bounds.width() <= 0.0 || node.bounds.height() <= 0.0) {
        problems.push(format!("{path}: a control with no area on screen"));
    }
    if let Some((_, containers)) = ITEMS.iter().find(|(role, _)| *role == node.role) {
        item_problems(nodes, visit, containers, places, problems);
    }
}

fn value_problems(node: &AccessibilityNode, path: &str, problems: &mut Vec<String>) {
    let in_range = |value: f64, min: f64, max: f64| min <= max && (min..=max).contains(&value);
    match (node.role, &node.value) {
        (role, Some(AccessibleValue::Toggle(_))) if TOGGLES.contains(&role) => {}
        (role, _) if TOGGLES.contains(&role) => {
            problems.push(format!("{path}: does not say whether it is on or off"));
        }
        (
            Role::Slider | Role::SpinButton,
            Some(AccessibleValue::Number {
                value, min, max, ..
            }),
        ) if in_range(*value, *min, *max) => {}
        (Role::Slider | Role::SpinButton, _) => {
            problems.push(format!("{path}: does not read a number within its range"));
        }
        (
            Role::ProgressIndicator,
            Some(AccessibleValue::Number {
                value, min, max, ..
            }),
        ) if !in_range(*value, *min, *max) => {
            problems.push(format!("{path}: reads progress outside its range"));
        }
        (Role::PasswordInput, Some(AccessibleValue::Text { .. })) => {
            problems.push(format!("{path}: reads its secret as text"));
        }
        _ => {}
    }
    if node.role == Role::PopUpButton && node.states.expanded.is_none() {
        problems.push(format!("{path}: does not say whether its menu is open"));
    }
}

fn item_problems(
    nodes: &HashMap<NodeId, &AccessibilityNode>,
    visit: &Visit,
    containers: &[Role],
    places: &mut HashMap<NodeId, Vec<(usize, String)>>,
    problems: &mut Vec<String>,
) {
    let (node, path) = (visit.node, &visit.path);
    let container = visit
        .ancestors
        .iter()
        .rev()
        .find(|id| nodes.get(id).is_some_and(|n| containers.contains(&n.role)));
    let Some(container) = container else {
        problems.push(format!("{path}: an item outside a {containers:?}"));
        return;
    };
    // a tree item's own count is the size of the level it sits at
    let count = node.item_count.or_else(|| nodes[container].item_count);
    match (node.item_index, count) {
        (None, _) => problems.push(format!("{path}: does not say its place in its list")),
        (Some(_), None) => problems.push(format!(
            "{path}: its list does not say how many items it holds"
        )),
        (Some(place), Some(count)) if place >= count => {
            problems.push(format!("{path}: place {place} of {count}"));
        }
        (Some(place), Some(_)) => {
            // a tree item's place is among the items of its own level
            let key = if node.role == Role::TreeItem {
                visit.ancestors.last().copied().unwrap_or(*container)
            } else {
                *container
            };
            places
                .entry(key)
                .or_default()
                .push((place, path.to_string()));
        }
    }
}

#[cfg(test)]
#[path = "audit-tests.rs"]
mod tests;
