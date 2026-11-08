//! A folder's stack as a popup of the bar: a panel on the bar's material with the folder's name, a grid of its newest
//! entries (each a button with its icon and name) and a button that opens the folder. It takes the keyboard and closes
//! on a press outside it or Escape. It comes in over [`stack::ENTRY_S`], fading and scaling up towards the bar, and
//! under Reduce Motion only fading. Where the parts go is [`StackLayout`]'s.

use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, MeasureCx, PaintCx, Popup,
    PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{Color, ColorRole, StateVariant};

use super::bar::{paint_edge, BAR_SURFACE};
use super::paint::{self, Root};
use super::{kit_rect, IconSet, Input};
use crate::config::Orientation;
use crate::stack::{self, StackEntry, StackLayout, EMPTY, OPEN_FOLDER};

/// How far the stack stands off its tile.
const GAP: f64 = 8.0;
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
    /// The folder tile it belongs to (`TileId.0`).
    pub tile: u64,
    pub orientation: Orientation,
    pub name: &'a str,
    pub entries: &'a [StackEntry],
    /// Icons by their theme name.
    pub icons: &'a dyn Fn(&str) -> IconSet,
    /// Seconds since it opened, and Reduce Motion.
    pub since: f64,
    pub reduce_motion: bool,
}

/// Scales about the side of the panel nearest the bar.
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
    let origin = match view.orientation {
        Orientation::Bottom => Point::new(w / 2.0, h),
        Orientation::Left => Point::new(0.0, h / 2.0),
        Orientation::Right => Point::new(w, h / 2.0),
    };
    let zoom = Zoom { origin, scale };
    let all = cupertino_wm::geometry::Rect::new(0.0, 0.0, w, h);
    let mut children: Vec<Element<M>> = vec![
        Element::new(PanelNode {
            rect: zoom.rect(&all),
            radius: StackLayout::RADIUS * scale,
            alpha,
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
    let tile = Key::from(format!("tile-{}", view.tile));
    let anchor = match view.orientation {
        // centred over the tile, above it: below has no room at the bottom of the output, so the popup flips
        Orientation::Bottom => PopupAnchor::Below {
            key: tile,
            gap: GAP,
        },
        // beside the tile, on the side away from the screen edge
        Orientation::Left | Orientation::Right => PopupAnchor::Beside {
            key: tile,
            offset: Point::new(GAP, 0.0),
        },
    };
    Window {
        key: key(),
        spec: WindowSpec {
            title: view.name.to_owned(),
            app_id: String::new(),
            size: Size::new(w, h),
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

/// The panel: the bar's material (or its opaque fill where the session shows none) with the bar's fine edge.
struct PanelNode {
    rect: Rect,
    radius: f64,
    alpha: f64,
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
        } else if self.radius != old.radius || self.alpha != old.alpha {
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
        if !cx.material(BAR_SURFACE, radius, self.alpha) {
            if let Some(fill) = cx.env().theme.surface(BAR_SURFACE).fill {
                cx.push(PaintOp::Fill {
                    shape: Shape::RoundedRect { rect: all, radius },
                    paint: Paint::Solid(fill),
                });
            }
        }
        paint_edge(cx, all, radius);
    }

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
