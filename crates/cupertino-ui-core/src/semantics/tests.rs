use super::*;
use crate::accessibility::{Live, Relation, TextSemantics};
use crate::route::Placement;
use crate::test_support::{group, leaf, spec};
use crate::{Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, Widget};
use cupertino_geometry::Size;
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured};

type M = String;

/// A password field that hands core its plaintext, as a careless widget might.
struct Secret;

impl Widget<M> for Secret {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PasswordInput);
        cx.name("Password");
        cx.value(AccessibleValue::Text {
            value: "hunter2".into(),
            editing: Some(TextSemantics {
                caret: 7,
                selection: 7..7,
            }),
        });
        cx.focusable();
        cx.protected();
    }
}

/// A form error that assistive technology announces at once.
struct Alert;

impl Widget<M> for Alert {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name("Incorrect password");
        cx.live(Live::Assertive);
    }
}

/// A badge: no node of its own, only words for the node it sits in.
struct Note(&'static str);

impl Widget<M> for Note {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.description(self.0);
    }
}

fn pass(view: Element<M>) -> (Tree<M>, NodeId, SemanticsPass) {
    let mut tree = Tree::new(20);
    let root = tree.reconcile(None, view).unwrap();
    let pass = build_for(&tree, root);
    (tree, root, pass)
}

fn build_for(tree: &Tree<M>, root: NodeId) -> SemanticsPass {
    let geometry: Geometry = tree
        .preorder(root)
        .into_iter()
        .map(|id| {
            (
                id,
                Placement {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    target: Rect::new(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    own_clip: None,
                    raised: Default::default(),
                },
            )
        })
        .collect();
    build(tree, root, &geometry, None, LayoutDirection::Ltr, 1.0, &[])
}

#[test]
fn a_protected_value_reaches_the_tree_as_protected_with_no_text_or_caret() {
    let (_, _, pass) = pass(Element::new(group(vec![Element::new(Secret)])));
    let field = pass
        .nodes
        .values()
        .find(|n| n.role == Role::PasswordInput)
        .unwrap();
    assert_eq!(field.value, Some(AccessibleValue::Protected));
    assert_eq!(field.name.as_deref(), Some("Password"));
    assert!(!format!("{:?}", pass.nodes).contains("hunter2"));
}

#[test]
fn a_live_region_reaches_the_tree_with_its_politeness_and_other_nodes_are_not_live() {
    let (_, root, pass) = pass(Element::new(group(vec![
        Element::new(Alert),
        Element::new(Secret),
    ])));
    let alert = pass
        .nodes
        .values()
        .find(|n| n.role == Role::StaticText)
        .unwrap();
    assert_eq!(alert.live, Some(Live::Assertive));
    let field = pass
        .nodes
        .values()
        .find(|n| n.role == Role::PasswordInput)
        .unwrap();
    assert_eq!(field.live, None);
    assert_eq!(pass.nodes[&root].live, None);
}

#[test]
fn children_of_a_node_without_a_role_belong_to_its_nearest_ancestor_with_one() {
    let (tree, root, pass) = pass(Element::new(group(vec![Element::new(group(vec![
        Element::new(leaf("a")),
        Element::new(leaf("b")),
    ]))])));
    let middle = tree.get(root).unwrap().children[0];
    let leaves = tree.get(middle).unwrap().children.clone();

    assert_eq!(pass.nodes[&root].role, Role::Window);
    assert!(!pass.nodes.contains_key(&middle));
    assert_eq!(pass.nodes[&root].children, leaves);
    assert_eq!(pass.focusable, leaves);
}

#[test]
fn a_description_without_a_role_joins_its_nearest_ancestor_with_one() {
    let (tree, root, pass) = pass(Element::new(group(vec![Element::new(group(vec![
        Element::new(Note("3 unread")),
        Element::new(Note("flagged")),
    ]))])));
    let middle = tree.get(root).unwrap().children[0];

    assert_eq!(
        pass.nodes[&root].description.as_deref(),
        Some("3 unread, flagged")
    );
    assert_eq!(pass.nodes.len(), 1, "{:?}", pass.nodes.keys());
    assert!(!pass.nodes.contains_key(&middle));
}

/// A tooltip's stand-in: no node of its own, only words for the first node under it.
struct Tip(&'static str, Vec<Element<M>>);

impl Widget<M> for Tip {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.1)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.describe_content(self.0);
    }
}

