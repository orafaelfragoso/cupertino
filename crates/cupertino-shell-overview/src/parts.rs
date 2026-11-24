//! The parts the overview's surface is built from on CupertinoKit: a root node that hands the pointer and the keys to
//! the overview's model before any node below it and places its children where the model laid them out, the placed
//! nodes that draw a part of the surface, describe it to assistive technology and may be a surface layer of their own
//! (a hole the server draws a window or a desktop in), a group that is one surface layer with the parts in it (the
//! chrome, which fades as one), and the drawing helpers. The model hit-tests itself, so the Kit tree changes how the
//! surface is drawn and described, not what a press or a key does.

use cupertino_kit::core::{
    AccessibleStates, Action, Change, Element, Event, EventCx, Live, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, SurfaceLayerMark, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, Overflow, SizeSpec,
};
use cupertino_kit::platform::{KeyEvent, MouseButton, PointerEvent, SurfaceLayerLook};
use cupertino_kit::render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{AttributedText, FontWeight, ParagraphStyle, TextAlign, TextStyle};
use cupertino_kit::theme::Color;
use cupertino_wm::geometry::Rect as WmRect;

/// A colour as red, green, blue and alpha from 0 to 1.
pub type Rgba = [f32; 4];

/// What the pointer did on a surface, in the surface's points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerInput {
    /// The pointer entered or moved to this point.
    Moved(Point),
    /// The pointer left the surface.
    Left,
    /// The left button went down here.
    Pressed(Point),
    /// The left button came up here.
    Released(Point),
}

/// The look the session asks for, read from the toolkit's environment where a surface draws.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Look {
    pub dark: bool,
    /// Increase Contrast: solid fills and stronger outlines.
    pub high_contrast: bool,
    /// Reduce Transparency: no translucent fills.
    pub reduce_transparency: bool,
}

impl Look {
    pub fn of(cx: &PaintCx<'_>) -> Look {
        let preferences = cx.env().theme.preferences;
        Look {
            dark: preferences.appearance.is_dark(),
            high_contrast: preferences.appearance.is_high_contrast(),
            reduce_transparency: preferences.reduce_transparency,
        }
    }

    /// Whether fills are drawn opaque: with Reduce Transparency or Increase Contrast.
    pub fn opaque(self) -> bool {
        self.high_contrast || self.reduce_transparency
    }
}

