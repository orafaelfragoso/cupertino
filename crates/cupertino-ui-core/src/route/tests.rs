use super::*;
use crate::test_support::{env, group, leaf, spec, Leaf};
use crate::{Change, Element, MeasureCx, PaintCx, SemanticsCx, Widget};
use cupertino_geometry::Size;
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{KeyEvent, Keysym, Mods, Timestamp};
use proptest::prelude::*;

type M = String;

fn status() -> Status {
    Status {
        focus: None,
        focus_visible: false,
        capture: None,
        window_active: true,
        now: Timestamp(0),
        opening_press: None,
    }
}

fn key() -> Event {
    Event::Key(
        KeyEvent {
            code: 30,
            keysym: Keysym(0x61),
            text: Some('a'),
            pressed: true,
            repeat: false,
            mods: Mods::NONE,
        },
        Timestamp(0),
    )
}

fn route(tree: &mut Tree<M>, path: &[NodeId]) -> (Propagation, Vec<String>) {
    let env = env();
    let mut effects = Effects::default();
    let result = Route {
        tree,
        geometry: &Geometry::default(),
        env: &env,
        status: status(),
        popups: &[],
        effects: &mut effects,
    }
    .dispatch(path, &key());
    (result, effects.messages)
}

fn nested(target: Leaf) -> (Tree<M>, Vec<NodeId>) {
    let mut tree = Tree::new(10);
    let root = tree
        .reconcile(
            None,
            Element::new(group(vec![Element::new(group(vec![Element::new(target)]))])),
        )
        .unwrap();
    let middle = tree.get(root).unwrap().children[0];
    let target = tree.get(middle).unwrap().children[0];
    (tree, vec![root, middle, target])
}

#[test]
fn an_event_goes_down_the_path_to_the_target_and_back_up() {
    let (mut tree, path) = nested(leaf("t"));
    let (result, seen) = route(&mut tree, &path);
    assert_eq!(result, Propagation::Continue);
    assert_eq!(
        seen,
        [
            "group:Capture",
            "group:Capture",
            "t:Target:key",
            "group:Bubble",
            "group:Bubble"
        ]
    );
}

#[test]
fn a_handler_that_stops_the_event_ends_its_route() {
    let (mut tree, path) = nested(Leaf {
        stop: true,
        ..leaf("t")
    });
    let (result, seen) = route(&mut tree, &path);
    assert_eq!(result, Propagation::Stop);
    assert_eq!(seen, ["group:Capture", "group:Capture", "t:Target:key"]);
}

fn place(geometry: &mut Geometry, id: NodeId, rect: Rect, clip: Option<Rect>) {
    geometry.insert(
        id,
        Placement {
            rect,
            target: rect,
            clip,
            own_clip: None,
            raised: Default::default(),
        },
    );
}

#[test]
fn the_later_sibling_is_on_top_and_a_clip_hides_what_it_cuts_off() {
    let mut tree = Tree::<M>::new(10);
    let view = Element::new(group(vec![
        Element::new(leaf("a")),
        Element::new(group(vec![Element::new(leaf("b"))])),
    ]));
    let root = tree.reconcile(None, view).unwrap();
    let [a, clipper] = tree.get(root).unwrap().children[..] else {
        unreachable!()
    };
    let b = tree.get(clipper).unwrap().children[0];
    let clip = Rect::new(0.0, 0.0, 40.0, 40.0);
    let mut geometry = Geometry::default();
    place(&mut geometry, root, Rect::new(0.0, 0.0, 100.0, 100.0), None);
    place(&mut geometry, a, Rect::new(0.0, 0.0, 60.0, 60.0), None);
    place(&mut geometry, clipper, clip, Some(clip));
    place(
        &mut geometry,
        b,
        Rect::new(20.0, 20.0, 60.0, 60.0),
        Some(clip),
    );

    let hit = |x, y| hit_test(&tree, root, &geometry, Point::new(x, y));
    assert_eq!(hit(30.0, 30.0), Some(vec![root, clipper, b]));
    assert_eq!(hit(50.0, 50.0), Some(vec![root, a]), "b is clipped here");
    assert_eq!(hit(5.0, 5.0), Some(vec![root, clipper]));
    assert_eq!(hit(90.0, 90.0), Some(vec![root]));
    assert_eq!(hit(150.0, 10.0), None);
}

