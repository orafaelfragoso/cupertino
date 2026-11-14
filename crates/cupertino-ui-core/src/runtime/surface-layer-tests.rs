//! Surface layers: a marked subtree draws in a frame of its own at its layout size, out of the window's; a change of
//! its properties alone draws no frame; a repaint elsewhere leaves its frame; one inside it draws only its layer's
//! damage; and where the window system shows none, the subtree paints in place as any other.

use super::*;
use crate::test_support::{options, FakeSystem};
use crate::{Change, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_platform::{
    Configure, DecorationMode, LayerChange, LayerProperty, SurfaceLayerLook, WindowStates,
    WmCapabilities,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use std::num::NonZeroU32;
use std::cell::RefCell;
use std::rc::Rc;

const WINDOW: Size = Size::new(200.0, 100.0);
const RED: Color = Color::new(0.9, 0.1, 0.1, 1.0);
const BLUE: Color = Color::new(0.1, 0.2, 0.9, 1.0);
const GREEN: Color = Color::new(0.1, 0.8, 0.2, 1.0);
/// The marked panel's place in the window.
const PANEL: Rect = Rect::new(100.0, 40.0, 60.0, 30.0);
const KEY: u64 = 7;
const LOOK: SurfaceLayerLook = SurfaceLayerLook {
    glass: Some(cupertino_ui_platform::GlassStyle::LargePanel),
    group: (0, 0.0),
    radius: 12.0,
    masks: true,
};

/// The colours of the window's block and of the block inside the marked panel.
#[derive(Clone, Debug, PartialEq)]
enum Message {
    Colors(Color, Color),
    /// Fades the panel's layer out over a tenth of a second.
    FadePanel,
}

struct Scene;

impl Application for Scene {
    type Model = (Color, Color);
    type Message = Message;

    fn init() -> ((Color, Color), Vec<Command<Message>>) {
        ((RED, BLUE), Vec::new())
    }

    fn update(model: &mut (Color, Color), message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Colors(a, b) => {
                *model = (a, b);
                Vec::new()
            }
            Message::FadePanel => vec![Command::Window(
                WindowKey("main".into()),
                WindowRequest::Layer {
                    key: KEY,
                    changes: vec![LayerChange {
                        property: LayerProperty::Opacity,
                        to: 0.0,
                        curve: Some(cupertino_animation::LayerCurve::Timed {
                            duration: 0.1,
                            bezier: cupertino_animation::CubicBezier::EASE_OUT,
                        }),
                        velocity: 0.0,
                    }],
                },
            )],
        }
    }

    fn view(model: &(Color, Color)) -> Vec<Window<Message>> {
        let panel = Element::new(Holder {
            rect: PANEL,
            mark: true,
            children: vec![Element::new(Block {
                rect: Rect::new(10.0, 5.0, 20.0, 20.0),
                color: model.1,
            })
            .key("inner")],
        })
        .key("panel");
        let block = Element::new(Block {
            rect: Rect::new(10.0, 10.0, 20.0, 20.0),
            color: model.0,
        })
        .key("outer");
        vec![Window {
            key: WindowKey("main".into()),
            spec: WindowSpec {
                title: "Layers".into(),
                app_id: "test.layers".into(),
                size: WINDOW,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            popup: None,
            background: WindowBackground::None,
            content: Element::new(Holder {
                rect: Rect::from_origin_size(Point::ZERO, WINDOW),
                mark: false,
                children: vec![block, panel],
            }),
            menus: None,
        }]
    }
}

/// Places its children where they say, at `rect` in its parent; marked, its subtree is the surface layer [`KEY`].
struct Holder {
    rect: Rect,
    mark: bool,
    children: Vec<Element<Message>>,
}

impl Widget<Message> for Holder {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        self.mark.then_some(SurfaceLayerMark {
            key: KEY,
            look: LOOK,
        })
    }
}

/// A square of one colour at `rect` in its parent.
struct Block {
    rect: Rect,
    color: Color,
}

impl Widget<Message> for Block {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.color != old.color {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
            paint: Paint::Solid(self.color),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

type Rt = Runtime<Scene, FakeSystem>;

fn steps() -> NonZeroU32 {
    NonZeroU32::new(16).unwrap()
}

/// The window drawn once on a window system that shows surface layers (`layers`), at `scale`, with the damage of
/// every request rendered.
fn start_with(layers: bool, scale: f64) -> (Rt, WindowId, Rc<RefCell<Vec<DamageRegion>>>) {
    let damage = Rc::default();
    let mut system = FakeSystem::default();
    system.surface_layers = layers;
    let mut rt = Rt::new(system, options(&damage));
    rt.step(steps()).unwrap();
    let window = rt.windows()[0].1;
    if scale != 1.0 {
        rt.window_system_mut()
            .send(window, WindowEvent::Scale(scale));
    }
    rt.window_system_mut().send(
        window,
        WindowEvent::Configure(Configure {
            id: ConfigureId(1),
            size: Some(WINDOW),
            states: WindowStates {
                activated: true,
                ..WindowStates::default()
            },
            bounds: None,
            server_decorations: true,
            wm_capabilities: WmCapabilities {
                window_menu: true,
                maximize: true,
                fullscreen: true,
                minimize: true,
            },
        }),
    );
    rt.step(steps()).unwrap();
    (rt, window, damage)
}

fn start(layers: bool, scale: f64) -> (Rt, WindowId) {
    let (rt, window, _) = start_with(layers, scale);
    (rt, window)
}

fn send(rt: &mut Rt, window: WindowId, message: Message, time: u64) {
    rt.messages.push_back(message);
    rt.window_system_mut()
        .send(window, WindowEvent::Frame(Timestamp(time)));
    rt.step(steps()).unwrap();
}

/// The fills of `list` with where they land, in its coordinates.
fn fills(list: &DisplayList) -> Vec<(Color, Rect)> {
    list.nodes
        .iter()
        .flat_map(|n| {
            n.ops.iter().filter_map(move |op| match op {
                PaintOp::Fill {
                    shape: Shape::Rect(r),
                    paint: Paint::Solid(c),
                } => {
                    let [a, _, _, d, e, f] = n.transform.0;
                    Some((*c, Rect::new(a * r.x() + e, d * r.y() + f, r.width(), r.height())))
                }
                _ => None,
            })
        })
        .collect()
}

fn layer_list(rt: &Rt) -> &DisplayList {
    &rt.windows[0].surface_layers.layers[&KEY].list
}

#[test]
fn a_marked_subtree_draws_in_its_surface_layer_at_its_layout_size_and_not_in_the_window() {
    let (rt, window) = start(true, 1.5);
    let presents = &rt.window_system().surface_presents;
    let (w, layers) = presents.last().expect("the layers were presented");
    assert_eq!(*w, window);
    assert_eq!(layers.len(), 1);
    let layer = &layers[0];
    assert_eq!((layer.key, layer.rect, layer.look), (KEY, PANEL, LOOK));
    let frame = layer.frame.as_ref().expect("its first frame");
    assert_eq!(frame.size, (90, 45), "its layout size at the window's scale");
    assert_eq!(
        fills(layer_list(&rt)),
        vec![(BLUE, Rect::new(10.0, 5.0, 20.0, 20.0))],
        "the panel's block, in the layer's coordinates"
    );
    assert_eq!(
        fills(&rt.windows[0].display_list),
        vec![(RED, Rect::new(10.0, 10.0, 20.0, 20.0))],
        "the window draws the rest and not the panel"
    );
    assert_eq!(rt.window_system().presented.len(), 1, "with the window's frame");
}

#[test]
fn a_repaint_elsewhere_leaves_the_layer_and_one_inside_it_draws_only_the_layer_s_damage() {
    let (mut rt, window, damage) = start_with(true, 1.0);
    let layer_presents = rt.window_system().surface_presents.len();
    send(&mut rt, window, Message::Colors(GREEN, BLUE), 1);
    assert_eq!(rt.window_system().presented.len(), 2, "the window repainted");
    assert_eq!(
        rt.window_system().surface_presents.len(),
        layer_presents,
        "the layer, which did not change, is not handed over again"
    );
    send(&mut rt, window, Message::Colors(GREEN, RED), 2);
    let (_, layers) = rt.window_system().surface_presents.last().unwrap();
    assert!(layers[0].frame.is_some(), "the layer drew");
    let rendered = damage.borrow();
    let inner = Rect::new(10.0, 5.0, 20.0, 20.0);
    assert!(
        matches!(&rendered[rendered.len() - 2], DamageRegion::Rects(r)
            if !r.is_empty() && r.iter().all(|r| *r == inner)),
        "the inner block in the layer's coordinates: {:?}",
        rendered[rendered.len() - 2]
    );
    assert_eq!(
        rendered[rendered.len() - 1],
        DamageRegion::Rects(Vec::new()),
        "and nothing of the window"
    );
    assert_eq!(fills(layer_list(&rt))[0].0, RED);
}

#[test]
fn a_change_of_a_layer_s_properties_alone_draws_no_frame() {
    let (mut rt, window) = start(true, 1.0);
    let (frames, layers) = (
        rt.window_system().presented.len(),
        rt.window_system().surface_presents.len(),
    );
    send(&mut rt, window, Message::FadePanel, 1);
    assert_eq!(rt.window_system().presented.len(), frames, "no window frame");
    assert_eq!(
        rt.window_system().surface_presents.len(),
        layers,
        "and no layer frame"
    );
    let asked = rt
        .window_system()
        .requests
        .iter()
        .filter(|(w, r)| *w == window && matches!(r, WindowRequest::Layer { key: KEY, .. }))
        .count();
    assert_eq!(asked, 1, "the fade went to the window system as it is");
}

#[test]
fn without_surface_layers_the_marked_subtree_paints_in_place() {
    let (rt, _) = start(false, 1.0);
    assert!(rt.window_system().surface_presents.is_empty());
    assert_eq!(
        fills(&rt.windows[0].display_list),
        vec![
            (RED, Rect::new(10.0, 10.0, 20.0, 20.0)),
            (BLUE, Rect::new(110.0, 45.0, 20.0, 20.0))
        ],
        "the panel's block in the window, where it lies"
    );
}
