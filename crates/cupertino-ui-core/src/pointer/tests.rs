use super::*;
use crate::cx::{Effects, Status};
use crate::route::Placement;
use crate::test_support::{env, group, leaf, Leaf};
use crate::tree::Tree;
use crate::Element;
use cupertino_geometry::Rect;
use cupertino_ui_platform::{Mods, MouseButton};

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
                target: rect,
                clip: None,
                own_clip: None,
                raised: Default::default(),
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
        self.send_at(event, Timestamp(0))
    }

    fn send_at(&mut self, event: PointerEvent, time: Timestamp) -> Vec<String> {
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
                now: time,
                opening_press: None,
            },
            popups: &[],
            effects: &mut effects,
        };
        self.pointer
            .handle(&mut route, self.root, &self.geometry, event, time);
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
        mods: Mods::NONE,
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

fn scroll(x: f64, source: ScrollSource, stop: bool) -> PointerEvent {
    PointerEvent::Scroll {
        position: Point::new(x, 10.0),
        delta: Point::new(0.0, if stop { 0.0 } else { 5.0 }),
        value120: None,
        source,
        stop,
        inverted: (false, false),
    }
}

fn ms(ms: u64) -> Timestamp {
    Timestamp(ms * 1_000_000)
}

/// A finger's scroll gesture stays with the node its first event hit, even where the pointer is now over another,
/// until its stop; the next gesture goes where the pointer is.
#[test]
fn a_finger_scroll_keeps_its_first_target_until_it_stops() {
    let mut f = fixture();
    f.send(motion(10.0));
    assert_eq!(
        f.send_at(scroll(10.0, ScrollSource::Finger, false), ms(1)),
        ["a:Target:scroll"]
    );
    // the content moved: b is under the pointer now
    let moved = f.send_at(scroll(60.0, ScrollSource::Finger, false), ms(900));
    assert_eq!(
        moved,
        ["a:Target:leave", "b:Target:enter", "a:Target:scroll"]
    );
    assert_eq!(
        f.send_at(scroll(60.0, ScrollSource::Finger, true), ms(910)),
        ["a:Target:scroll"],
        "the stop goes to the gesture's target"
    );
    assert_eq!(
        f.send_at(scroll(60.0, ScrollSource::Finger, false), ms(920)),
        ["b:Target:scroll"]
    );
}

/// A wheel's gesture ends after a pause, a pointer motion or a scroll of another source.
#[test]
fn a_wheel_scroll_keeps_its_target_until_it_pauses_or_the_pointer_moves() {
    let mut f = fixture();
    f.send(motion(10.0));
    assert_eq!(
        f.send_at(scroll(10.0, ScrollSource::Wheel, false), ms(0)),
        ["a:Target:scroll"]
    );
    f.send_at(motion(60.0), ms(1));
    assert_eq!(
        f.send_at(scroll(60.0, ScrollSource::Wheel, false), ms(2)),
        ["b:Target:scroll"],
        "the motion aimed the wheel anew"
    );
    // the content moved under the still pointer: within the pause the wheel stays with b
    assert_eq!(
        f.send_at(scroll(10.0, ScrollSource::Wheel, false), ms(100)),
        ["b:Target:leave", "a:Target:enter", "b:Target:scroll"]
    );
    assert_eq!(
        f.send_at(scroll(10.0, ScrollSource::Finger, false), ms(150)),
        ["a:Target:scroll"],
        "another source starts a gesture of its own"
    );
    assert_eq!(
        f.send_at(scroll(60.0, ScrollSource::Wheel, false), ms(160)),
        ["a:Target:leave", "b:Target:enter", "b:Target:scroll"]
    );
    // after the pause the next notch goes where the pointer is
    f.send_at(scroll(10.0, ScrollSource::Wheel, false), ms(200));
    assert_eq!(
        f.send_at(scroll(10.0, ScrollSource::Wheel, false), ms(200 + 301)),
        ["a:Target:scroll"]
    );
}

/// Leaving the surface ends the gesture.
#[test]
fn leaving_the_surface_ends_the_scroll_gesture() {
    let mut f = fixture();
    f.send(motion(10.0));
    f.send_at(scroll(10.0, ScrollSource::Finger, false), ms(1));
    f.send(PointerEvent::Leave);
    assert!(f.pointer.scroll.is_none());
}
