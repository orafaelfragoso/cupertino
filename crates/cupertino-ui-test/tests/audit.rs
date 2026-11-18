//! What the accessibility audit says of a tree a screen reader could not use, and of the secrets and the reading
//! order it checks.

use cupertino_geometry::Rect;
use cupertino_ui_core::platform::WindowId;
use cupertino_ui_core::{
    AccessibilityNode, AccessibilityUpdate, AccessibleStates, AccessibleValue, Action, Checked,
    NodeId, Relation, Role,
};
use cupertino_ui_test::{accessibility_audit, reading_order_breaks, secret_leaks};
use std::num::NonZeroU32;

/// A tree under construction: nodes by the order they were added, the first being the window.
struct Tree {
    nodes: Vec<(NodeId, AccessibilityNode)>,
    focus: Option<NodeId>,
}

fn id(index: u32) -> NodeId {
    NodeId::new(index, NonZeroU32::new(1).unwrap())
}

fn node(role: Role, name: Option<&str>, bounds: Rect) -> AccessibilityNode {
    AccessibilityNode {
        role,
        name: name.map(str::to_owned),
        description: None,
        value: None,
        states: AccessibleStates::default(),
        bounds,
        actions: Vec::new(),
        relations: Vec::new(),
        children: Vec::new(),
        live: None,
        item_count: None,
        level: None,
        item_index: None,
        column_index: None,
        sort: None,
    }
}

impl Tree {
    fn new() -> Tree {
        let mut window = node(
            Role::Window,
            Some("Settings"),
            Rect::new(0.0, 0.0, 200.0, 200.0),
        );
        window.states.focusable = false;
        Tree {
            nodes: vec![(id(1), window)],
            focus: None,
        }
    }

    /// Adds a node under `parent` and returns its id.
    fn add(&mut self, parent: NodeId, node: AccessibilityNode) -> NodeId {
        let new = id(self.nodes.len() as u32 + 1);
        self.nodes.push((new, node));
        self.get(parent).children.push(new);
        new
    }

    fn get(&mut self, id: NodeId) -> &mut AccessibilityNode {
        &mut self.nodes.iter_mut().find(|(n, _)| *n == id).unwrap().1
    }

    fn update(&self) -> AccessibilityUpdate {
        AccessibilityUpdate {
            window: WindowId(1),
            root: id(1),
            focus: self.focus,
            nodes: self.nodes.clone(),
            removed: Vec::new(),
            full: true,
        }
    }
}

const ROW: Rect = Rect::new(10.0, 10.0, 60.0, 20.0);

/// A window of a button, a switch, a slider, a password field and a list of two items, all as they should be.
fn sound() -> (Tree, [NodeId; 6]) {
    let mut tree = Tree::new();
    let window = id(1);
    let mut button = node(Role::Button, Some("OK"), ROW);
    button.states.focusable = true;
    button.actions = vec![Action::Click];
    let button = tree.add(window, button);
    let mut switch = node(Role::Switch, Some("Wi-Fi"), ROW.translate(0.0, 30.0));
    switch.value = Some(AccessibleValue::Toggle(Checked::True));
    switch.actions = vec![Action::Click];
    let switch = tree.add(window, switch);
    let mut slider = node(Role::Slider, Some("Volume"), ROW.translate(0.0, 60.0));
    slider.value = Some(AccessibleValue::Number {
        value: 0.5,
        min: 0.0,
        max: 1.0,
        step: None,
    });
    slider.actions = vec![Action::Increment, Action::Decrement];
    let slider = tree.add(window, slider);
    let mut password = node(
        Role::PasswordInput,
        Some("Password"),
        ROW.translate(0.0, 90.0),
    );
    password.value = Some(AccessibleValue::Protected);
    password.states.focusable = true;
    let password = tree.add(window, password);
    let mut list = node(Role::List, Some("Items"), ROW.translate(0.0, 120.0));
    list.item_count = Some(2);
    let list = tree.add(window, list);
    for (index, name) in ["One", "Two"].into_iter().enumerate() {
        let mut item = node(
            Role::ListItem,
            Some(name),
            ROW.translate(0.0, 120.0 + 10.0 * index as f64),
        );
        item.item_index = Some(index);
        tree.add(list, item);
    }
    (tree, [button, switch, slider, password, list, id(2)])
}