#[test]
fn a_content_description_goes_to_the_first_node_with_a_role_under_its_giver() {
    let (tree, root, pass) = pass(Element::new(group(vec![
        Element::new(Tip(
            "Deletes the file",
            vec![
                Element::new(group(vec![Element::new(leaf("a")).key("a")])),
                Element::new(leaf("b")).key("b"),
            ],
        )),
        Element::new(leaf("c")).key("c"),
    ])));
    let id = |key: &'static str| {
        tree.preorder(root)
            .into_iter()
            .find(|&n| tree.get(n).unwrap().key == Some(key.into()))
            .unwrap()
    };
    let description = |key| pass.nodes[&id(key)].description.clone();
    assert_eq!(description("a").as_deref(), Some("Deletes the file"));
    assert_eq!(description("b"), None, "only the first node takes it");
    assert_eq!(description("c"), None, "nothing outside the giver takes it");
    assert_eq!(pass.nodes[&root].description, None);
}

/// A button that already describes itself.
struct Described;

impl Widget<M> for Described {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name("Trash");
        cx.description("3 items");
    }
}

#[test]
fn a_content_description_survives_nested_wrappers_and_follows_the_nodes_own() {
    let (tree, root, pass) = pass(Element::new(group(vec![Element::new(Tip(
        "Deletes the file",
        vec![Element::new(group(vec![Element::new(group(vec![
            Element::new(Described).key("trash"),
        ]))]))],
    ))])));
    let trash = tree
        .preorder(root)
        .into_iter()
        .find(|&n| tree.get(n).unwrap().key == Some("trash".into()))
        .unwrap();
    assert_eq!(
        pass.nodes[&trash].description.as_deref(),
        Some("3 items, Deletes the file")
    );
}

#[test]
fn a_diff_carries_changed_and_new_nodes_and_names_the_removed_ones() {
    let (tree, root, before) = pass(Element::new(group(vec![
        Element::new(leaf("a")).key(1u64),
        Element::new(leaf("b")).key(2u64),
    ])));
    let [a, b] = tree.get(root).unwrap().children[..] else {
        unreachable!()
    };
    let mut after = before.nodes.clone();
    after.remove(&b);
    after.get_mut(&a).unwrap().name = Some("renamed".into());
    after.get_mut(&root).unwrap().children = vec![a];

    let (changed, removed) = diff(&before.nodes, &after);
    let mut expected = vec![root, a];
    expected.sort();
    assert_eq!(
        changed.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        expected
    );
    assert_eq!(removed, vec![b]);
    assert!(diff(&after, &after).0.is_empty());
}

/// A scroll view's stand-in: a Tab stop only when nothing under it is one.
struct Fallback(Vec<Element<M>>);

impl Widget<M> for Fallback {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.0)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ScrollView);
        cx.focus_fallback();
    }
}

#[test]
fn a_fallback_is_a_tab_stop_only_when_no_node_under_it_is() {
    let (tree, root, pass) = pass(Element::new(group(vec![
        Element::new(Fallback(vec![Element::new(leaf("a")).key("a")])).key("around a"),
        Element::new(Fallback(vec![
            Element::new(Fallback(Vec::new())).key("empty")
        ]))
        .key("around empty"),
        Element::new(leaf("b")).key("b"),
    ])));
    let id = |key: &'static str| {
        tree.preorder(root)
            .into_iter()
            .find(|&n| tree.get(n).unwrap().key == Some(key.into()))
            .unwrap()
    };
    assert_eq!(pass.focusable, [id("a"), id("empty"), id("b")]);
    assert!(pass.nodes[&id("empty")].states.focusable);
    assert!(!pass.nodes[&id("around a")].states.focusable);
}

/// A button that controls the nearest node keyed `"panel"`.
struct Controller;

impl Widget<M> for Controller {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.relation_to(Relation::Controls, "panel");
    }
}

fn controller(pass: &SemanticsPass, tree: &Tree<M>) -> Vec<Relation> {
    let (&id, _) = pass
        .nodes
        .iter()
        .find(|(&id, _)| {
            tree.get(id).unwrap().widget.widget_type() == std::any::TypeId::of::<Controller>()
        })
        .unwrap();
    pass.nodes[&id].relations.clone()
}

