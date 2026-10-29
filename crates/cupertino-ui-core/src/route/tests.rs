use super::*;
use crate::test_support::{env, group, leaf, Leaf};
use crate::Element;
use cupertino_ui_platform::{KeyEvent, Keysym, Mods, Timestamp};

type M = String;

fn status() -> Status {
    Status {
        focus: None,
        focus_visible: false,
        capture: None,
        window_active: true,
        now: Timestamp(0),
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
        geometry: &Geometry::new(),
        env: &env,
        status: status(),
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
            clip,
            own_clip: None,
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
    let mut geometry = Geometry::new();
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
fn a_node_is_hit_only_inside_its_rect_and_every_clip_above_it() {
    let placement = Placement {
        rect: Rect::new(0.0, 0.0, 100.0, 20.0),
        clip: Some(Rect::new(0.0, 0.0, 50.0, 50.0)),
        own_clip: None,
    };
    assert!(placement.hit(Point::new(10.0, 10.0)));
    assert!(!placement.hit(Point::new(70.0, 10.0)), "clipped away");
    assert!(!placement.hit(Point::new(10.0, 30.0)), "outside the rect");
}
