use super::*;
use crate::route::Placement;
use crate::test_support::{env, spec};
use crate::{Change, Element, Event, EventCx, MeasureCx, Propagation, SemanticsCx, Widget};
use cupertino_geometry::Point;
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_render::{Paint, Shape};
use cupertino_ui_theme::Color;
use std::cell::Cell;
use std::rc::Rc;

type M = String;

/// Fills its whole size and counts how often it painted.
struct Swatch {
    label: &'static str,
    paints: Rc<Cell<u32>>,
}

impl Widget<M> for Swatch {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.label == self.label {
            Change::None
        } else {
            Change::Paint
        }
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        self.paints.set(self.paints.get() + 1);
        let alpha = if cx.focus_visible() { 0.5 } else { 1.0 };
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::from_origin_size(Point::new(0.0, 0.0), cx.size())),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

struct Fixture {
    tree: Tree<M>,
    root: NodeId,
    painter: Painter,
    paints: Rc<Cell<u32>>,
}

impl Fixture {
    fn new() -> Self {
        let paints = Rc::default();
        let mut tree = Tree::new(4);
        let root = tree.reconcile(None, Self::view(&paints, "a")).unwrap();
        Self {
            tree,
            root,
            painter: Painter::default(),
            paints,
        }
    }

    fn view(paints: &Rc<Cell<u32>>, label: &'static str) -> Element<M> {
        Element::new(Swatch {
            label,
            paints: Rc::clone(paints),
        })
    }

    fn frame(&mut self, rect: Rect, clip: Option<Rect>, focus: bool) -> PaintNode {
        let env = env();
        let geometry = Geometry::from([(
            self.root,
            Placement {
                rect,
                clip,
                own_clip: None,
            },
        )]);
        let mut list = self.painter.paint(
            &self.tree,
            self.root,
            &geometry,
            &PaintInput {
                env: &env,
                focus: focus.then_some(self.root),
                focus_visible: true,
                window_active: true,
                now: Timestamp(0),
            },
        );
        list.nodes.remove(0)
    }
}

#[test]
fn a_node_paints_again_only_when_it_changed_its_size_changed_or_its_focus_did() {
    let mut f = Fixture::new();
    let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
    let first = f.frame(rect, None, false);
    let again = f.frame(rect, None, false);
    assert_eq!(f.paints.get(), 1);
    assert_eq!(again.revision, first.revision);

    let moved = f.frame(Rect::new(50.0, 20.0, 30.0, 40.0), None, false);
    assert_eq!(f.paints.get(), 1, "moving keeps the paint");
    assert_eq!(moved.revision, first.revision);
    assert_eq!(moved.transform, Transform([1.0, 0.0, 0.0, 1.0, 50.0, 20.0]));

    f.frame(Rect::new(50.0, 20.0, 31.0, 40.0), None, false);
    assert_eq!(f.paints.get(), 2, "a new size repaints");

    let focused = f.frame(Rect::new(50.0, 20.0, 31.0, 40.0), None, true);
    assert_eq!(f.paints.get(), 3, "the focus ring is paint");
    assert_ne!(focused.revision, moved.revision);

    f.tree
        .reconcile(Some(f.root), Fixture::view(&f.paints, "b"))
        .unwrap();
    f.frame(Rect::new(50.0, 20.0, 31.0, 40.0), None, true);
    assert_eq!(f.paints.get(), 4, "a paint change repaints");
}

#[test]
fn an_ancestor_clip_cuts_the_hit_area_and_the_bounds() {
    let mut f = Fixture::new();
    let node = f.frame(
        Rect::new(10.0, 10.0, 100.0, 100.0),
        Some(Rect::new(0.0, 0.0, 60.0, 60.0)),
        false,
    );
    assert_eq!(node.clip, None, "the node clips no descendants of its own");
    assert_eq!(node.hit, Rect::new(10.0, 10.0, 50.0, 50.0));
    assert_eq!(node.bounds, Rect::new(10.0, 10.0, 50.0, 50.0));
}

