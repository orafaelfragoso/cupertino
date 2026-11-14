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
        let geometry = Geometry::from_iter([(
            self.root,
            Placement {
                rect,
                target: rect,
                clip,
                own_clip: None,
                raised: Default::default(),
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
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
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
    assert_eq!(
        node.bounds,
        Rect::new(10.0, 10.0, 50.0 + CLIP_EDGE, 50.0 + CLIP_EDGE),
        "the bounds keep the pixels the clip's anti-aliased edge covers"
    );
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
    let geometry = Geometry::from_iter([
        (
            root,
            Placement {
                rect: viewport,
                target: viewport,
                clip: None,
                own_clip: Some(viewport),
                raised: Default::default(),
            },
        ),
        (
            child,
            Placement {
                rect: Rect::new(10.0, -20.0, 50.0, 200.0),
                target: Rect::new(10.0, -20.0, 50.0, 200.0),
                clip: Some(viewport),
                own_clip: None,
                raised: Default::default(),
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
            raised: &[],
            obscuring: &[],
            layer: false,
            pinned: &[None, None],
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
    let cut = Rect::from_ltrb(10.0, 10.0 - CLIP_EDGE, 60.0, 60.0 + CLIP_EDGE);
    assert_eq!(
        content.bounds, cut,
        "the content is cut to the viewport and its anti-aliased edge"
    );
    assert_eq!(scroller.bounds, cut);
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
        Rect::from_ltrb(98.0 - CLIP_EDGE, 98.0 - CLIP_EDGE, 114.0, 114.0)
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
    let geometry = Geometry::from_iter([(
        root,
        Placement {
            rect,
            target: rect,
            clip: None,
            own_clip: None,
            raised: Default::default(),
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
            raised: &[],
            obscuring: &[],
            layer: false,
            pinned: &[None, None],
        },
    );
    let node = &painting.list.nodes[0];
    assert_eq!(node.group.as_ref().map(|g| g.opacity), Some(0.5));
    assert_eq!(node.bounds, Rect::from_ltrb(4.0, 8.0, 36.0, 40.0));
    assert_eq!(
        painting.own[0], node.bounds,
        "its opacity and shadow reach all it and its subtree draw, so it damages its widened bounds"
    );
    assert!(
        !painting.blank[0],
        "its opacity reaches its children, so it is never taken to draw nothing"
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
    let geometry = Geometry::from_iter([(
        root,
        Placement {
            rect: Rect::new(4.0, 4.0, 16.0, 16.0),
            target: Rect::new(4.0, 4.0, 16.0, 16.0),
            clip: None,
            own_clip: None,
            raised: Default::default(),
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
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
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
    let mut geometry = Geometry::from_iter([(
        root,
        Placement {
            rect: viewport,
            target: viewport,
            clip: None,
            own_clip: Some(viewport),
            raised: Default::default(),
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
                raised: Default::default(),
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
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
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
            material: cupertino_ui_theme::DENSE_GLASS_MATERIAL.into(),
            glass: Some(cupertino_ui_platform::GlassRegion {
                style: cupertino_ui_platform::GlassStyle::Dense,
                container: 0,
                spacing: 0.0,
            }),
            rect: Rect::new(10.0, 0.0, 80.0, 5.0),
            radius: 6.0,
            alpha: 0.5,
            raised: false,
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

#[test]
fn a_node_that_draws_tells_the_damage_and_a_group_draws_even_with_no_operation() {
    let mut f = Fixture::new();
    let rect = Rect::new(10.0, 20.0, 30.0, 40.0);
    assert_eq!(
        f.painting(rect, None, false).blank,
        [false],
        "a swatch fills"
    );
}

#[test]
fn the_theme_and_the_platform_name_glass_alike() {
    use cupertino_ui_platform::GlassStyle;
    assert_eq!(
        GlassStyle::Regular.material(),
        cupertino_ui_theme::GLASS_MATERIAL
    );
    assert_eq!(
        GlassStyle::Clear.material(),
        cupertino_ui_theme::CLEAR_GLASS_MATERIAL
    );
    assert_eq!(
        GlassStyle::of_material(cupertino_ui_theme::GLASS_MATERIAL),
        Some(GlassStyle::Regular)
    );
    assert_eq!(GlassStyle::of_material("Menu"), None);
}

/// A capsule of glass in container 2, blending over 8 points.
struct GlassButton;

impl Widget<M> for GlassButton {
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
        cx.glass(
            crate::Glass::clear()
                .capsule()
                .container(2, 8.0)
                .alpha(f64::NAN),
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[test]
fn glass_nodes_join_their_container_with_round_ends() {
    use cupertino_ui_platform::{GlassRegion, GlassStyle};
    let mut tree = Tree::<M>::new(4);
    let children = (0..2).map(|_| Element::new(GlassButton)).collect();
    let root = tree
        .reconcile(None, Element::new(crate::test_support::group(children)))
        .unwrap();
    let ids = tree.get(root).unwrap().children.clone();
    let window = Rect::new(0.0, 0.0, 300.0, 100.0);
    let mut geometry = Geometry::from_iter([(
        root,
        Placement {
            rect: window,
            target: window,
            clip: None,
            own_clip: Some(window),
            raised: Default::default(),
        },
    )]);
    for (i, id) in ids.iter().enumerate() {
        let rect = Rect::new(10.0 + i as f64 * 110.0, 20.0, 100.0, 36.0);
        geometry.insert(
            *id,
            Placement {
                rect,
                target: rect,
                clip: None,
                own_clip: None,
                raised: Default::default(),
            },
        );
    }
    let env = env();
    let mut painter = Painter::default();
    let painting = painter.paint(
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
            materials: true,
            now: Timestamp(0),
            raised: &[],
            obscuring: &[],
            layer: false,
            pinned: &[None, None],
        },
    );
    let glass = GlassRegion {
        style: GlassStyle::Clear,
        container: 2,
        spacing: 8.0,
    };
    assert_eq!(
        painting.materials,
        vec![
            MaterialRegion {
                material: "ClearGlass".into(),
                rect: Rect::new(10.0, 20.0, 100.0, 36.0),
                radius: 18.0,
                alpha: 1.0,
                glass: Some(glass),
                raised: false,
            },
            MaterialRegion {
                material: "ClearGlass".into(),
                rect: Rect::new(120.0, 20.0, 100.0, 36.0),
                radius: 18.0,
                alpha: 1.0,
                glass: Some(glass),
                raised: false,
            },
        ],
        "both capsules, in the same container, opaque for an alpha that is not finite"
    );
}

/// A panel that is glass when `glass`, with children; it paints a label in the theme's text colour after asking.
struct GlassPanel {
    glass: bool,
    children: Vec<Element<M>>,
}

/// A label in the theme's text colour and secondary text colour, as a fill each.
struct TextSwatch;

fn text_fills(cx: &mut PaintCx<'_>) {
    for role in [ColorRole::Text, ColorRole::SecondaryText] {
        let color = cx
            .env()
            .theme
            .colors
            .get(role, StateVariant::NORMAL)
            .unwrap();
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 10.0, 10.0)),
            paint: Paint::Solid(color),
        });
    }
}

impl Widget<M> for GlassPanel {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.glass {
            cx.glass(crate::Glass::regular().capsule());
        }
        text_fills(cx);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl Widget<M> for TextSwatch {
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
        text_fills(cx);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The colours of the fills a node painted, in order.
fn fills(node: &PaintNode) -> Vec<Color> {
    node.ops
        .iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                paint: Paint::Solid(c),
                ..
            } => Some(*c),
            _ => None,
        })
        .collect()
}

#[test]
fn text_on_glass_takes_the_matrix_of_its_face_and_text_beside_it_keeps_the_theme() {
    use cupertino_ui_platform::GlassTone;
    use cupertino_ui_theme::{Appearance, ColorTokens};
    // the dark appearance's labels: white at 85 % and at 55 %
    let label = Color::new(1.0, 1.0, 1.0, 0.85);
    let secondary = Color::new(1.0, 1.0, 1.0, 0.55);
    let mut env = env();
    let mut theme = (*env.theme).clone();
    theme.preferences.appearance = Appearance::Dark;
    theme.colors = ColorTokens::new([
        ((ColorRole::Text, StateVariant::NORMAL), label),
        ((ColorRole::SecondaryText, StateVariant::NORMAL), secondary),
    ]);
    env.theme = Arc::new(theme);

    // a small glass capsule with a label inside it, and a plain label beside it
    let mut tree = Tree::<M>::new(8);
    let panel = Element::new(GlassPanel {
        glass: true,
        children: vec![Element::new(TextSwatch)],
    });
    let plain = Element::new(GlassPanel {
        glass: false,
        children: vec![Element::new(TextSwatch)],
    });
    let root = tree
        .reconcile(
            None,
            Element::new(crate::test_support::group(vec![panel, plain])),
        )
        .unwrap();
    let [panel, plain] = tree.get(root).unwrap().children.clone()[..] else {
        unreachable!()
    };
    let inner = |id: NodeId| tree.get(id).unwrap().children[0];
    let window = Rect::new(0.0, 0.0, 300.0, 100.0);
    let mut geometry = Geometry::default();
    for (id, rect) in [
        (root, window),
        (panel, Rect::new(10.0, 20.0, 100.0, 36.0)),
        (inner(panel), Rect::new(20.0, 24.0, 60.0, 20.0)),
        (plain, Rect::new(130.0, 20.0, 100.0, 36.0)),
        (inner(plain), Rect::new(140.0, 24.0, 60.0, 20.0)),
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
    let mut painter = Painter::default();
    let paint = |painter: &mut Painter| {
        let painting = painter.paint(
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
                materials: true,
                now: Timestamp(0),
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
            },
        );
        let of = |id: NodeId| {
            painting
                .list
                .nodes
                .iter()
                .find(|n| n.id == id.into())
                .map(fills)
                .unwrap()
        };
        (
            of(panel),
            of(inner(panel)),
            of(plain),
            of(inner(plain)),
            painting.materials.len(),
        )
    };
    let white = |a: f32| Color::new(1.0, 1.0, 1.0, a);
    let black = |a: f32| Color::new(0.0, 0.0, 0.0, a);
    // a secondary label takes the strength of the side whose foreground it takes, or its own where that is more
    let secondary_on = |dark: bool| secondary.a.max(cupertino_ui_theme::vibrant_secondary(dark));
    let primary = cupertino_ui_theme::VIBRANT_PRIMARY;

    // nothing measured: the appearance's matrix, labels at their strength on glass, on the glass node and inside it
    let (glass, inside, beside, beside_inside, regions) = paint(&mut painter);
    assert_eq!(regions, 1);
    let on_glass = vec![white(primary), white(secondary_on(true))];
    assert_eq!((&glass, &inside), (&on_glass, &on_glass));
    // text beside the glass keeps the theme's colours
    assert_eq!(beside, vec![label, secondary]);
    assert_eq!(beside_inside, vec![label, secondary]);

    // the compositor measured a light face under the small capsule: its text turns dark, the rest stays
    painter.tones.regions = vec![GlassTone::Light];
    let (glass, inside, beside, _, _) = paint(&mut painter);
    let flipped = vec![black(primary), black(secondary_on(false))];
    assert_eq!((&glass, &inside), (&flipped, &flipped));
    assert_eq!(beside, vec![label, secondary]);

    // and back to dark
    painter.tones.regions = vec![GlassTone::Dark];
    let (glass, inside, _, _, _) = paint(&mut painter);
    assert_eq!((&glass, &inside), (&on_glass, &on_glass));
}

#[test]
fn text_drawn_after_a_glass_node_and_inside_it_lies_on_that_glass() {
    use cupertino_ui_platform::GlassTone;
    use cupertino_ui_theme::{Appearance, ColorTokens};
    let label = Color::new(1.0, 1.0, 1.0, 0.85);
    let secondary = Color::new(1.0, 1.0, 1.0, 0.55);
    let mut env = env();
    let mut theme = (*env.theme).clone();
    theme.preferences.appearance = Appearance::Dark;
    theme.colors = ColorTokens::new([
        ((ColorRole::Text, StateVariant::NORMAL), label),
        ((ColorRole::SecondaryText, StateVariant::NORMAL), secondary),
    ]);
    env.theme = Arc::new(theme);

    // a panel's glass as a node of its own, then its content beside it in the tree: a label over the glass, one
    // that straddles its edge and one past it
    let mut tree = Tree::<M>::new(8);
    let glass = Element::new(GlassPanel {
        glass: true,
        children: Vec::new(),
    });
    let labels = [TextSwatch, TextSwatch, TextSwatch].map(Element::new);
    let root = tree
        .reconcile(
            None,
            Element::new(crate::test_support::group(
                std::iter::once(glass).chain(labels).collect(),
            )),
        )
        .unwrap();
    let [glass, over, straddling, past] = tree.get(root).unwrap().children.clone()[..] else {
        unreachable!()
    };
    let mut geometry = Geometry::default();
    for (id, rect) in [
        (root, Rect::new(0.0, 0.0, 400.0, 200.0)),
        (glass, Rect::new(20.0, 20.0, 240.0, 120.0)),
        (over, Rect::new(40.0, 40.0, 80.0, 20.0)),
        (straddling, Rect::new(220.0, 40.0, 80.0, 20.0)),
        (past, Rect::new(300.0, 40.0, 80.0, 20.0)),
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
    let mut painter = Painter::default();
    let paint = |painter: &mut Painter| {
        let painting = painter.paint(
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
                materials: true,
                now: Timestamp(0),
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
            },
        );
        let of = |id: NodeId| {
            painting
                .list
                .nodes
                .iter()
                .find(|n| n.id == id.into())
                .map(fills)
                .unwrap()
        };
        [of(over), of(straddling), of(past)]
    };
    let vibrant = |c: Color| {
        // white is the dark side's foreground
        let dark = c.r > 0.5;
        vec![
            Color {
                a: cupertino_ui_theme::VIBRANT_PRIMARY,
                ..c
            },
            Color {
                a: secondary.a.max(cupertino_ui_theme::vibrant_secondary(dark)),
                ..c
            },
        ]
    };
    let theme = vec![label, secondary];
    let [over_glass, straddling, past] = paint(&mut painter);
    assert_eq!(over_glass, vibrant(Color::new(1.0, 1.0, 1.0, 1.0)));
    assert_eq!((&straddling, &past), (&theme, &theme));
    // the label over the glass follows the tone of the glass's face
    painter.tones.regions = vec![GlassTone::Light];
    let [over_glass, _, past] = paint(&mut painter);
    assert_eq!(over_glass, vibrant(Color::new(0.0, 0.0, 0.0, 1.0)));
    assert_eq!(past, theme);
}

#[test]
fn text_on_a_window_of_glass_takes_the_windows_tone() {
    use cupertino_ui_platform::GlassTone;
    use cupertino_ui_theme::{Appearance, ColorTokens, Surface};
    let label = Color::new(0.0, 0.0, 0.0, 0.85);
    let mut env = env();
    let mut theme = (*env.theme).clone();
    theme.preferences.appearance = Appearance::Light;
    theme.colors = ColorTokens::new([
        ((ColorRole::Text, StateVariant::NORMAL), label),
        ((ColorRole::SecondaryText, StateVariant::NORMAL), label),
    ]);
    env.theme = Arc::new(theme);
    let mut tree = Tree::<M>::new(4);
    let root = tree.reconcile(None, Element::new(TextSwatch)).unwrap();
    let window = Rect::new(0.0, 0.0, 300.0, 100.0);
    let geometry = Geometry::from_iter([(
        root,
        Placement {
            rect: window,
            target: window,
            clip: None,
            own_clip: None,
            raised: Default::default(),
        },
    )]);
    let mut painter = Painter::default();
    let paint = |painter: &mut Painter, backdrop: bool| {
        let painting = painter.paint(
            &tree,
            root,
            &geometry,
            &PaintInput {
                env: &env,
                focus: None,
                focus_visible: false,
                window_active: true,
                background: WindowBackground::Surface {
                    surface: Surface::Menu,
                    radius: 10.0,
                    alpha: 1.0,
                },
                backdrop,
                materials: true,
                now: Timestamp(0),
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
            },
        );
        fills(&painting.list.nodes[0])[0]
    };
    // a menu window whose glass the window system shows: its labels are on glass
    assert_eq!(
        paint(&mut painter, true),
        Color::new(0.0, 0.0, 0.0, cupertino_ui_theme::VIBRANT_PRIMARY)
    );
    painter.tones.window = GlassTone::Dark;
    assert_eq!(paint(&mut painter, true), Color::new(1.0, 1.0, 1.0, 1.0));
    // without the material the window paints its own fill, and its labels keep the theme's colours
    assert_eq!(paint(&mut painter, false), label);
}

/// What a node under [`Under`] asks for beneath itself.
#[derive(Clone, Copy)]
enum Beneath {
    Nothing,
    Surface(cupertino_ui_theme::Surface),
    Glass,
    LargePanel,
}

/// A box that asks for a material or glass under itself, around one child.
struct Asking {
    beneath: Beneath,
    children: Vec<Element<M>>,
}

impl Widget<M> for Asking {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        match self.beneath {
            Beneath::Nothing => {}
            Beneath::Surface(surface) => {
                cx.material(surface, 0.0, 1.0);
            }
            Beneath::Glass => {
                cx.glass(crate::Glass::regular());
            }
            Beneath::LargePanel => {
                cx.glass(crate::Glass::large_panel());
            }
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Records whether it lay on a material when it painted.
struct MaterialProbe(Rc<Cell<Option<bool>>>);

impl Widget<M> for MaterialProbe {
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
        self.0.set(Some(cx.on_material()));
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Whether a probe under a box asking for `beneath`, in a window with `background`, lies on a material, with the
/// window's materials shown or not.
fn probed(beneath: Beneath, background: WindowBackground, shown: bool) -> bool {
    let seen = Rc::new(Cell::new(None));
    let mut tree = Tree::<M>::new(4);
    let root = tree
        .reconcile(
            None,
            Element::new(Asking {
                beneath,
                children: vec![Element::new(MaterialProbe(Rc::clone(&seen)))],
            }),
        )
        .unwrap();
    let probe = tree.get(root).unwrap().children[0];
    let rect = Rect::new(0.0, 0.0, 100.0, 40.0);
    let geometry = Geometry::from_iter([root, probe].map(|id| {
        (
            id,
            Placement {
                rect,
                target: rect,
                clip: None,
                own_clip: None,
                raised: Default::default(),
            },
        )
    }));
    Painter::default().paint(
        &tree,
        root,
        &geometry,
        &PaintInput {
            env: &env(),
            focus: None,
            focus_visible: false,
            window_active: true,
            background,
            backdrop: shown,
            materials: shown,
            now: Timestamp(0),
            raised: &[],
            obscuring: &[],
            layer: false,
            pinned: &[None, None],
        },
    );
    seen.get().expect("the probe painted")
}

#[test]
fn a_node_lies_on_a_material_under_glass_a_translucent_surface_or_such_a_window_background() {
    use cupertino_ui_theme::Surface;
    let surface = |surface| WindowBackground::Surface {
        surface,
        radius: 0.0,
        alpha: 1.0,
    };
    for shown in [false, true] {
        let on = |beneath, background| probed(beneath, background, shown);
        assert!(!on(Beneath::Nothing, WindowBackground::Theme), "content");
        assert!(!on(Beneath::Nothing, WindowBackground::None));
        assert!(
            on(Beneath::Surface(Surface::Sidebar), WindowBackground::Theme),
            "a sidebar"
        );
        assert!(on(
            Beneath::Surface(Surface::Popover),
            WindowBackground::Theme
        ));
        assert!(on(Beneath::Glass, WindowBackground::Theme), "glass");
        assert!(
            on(Beneath::LargePanel, WindowBackground::Theme),
            "large-panel glass"
        );
        assert!(
            on(Beneath::Nothing, surface(Surface::Sidebar)),
            "a sidebar window"
        );
        assert!(on(Beneath::Nothing, surface(Surface::Menu)));
        // the window's and a form's own backgrounds drawn by the compositor are its content
        assert!(!on(
            Beneath::Surface(Surface::WindowBackground),
            WindowBackground::Theme
        ));
        assert!(!on(
            Beneath::Surface(Surface::TextBackground),
            WindowBackground::Theme
        ));
        assert!(!on(Beneath::Nothing, surface(Surface::WindowBackground)));
    }
}

/// Paints a clip and a fill on the window's device pixels, and a plain fill.
struct OnPixels(Rc<Cell<u32>>);

impl Widget<M> for OnPixels {
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
        self.0.set(self.0.get() + 1);
        cx.clip_pixels(Rect::new(0.3, 0.3, 20.0, 9.1));
        cx.fill_pixels(
            Rect::new(1.2, 2.7, 10.1, 0.2),
            Color::new(0.0, 0.0, 0.0, 1.0),
        );
        cx.push(PaintOp::PopClip);
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(1.2, 2.7, 10.1, 0.2)),
            paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, 1.0)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[test]
fn what_a_node_puts_on_device_pixels_lands_on_them_wherever_a_frame_draws_it() {
    let paints = Rc::new(Cell::new(0));
    let mut tree = Tree::<M>::new(4);
    let root = tree
        .reconcile(None, Element::new(OnPixels(Rc::clone(&paints))))
        .unwrap();
    let mut env = env();
    env.scale = 1.5;
    let mut painter = Painter::default();
    let mut revision = None;
    for origin in [
        Point::new(10.1, 20.35),
        Point::new(10.4, 20.0),
        Point::new(3.7, 0.9),
    ] {
        let rect = Rect::new(origin.x, origin.y, 40.0, 20.0);
        let geometry = Geometry::from_iter([(
            root,
            Placement {
                rect,
                target: rect,
                clip: None,
                own_clip: None,
                raised: Default::default(),
            },
        )]);
        let painting = painter.paint(
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
                materials: false,
                now: Timestamp(0),
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
            },
        );
        let node = &painting.list.nodes[0];
        assert_eq!(
            *revision.get_or_insert(node.revision),
            node.revision,
            "moving keeps the paint"
        );
        let in_pixels = |r: &Rect| {
            let r = r.translate(origin.x, origin.y);
            [r.min_x(), r.min_y(), r.max_x(), r.max_y()].map(|v| v * 1.5)
        };
        let whole = |v: f64| (v - v.round()).abs() < 1e-9;
        let [PaintOp::PushClip(Shape::Rect(clip)), PaintOp::Fill {
            shape: Shape::Rect(fill),
            ..
        }, PaintOp::PopClip, PaintOp::Fill {
            shape: Shape::Rect(plain),
            ..
        }] = &node.ops[..]
        else {
            panic!("the ops as painted: {:?}", node.ops);
        };
        let at = format!("at {origin:?}");
        assert!(in_pixels(clip).iter().all(|&v| whole(v)), "the clip {at}");
        assert!(in_pixels(fill).iter().all(|&v| whole(v)), "the fill {at}");
        // the clip widens to the pixels it touches; the fill moves to the nearest edges, a pixel high at least
        let [l, t, r, b] = in_pixels(&Rect::new(0.3, 0.3, 20.0, 9.1));
        assert_eq!(
            in_pixels(clip).map(f64::round),
            [l.floor(), t.floor(), r.ceil(), b.ceil()],
            "{at}"
        );
        let [l, t, r, _] = in_pixels(&Rect::new(1.2, 2.7, 10.1, 0.2));
        assert_eq!(
            in_pixels(fill).map(f64::round),
            [l.round(), t.round(), r.round(), t.round() + 1.0],
            "{at}"
        );
        assert_eq!(
            *plain,
            Rect::new(1.2, 2.7, 10.1, 0.2),
            "a plain fill stays as painted {at}"
        );
        // the pixels the node's bounds touch hold the fill grown to a pixel high
        let [l, t, r, b] = in_pixels(fill).map(f64::round);
        let bounds = node.bounds;
        let [bl, bt, br, bb] = [
            bounds.min_x(),
            bounds.min_y(),
            bounds.max_x(),
            bounds.max_y(),
        ]
        .map(|v| v * 1.5);
        assert!(
            bl.floor() <= l && bt.floor() <= t && br.ceil() >= r && bb.ceil() >= b,
            "{at}: {bounds:?} holds {fill:?}"
        );
    }
    assert_eq!(paints.get(), 1);
}

/// Asks for an overlay while its label is "caret": a 1-point bar 2 points in, clipped to its own rect. It holds a child.
struct Caret {
    label: &'static str,
    paints: Rc<Cell<u32>>,
    child: Option<Element<M>>,
}

impl Widget<M> for Caret {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        self.child.take().into_iter().collect()
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
        if self.label == "caret" {
            cx.overlay();
        }
    }

    fn overlay(&self, _: &(), cx: &mut OverlayCx<'_>) {
        let size = cx.size();
        cx.clip(Rect::from_origin_size(Point::new(0.0, 0.0), size));
        cx.fill_pixels(
            Rect::new(2.0, 1.0, 1.0, size.height - 2.0),
            Color::new(0.0, 0.0, 1.0, 1.0),
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A clipping viewport holding a field that asks for an overlay, with a child inside the field and a sibling after it.
struct Overlaid {
    tree: Tree<M>,
    root: NodeId,
    field: NodeId,
    inner: NodeId,
    after: NodeId,
    paints: Rc<Cell<u32>>,
    painter: Painter,
    env: Environment,
}

impl Overlaid {
    fn view(paints: &Rc<Cell<u32>>, label: &'static str) -> Element<M> {
        Element::new(crate::test_support::group(vec![
            Element::new(Caret {
                label,
                paints: Rc::clone(paints),
                child: Some(Fixture::view(paints, "inner")),
            }),
            Fixture::view(paints, "after"),
        ]))
    }

    fn new() -> Self {
        let paints = Rc::default();
        let mut tree = Tree::<M>::new(8);
        let root = tree.reconcile(None, Self::view(&paints, "caret")).unwrap();
        let [field, after] = tree.get(root).unwrap().children[..] else {
            unreachable!()
        };
        let inner = tree.get(field).unwrap().children[0];
        let mut env = env();
        env.scale = 1.5;
        Self {
            tree,
            root,
            field,
            inner,
            after,
            paints,
            painter: Painter::default(),
            env,
        }
    }

    /// Paints with the field's top-left corner at `at` in a viewport of 100 × 50 points at the window's corner.
    fn paint(&mut self, at: Point) -> Painting {
        let viewport = Rect::new(0.0, 0.0, 100.0, 50.0);
        let field = Rect::new(at.x, at.y, 60.0, 22.0);
        let placed = |rect: Rect, clip: Option<Rect>, own_clip: Option<Rect>| Placement {
            rect,
            target: rect,
            clip,
            own_clip,
            raised: Default::default(),
        };
        let geometry = Geometry::from_iter([
            (self.root, placed(viewport, None, Some(viewport))),
            (self.field, placed(field, Some(viewport), None)),
            (self.inner, placed(field, Some(viewport), None)),
            (
                self.after,
                placed(Rect::new(0.0, 60.0, 100.0, 10.0), Some(viewport), None),
            ),
        ]);
        self.painter.paint(
            &self.tree,
            self.root,
            &geometry,
            &PaintInput {
                env: &self.env,
                focus: None,
                focus_visible: false,
                window_active: true,
                background: WindowBackground::Theme,
                backdrop: false,
                materials: false,
                now: Timestamp(0),
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned: &[None, None],
            },
        )
    }
}

#[test]
fn an_overlay_comes_after_its_nodes_subtree_inside_its_clips_and_takes_no_hits() {
    let mut f = Overlaid::new();
    let caret = f.field;
    let painting = f.paint(Point::new(10.3, 37.1));
    let ids: Vec<PaintNodeId> = painting.list.nodes.iter().map(|n| n.id).collect();
    assert_eq!(
        ids,
        [
            f.root.into(),
            caret.into(),
            f.inner.into(),
            overlay_id(caret),
            f.after.into(),
        ],
        "the overlay closes its node's subtree, under what follows"
    );
    let overlay = &painting.list.nodes[3];
    assert_eq!(
        overlay.parent,
        Some(caret.into()),
        "the clips around its node cut it"
    );
    assert_eq!(overlay.transform, Transform::IDENTITY);
    assert_eq!(
        overlay.hit.width() * overlay.hit.height(),
        0.0,
        "it takes no hits"
    );
    assert!(
        overlay.bounds.max_y() <= 50.0 + CLIP_EDGE,
        "its bounds stop at the viewport: {:?}",
        overlay.bounds
    );
    let PaintOp::PushClip(Shape::Rect(clip)) = overlay.ops[0] else {
        panic!("the overlay's own clip first: {:?}", overlay.ops);
    };
    let whole = |v: f64| ((v * 1.5) - (v * 1.5).round()).abs() < 1e-9;
    assert!(
        [clip.min_x(), clip.min_y(), clip.max_x(), clip.max_y()]
            .into_iter()
            .all(whole),
        "its clip lies on device pixels: {clip:?}"
    );
    assert!(
        clip.min_x() <= 10.3
            && clip.min_y() <= 37.1
            && clip.max_x() >= 70.3
            && clip.max_y() >= 59.1,
        "and holds the whole node: {clip:?}"
    );
}

#[test]
fn an_overlay_keeps_its_revision_until_what_it_draws_changes_and_goes_with_its_request() {
    let mut f = Overlaid::new();
    let caret = f.field;
    let find = |painting: &Painting, id: PaintNodeId| {
        painting.list.nodes.iter().find(|n| n.id == id).cloned()
    };
    let first = f.paint(Point::new(10.3, 7.1));
    let paints = f.paints.get();
    let shown = find(&first, overlay_id(caret)).expect("the overlay");
    let again = f.paint(Point::new(10.3, 7.1));
    assert_eq!(
        find(&again, overlay_id(caret)).unwrap().revision,
        shown.revision,
        "drawn again in the same place it is the same paint"
    );
    // scrolled, the node keeps its paint and its overlay moves with it
    let scrolled = f.paint(Point::new(10.3, 2.6));
    assert_eq!(f.paints.get(), paints, "no node paints again");
    assert_eq!(
        find(&scrolled, caret.into()).unwrap().revision,
        find(&first, caret.into()).unwrap().revision
    );
    let moved = find(&scrolled, overlay_id(caret)).unwrap();
    assert_ne!(moved.revision, shown.revision, "moved, it is new paint");
    assert!(
        (moved.bounds.min_y() - (shown.bounds.min_y() - 4.5)).abs() <= 1.0 / 1.5,
        "it follows the node: {:?} then {:?}",
        shown.bounds,
        moved.bounds
    );
    // scrolled out of the viewport it draws nothing
    let out = f.paint(Point::new(10.3, 60.0));
    assert!(find(&out, overlay_id(caret)).is_none());
    // a node that paints again without asking has none
    f.tree
        .reconcile(Some(f.root), Overlaid::view(&f.paints, "plain"))
        .unwrap();
    let plain = f.paint(Point::new(10.3, 7.1));
    assert!(find(&plain, overlay_id(caret)).is_none());
    assert_eq!(plain.list.nodes.len(), 4);
}

/// A panel that pins its children to the dark appearance, or the light one, and paints a label itself.
struct Pinned {
    dark: bool,
    children: Vec<Element<M>>,
}

impl Widget<M> for Pinned {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.pin_appearance(self.dark);
        text_fills(cx);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[test]
fn a_subtree_pinned_to_an_appearance_paints_in_its_theme_and_the_rest_keeps_the_window_s() {
    use cupertino_ui_theme::{Appearance, ColorTokens};
    let themed = |appearance, text: Color, secondary: Color| {
        let mut theme = (*env().theme).clone();
        theme.preferences.appearance = appearance;
        theme.colors = ColorTokens::new([
            ((ColorRole::Text, StateVariant::NORMAL), text),
            ((ColorRole::SecondaryText, StateVariant::NORMAL), secondary),
        ]);
        Arc::new(theme)
    };
    let black = |a: f32| Color::new(0.0, 0.0, 0.0, a);
    let white = |a: f32| Color::new(1.0, 1.0, 1.0, a);
    let light = themed(Appearance::Light, black(0.85), black(0.5));
    let dark = themed(Appearance::Dark, white(0.85), white(0.55));
    let mut env = env();
    env.theme = Arc::clone(&light);

    // a dark panel holding a label and a light panel inside it, and a label beside the panel
    let mut tree = Tree::<M>::new(8);
    let panel = Element::new(Pinned {
        dark: true,
        children: vec![
            Element::new(TextSwatch),
            Element::new(Pinned {
                dark: false,
                children: vec![Element::new(TextSwatch)],
            }),
        ],
    });
    let root = tree
        .reconcile(
            None,
            Element::new(crate::test_support::group(vec![
                panel,
                Element::new(TextSwatch),
            ])),
        )
        .unwrap();
    let [panel, beside] = tree.get(root).unwrap().children.clone()[..] else {
        unreachable!()
    };
    let [label, light_panel] = tree.get(panel).unwrap().children.clone()[..] else {
        unreachable!()
    };
    let light_label = tree.get(light_panel).unwrap().children[0];
    let mut geometry = Geometry::default();
    for (i, id) in [root, panel, label, light_panel, light_label, beside]
        .into_iter()
        .enumerate()
    {
        let rect = Rect::new(i as f64 * 10.0, 0.0, 10.0, 10.0);
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
    let mut painter = Painter::default();
    let mut paint = |pinned: &[Option<Arc<ThemeTokens>>; 2]| {
        let painting = painter.paint(
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
                materials: false,
                now: Timestamp(0),
                raised: &[],
                obscuring: &[],
                layer: false,
                pinned,
            },
        );
        let of = |id: NodeId| {
            painting
                .list
                .nodes
                .iter()
                .find(|n| n.id == id.into())
                .map(fills)
                .unwrap()
        };
        (
            painting.pins,
            [of(panel), of(label), of(light_label), of(beside)],
        )
    };
    let in_light = vec![black(0.85), black(0.5)];
    let in_dark = vec![white(0.85), white(0.55)];

    // the pinning node paints in the window's appearance; its subtree in the dark one, down to a subtree pinned
    // back to light; the label beside the panel keeps the window's
    let (pins, [own, inside, back, beside]) = paint(&[Some(Arc::clone(&light)), Some(dark)]);
    assert!(pins);
    assert_eq!(own, in_light);
    assert_eq!(inside, in_dark);
    assert_eq!(back, in_light);
    assert_eq!(beside, in_light);

    // without the themes of the appearances, the subtree keeps the window's
    let (_, [_, inside, _, _]) = paint(&[None, None]);
    assert_eq!(inside, in_light);
}

#[test]
fn a_popover_glass_is_regular_glass_of_the_popover_style() {
    let popover = crate::cx::Glass::popover();
    let regular = crate::cx::Glass::regular();
    assert_eq!(popover.style, cupertino_ui_platform::GlassStyle::Popover);
    assert_eq!(popover.style.material(), "PopoverGlass");
    assert_eq!(
        (popover.alpha, popover.container, popover.spacing),
        (regular.alpha, regular.container, regular.spacing)
    );
}

#[test]
fn a_switcher_glass_is_regular_glass_of_the_switcher_style() {
    let switcher = crate::cx::Glass::switcher();
    let regular = crate::cx::Glass::regular();
    assert_eq!(switcher.style, cupertino_ui_platform::GlassStyle::Switcher);
    assert_eq!(switcher.style.material(), "SwitcherGlass");
    assert_eq!(
        (switcher.alpha, switcher.container, switcher.spacing),
        (regular.alpha, regular.container, regular.spacing)
    );
}

#[test]
fn the_sidebar_subdued_module_snippet_and_veiled_glass_are_regular_glass_of_their_styles() {
    use cupertino_ui_platform::GlassStyle;
    let regular = crate::cx::Glass::regular();
    for (glass, style, name) in [
        (
            crate::cx::Glass::sidebar(),
            GlassStyle::Sidebar,
            "SidebarGlass",
        ),
        (
            crate::cx::Glass::subdued_module(),
            GlassStyle::SubduedModule,
            "SubduedModuleGlass",
        ),
        (
            crate::cx::Glass::snippet(),
            GlassStyle::Snippet,
            "SnippetGlass",
        ),
        (
            crate::cx::Glass::veiled(),
            GlassStyle::Veiled,
            "VeiledGlass",
        ),
        (
            crate::cx::Glass::toolbar(),
            GlassStyle::Toolbar,
            "ToolbarGlass",
        ),
    ] {
        assert_eq!(glass.style, style);
        assert_eq!(glass.style.material(), name);
        assert_eq!(
            (glass.alpha, glass.container, glass.spacing),
            (regular.alpha, regular.container, regular.spacing)
        );
    }
}

/// A bar of a fixed size that fills itself in `tone` and counts its paints.
struct Bar {
    tone: f32,
    paints: Rc<Cell<u32>>,
}

const BAR: Size = Size {
    width: 80.0,
    height: 10.0,
};

impl Widget<M> for Bar {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: cupertino_ui_layout::SizeSpec::Fixed(BAR.width),
            height: cupertino_ui_layout::SizeSpec::Fixed(BAR.height),
            ..spec(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.tone == self.tone {
            Change::None
        } else {
            Change::Paint
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: BAR,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        self.paints.set(self.paints.get() + 1);
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::from_origin_size(Point::new(0.0, 0.0), cx.size()),
                radius: 3.0,
            },
            paint: Paint::Solid(Color::new(self.tone, 0.2, 0.4, 1.0)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Its children one under the other, at their sizes.
struct Pile(Vec<Element<M>>);

impl Widget<M> for Pile {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        spec(ContainerKind::Stack {
            axis: cupertino_ui_layout::Axis::Vertical,
            spacing: 2.0,
            align: cupertino_ui_layout::Align::Start,
            justify: cupertino_ui_layout::Justify::Start,
        })
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
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Its content scrolled up by `offset`, inside its fixed viewport.
struct View {
    content: Vec<Element<M>>,
    offset: f64,
}

impl Widget<M> for View {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: cupertino_ui_layout::SizeSpec::Fixed(100.0),
            height: cupertino_ui_layout::SizeSpec::Fixed(50.0),
            ..spec(ContainerKind::Scroll {
                horizontal: false,
                vertical: true,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.content)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if old.offset == self.offset {
            Change::None
        } else {
            Change::Paint
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::from_origin_size(Point::new(0.0, 0.0), cx.size())),
            paint: Paint::Solid(Color::new(1.0, 1.0, 1.0, 1.0)),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn scroll_offset(&self, _: &()) -> Point {
        Point::new(0.0, self.offset)
    }
}

/// Twenty kept rows of a bar each in a view scrolled by `offset`; row `i` is made by `Element::kept` with the mark and
/// the tone `tones[i]`.
fn kept_rows(paints: &Rc<Cell<u32>>, tones: &[f32], offset: f64) -> Element<M> {
    let rows = tones
        .iter()
        .enumerate()
        .map(|(i, &tone)| {
            let paints = Rc::clone(paints);
            Element::kept(i as u64, u64::from(tone.to_bits()), move || {
                Element::new(crate::test_support::group(vec![Element::new(Bar {
                    tone,
                    paints,
                })]))
            })
        })
        .collect();
    Element::new(View {
        content: vec![Element::new(Pile(rows))],
        offset,
    })
}

/// The tree laid out in a 100 x 50 window and painted by `painter` with the window `active` or not.
fn paint_laid_out(
    painter: &mut Painter,
    tree: &Tree<M>,
    root: NodeId,
    env: &Environment,
    active: bool,
) -> Painting {
    let result = cupertino_ui_layout::layout(
        &crate::layout_view::TreeView { tree, env },
        root,
        Constraints::tight(Size::new(100.0, 50.0)).unwrap(),
        &cupertino_ui_layout::LayoutOptions {
            direction: env.direction,
            node_limit: 1000,
        },
    )
    .unwrap();
    let geometry = crate::layout_view::placements(tree, root, &result, env, &[]);
    painter.paint(
        tree,
        root,
        &geometry,
        &PaintInput {
            env,
            focus: None,
            focus_visible: false,
            window_active: active,
            background: WindowBackground::Theme,
            backdrop: false,
            materials: false,
            now: Timestamp(0),
            raised: &[],
            obscuring: &[],
            layer: false,
            pinned: &[None, None],
        },
    )
}

/// The list with every revision taken out, which two painters count apart.
fn unrevised(list: &DisplayList) -> Vec<PaintNode> {
    list.nodes
        .iter()
        .map(|n| PaintNode {
            revision: PaintRevision(0),
            ..n.clone()
        })
        .collect()
}

#[test]
fn a_kept_subtree_that_did_not_change_is_laid_in_as_it_was_unwalked_and_the_list_is_the_one_painted_whole(
) {
    let paints = Rc::default();
    let env = env();
    let mut tones = [0.5_f32; 20];
    let mut tree = Tree::<M>::new(100);
    let root = tree
        .reconcile(None, kept_rows(&paints, &tones, 0.0))
        .unwrap();
    let mut painter = Painter::default();
    let first = paint_laid_out(&mut painter, &tree, root, &env, true);
    // the view, its pile and twenty rows of two nodes each
    let nodes = 2 + 20 * 2;
    assert_eq!(
        painter.stats,
        PaintStats {
            walked: nodes,
            kept: 0
        }
    );
    assert_eq!(paints.get(), 20);
    let revisions = |list: &DisplayList| -> Vec<PaintRevision> {
        list.nodes.iter().map(|n| n.revision).collect()
    };

    // scrolled by whole points and by part of one, rows across the viewport's edges among them: the rows are laid in
    // with their revisions, nothing of them is painted or walked, and the list is the list a new painter makes
    for offset in [7.0, 13.5, 40.0] {
        tree.reconcile(Some(root), kept_rows(&paints, &tones, offset))
            .unwrap();
        let before = paints.get();
        let painting = paint_laid_out(&mut painter, &tree, root, &env, true);
        assert_eq!(
            painter.stats,
            PaintStats {
                walked: 2,
                kept: nodes - 2
            },
            "at {offset}"
        );
        assert_eq!(paints.get(), before, "no row painted at {offset}");
        assert_eq!(revisions(&painting.list)[2..], revisions(&first.list)[2..]);
        let whole = paint_laid_out(&mut Painter::default(), &tree, root, &env, true);
        assert_eq!(
            unrevised(&painting.list),
            unrevised(&whole.list),
            "at {offset}"
        );
        assert_eq!(painting.own, whole.own);
        assert_eq!(painting.blank, whole.blank);
    }

    // a row made anew is walked and painted, the others are laid in
    tones[3] = 0.25;
    tree.reconcile(Some(root), kept_rows(&paints, &tones, 40.0))
        .unwrap();
    let before = paints.get();
    let painting = paint_laid_out(&mut painter, &tree, root, &env, true);
    assert_eq!(
        painter.stats,
        PaintStats {
            walked: 4,
            kept: nodes - 4
        }
    );
    assert_eq!(paints.get(), before + 1, "the new row alone painted");
    let whole = paint_laid_out(&mut Painter::default(), &tree, root, &env, true);
    assert_eq!(unrevised(&painting.list), unrevised(&whole.list));

    // a change every node paints with walks every node
    let before = paints.get();
    paint_laid_out(&mut painter, &tree, root, &env, false);
    assert_eq!(
        painter.stats,
        PaintStats {
            walked: nodes,
            kept: 0
        }
    );
    assert_eq!(paints.get(), before + 20);
}