#[test]
fn a_keyed_relation_goes_to_the_nearest_node_of_that_key() {
    let (tree, root, pass) = pass(Element::new(group(vec![
        Element::new(group(vec![
            Element::new(Controller),
            Element::new(leaf("inner")).key("panel"),
        ])),
        Element::new(leaf("outer")).key("panel"),
    ])));
    let inner = tree
        .get(tree.get(root).unwrap().children[0])
        .unwrap()
        .children[1];

    assert_eq!(controller(&pass, &tree), [Relation::Controls(inner)]);
}

#[test]
fn a_keyed_relation_with_no_node_in_the_tree_adds_nothing() {
    for target in [
        Element::new(leaf("other")).key("tab"),
        Element::new(group(vec![])).key("panel"),
    ] {
        let (tree, _, pass) = pass(Element::new(group(vec![Element::new(Controller), target])));
        assert_eq!(controller(&pass, &tree), []);
    }
}

#[test]
fn a_keyed_relation_among_siblings_sharing_the_key_goes_to_the_surviving_one() {
    let view = || {
        Element::new(group(vec![
            Element::new(Controller),
            Element::new(leaf("first")).key("panel"),
            Element::new(leaf("second")).key("panel"),
        ]))
    };
    let mut tree = Tree::new(20);
    let root = tree.reconcile(None, view()).unwrap();
    let first = tree.get(root).unwrap().children[1];
    tree.reconcile(Some(root), view()).unwrap();
    let pass = build_for(&tree, root);

    assert_eq!(controller(&pass, &tree), [Relation::Controls(first)]);
}

/// A popover that is dismissible while open, or a field that composes text.
struct Closable {
    open: bool,
    composing: bool,
}

impl Widget<M> for Closable {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.open {
            cx.dismissible();
        }
        if self.composing {
            cx.composing();
        }
    }
}

#[test]
fn the_dismissible_and_composing_nodes_are_read_as_they_declare_now() {
    let closable = |open, composing| Element::new(Closable { open, composing });
    let mut tree = Tree::<M>::new(20);
    let root = tree
        .reconcile(
            None,
            Element::new(group(vec![
                closable(true, false),
                closable(false, true),
                Element::new(group(vec![closable(true, false)])),
            ])),
        )
        .unwrap();
    let children = tree.get(root).unwrap().children.clone();
    let inner = tree.get(children[2]).unwrap().children[0];

    assert_eq!(dismissible(&tree, root), [children[0], inner]);
    assert!(composing(&tree, children[1]));
    assert!(!composing(&tree, children[0]));
}

/// The content behind a modal sheet: there, but not to assistive technology or the keyboard.
struct Hidden(Vec<Element<M>>);

impl Widget<M> for Hidden {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.0)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.hidden();
    }
}

#[test]
fn a_hidden_subtree_is_gone_from_the_tree_and_the_tab_order() {
    let (tree, root, pass) = pass(Element::new(group(vec![
        Element::new(Hidden(vec![
            Element::new(leaf("a")).key("a"),
            Element::new(Fallback(Vec::new())).key("empty"),
        ]))
        .key("hidden"),
        Element::new(leaf("b")).key("b"),
    ])));
    let id = |key: &'static str| {
        tree.preorder(root)
            .into_iter()
            .find(|&n| tree.get(n).unwrap().key == Some(key.into()))
            .unwrap()
    };
    assert_eq!(pass.focusable, [id("b")]);
    let mut nodes: Vec<NodeId> = pass.nodes.keys().copied().collect();
    nodes.sort();
    let mut expected = vec![root, id("b")];
    expected.sort();
    assert_eq!(nodes, expected);
    assert_eq!(pass.nodes[&root].children, [id("b")]);
}

#[test]
fn a_dismissible_node_behind_hidden_content_does_not_take_escape() {
    let closable = || {
        Element::new(Closable {
            open: true,
            composing: false,
        })
    };
    let mut tree = Tree::<M>::new(20);
    let root = tree
        .reconcile(
            None,
            Element::new(group(vec![
                Element::new(Hidden(vec![Element::new(group(vec![closable()]))])),
                closable(),
            ])),
        )
        .unwrap();
    let children = tree.get(root).unwrap().children.clone();

    assert_eq!(dismissible(&tree, root), [children[1]]);
}