#[test]
fn a_press_beside_a_control_reaches_the_nearest_target_and_the_topmost_on_a_tie() {
    let mut tree = Tree::<M>::new(10);
    let view = Element::new(group(vec![
        Element::new(leaf("a")),
        Element::new(leaf("b")),
    ]));
    let root = tree.reconcile(None, view).unwrap();
    let [a, b] = tree.get(root).unwrap().children[..] else {
        unreachable!()
    };
    let mut geometry = Geometry::default();
    place(&mut geometry, root, Rect::new(0.0, 0.0, 100.0, 100.0), None);
    for (id, x) in [(a, 10.0), (b, 40.0)] {
        let rect = Rect::new(x, 10.0, 20.0, 20.0);
        geometry.insert(
            id,
            Placement {
                rect,
                target: rect.outset(6.0),
                clip: None,
                own_clip: None,
                raised: Default::default(),
            },
        );
    }

    let hit = |x, y| hit_test(&tree, root, &geometry, Point::new(x, y));
    assert_eq!(hit(5.0, 20.0), Some(vec![root, a]), "a lone outset");
    assert_eq!(hit(34.5, 20.0), Some(vec![root, a]), "a is nearer");
    assert_eq!(hit(35.5, 20.0), Some(vec![root, b]), "b is nearer");
    assert_eq!(
        hit(35.0, 20.0),
        Some(vec![root, b]),
        "a tie goes to the topmost"
    );
    assert_eq!(hit(35.0, 45.0), Some(vec![root]), "outside every target");
}

#[test]
fn a_rect_that_holds_the_point_beats_a_target_that_reaches_it() {
    let mut tree = Tree::<M>::new(10);
    let view = Element::new(group(vec![
        Element::new(leaf("under")),
        Element::new(leaf("over")),
    ]));
    let root = tree.reconcile(None, view).unwrap();
    let [under, over] = tree.get(root).unwrap().children[..] else {
        unreachable!()
    };
    let mut geometry = Geometry::default();
    place(&mut geometry, root, Rect::new(0.0, 0.0, 100.0, 100.0), None);
    place(&mut geometry, under, Rect::new(0.0, 0.0, 30.0, 30.0), None);
    let rect = Rect::new(32.0, 0.0, 20.0, 20.0);
    geometry.insert(
        over,
        Placement {
            rect,
            target: rect.outset(6.0),
            clip: None,
            own_clip: None,
            raised: Default::default(),
        },
    );

    let hit = |x, y| hit_test(&tree, root, &geometry, Point::new(x, y));
    assert_eq!(hit(29.0, 10.0), Some(vec![root, under]));
    assert_eq!(hit(31.0, 10.0), Some(vec![root, over]));
}

#[test]
fn a_node_is_hit_only_inside_its_target_and_every_clip_above_it() {
    let placement = Placement {
        rect: Rect::new(0.0, 0.0, 100.0, 20.0),
        target: Rect::new(0.0, -2.0, 100.0, 24.0),
        clip: Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        own_clip: None,
        raised: Default::default(),
    };
    assert!(placement.hit(Point::new(10.0, 10.0)));
    assert!(placement.hit(Point::new(10.0, 21.0)), "inside the target");
    assert!(!placement.hit(Point::new(10.0, -1.0)), "clipped away");
    assert!(!placement.hit(Point::new(70.0, 10.0)), "clipped away");
    assert!(!placement.hit(Point::new(10.0, 30.0)), "outside the target");
}

/// A node that records each delivery as `label:phase` and stops the event in the phase `stop` names.
struct Probe {
    label: usize,
    stop: Option<Phase>,
    children: Vec<Element<M>>,
}

impl Widget<M> for Probe {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
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