#[test]
fn a_node_carries_the_clip_it_puts_on_its_children_and_bounds_that_cover_its_subtree() {
    let paints = Rc::default();
    let mut tree = Tree::<M>::new(4);
    let root = tree
        .reconcile(
            None,
            Element::new(crate::test_support::group(vec![Fixture::view(
                &paints, "child",
            )])),
        )
        .unwrap();
    let child = tree.get(root).unwrap().children[0];
    let viewport = Rect::new(10.0, 10.0, 50.0, 50.0);
    let geometry = Geometry::from([
        (
            root,
            Placement {
                rect: viewport,
                clip: None,
                own_clip: Some(viewport),
            },
        ),
        (
            child,
            Placement {
                rect: Rect::new(10.0, -20.0, 50.0, 200.0),
                clip: Some(viewport),
                own_clip: None,
            },
        ),
    ]);
    let env = env();
    let list = Painter::default().paint(
        &tree,
        root,
        &geometry,
        &PaintInput {
            env: &env,
            focus: None,
            focus_visible: false,
            window_active: true,
            now: Timestamp(0),
        },
    );
    let [scroller, content] = &list.nodes[..] else {
        unreachable!()
    };
    assert_eq!(content.parent, Some(scroller.id));
    assert_eq!(
        scroller.clip,
        Some(Shape::Rect(Rect::new(0.0, 0.0, 50.0, 50.0)))
    );
    assert_eq!(
        content.bounds, viewport,
        "the content is cut to the viewport"
    );
    assert_eq!(scroller.bounds, viewport);
}

#[test]
fn the_extent_covers_stroke_edges_and_effects_but_an_inner_shadow_stays_inside() {
    let square = Shape::Rect(Rect::new(0.0, 0.0, 10.0, 10.0));
    let fill = PaintOp::Fill {
        shape: square.clone(),
        paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
    };
    let stroke = PaintOp::Stroke {
        shape: square.clone(),
        width: 4.0,
        paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
    };
    assert_eq!(extent(&[stroke]), Some(Rect::new(-2.0, -2.0, 14.0, 14.0)));

    let layer = |effects| {
        [
            PaintOp::PushLayer {
                opacity: 1.0,
                effects,
            },
            fill.clone(),
            PaintOp::PopLayer,
        ]
    };
    let shadow = Effect::DropShadow {
        offset: Point::new(0.0, 5.0),
        blur: 2.0,
        color: Color::new(0.0, 0.0, 0.0, 0.5),
    };
    assert_eq!(
        extent(&layer(vec![shadow])),
        Some(Rect::from_ltrb(-6.0, -1.0, 16.0, 21.0))
    );
    assert_eq!(
        extent(&layer(vec![Effect::Blur { radius: 1.0 }])),
        Some(Rect::new(-3.0, -3.0, 16.0, 16.0))
    );
    let inner = Effect::InnerShadow {
        offset: Point::new(1.0, 1.0),
        blur: 3.0,
        color: Color::new(0.0, 0.0, 0.0, 0.5),
    };
    assert_eq!(
        extent(&layer(vec![inner])),
        Some(Rect::new(0.0, 0.0, 10.0, 10.0))
    );
    assert_eq!(extent(&[]), None);
}

#[test]
fn bounds_reach_past_the_layout_rect_to_what_the_node_draws_and_stop_at_its_clip() {
    let rect = Rect::new(100.0, 100.0, 10.0, 10.0);
    let ring = Some(Rect::new(-4.0, -4.0, 18.0, 18.0));
    assert_eq!(bounds(rect, ring, None), Rect::new(96.0, 96.0, 18.0, 18.0));
    assert_eq!(
        bounds(rect, ring, Some(Rect::new(98.0, 98.0, 100.0, 100.0))),
        Rect::new(98.0, 98.0, 16.0, 16.0)
    );
    assert_eq!(bounds(rect, None, None), rect);
}