#[test]
fn a_sound_tree_has_no_problems_and_each_defect_is_named() {
    let (tree, _) = sound();
    assert_eq!(accessibility_audit(&tree.update()), Vec::<String>::new());

    type Break = fn(&mut Tree, [NodeId; 6]);
    let rows: Vec<(&str, Break, &str)> = vec![
        (
            "a button without a name",
            |t, [button, ..]| t.get(button).name = None,
            "no name",
        ),
        (
            "a button with no area",
            |t, [button, ..]| t.get(button).bounds = Rect::new(5.0, 5.0, 0.0, 20.0),
            "no area on screen",
        ),
        (
            "a switch that is neither on nor off",
            |t, [_, switch, ..]| t.get(switch).value = None,
            "whether it is on or off",
        ),
        (
            "a slider outside its range",
            |t, [_, _, slider, ..]| {
                t.get(slider).value = Some(AccessibleValue::Number {
                    value: 3.0,
                    min: 0.0,
                    max: 1.0,
                    step: None,
                });
            },
            "a number within its range",
        ),
        (
            "a password read as text",
            |t, [.., password, _, _]| {
                t.get(password).value = Some(AccessibleValue::Text {
                    value: "hunter2".into(),
                    editing: None,
                });
            },
            "reads its secret as text",
        ),
        (
            "a name that is a row of bullets",
            |t, [button, ..]| t.get(button).name = Some("\u{2022}\u{2022}\u{2022}\u{2022}".into()),
            "reads a mask",
        ),
        (
            "a list that does not count its items",
            |t, [.., list, _]| t.get(list).item_count = None,
            "does not say how many items",
        ),
        (
            "an item without a place",
            |t, _| t.nodes.last_mut().unwrap().1.item_index = None,
            "does not say its place",
        ),
        (
            "two items in one place",
            |t, _| t.nodes.last_mut().unwrap().1.item_index = Some(0),
            "has place 0",
        ),
        (
            "a place past the count",
            |t, _| t.nodes.last_mut().unwrap().1.item_index = Some(2),
            "place 2 of 2",
        ),
        (
            "a relation to a node that is gone",
            |t, [button, ..]| t.get(button).relations = vec![Relation::LabelledBy(id(99))],
            "names a node not in the tree",
        ),
        (
            "a focus on a node that cannot take it",
            |t, [_, switch, ..]| t.focus = Some(switch),
            "cannot take focus",
        ),
        (
            "a focus the tree does not hold",
            |t, _| t.focus = Some(id(77)),
            "not in the tree",
        ),
        (
            "a child the tree does not hold",
            |t, _| t.nodes[0].1.children.push(id(55)),
            "not in the tree",
        ),
        (
            "a click on text",
            |t, _| {
                let text = node(Role::StaticText, Some("Hello"), ROW);
                let text = t.add(id(1), text);
                t.get(text).actions = vec![Action::Click];
            },
            "its role is no control",
        ),
        (
            "a pop-up button that does not say whether it is open",
            |t, _| {
                let mut pop = node(Role::PopUpButton, Some("Size"), ROW);
                pop.states.focusable = true;
                t.add(id(1), pop);
            },
            "whether its menu is open",
        ),
    ];
    for (what, change, expected) in rows {
        let (mut tree, handles) = sound();
        change(&mut tree, handles);
        let problems = accessibility_audit(&tree.update());
        assert!(
            problems.iter().any(|p| p.contains(expected)),
            "{what}: no problem says {expected:?}: {problems:?}"
        );
    }
}

#[test]
fn a_secret_is_found_in_what_a_node_reads_out_and_the_sequence_that_goes_backwards_is_listed() {
    let (mut tree, [button, switch, slider, ..]) = sound();
    tree.get(button).name = Some("Sign in as hunter2".into());
    tree.get(switch).description = Some("remember hunter2?".into());
    tree.get(slider).value = Some(AccessibleValue::Text {
        value: "my-secret-pass".into(),
        editing: None,
    });
    let update = tree.update();
    assert_eq!(
        secret_leaks(&update, &["hunter2"]).len(),
        2,
        "the name and the description"
    );
    assert_eq!(
        secret_leaks(&update, &["my-secret"]).len(),
        1,
        "the value of text"
    );
    assert_eq!(secret_leaks(&update, &["absent"]), Vec::<String>::new());
    assert_eq!(
        secret_leaks(&update, &[""]),
        Vec::<String>::new(),
        "an empty secret is nothing"
    );

    // a row of two and a row under them
    let mut grid = Tree::new();
    let place = |tree: &mut Tree, name, x, y| {
        let mut n = node(Role::Button, Some(name), Rect::new(x, y, 40.0, 20.0));
        n.states.focusable = true;
        tree.add(id(1), n)
    };
    let (a, b) = (
        place(&mut grid, "a", 0.0, 0.0),
        place(&mut grid, "b", 50.0, 0.0),
    );
    let (c, d) = (
        place(&mut grid, "c", 0.0, 30.0),
        place(&mut grid, "d", 50.0, 30.0),
    );
    let update = grid.update();
    assert_eq!(
        reading_order_breaks(&update, &[a, b, c, d]),
        Vec::<String>::new(),
        "along the rows, down, along"
    );
    assert_eq!(
        reading_order_breaks(&update, &[a, c, b, d]),
        Vec::<String>::new(),
        "down the columns to the next one"
    );
    assert_eq!(
        reading_order_breaks(&update, &[b, a]).len(),
        1,
        "left along a row"
    );
    assert_eq!(
        reading_order_breaks(&update, &[c, a]).len(),
        1,
        "up a column"
    );
    assert_eq!(
        reading_order_breaks(&update, &[d, c, a, b]).len(),
        2,
        "each backwards step, not the jump after"
    );
    assert_eq!(
        reading_order_breaks(&update, &[a, id(40)]).len(),
        1,
        "a node the tree lacks"
    );
}
