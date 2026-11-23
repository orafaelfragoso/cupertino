//! A folder's stack as a popup of the bar: a panel of glass with the folder's name, a grid of its newest entries (each a
//! button with its icon and name) and a button that opens the folder. Over a bar at the bottom of the output a pointer
//! under the panel, one piece of glass with it, points at the folder's tile; where the panel was moved to stay inside
//! the output the pointer stays over the tile. It takes the keyboard and closes on a press outside it or Escape. It
//! comes in as [`Timing::DOCK_STACK_ENTRY`](cupertino_kit::motion::Timing::DOCK_STACK_ENTRY) runs, fading and scaling up out of the pointer's tip, and under Reduce Motion only
//! fading. Where the parts go is [`StackLayout`]'s, and where the popup stands [`StackPlacement`]'s.

use cupertino_kit::controls::CalloutPointer;
use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Glass, Key, MeasureCx, PaintCx,
    Popup, PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::core::{GlassSize, GlassVariant};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};

use super::bar::STACK_ANCHOR;
use super::paint::{self, Root};
use super::{kit_rect, IconSet, Input};
use crate::config::Orientation;
use crate::stack::{
    self, StackEntry, StackLayout, StackPlacement, EMPTY, OPEN_FOLDER, POINTER_DEPTH,
    POINTER_WIDTH, TIP_GAP,
};

/// The surface whose opaque fill the panel and its pointer paint where the session shows no glass.
pub const SURFACE: Surface = Surface::Popover;
/// The glass container the panel and its pointer share, and how far their outlines blend into each other in points:
/// the pointer's sides curve out into the panel's bottom edge as one piece of glass.
const GLASS_CONTAINER: u32 = 1;
const GLASS_BLEND: f64 = 8.0;
/// The panel's own edge, drawn only over the opaque fill; twice as wide with Increase Contrast.
const EDGE: f64 = 1.0;
/// The button's fill: the label colour at this share.
const BUTTON_FILL_SHARE: f32 = 0.1;
/// A label's colour where the theme has none.
const LABEL_FALLBACK: Color = Color {
    r: 0.0,
    g: 0.0,
    b: 0.0,
    a: 1.0,
};

pub fn key() -> WindowKey {
    WindowKey(Key::from("stack"))
}

/// What the stack shows this frame.
pub struct StackView<'a> {
    /// The bar's window, which the stack is a popup of.
    pub parent: WindowKey,
    /// Where the popup stands by the folder's tile.
    pub placement: StackPlacement,
    pub orientation: Orientation,
    pub name: &'a str,
    pub entries: &'a [StackEntry],
    /// Icons by their theme name.
    pub icons: &'a dyn Fn(&str) -> IconSet,
    /// Seconds since it opened, and Reduce Motion.
    pub since: f64,
    pub reduce_motion: bool,
}

/// Scales about the point of the popup nearest the tile.
#[derive(Clone, Copy)]
struct Zoom {
    origin: Point,
    scale: f64,
}

impl Zoom {
    fn rect(&self, r: &cupertino_wm::geometry::Rect) -> Rect {
        let r = kit_rect(r);
        let s = self.scale;
        Rect::new(
            self.origin.x + (r.x() - self.origin.x) * s,
            self.origin.y + (r.y() - self.origin.y) * s,
            r.width() * s,
            r.height() * s,
        )
    }
}

fn label_color(cx: &PaintCx<'_>) -> Color {
    cx.env()
        .theme
        .colors
        .get(ColorRole::Text, StateVariant::NORMAL)
        .unwrap_or(LABEL_FALLBACK)
}

