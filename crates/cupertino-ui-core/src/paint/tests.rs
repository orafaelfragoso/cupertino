use super::*;
use crate::route::Placement;
use crate::test_support::{env, spec};
use crate::{Change, Element, Event, EventCx, Icons, MeasureCx, Propagation, SemanticsCx, Widget};
use cupertino_geometry::Point;
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_render::{Effect, Paint, Shape};
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
    /// One environment for every frame, as a window keeps.
    env: Environment,
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
            env: env(),
        }
    }

    fn view(paints: &Rc<Cell<u32>>, label: &'static str) -> Element<M> {
        Element::new(Swatch {
            label,
            paints: Rc::clone(paints),
        })
    }

    fn frame(&mut self, rect: Rect, clip: Option<Rect>, focus: bool) -> PaintNode {
        self.painting(rect, clip, focus).list.nodes.remove(0)
    }

    fn painting(&mut self, rect: Rect, clip: Option<Rect>, focus: bool) -> Painting {
        let geometry = Geometry::from([(
            self.root,
            Placement {
                rect,
                target: rect,
                clip,
                own_clip: None,
            },
        )]);
        self.painter.paint(
            &self.tree,
            self.root,
            &geometry,
            &PaintInput {
                env: &self.env,
                focus: focus.then_some(self.root),
                focus_visible: true,
                window_active: true,
                background: WindowBackground::Theme,
                backdrop: false,
                materials: false,
                now: Timestamp(0),
            },
        )
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

    f.env.scale = 2.0;
    f.frame(Rect::new(50.0, 20.0, 31.0, 40.0), None, true);
    assert_eq!(f.paints.get(), 5, "another environment repaints");
    f.env.theme = Arc::new((*f.env.theme).clone());
    f.frame(Rect::new(50.0, 20.0, 31.0, 40.0), None, true);
    assert_eq!(f.paints.get(), 6, "so does another theme");
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
                target: viewport,
                clip: None,
                own_clip: Some(viewport),
            },
        ),
        (
            child,
            Placement {
                rect: Rect::new(10.0, -20.0, 50.0, 200.0),
                target: Rect::new(10.0, -20.0, 50.0, 200.0),
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
            background: WindowBackground::Theme,
            backdrop: false,
            materials: false,
            now: Timestamp(0),
        },
    );
    let [scroller, content] = &list.list.nodes[..] else {
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

/// Casts a shadow of its whole subtree.
struct Shadowed;

impl Widget<M> for Shadowed {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.set_group(
            0.5,
            vec![Effect::DropShadow {
                offset: Point::new(0.0, 4.0),
                blur: 2.0,
                color: Color::new(0.0, 0.0, 0.0, 0.5),
            }],
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[test]
fn a_group_reaches_the_renderer_and_its_effects_widen_the_subtree_bounds() {
    let mut tree = Tree::<M>::new(2);
    let root = tree.reconcile(None, Element::new(Shadowed)).unwrap();
    let rect = Rect::new(10.0, 10.0, 20.0, 20.0);
    let geometry = Geometry::from([(
        root,
        Placement {
            rect,
            target: rect,
            clip: None,
            own_clip: None,
        },
    )]);
    let env = env();
    let painting = Painter::default().paint(
        &tree,
        root,
        &geometry,
        &PaintInput {
            env: &env,
            focus: None,
            focus_visible: false,
            window_active: true,
            background: WindowBackground::Theme,
            backdrop: false,
            materials: false,
            now: Timestamp(0),
        },
    );
    let node = &painting.list.nodes[0];
    assert_eq!(node.group.as_ref().map(|g| g.opacity), Some(0.5));
    assert_eq!(node.bounds, Rect::from_ltrb(4.0, 8.0, 36.0, 40.0));
    assert_eq!(
        painting.own[0], rect,
        "damage still tracks what the node itself draws"
    );
    assert_eq!(
        painting.reach[0], None,
        "its opacity reaches its children, which its own operations do not tell"
    );
}

/// Draws one image over its whole size and counts how often it painted.
struct Pictured {
    image: crate::IconImage,
    paints: Rc<Cell<u32>>,
}

impl Widget<M> for Pictured {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        self.paints.set(self.paints.get() + 1);
        let dst = Rect::from_origin_size(Point::new(0.0, 0.0), cx.size());
        cx.image(
            self.image.id,
            &self.image.image,
            dst,
            cupertino_ui_render::Sampling::Linear,
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[test]
fn an_image_a_node_draws_travels_with_the_list_and_stays_when_the_node_is_reused() {
    let image = crate::PlaceholderIcons::default().symbol(
        "symbol.house",
        16,
        Color::new(0.0, 0.0, 0.0, 1.0),
        false,
    );
    let paints = Rc::default();
    let mut tree = Tree::<M>::new(2);
    let root = tree
        .reconcile(
            None,
            Element::new(Pictured {
                image: image.clone(),
                paints: Rc::clone(&paints),
            }),
        )
        .unwrap();
    let geometry = Geometry::from([(
        root,
        Placement {
            rect: Rect::new(4.0, 4.0, 16.0, 16.0),
            target: Rect::new(4.0, 4.0, 16.0, 16.0),
            clip: None,
            own_clip: None,
        },
    )]);
    let env = env();
    let mut painter = Painter::default();
    let mut paint = || {
        painter.paint(
            &tree,
            root,
            &geometry,
            &PaintInput {
                env: &env,
                focus: None,
                focus_visible: false,
                window_active: true,
                background: WindowBackground::Theme,
                backdrop: false,
                materials: false,
                now: Timestamp(0),
            },
        )
    };
    for frame in 0..2 {
        let painting = paint();
        assert_eq!(
            painting.list.nodes[0].ops,
            vec![PaintOp::Image {
                image: cupertino_ui_render::ResourceRef {
                    id: image.id,
                    revision: 1
                },
                src: Rect::new(0.0, 0.0, 16.0, 16.0),
                dst: Rect::new(0.0, 0.0, 16.0, 16.0),
                sampling: cupertino_ui_render::Sampling::Linear,
            }],
            "frame {frame}"
        );
        assert_eq!(
            painting.resources.images.get(&image.id),
            Some(&image.image),
            "frame {frame}: the pixels travel with the list"
        );
    }
    assert_eq!(paints.get(), 1, "the second frame reused the node");
}

/// Asks for the popover material under itself and paints the opaque fill where the window does not show it.
struct Platter;

impl Widget<M> for Platter {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !cx.material(cupertino_ui_theme::Surface::Popover, 6.0, 0.5) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::from_origin_size(Point::new(0.0, 0.0), cx.size())),
                paint: Paint::Solid(Color::new(0.5, 0.5, 0.5, 1.0)),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[test]
fn a_node_s_material_is_cut_to_what_shows_and_capped_per_window_and_a_refused_one_is_painted() {
    let platters = MAX_MATERIALS + 2;
    let mut tree = Tree::<M>::new(platters + 2);
    let children = (0..platters).map(|_| Element::new(Platter)).collect();
    let root = tree
        .reconcile(None, Element::new(crate::test_support::group(children)))
        .unwrap();
    let ids = tree.get(root).unwrap().children.clone();
    let viewport = Rect::new(0.0, 0.0, 100.0, 1000.0);
    let mut geometry = Geometry::from([(
        root,
        Placement {
            rect: viewport,
            target: viewport,
            clip: None,
            own_clip: Some(viewport),
        },
    )]);
    // the first platter is half scrolled out of the viewport's top, the others below it, 20 pt apart
    for (i, id) in ids.iter().enumerate() {
        let rect = Rect::new(10.0, i as f64 * 20.0 - 5.0, 80.0, 10.0);
        geometry.insert(
            *id,
            Placement {
                rect,
                target: rect,
                clip: Some(viewport),
                own_clip: None,
            },
        );
    }
    let env = env();
    let mut painter = Painter::default();
    let mut paint = |materials| {
        painter.paint(
            &tree,
            root,
            &geometry,
            &PaintInput {
                env: &env,
                focus: None,
                focus_visible: false,
                window_active: true,
                background: WindowBackground::None,
                backdrop: false,
                materials,
                now: Timestamp(0),
            },
        )
    };
    let filled = |painting: &Painting| {
        painting
            .list
            .nodes
            .iter()
            .filter(|n| !n.ops.is_empty())
            .count()
    };

    let shown = paint(true);
    assert_eq!(
        shown.materials.len(),
        MAX_MATERIALS,
        "no more than a window shows"
    );
    assert_eq!(
        shown.materials[0],
        MaterialRegion {
            material: "Menu".into(),
            rect: Rect::new(10.0, 0.0, 80.0, 5.0),
            radius: 6.0,
            alpha: 0.5,
        },
        "the popover's material, in window coordinates, cut by the viewport"
    );
    assert_eq!(shown.materials[1].rect, Rect::new(10.0, 15.0, 80.0, 10.0));
    assert_eq!(
        filled(&shown),
        2,
        "the platters past the limit paint their fill"
    );

    let refused = paint(false);
    assert!(refused.materials.is_empty());
    assert_eq!(
        filled(&refused),
        platters,
        "every platter paints its fill when the window shows none"
    );
}

/// Whether `outer` holds all of `inner`.
fn covers(outer: Rect, inner: Rect) -> bool {
    outer.min_x() <= inner.min_x()
        && outer.min_y() <= inner.min_y()
        && outer.max_x() >= inner.max_x()
        && outer.max_y() >= inner.max_y()
}

fn fill(rect: Rect, alpha: f32) -> PaintOp {
    PaintOp::Fill {
        shape: Shape::Rect(rect),
        paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha)),
    }
}

#[test]
fn operations_that_compare_equal_change_nothing_and_a_changed_one_changes_where_it_was_and_is() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let b = Rect::new(50.0, 0.0, 10.0, 10.0);
    let c = Rect::new(80.0, 0.0, 10.0, 10.0);
    let old = [fill(a, 1.0), fill(b, 1.0)];
    assert_eq!(
        changed_region(&old, &old),
        Some(Rect::new(0.0, 0.0, 0.0, 0.0))
    );
    assert_eq!(
        changed_region(&old, &[fill(a, 1.0), fill(b, 0.5)]),
        Some(b),
        "the unchanged fill is not repainted"
    );
    assert_eq!(
        changed_region(&old, &[fill(a, 1.0), fill(c, 1.0)]),
        Some(b.union(&c))
    );
}

#[test]
fn an_operation_that_comes_or_goes_between_equal_ones_changes_only_where_it_reaches() {
    let a = Rect::new(0.0, 0.0, 100.0, 20.0);
    let caret = Rect::new(40.0, 4.0, 1.0, 12.0);
    let c = Rect::new(60.0, 0.0, 10.0, 10.0);
    let shown = [fill(a, 1.0), fill(caret, 1.0), fill(c, 1.0)];
    let hidden = [fill(a, 1.0), fill(c, 1.0)];
    assert_eq!(changed_region(&shown, &hidden), Some(caret));
    assert_eq!(changed_region(&hidden, &shown), Some(caret));
    assert_eq!(
        changed_region(&hidden, &[fill(a, 1.0), fill(c, 1.0), fill(caret, 1.0)]),
        Some(caret)
    );
    let clipped = [
        fill(a, 1.0),
        PaintOp::PushClip(Shape::Rect(caret)),
        PaintOp::PopClip,
        fill(c, 1.0),
    ];
    assert_eq!(
        changed_region(&hidden, &clipped),
        Some(a.union(&c)),
        "a scope that comes or goes changes what paints after it"
    );
}

#[test]
fn a_changed_scope_or_layer_content_changes_everything_either_list_reaches() {
    let a = Rect::new(0.0, 0.0, 10.0, 10.0);
    let b = Rect::new(50.0, 0.0, 10.0, 10.0);
    let both = Some(a.union(&b));
    let clipped = |clip: Rect| {
        [
            fill(a, 1.0),
            PaintOp::PushClip(Shape::Rect(clip)),
            fill(b, 1.0),
            PaintOp::PopClip,
        ]
    };
    assert_eq!(changed_region(&clipped(b), &clipped(a)), both);
    let layered = |alpha: f32| {
        [
            fill(a, 1.0),
            PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![Effect::Blur { radius: 2.0 }],
            },
            fill(b, alpha),
            PaintOp::PopLayer,
        ]
    };
    let spread = changed_region(&layered(1.0), &layered(0.5)).unwrap();
    assert!(
        covers(spread, a) && covers(spread, b),
        "an effect spreads a change inside a layer: {spread:?}"
    );
}

#[test]
fn a_stroke_s_change_reaches_its_miter_joins() {
    let a = Rect::new(10.0, 10.0, 10.0, 10.0);
    let stroke = |alpha: f32| PaintOp::Stroke {
        shape: Shape::Rect(a),
        width: 2.0,
        paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, alpha)),
    };
    let region = changed_region(&[stroke(1.0)], &[stroke(0.5)]).unwrap();
    assert!(
        covers(region, a.outset(2.0)),
        "past the half width: {region:?}"
    );
}

#[test]
fn a_node_painted_again_tells_where_it_reaches_and_what_changed_in_the_window() {
    let mut f = Fixture::new();
    let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
    let first = f.painting(rect, None, false);
    assert_eq!(
        first.changed[0], None,
        "a first paint has nothing to compare"
    );
    assert!(first.reach[0].is_some_and(|r| covers(r, rect)));
    let kept = f.painting(rect, None, false);
    assert_eq!(kept.changed[0], None, "a node that did not paint again");
    let focused = f.painting(rect, None, true);
    let changed = focused.changed[0].expect("painted again in place");
    assert!(
        covers(rect, changed) && changed.width() > 0.0,
        "only the swatch's own fill changed: {changed:?}"
    );
    let cut = f.painting(rect, Some(Rect::new(0.0, 0.0, 20.0, 30.0)), false);
    assert!(cut.reach[0].is_some_and(|r| r.max_x() <= 20.0 && r.max_y() <= 30.0));
}