#[test]
fn a_node_holding_the_focus_says_it_can_take_focus() {
    let (tree, root, _) = pass(Element::new(group(vec![Element::new(Alert)])));
    let alert = tree.preorder(root)[1];
    let geometry: Geometry = tree
        .preorder(root)
        .into_iter()
        .map(|id| {
            (
                id,
                Placement {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    target: Rect::new(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    own_clip: None,
                    raised: Default::default(),
                },
            )
        })
        .collect();
    let unfocused = build(&tree, root, &geometry, None, LayoutDirection::Ltr, 1.0, &[]);
    let focused = build(
        &tree,
        root,
        &geometry,
        Some(alert),
        LayoutDirection::Ltr,
        1.0,
        &[],
    );
    let states = |pass: &SemanticsPass| {
        let s = pass.nodes[&alert].states;
        (s.focusable, s.focused)
    };
    assert_eq!(
        (states(&unfocused), states(&focused)),
        ((false, false), (true, true))
    );
}

#[test]
fn a_focus_on_a_node_without_a_role_is_told_on_its_nearest_ancestor_in_the_tree() {
    // the root (the window), a container with no role, and a button in it
    let (tree, root, _) = pass(Element::new(group(vec![Element::new(group(vec![
        Element::new(leaf("deep")),
    ]))])));
    let order = tree.preorder(root);
    let (container, deep) = (order[1], order[2]);
    let geometry: Geometry = order
        .iter()
        .map(|&id| {
            (
                id,
                Placement {
                    rect: Rect::new(0.0, 0.0, 10.0, 10.0),
                    target: Rect::new(0.0, 0.0, 10.0, 10.0),
                    clip: None,
                    own_clip: None,
                    raised: Default::default(),
                },
            )
        })
        .collect();
    let at = |focus| {
        build(
            &tree,
            root,
            &geometry,
            focus,
            LayoutDirection::Ltr,
            1.0,
            &[],
        )
    };
    let focused = |pass: &SemanticsPass| {
        let mut on: Vec<NodeId> = pass
            .nodes
            .iter()
            .filter(|(_, n)| n.states.focused)
            .map(|(&id, _)| id)
            .collect();
        on.sort();
        on
    };

    let left = at(Some(container));
    assert!(
        !left.nodes.contains_key(&container),
        "the container has no role"
    );
    assert_eq!(left.focus, Some(root), "told on the window instead");
    assert_eq!(focused(&left), [root]);
    assert!(
        left.nodes[&root].states.focusable,
        "a node holding the focus can take it"
    );

    let on = at(Some(deep));
    assert_eq!(on.focus, Some(deep), "a node in the tree is told as it is");
    assert_eq!(focused(&on), [deep]);
    assert!(!on.nodes[&root].states.focusable);

    let none = at(None);
    assert_eq!(none.focus, None);
    assert!(focused(&none).is_empty());
}

/// What a [`Declares`] node says about itself.
#[derive(Clone, Default, PartialEq)]
struct Said {
    role: Option<Role>,
    name: Option<&'static str>,
    description: Option<&'static str>,
    content: Option<&'static str>,
    focusable: bool,
    fallback: bool,
    scope: bool,
    roving_group: bool,
    roving_item: bool,
    hidden: bool,
    controls_panel: bool,
    bounds: Option<Rect>,
}

/// A node 90 points wide, `height` tall (or as tall as its children), its children stacked, saying `said`.
struct Declares {
    said: Said,
    height: Option<f64>,
    children: Vec<Element<M>>,
}

fn declares(said: Said, height: Option<f64>, children: Vec<Element<M>>) -> Element<M> {
    Element::new(Declares {
        said,
        height,
        children,
    })
}

impl Widget<M> for Declares {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: cupertino_ui_layout::SizeSpec::Fixed(90.0),
            height: self.height.map_or(cupertino_ui_layout::SizeSpec::Fit, |h| {
                cupertino_ui_layout::SizeSpec::Fixed(h)
            }),
            ..spec(ContainerKind::Stack {
                axis: cupertino_ui_layout::Axis::Vertical,
                spacing: 2.0,
                align: cupertino_ui_layout::Align::Start,
                justify: cupertino_ui_layout::Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.height != old.height {
            Change::Layout
        } else if self.said != old.said {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        let s = &self.said;
        if let Some(role) = s.role {
            cx.role(role);
        }
        if let Some(name) = s.name {
            cx.name(name);
        }
        if let Some(text) = s.description {
            cx.description(text);
        }
        if let Some(text) = s.content {
            cx.describe_content(text);
        }
        if s.focusable {
            cx.focusable();
        }
        if s.fallback {
            cx.focus_fallback();
        }
        if s.scope {
            cx.focus_scope();
        }
        if s.roving_group {
            cx.roving_group();
        }
        if s.roving_item {
            cx.roving_item();
        }
        if s.hidden {
            cx.hidden();
        }
        if s.controls_panel {
            cx.relation_to(Relation::Controls, "panel");
        }
        if let Some(bounds) = s.bounds {
            cx.bounds(bounds);
        }
    }
}

/// Row `i` named `name`: an item of the list when `i` is even, a plain wrapper whose words go to the list when it is
/// odd; in it a button controlling the panel with bounds of its own, a badge describing the row, a part hidden in
/// every third row, a fallback around a scope, and words handed down to its first node with a role.
fn kept_row(i: usize, name: &'static str) -> Element<M> {
    let mark = name.len() as u64 * 1000 + i as u64;
    Element::kept(i as u64, mark, move || {
        let item = i.is_multiple_of(2);
        declares(
            Said {
                role: item.then_some(Role::ListItem),
                name: Some(name),
                description: (!item).then_some("odd row"),
                content: Some("in the list"),
                roving_item: item,
                ..Said::default()
            },
            Some(30.0),
            vec![
                declares(
                    Said {
                        role: Some(Role::Button),
                        name: Some("open"),
                        focusable: true,
                        controls_panel: true,
                        bounds: Some(Rect::new(2.0, 3.0, 10.0, 5.0)),
                        ..Said::default()
                    },
                    Some(6.0),
                    Vec::new(),
                ),
                declares(
                    Said {
                        description: Some("badge"),
                        ..Said::default()
                    },
                    Some(4.0),
                    Vec::new(),
                ),
                declares(
                    Said {
                        role: Some(Role::StaticText),
                        hidden: i.is_multiple_of(3),
                        ..Said::default()
                    },
                    Some(3.0),
                    Vec::new(),
                ),
                declares(
                    Said {
                        fallback: true,
                        ..Said::default()
                    },
                    None,
                    vec![declares(
                        Said {
                            scope: true,
                            role: Some(Role::Group),
                            ..Said::default()
                        },
                        Some(2.0),
                        Vec::new(),
                    )],
                ),
            ],
        )
    })
}

/// A scrolled list of kept rows, which hands its first row words of its own, beside the panel the rows' buttons
/// control.
fn kept_list(names: &[&'static str], offset: f64) -> Element<M> {
    let rows = names
        .iter()
        .enumerate()
        .map(|(i, &name)| kept_row(i, name))
        .collect();
    Element::new(group(vec![
        Element::new(crate::test_support::Scroller {
            children: vec![declares(
                Said {
                    role: Some(Role::List),
                    roving_group: true,
                    content: Some("files"),
                    ..Said::default()
                },
                None,
                rows,
            )],
            offset: cupertino_geometry::Point::new(0.0, offset),
        }),
        declares(
            Said {
                role: Some(Role::Group),
                name: Some("panel"),
                ..Said::default()
            },
            Some(10.0),
            Vec::new(),
        )
        .key("panel"),
    ]))
}

/// The pass's every part, compared with another's.
fn assert_same(got: &SemanticsPass, whole: &SemanticsPass, at: &str) {
    assert_eq!(got.nodes, whole.nodes, "nodes {at}");
    assert_eq!(got.focusable, whole.focusable, "focus order {at}");
    assert_eq!(got.scopes, whole.scopes, "scopes {at}");
    assert_eq!(got.roving_groups, whole.roving_groups, "roving groups {at}");
    assert_eq!(got.roving_items, whole.roving_items, "roving items {at}");
    assert_eq!(got.hidden, whole.hidden, "hidden {at}");
    assert_eq!(got.focus, whole.focus, "focus {at}");
}

/// A kept subtree that did not change has its semantics laid in from its run, unwalked, and after scrolls at whole
/// and fractional offsets, the focus moving into a row and out, a row made anew, a row dropped and a resize, the
/// nodes, the focus order, the scopes, roving groups and items, the hidden roots and the accessibility diff are those
/// of a full build after the same frames.
#[test]
fn kept_semantics_laid_in_from_their_runs_are_those_of_a_full_build() {
    let env = crate::test_support::env();
    let mut names = vec![
        "a", "bb", "ccc", "dd", "e", "ff", "ggg", "h", "ii", "jjj", "k", "ll",
    ];
    let mut tree = Tree::<M>::new(200);
    let root = tree.reconcile(None, kept_list(&names, 0.0)).unwrap();
    let mut kept = KeptSemantics::default();
    let mut last_got = BTreeMap::new();
    let mut last_whole = BTreeMap::new();
    let mut frame = |tree: &Tree<M>, kept: &mut KeptSemantics, size: Size, focus, at: &str| {
        let result = cupertino_ui_layout::layout(
            &crate::layout_view::TreeView::new(tree, &env),
            root,
            Constraints::tight(size).unwrap(),
            &cupertino_ui_layout::LayoutOptions {
                direction: env.direction,
                node_limit: 1000,
            },
        )
        .unwrap();
        let geometry = crate::layout_view::placements(tree, root, &result, &env, &[]);
        let got = build_kept(
            tree,
            root,
            &geometry,
            focus,
            env.direction,
            env.scale,
            &[],
            kept,
        );
        let whole = build(tree, root, &geometry, focus, env.direction, env.scale, &[]);
        assert_same(&got, &whole, at);
        assert_eq!(
            diff(&last_got, &got.nodes),
            diff(&last_whole, &whole.nodes),
            "the diff {at}"
        );
        last_got = got.nodes;
        last_whole = whole.nodes;
        kept.laid
    };
    // six nodes asked in each row; the first is handed the list's words and is always walked
    let per_row = 6;
    let window = Size::new(100.0, 120.5);
    assert_eq!(frame(&tree, &mut kept, window, None, "at first"), 0);
    for offset in [7.0, 13.5, 40.25, 0.0] {
        tree.reconcile(Some(root), kept_list(&names, offset))
            .unwrap();
        let at = format!("at {offset}");
        assert_eq!(
            frame(&tree, &mut kept, window, None, &at),
            11 * per_row,
            "{at}"
        );
    }
    // the focus in a row: the row is walked while it holds it, and kept again once it leaves
    let row = |tree: &Tree<M>, i: usize| {
        let scroller = tree.get(root).unwrap().children[0];
        let list = tree.get(scroller).unwrap().children[0];
        tree.get(list).unwrap().children[i]
    };
    let button = tree.get(row(&tree, 4)).unwrap().children[0];
    tree.reconcile(Some(root), kept_list(&names, 20.0)).unwrap();
    assert_eq!(
        frame(&tree, &mut kept, window, Some(button), "focused"),
        10 * per_row
    );
    // the row it left has no run yet, and the one it is on, which has no role, is walked
    assert_eq!(
        frame(
            &tree,
            &mut kept,
            window,
            Some(row(&tree, 5)),
            "focus on a row"
        ),
        9 * per_row
    );
    assert_eq!(
        frame(&tree, &mut kept, window, None, "focus gone"),
        10 * per_row
    );
    assert_eq!(frame(&tree, &mut kept, window, None, "again"), 11 * per_row);
    // a row made anew is walked, the others laid in
    names[3] = "dddd";
    tree.reconcile(Some(root), kept_list(&names, 13.5)).unwrap();
    assert_eq!(
        frame(&tree, &mut kept, window, None, "a row made anew"),
        10 * per_row
    );
    // a row dropped: its run goes with it
    names.pop();
    tree.reconcile(Some(root), kept_list(&names, 20.0)).unwrap();
    assert_eq!(
        frame(&tree, &mut kept, window, None, "a row dropped"),
        10 * per_row
    );
    // the window resized: the rows keep their sizes, so their runs still hold
    assert_eq!(
        frame(&tree, &mut kept, Size::new(97.0, 50.0), None, "resized"),
        10 * per_row
    );
}