pub fn window<M: Clone + 'static>(view: StackView<'_>, input: fn(Input) -> M) -> Window<M> {
    let layout = StackLayout::new(view.entries.len());
    let (alpha, scale) = stack::entry(view.since, view.reduce_motion);
    let moving = alpha < 1.0;
    let (w, h) = (layout.width, layout.height);
    let pointer = view
        .placement
        .pointer
        .filter(|_| view.orientation == Orientation::Bottom);
    let origin = match (view.orientation, pointer) {
        // the pointer's tip
        (Orientation::Bottom, Some(x)) => Point::new(x, h + POINTER_DEPTH),
        (Orientation::Bottom, None) => Point::new(w / 2.0, h),
        (Orientation::Left, _) => Point::new(0.0, h / 2.0),
        (Orientation::Right, _) => Point::new(w, h / 2.0),
    };
    let zoom = Zoom { origin, scale };
    let all = cupertino_wm::geometry::Rect::new(0.0, 0.0, w, h);
    // the pointer under the panel, in the window
    let pointer = pointer.map(|x| {
        zoom.rect(&cupertino_wm::geometry::Rect::new(
            x - POINTER_WIDTH / 2.0,
            h,
            POINTER_WIDTH,
            POINTER_DEPTH,
        ))
    });
    let mut children: Vec<Element<M>> = vec![
        Element::new(PanelNode {
            rect: zoom.rect(&all),
            radius: StackLayout::RADIUS * scale,
            alpha,
            pointer,
        })
        .key("panel"),
        Element::new(LabelNode {
            rect: zoom.rect(&layout.title),
            text: view.name.to_owned(),
            size: StackLayout::TITLE_FONT_PT * scale,
            weight: StackLayout::TITLE_WEIGHT,
        })
        .key("title"),
    ];
    for (index, entry) in view.entries.iter().take(stack::MAX_ENTRIES).enumerate() {
        let cell = layout.cell(index);
        children.push(
            Element::new(EntryNode {
                index,
                name: entry.name.clone(),
                cell: zoom.rect(&cell),
                icon: zoom.rect(&StackLayout::icon(cell)),
                label_cy: StackLayout::LABEL_CY * scale,
                scale,
                image: (view.icons)(&entry.icon),
                input,
            })
            .key(Key::from(format!("entry-{index}"))),
        );
    }
    if view.entries.is_empty() {
        children.push(
            Element::new(LabelNode {
                rect: zoom.rect(&layout.grid),
                text: EMPTY.to_owned(),
                size: StackLayout::TITLE_FONT_PT * scale,
                weight: 400,
            })
            .key("empty"),
        );
    }
    children.push(
        Element::new(ButtonNode {
            rect: zoom.rect(&layout.button),
            scale,
            input,
        })
        .key("open-folder"),
    );
    if let Some(rect) = pointer {
        children.push(Element::new(PointerNode { rect, alpha }).key("pointer"));
    }
    let anchor = match view.orientation {
        // over the bar, the pointer's tip a little above its top edge
        Orientation::Bottom => PopupAnchor::CentredAbove {
            key: Key::from(STACK_ANCHOR),
            gap: TIP_GAP,
        },
        // beside the bar, on the side away from the screen edge: the anchor ends where the panel begins
        Orientation::Left | Orientation::Right => PopupAnchor::Beside {
            key: Key::from(STACK_ANCHOR),
            offset: Point::ZERO,
        },
    };
    let window = view.placement.window;
    Window {
        key: key(),
        spec: WindowSpec {
            title: view.name.to_owned(),
            app_id: String::new(),
            size: Size::new(window.w, window.h),
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::StackClosed)),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Dialog,
            name: view.name.to_owned(),
            pointer: Some(Box::new(move |p| input(Input::Stack(p)))),
            key: Some(Box::new(move |k| Some(input(Input::StackKey(k))))),
            roving: true,
            on_blur: None,
            alpha,
            revision: (alpha * 1000.0) as u64,
            under: None,
            frame: moving.then(|| input(Input::Frame)),
            children,
        }),
        popup: Some(Popup {
            parent: view.parent,
            anchor,
            grab: true,
            on_dismiss: input(Input::StackClosed),
        }),
        menus: None,
    }
}

/// The glass of the panel and its pointer at `alpha`: a cartouche popover's at the medium size, not adapting to what is
/// behind it, joined into one shape.
fn panel_glass(alpha: f64) -> Glass {
    Glass::dense()
        .variant(GlassVariant::CartouchePopover)
        .size_class(GlassSize::Medium)
        .container(GLASS_CONTAINER, GLASS_BLEND)
        .alpha(alpha)
}

/// The sides of a pointer under a panel, from the trailing end of its base at `top` round its tip to the leading end:
/// the base is `rect`'s width, and the sides curve in to a rounded tip at `rect`'s bottom edge.
fn pointer_sides(rect: Rect, top: f64) -> Vec<PathVerb> {
    let (w, mid, tip) = (rect.width(), rect.center().x, rect.max_y());
    vec![
        PathVerb::MoveTo(Point::new(rect.max_x(), top)),
        PathVerb::CubicTo(
            Point::new(rect.max_x() - 0.2 * w, top),
            Point::new(mid + 0.18 * w, tip),
            Point::new(mid, tip),
        ),
        PathVerb::CubicTo(
            Point::new(mid - 0.18 * w, tip),
            Point::new(rect.min_x() + 0.2 * w, top),
            Point::new(rect.min_x(), top),
        ),
    ]
}

/// The panel: glass or, where the session shows none, its opaque fill with a fine edge, the pointer under it filled
/// and edged with it as one outline.
struct PanelNode {
    rect: Rect,
    radius: f64,
    alpha: f64,
    /// The pointer under the panel, in the window.
    pointer: Option<Rect>,
}

