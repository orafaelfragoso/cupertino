use super::*;
use crate::test_support::{group, leaf, Group};
use crate::Element;

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
        focus.sync(&self.tree, &self.scopes(), &self.focusable())
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