    fn event(&self, _: &mut (), _: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        cx.emit(format!("{}:{:?}", self.label, cx.phase()));
        if self.stop == Some(cx.phase()) {
            Propagation::Stop
        } else {
            Propagation::Continue
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The element for node `label` of the tree `parents` describes (`parents[i] < i` for every node but the root).
fn probe(parents: &[usize], label: usize, stop: Option<(usize, Phase)>) -> Element<M> {
    let children = (1..parents.len())
        .filter(|&child| parents[child] == label)
        .map(|child| probe(parents, child, stop))
        .collect();
    Element::new(Probe {
        label,
        stop: stop.filter(|(at, _)| *at == label).map(|(_, phase)| phase),
        children,
    })
}

/// The node of each label, found by walking the mounted tree in the order `probe` built it.
fn ids(tree: &Tree<M>, parents: &[usize], label: usize, id: NodeId, out: &mut [NodeId]) {
    out[label] = id;
    let kids = (1..parents.len()).filter(|&child| parents[child] == label);
    for (child, &node) in kids.zip(&tree.get(id).unwrap().children) {
        ids(tree, parents, child, node, out);
    }
}

fn trees() -> impl Strategy<Value = Vec<usize>> {
    (1usize..24).prop_flat_map(|n| {
        (1..n)
            .map(|i| (0..i).boxed())
            .collect::<Vec<_>>()
            .prop_map(|rest| std::iter::once(0).chain(rest).collect())
    })
}

proptest! {
    #[test]
    fn delivery_captures_from_the_root_bubbles_back_in_reverse_and_ends_at_a_stop(
        (parents, target, stop) in trees().prop_flat_map(|parents| {
            let n = parents.len();
            (
                Just(parents),
                0..n,
                prop::option::of((0..n, prop_oneof![
                    Just(Phase::Capture),
                    Just(Phase::Target),
                    Just(Phase::Bubble),
                ])),
            )
        })
    ) {
        let mut tree = Tree::new(64);
        let root = tree.reconcile(None, probe(&parents, 0, stop)).unwrap();
        let mut id = vec![root; parents.len()];
        ids(&tree, &parents, 0, root, &mut id);

        let mut labels = vec![target];
        while let Some(&last) = labels.last().filter(|&&l| l != 0) {
            labels.push(parents[last]);
        }
        labels.reverse();
        let path: Vec<NodeId> = labels.iter().map(|&l| id[l]).collect();

        let (ancestors, _) = labels.split_at(labels.len() - 1);
        let steps: Vec<(usize, Phase)> = ancestors
            .iter()
            .map(|&l| (l, Phase::Capture))
            .chain([(target, Phase::Target)])
            .chain(ancestors.iter().rev().map(|&l| (l, Phase::Bubble)))
            .collect();
        let end = stop.and_then(|s| steps.iter().position(|step| *step == s));
        let expected: Vec<String> = steps[..end.map_or(steps.len(), |e| e + 1)]
            .iter()
            .map(|(l, phase)| format!("{l}:{phase:?}"))
            .collect();

        let (result, seen) = route(&mut tree, &path);
        prop_assert_eq!(seen, expected);
        prop_assert_eq!(
            result,
            if end.is_some() { Propagation::Stop } else { Propagation::Continue }
        );
    }
}

#[test]
fn a_raised_node_takes_a_point_over_the_siblings_after_it() {
    let mut tree = Tree::<M>::new(10);
    let view = Element::new(group(vec![
        Element::new(group(vec![Element::new(leaf("header"))])),
        Element::new(leaf("rows")),
    ]));
    let root = tree.reconcile(None, view).unwrap();
    let [holder, rows] = tree.get(root).unwrap().children[..] else {
        unreachable!()
    };
    let header = tree.get(holder).unwrap().children[0];
    let mut geometry = Geometry::default();
    place(&mut geometry, root, Rect::new(0.0, 0.0, 100.0, 100.0), None);
    place(
        &mut geometry,
        holder,
        Rect::new(0.0, 0.0, 100.0, 30.0),
        None,
    );
    place(
        &mut geometry,
        header,
        Rect::new(0.0, 0.0, 100.0, 30.0),
        None,
    );
    place(&mut geometry, rows, Rect::new(0.0, 0.0, 100.0, 100.0), None);
    let hit = |geometry: &Geometry, y| hit_test(&tree, root, geometry, Point::new(50.0, y));
    assert_eq!(
        hit(&geometry, 10.0),
        Some(vec![root, rows]),
        "the later sibling"
    );

    for (id, raised) in [
        (root, Raised::Holds),
        (holder, Raised::Holds),
        (header, Raised::In),
    ] {
        geometry.get_mut(&id).unwrap().raised = raised;
    }
    assert_eq!(
        hit(&geometry, 10.0),
        Some(vec![root, holder, header]),
        "the raised header, over the rows after it"
    );
    assert_eq!(
        hit(&geometry, 50.0),
        Some(vec![root, rows]),
        "below it, the rows"
    );
}
