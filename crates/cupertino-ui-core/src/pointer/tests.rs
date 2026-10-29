use super::*;
use crate::cx::{Effects, Status};
use crate::route::Placement;
use crate::test_support::{env, group, leaf, Leaf};
use crate::tree::Tree;
use crate::Element;
use cupertino_geometry::Rect;
use cupertino_ui_platform::MouseButton;

type M = String;

struct Fixture {
    tree: Tree<M>,
    root: NodeId,
    geometry: Geometry,
    pointer: Pointer,
}

/// Two buttons side by side in a 100×50 window: `a` on the left captures the pointer while pressed.
fn fixture() -> Fixture {
    let mut tree = Tree::new(10);
    let view = Element::new(group(vec![
        Element::new(Leaf {
            capture: true,
            ..leaf("a")
        }),
        Element::new(leaf("b")),
    ]));
    let root = tree.reconcile(None, view).unwrap();
    let [a, b] = tree.get(root).unwrap().children[..] else {
        unreachable!()
    };
    let mut geometry = Geometry::new();
    for (id, rect) in [
        (root, Rect::new(0.0, 0.0, 100.0, 50.0)),
        (a, Rect::new(0.0, 0.0, 50.0, 50.0)),
        (b, Rect::new(50.0, 0.0, 50.0, 50.0)),
    ] {
        geometry.insert(
            id,
            Placement {
                rect,
                clip: None,
                own_clip: None,
            },
        );
    }
    Fixture {
        tree,
        root,
        geometry,
        pointer: Pointer::default(),
    }
}

impl Fixture {
    fn send(&mut self, event: PointerEvent) -> Vec<String> {
        let env = env();
        let mut effects = Effects::default();
        let mut route = Route {
            tree: &mut self.tree,
            geometry: &self.geometry,
            env: &env,
            status: Status {
                focus: None,
                focus_visible: false,
                capture: None,
                window_active: true,
                now: Timestamp(0),
            },
            effects: &mut effects,
        };
        self.pointer
            .handle(&mut route, self.root, &self.geometry, event, Timestamp(0));
        effects
            .messages
            .into_iter()
            .filter(|m| !m.starts_with("group"))
            .collect()
    }
}

fn motion(x: f64) -> PointerEvent {
    PointerEvent::Motion {
        position: Point::new(x, 10.0),
    }
}

fn button(x: f64, pressed: bool) -> PointerEvent {
    PointerEvent::Button {
        position: Point::new(x, 10.0),
        button: MouseButton::Left,
        pressed,
        serial: 1,
    }
}

#[test]
fn moving_across_siblings_leaves_one_and_enters_the_other() {
    let mut f = fixture();
    assert_eq!(f.send(motion(10.0)), ["a:Target:enter", "a:Target:motion"]);
    assert_eq!(
        f.send(motion(60.0)),
        ["a:Target:leave", "b:Target:enter", "b:Target:motion"]
    );
}

#[test]
fn a_captured_pointer_goes_to_its_node_wherever_it_is_until_released() {
    let mut f = fixture();
    f.send(motion(10.0));
    assert_eq!(f.send(button(10.0, true)), ["a:Target:press"]);
    assert!(f.pointer.capture().is_some());

    let over_b = f.send(motion(60.0));
    assert_eq!(
        over_b,
        ["a:Target:leave", "b:Target:enter", "a:Target:motion"]
    );
    assert_eq!(f.send(button(60.0, false)), ["a:Target:release"]);
    assert_eq!(f.pointer.capture(), None);
    assert_eq!(f.send(motion(61.0)), ["b:Target:motion"]);
}

#[test]
fn leaving_the_surface_cancels_the_capture() {
    let mut f = fixture();
    f.send(motion(10.0));
    f.send(button(10.0, true));
    assert_eq!(
        f.send(PointerEvent::Leave),
        ["a:Target:leave", "a:Target:cancel"]
    );
    assert_eq!(f.pointer.capture(), None);
}
