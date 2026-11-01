use super::*;
use crate::test_support::{group, leaf, Group, Leaf};
use crate::{Element, Role};

type M = String;

/// Three buttons, and a dialog holding two more when `dialog` is set.
fn view(dialog: bool) -> Element<M> {
    let mut children: Vec<Element<M>> =
        ["a", "b", "c"].map(|k| Element::new(leaf(k)).key(k)).into();
    if dialog {
        children.push(
            Element::new(Group {
                children: ["d1", "d2"].map(|k| Element::new(leaf(k)).key(k)).into(),
                scope: true,
                row: false,
                roving: false,
            })
            .key("dialog"),
        );
    }
    Element::new(group(children))
}

struct Fixture {
    tree: Tree<M>,
    root: NodeId,
}

impl Fixture {
    fn new() -> Self {
        let mut tree = Tree::new(20);
        let root = tree.reconcile(None, view(false)).unwrap();
        Self { tree, root }
    }

    fn show(&mut self, dialog: bool) {
        self.tree.reconcile(Some(self.root), view(dialog)).unwrap();
    }

    fn id(&self, key: &'static str) -> NodeId {
        self.tree
            .preorder(self.root)
            .into_iter()
            .find(|&n| self.tree.get(n).unwrap().key == Some(key.into()))
            .unwrap()
    }

    fn focusable(&self) -> Vec<NodeId> {
        self.tree
            .preorder(self.root)
            .into_iter()
            .filter(|&n| self.tree.get(n).unwrap().children.is_empty() && n != self.root)
            .collect()
    }

    fn scopes(&self) -> Vec<NodeId> {
        self.tree
            .preorder(self.root)
            .into_iter()
            .filter(|&n| self.tree.get(n).unwrap().key == Some("dialog".into()))
            .collect()
    }

    fn sync(&self, focus: &mut Focus) -> Option<FocusChange> {
        focus.sync(&self.tree, &self.scopes(), &self.focusable(), &[])
    }
}

#[test]
fn tab_walks_the_focusable_nodes_in_reading_order_and_wraps() {
    let f = Fixture::new();
    let mut focus = Focus::default();
    let order: Vec<_> = (0..4)
        .map(|_| {
            let next = focus.step(&f.tree, &f.focusable(), true);
            focus.set(&f.tree, next, true);
            next.unwrap()
        })
        .collect();
    assert_eq!(order, [f.id("a"), f.id("b"), f.id("c"), f.id("a")]);
    assert_eq!(focus.step(&f.tree, &f.focusable(), false), Some(f.id("c")));
}

#[test]
fn a_dialog_takes_the_focus_keeps_tab_inside_and_gives_it_back_when_it_closes() {
    let mut f = Fixture::new();
    let mut focus = Focus::default();
    focus.set(&f.tree, Some(f.id("b")), true);

    f.show(true);
    let change = f.sync(&mut focus).unwrap();
    assert_eq!(
        (change.from, change.to),
        (Some(f.id("b")), Some(f.id("d1")))
    );
    let tab = |focus: &mut Focus, f: &Fixture| {
        let next = focus.step(&f.tree, &f.focusable(), true);
        focus.set(&f.tree, next, true);
        focus.node().unwrap()
    };
    assert_eq!(tab(&mut focus, &f), f.id("d2"));
    assert_eq!(
        tab(&mut focus, &f),
        f.id("d1"),
        "Tab wraps inside the dialog"
    );

    f.show(false);
    f.tree.take_removed();
    let change = f.sync(&mut focus).unwrap();
    assert_eq!(change.to, Some(f.id("b")));
    assert!(focus.visible());
}

#[test]
fn a_dialog_gives_the_focus_to_its_initial_focus_node_rather_than_its_first_tab_stop() {
    let mut f = Fixture::new();
    let mut focus = Focus::default();
    let initial = Element::new(group(vec![
        Element::new(leaf("a")).key("a"),
        Element::new(Group {
            children: vec![
                Element::new(leaf("d1")).key("d1"),
                Element::new(Leaf {
                    initial: true,
                    ..leaf("d2")
                })
                .key("d2"),
            ],
            scope: true,
            row: false,
            roving: false,
        })
        .key("dialog"),
    ]));
    f.tree.reconcile(Some(f.root), initial).unwrap();

    let change = f.sync(&mut focus).unwrap();
    assert_eq!(change.to, Some(f.id("d2")));
}

/// Three buttons, the one keyed `initial` declaring initial focus.
fn window(initial: &'static str) -> Element<M> {
    Element::new(group(
        ["a", "b", "c"]
            .map(|k| {
                Element::new(Leaf {
                    initial: k == initial,
                    ..leaf(k)
                })
                .key(k)
            })
            .into(),
    ))
}

#[test]
fn a_window_gives_the_focus_to_its_initial_focus_node_when_it_opens_and_only_then() {
    let mut f = Fixture::new();
    f.tree.reconcile(Some(f.root), window("b")).unwrap();
    let mut focus = Focus::default();
    let change = f.sync(&mut focus).unwrap();
    assert_eq!((change.from, change.to), (None, Some(f.id("b"))));
    assert!(
        !focus.visible(),
        "a button taking the focus on open shows no ring"
    );

    focus.set(&f.tree, None, false);
    f.tree.reconcile(Some(f.root), window("c")).unwrap();
    assert_eq!(f.sync(&mut focus), None);
    assert_eq!(focus.node(), None);
}

