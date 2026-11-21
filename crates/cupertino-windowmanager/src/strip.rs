//! The stage strip's surface on CupertinoKit: one window on a bottom layer surface along the output's left edge,
//! anchored to its top, bottom and left edges, so the window's coordinates are the output's. Each thumbnail is a hole
//! ([`hole_layer`]): a surface layer the server draws a window of the strip in, claiming it, so the strip draws
//! nothing of its own in the shell. Only the thumbnails take the pointer ([`Scene::input`]); a press there reaches
//! the component, which finds the thumbnail under it, and assistive technology sees each as a button named by its application.
//! Nothing here speaks Wayland.

use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, SurfaceLayerMark, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{
    DecorationMode, MouseButton, PointerEvent, SurfaceLayerLook, WindowSpec,
};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::Color;

/// The `app_id` of the strip's window, and the namespace of its layer surface.
pub const NAMESPACE: &str = "cupertino-stage-strip";

/// The key the strip's surface is persisted with: the server holds it, with its holes and the window manager's
/// placements, while the component starts again.
pub const PERSIST_KEY: &str = "strip";

/// The first layer key of the thumbnails' holes: a window's id is added.
const HOLES: u64 = 1 << 32;

/// The corner radius of what a capture paints where the server would draw a window: the strip's thumbnail radius of
/// `cupertino_wm::stage::StageConfig`.
const HOLE_RADIUS: f64 = 8.0;

/// What assistive technology calls the strip.
const TITLE: &str = "Stage Strip";

/// The surface layer of window `id`'s hole.
pub fn hole_layer(id: u32) -> u64 {
    HOLES + u64::from(id)
}

/// The window whose hole is surface layer `layer`, if it is one.
pub fn layer_window(layer: u64) -> Option<u32> {
    layer
        .checked_sub(HOLES)
        .and_then(|id| u32::try_from(id).ok())
}

/// One thumbnail: the hole of window `id` laid out at `rect`, in the window's points.
#[derive(Clone, Debug, PartialEq)]
pub struct Thumb {
    pub id: u32,
    pub rect: Rect,
    /// What assistive technology calls it: the window's application.
    pub name: String,
    /// It takes the pointer and is offered to assistive technology: a thumbnail at rest in the strip, not one on its
    /// way out of it.
    pub live: bool,
}

/// What the strip's window shows.
pub struct Scene {
    pub key: WindowKey,
    /// The window's size in points.
    pub size: Size,
    /// The thumbnails, in the order they stack (the first lowest).
    pub thumbs: Vec<Thumb>,
    /// The part of the window that takes the pointer: where the thumbnails at rest in the strip rest.
    pub input: Vec<Rect>,
    /// What a hole paints itself: nothing in the shell, where the server draws the window in it; a flat colour in a
    /// capture, which has no server, as red, green, blue and alpha from 0 to 1.
    pub hole_fill: Option<[f32; 4]>,
}

/// The strip's window: `pressed` is sent with the point the left button went down at, `pick` with the window of a
/// thumbnail assistive technology clicked.
pub fn window<M, P, Q>(scene: &Scene, pressed: P, pick: Q) -> Window<M>
where
    M: Clone + 'static,
    P: Fn(Point) -> M + 'static,
    Q: Fn(u32) -> M,
{
    let children = scene
        .thumbs
        .iter()
        .map(|thumb| {
            Element::new(Hole {
                rect: thumb.rect,
                layer: hole_layer(thumb.id),
                name: thumb.name.clone(),
                live: thumb.live,
                click: pick(thumb.id),
                fill: scene.hole_fill,
            })
            .key(Key::from(format!("thumb-{}", thumb.id)))
        })
        .collect();
    Window {
        key: scene.key.clone(),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: NAMESPACE.to_owned(),
            size: scene.size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: Some(scene.input.clone()),
        },
        shortcuts: CommandRegistry::new(),
        on_close: None,
        background: WindowBackground::None,
        content: Element::new(Root {
            pressed: Box::new(pressed),
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// The root of the strip: fills the window, lays the thumbnails out at their places and hands a press of the left
/// button to the component, which finds the thumbnail under it.
struct Root<M> {
    pressed: Box<dyn Fn(Point) -> M>,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Root<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
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
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Bubble => {
                cx.emit((self.pressed)(*position));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(TITLE.to_owned());
    }
}

/// A thumbnail: the surface layer the server draws its window in, at the place the component gave it.
struct Hole<M> {
    rect: Rect,
    layer: u64,
    name: String,
    live: bool,
    click: M,
    fill: Option<[f32; 4]>,
}

impl<M: Clone + 'static> Widget<M> for Hole<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.size.width),
            height: SizeSpec::Fixed(self.rect.size.height),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect || self.layer != old.layer {
            Change::Layout
        } else if self.name != old.name || self.live != old.live || self.fill != old.fill {
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if self.live && request.action == Action::Click => {
                cx.emit(self.click.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    /// The pointer goes to the component through the root; a thumbnail is never the pointer's target.
    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        Some(SurfaceLayerMark {
            key: self.layer,
            look: SurfaceLayerLook::default(),
        })
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let Some([r, g, b, a]) = self.fill else {
            return;
        };
        let size = cx.size();
        let radius = HOLE_RADIUS.min(size.width / 2.0).min(size.height / 2.0);
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect: Rect::new(0.0, 0.0, size.width, size.height),
                radius,
            },
            paint: Paint::Solid(Color::new(r, g, b, a)),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if !self.live {
            cx.hidden();
            return;
        }
        cx.role(Role::Button);
        cx.name(self.name.clone());
        cx.action(Action::Click);
    }
}