/// The root of a surface: fills its window, takes the pointer and the keys before the nodes below it and turns them
/// into the surface's input, and lays its children out at their absolute places.
pub struct Root<M> {
    pub role: Role,
    pub name: String,
    /// How many items the window holds when it is a list.
    pub items: Option<usize>,
    /// The whole surface's opacity (a fade), applied once to everything it draws.
    pub opacity: f64,
    pub pointer: Option<Box<dyn Fn(PointerInput) -> M>>,
    pub key: Option<Box<dyn Fn(KeyEvent) -> M>>,
    /// Sent at every frame the compositor gives the window while it is set: what moves by itself advances with the
    /// display's frames.
    pub frame: Option<M>,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Root<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.opacity != old.opacity
            || self.name != old.name
            || self.items != old.items
            || self.role != old.role
            || self.frame.is_some() != old.frame.is_some()
        {
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(pointer, _) if cx.phase() != Phase::Bubble => {
                let Some(input) = &self.pointer else {
                    return Propagation::Continue;
                };
                let pointer = match *pointer {
                    PointerEvent::Enter { position } | PointerEvent::Motion { position } => {
                        PointerInput::Moved(position)
                    }
                    PointerEvent::Leave => PointerInput::Left,
                    PointerEvent::Button {
                        position,
                        button: MouseButton::Left,
                        pressed,
                        ..
                    } => {
                        if pressed {
                            PointerInput::Pressed(position)
                        } else {
                            PointerInput::Released(position)
                        }
                    }
                    _ => return Propagation::Stop,
                };
                cx.emit(input(pointer));
                Propagation::Stop
            }
            Event::AnimationFrame(_) => {
                if let Some(frame) = &self.frame {
                    cx.emit(frame.clone());
                    cx.request_animation_frame();
                }
                Propagation::Continue
            }
            Event::Key(key, _) if cx.phase() != Phase::Bubble => match &self.key {
                Some(input) => {
                    cx.emit(input(*key));
                    Propagation::Stop
                }
                None => Propagation::Continue,
            },
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.frame.is_some() {
            cx.request_animation_frame();
        }
        if self.opacity < 1.0 {
            cx.set_group(self.opacity.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.clone());
        if let Some(items) = self.items {
            cx.item_count(items);
        }
    }
}

/// What a [`Part`] draws, in its own coordinates (its top-left corner at the origin).
pub type Painter = Box<dyn Fn(&mut PaintCx<'_>, Look)>;

/// One part of a surface at the rectangle the model gave it: it draws through `paint` and describes itself to
/// assistive technology. A click by assistive technology sends `click`; the pointer reaches the model through the
/// [`Root`] instead, so a part never takes it.
pub struct Part<M> {
    pub rect: Rect,
    /// `None` for a part assistive technology skips (a highlight, a title the window's part already names).
    pub role: Option<Role>,
    pub name: String,
    pub states: AccessibleStates,
    pub click: Option<M>,
    /// The model selected this part: it takes the keyboard focus, so assistive technology follows the selection.
    pub take_focus: bool,
    /// Its place among the items of the list its window is, which a screen reader reads with the list's count.
    pub place: Option<usize>,
    /// Its name is announced whenever it changes (the selected application's name of a panel without the keyboard).
    pub live: bool,
    /// Compared to skip a repaint; anything `paint` draws from goes in it.
    pub look_key: String,
    /// The key of the surface layer the part is, for a hole the server draws a source in; `None` for a part drawn
    /// in its window.
    pub layer: Option<u64>,
    pub paint: Painter,
}

impl<M: Clone + 'static> Widget<M> for Part<M> {
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
        } else if self.look_key != old.look_key
            || self.name != old.name
            || self.states != old.states
            || self.role != old.role
            || self.take_focus != old.take_focus
            || self.place != old.place
            || self.live != old.live
        {
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
        match (event, &self.click) {
            (Event::Action(request), Some(click)) if request.action == Action::Click => {
                cx.emit(click.clone());
                Propagation::Stop
            }
            // the frame a selected part asked for when it painted without the focus
            (Event::AnimationFrame(_), _) if self.take_focus && !cx.is_focused() => {
                cx.request_focus();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    /// The pointer goes to the model through the root; a part is never the pointer's target.
    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        self.layer.map(|key| SurfaceLayerMark {
            key,
            look: SurfaceLayerLook::default(),
        })
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.take_focus && !cx.is_focused() {
            cx.request_animation_frame();
        }
        let look = Look::of(cx);
        (self.paint)(cx, look);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        let Some(role) = self.role else {
            cx.hidden();
            return;
        };
        cx.role(role);
        if !self.name.is_empty() {
            cx.name(self.name.clone());
        }
        cx.states(self.states);
        if let Some(place) = self.place {
            cx.item_index(place);
        }
        if self.click.is_some() {
            cx.action(Action::Click);
            cx.focusable();
        }
        if self.live {
            cx.live(Live::Polite);
        }
    }
}

/// A part of a surface that is one surface layer with the parts in it, laid out at their absolute places: what
/// fades or moves as one.
pub struct Group<M> {
    pub rect: Rect,
    /// The key of its surface layer.
    pub layer: u64,
    /// Its opacity where the window system has no surface layers and it paints in place (a capture): the shell
    /// fades its layer instead, so there it is 1.
    pub opacity: f64,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Group<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.size.width),
            height: SizeSpec::Fixed(self.rect.size.height),
            position: self.rect.origin,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect || self.layer != old.layer {
            Change::Layout
        } else if self.opacity != old.opacity {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.opacity < 1.0 {
            cx.set_group(self.opacity.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        Some(SurfaceLayerMark {
            key: self.layer,
            look: SurfaceLayerLook::default(),
        })
    }
}

/// A pill under a name, centred in the row `rect`: as wide as `name` set at `size` and `weight` with `side` points
/// beside it, no wider than the row and as tall. Its face draws through `paint` at its own size, so a material it asks
/// for has the pill's shape.
pub fn pill<M: Clone + 'static>(
    rect: Rect,
    name: String,
    (size, weight, side): (f64, u16, f64),
    look_key: String,
    paint: Painter,
) -> Element<M> {
    Element::new(PillRow {
        rect,
        children: vec![Element::new(PillFace {
            name,
            size,
            weight,
            side,
            look_key,
            paint,
        })],
    })
}

/// The row a [`PillFace`] is centred in.
struct PillRow<M> {
    rect: Rect,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for PillRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.size.width),
            height: SizeSpec::Fixed(self.rect.size.height),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Center,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect == old.rect {
            Change::None
        } else {
            Change::Layout
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The pill itself, as wide as its name asks.
struct PillFace {
    name: String,
    size: f64,
    weight: u16,
    side: f64,
    look_key: String,
    paint: Painter,
}

impl<M: Clone + 'static> Widget<M> for PillFace {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.name != old.name
            || self.size != old.size
            || self.weight != old.weight
            || self.side != old.side
        {
            Change::Layout
        } else if self.look_key != old.look_key {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let style = TextStyle {
            weight: FontWeight(self.weight),
            ..TextStyle::new(self.size as f32)
        };
        let text = cx
            .env()
            .text
            .paragraph(
                &AttributedText::plain(&self.name, style),
                &ParagraphStyle::default(),
            )
            .map_or(0.0, |p| p.layout(None).size().width);
        Measured {
            size: Size::new(
                constraints.width.clamp(text + 2.0 * self.side),
                constraints.height.clamp(0.0),
            ),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let look = Look::of(cx);
        (self.paint)(cx, look);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A model rectangle as the toolkit's.
pub fn kit_rect(r: WmRect) -> Rect {
    Rect::new(r.x, r.y, r.w, r.h)
}

pub fn color(c: Rgba) -> Color {
    Color::new(c[0], c[1], c[2], c[3])
}

/// `c` with its alpha multiplied by `alpha`.
pub fn faded(c: Rgba, alpha: f64) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha.clamp(0.0, 1.0) as f32]
}

fn rounded(rect: Rect, radius: f64) -> Shape {
    if radius > 0.0 {
        Shape::RoundedRect {
            rect,
            radius: radius
                .min(rect.size.width / 2.0)
                .min(rect.size.height / 2.0),
        }
    } else {
        Shape::Rect(rect)
    }
}

pub fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, c: Rgba) {
    cx.push(PaintOp::Fill {
        shape: rounded(rect, radius),
        paint: Paint::Solid(color(c)),
    });
}

/// An outline `width` points wide inside `rect`.
pub fn outline(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, width: f64, c: Rgba) {
    let half = width / 2.0;
    let inner = Rect::new(
        rect.origin.x + half,
        rect.origin.y + half,
        (rect.size.width - width).max(0.0),
        (rect.size.height - width).max(0.0),
    );
    cx.push(PaintOp::Stroke {
        shape: rounded(inner, (radius - half).max(0.0)),
        width,
        paint: Paint::Solid(color(c)),
    });
}

/// A straight line `width` points wide.
pub fn line(cx: &mut PaintCx<'_>, a: Point, b: Point, width: f64, c: Rgba) {
    cx.push(PaintOp::Stroke {
        shape: Shape::Path {
            verbs: vec![PathVerb::MoveTo(a), PathVerb::LineTo(b)],
            fill_rule: Default::default(),
        },
        width,
        paint: Paint::Solid(color(c)),
    });
}

/// One line of `s` at `size` points and `weight`, centred in `rect`, ending in an ellipsis when it is wider.
pub fn centred_text(cx: &mut PaintCx<'_>, s: &str, rect: Rect, size: f64, weight: u16, c: Rgba) {
    if s.is_empty() || rect.size.width <= 0.0 {
        return;
    }
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    let paragraph = ParagraphStyle {
        align: TextAlign::Center,
        max_lines: Some(1),
        ellipsis: true,
        ..ParagraphStyle::default()
    };
    let system = std::sync::Arc::clone(&cx.env().text);
    let Ok(text) = system.paragraph(&AttributedText::plain(s, style), &paragraph) else {
        return;
    };
    let layout = text.layout(Some(rect.size.width as f32));
    let top = rect.origin.y + (rect.size.height - layout.size().height) / 2.0;
    let origin = Point::new(rect.origin.x, top);
    for line in layout.lines() {
        for run in &line.runs {
            cx.push(PaintOp::Glyphs {
                // the run's origin is on its baseline, from the layout's top-left corner
                origin: Point::new(origin.x + run.origin.x, origin.y + run.origin.y),
                run: run.run.clone(),
                color: color(c),
            });
        }
    }
}

/// The width one line of `s` takes at `size` points and `weight`, in points.
pub fn text_width(cx: &PaintCx<'_>, s: &str, size: f64, weight: u16) -> f64 {
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    cx.env()
        .text
        .paragraph(&AttributedText::plain(s, style), &ParagraphStyle::default())
        .map_or(0.0, |p| p.layout(None).size().width)
}