#[test]
fn a_text_input_taking_the_focus_when_its_window_opens_shows_its_ring() {
    let mut f = Fixture::new();
    let field = Element::new(Leaf {
        role: Some(Role::TextInput),
        initial: true,
        ..leaf("name")
    })
    .key("name");
    f.tree
        .reconcile(Some(f.root), Element::new(group(vec![field])))
        .unwrap();
    let mut focus = Focus::default();
    let change = f.sync(&mut focus).unwrap();
    assert_eq!(change.to, Some(f.id("name")));
    assert!(focus.visible());
}

#[test]
fn a_text_input_taking_the_focus_when_its_scope_opens_shows_its_ring() {
    let mut f = Fixture::new();
    let mut focus = Focus::default();
    f.tree
        .reconcile(
            Some(f.root),
            Element::new(group(vec![Element::new(leaf("a")).key("a")])),
        )
        .unwrap();
    assert_eq!(f.sync(&mut focus), None);
    let sheet = Element::new(Group {
        children: vec![Element::new(Leaf {
            role: Some(Role::TextInput),
            initial: true,
            ..leaf("name")
        })
        .key("name")],
        scope: true,
        row: false,
        roving: false,
    })
    .key("dialog");
    f.tree
        .reconcile(
            Some(f.root),
            Element::new(group(vec![Element::new(leaf("a")).key("a"), sheet])),
        )
        .unwrap();
    assert_eq!(f.sync(&mut focus).unwrap().to, Some(f.id("name")));
    assert!(focus.visible());
}

#[test]
fn a_window_without_an_initial_focus_node_opens_unfocused() {
    let f = Fixture::new();
    let mut focus = Focus::default();
    assert_eq!(f.sync(&mut focus), None);
    assert_eq!(focus.node(), None);
}

#[test]
fn a_focus_whose_node_unmounts_moves_to_its_nearest_surviving_ancestor() {
    let mut f = Fixture::new();
    f.show(true);
    let mut focus = Focus::default();
    f.sync(&mut focus);
    let dialog = f.id("dialog");
    focus.set(&f.tree, Some(f.id("d2")), false);

    // d2 goes away while the dialog stays.
    let only_d1 = Element::new(group(vec![
        Element::new(leaf("a")).key("a"),
        Element::new(leaf("b")).key("b"),
        Element::new(leaf("c")).key("c"),
        Element::new(Group {
            children: vec![Element::new(leaf("d1")).key("d1")],
            scope: true,
            row: false,
            roving: false,
        })
        .key("dialog"),
    ]));
    f.tree.reconcile(Some(f.root), only_d1).unwrap();

    let change = f.sync(&mut focus).unwrap();
    assert_eq!(change.to, Some(dialog));
    assert!(!focus.visible());
}

#[test]
fn a_move_names_the_ancestors_the_focus_came_into_and_left() {
    let mut f = Fixture::new();
    f.show(true);
    let (root, dialog) = (f.root, f.id("dialog"));
    let mut focus = Focus::default();
    let mut moves = Vec::new();
    for key in ["a", "d1", "d2", "b"] {
        let change = focus.set(&f.tree, Some(f.id(key)), true).unwrap();
        moves.push((change.left, change.entered));
    }
    assert_eq!(
        moves,
        [
            (vec![], vec![root]),
            (vec![], vec![dialog]),
            (vec![], vec![]),
            (vec![dialog], vec![]),
        ]
    );
}

#[test]
fn when_the_focused_node_unmounts_its_surviving_ancestors_lose_the_focus_within_once() {
    let mut f = Fixture::new();
    f.show(true);
    let mut focus = Focus::default();
    focus.set(&f.tree, Some(f.id("d1")), true);

    f.show(false);
    f.tree.take_removed();
    let change = f.sync(&mut focus).unwrap();
    assert_eq!(change.to, Some(f.root));
    assert_eq!(change.left, [f.root], "the unmounted dialog hears nothing");
    assert!(change.entered.is_empty());
    assert_eq!(f.sync(&mut focus), None);
}

#[test]
fn a_focus_in_a_hidden_subtree_moves_to_the_next_tab_stop_after_it_or_else_the_one_before() {
    let f = Fixture::new();
    let (a, b, c) = (f.id("a"), f.id("b"), f.id("c"));
    let mut focus = Focus::default();
    focus.set(&f.tree, Some(b), true);
    let change = focus.sync(&f.tree, &[], &[a, c], &[b]).unwrap();
    assert_eq!(change.to, Some(c));
    assert!(change.visible);

    let change = focus.sync(&f.tree, &[], &[a], &[c]).unwrap();
    assert_eq!(change.to, Some(a), "nothing follows c");
    assert_eq!(focus.sync(&f.tree, &[], &[a], &[c]), None);
}