impl<M: Clone + 'static> Widget<M> for PanelNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.radius != old.radius
            || self.alpha != old.alpha
            || self.pointer != old.pointer
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let all = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = self.radius.min(size.width.min(size.height) / 2.0);
        // the glass lights its own rim
        if cx.glass(panel_glass(self.alpha).radius(radius)) {
            return;
        }
        let env = cx.env().clone();
        if let Some(fill) = env.theme.surface(SURFACE).fill {
            paint::fill(cx, all, radius, fill);
        }
        let width = if paint::is_high_contrast(&env) {
            2.0 * EDGE
        } else {
            EDGE
        };
        let edge = env
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL);
        let d = width / 2.0;
        if let Some(edge) = edge {
            let inner = Rect::new(d, d, size.width - 2.0 * d, size.height - 2.0 * d);
            paint::stroke(cx, inner, (radius - d).max(0.0), width, edge);
        }
        // the pointer, in the panel's coordinates: its fill covers the panel's edge along its base, and the edge
        // goes on round its sides
        let Some(pointer) = self.pointer else {
            return;
        };
        let pointer = Rect::new(
            pointer.x() - self.rect.x(),
            pointer.y() - self.rect.y(),
            pointer.width(),
            pointer.height(),
        );
        if let Some(fill) = env.theme.surface(SURFACE).fill {
            let mut verbs = pointer_sides(pointer, pointer.y() - width);
            verbs.push(PathVerb::Close);
            cx.push(PaintOp::Fill {
                shape: Shape::Path {
                    verbs,
                    fill_rule: FillRule::NonZero,
                },
                paint: Paint::Solid(fill),
            });
        }
        if let Some(edge) = edge {
            cx.push(PaintOp::Stroke {
                shape: Shape::Path {
                    verbs: pointer_sides(
                        Rect::new(
                            pointer.x() + d,
                            pointer.y(),
                            pointer.width() - 2.0 * d,
                            pointer.height() - d,
                        ),
                        pointer.y() - d,
                    ),
                    fill_rule: FillRule::NonZero,
                },
                width,
                paint: Paint::Solid(edge),
            });
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// The pointer under the panel, towards the folder's tile, as glass joined to the panel's; where the session shows no
/// glass the panel paints it.
struct PointerNode {
    rect: Rect,
    alpha: f64,
}

impl<M: Clone + 'static> Widget<M> for PointerNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width().max(0.0)),
            height: SizeSpec::Fixed(self.rect.height().max(0.0)),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        vec![CalloutPointer {
            width: self.rect.width(),
            depth: self.rect.height(),
            glass: panel_glass(self.alpha),
            surface: SURFACE,
            fills_without_glass: false,
            up: false,
        }
        .element()
        .key("shape")]
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect || self.alpha != old.alpha {
            Change::Layout
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// A line of text centred in its rectangle: the folder's name, or what an empty folder says.
struct LabelNode {
    rect: Rect,
    text: String,
    size: f64,
    weight: u16,
}

impl<M: Clone + 'static> Widget<M> for LabelNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.text != old.text || self.size != old.size {
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let Some(text) = paint::line(
            &env,
            &self.text,
            self.size,
            self.weight,
            Some(self.rect.width()),
        ) else {
            return;
        };
        let (w, h) = (text.size().width, text.size().height);
        let origin = Point::new(
            (self.rect.width() - w) / 2.0,
            (self.rect.height() - h) / 2.0,
        );
        let color = label_color(cx);
        paint::glyphs(cx, &text, origin, color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// One entry: its icon and its name under it; a button for assistive technology, which can click it.
struct EntryNode<M> {
    index: usize,
    name: String,
    cell: Rect,
    icon: Rect,
    label_cy: f64,
    scale: f64,
    image: IconSet,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for EntryNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.cell)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.cell != old.cell {
            Change::Layout
        } else if self.name != old.name || self.image != old.image || self.icon != old.icon {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.cell.size,
            baseline: None,
        }
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::StackEntry(self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let icon = Rect::new(
            self.icon.x() - self.cell.x(),
            self.icon.y() - self.cell.y(),
            self.icon.width(),
            self.icon.height(),
        );
        paint::image(cx, &self.image, icon);
        let env = cx.env().clone();
        let size = StackLayout::LABEL_FONT_PT * self.scale;
        let max = self.cell.width() - 2.0 * StackLayout::LABEL_INSET * self.scale;
        let Some(text) = paint::line(&env, &self.name, size, 400, Some(max)) else {
            return;
        };
        let (w, h) = (text.size().width, text.size().height);
        let origin = Point::new((self.cell.width() - w) / 2.0, self.label_cy - h / 2.0);
        let color = label_color(cx);
        paint::glyphs(cx, &text, origin, color);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focusable();
        cx.role(Role::Button);
        cx.name(self.name.clone());
        cx.action(Action::Click);
    }
}

/// The button that opens the folder in the file manager.
struct ButtonNode<M> {
    rect: Rect,
    scale: f64,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for ButtonNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.scale != old.scale {
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

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::StackOpenFolder));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let all = Rect::new(0.0, 0.0, size.width, size.height);
        let label = label_color(cx);
        let fill = Color {
            a: label.a * BUTTON_FILL_SHARE,
            ..label
        };
        paint::fill(cx, all, size.height / 2.0, fill);
        let env = cx.env().clone();
        let Some(text) = paint::line(
            &env,
            OPEN_FOLDER,
            StackLayout::BUTTON_FONT_PT * self.scale,
            500,
            Some(size.width),
        ) else {
            return;
        };
        let (w, h) = (text.size().width, text.size().height);
        let origin = Point::new((size.width - w) / 2.0, (size.height - h) / 2.0);
        paint::glyphs(cx, &text, origin, label);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.focusable();
        cx.role(Role::Button);
        cx.name(OPEN_FOLDER.to_owned());
        cx.action(Action::Click);
    }
}
