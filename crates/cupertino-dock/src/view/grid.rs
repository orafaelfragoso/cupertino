//! The application grid: one window over the output with the keyboard, drawing a floating panel on the application
//! grid's material with a soft shadow. The search, the pills, the rows, the selection, the scroll and the motion are
//! the model's ([`AppGrid`]); the parts are where [`GridLayout`] puts them, scaled about the panel's centre by the
//! entry's and the exit's zoom, and the whole content fades with them. The rows scroll inside a clip; only the rows
//! shown are nodes. Every application shown is a button for assistive technology, which can click it; every pill is a
//! check box. When the panel folds into its search capsule, the pills, the rows and the separators fade out as one
//! layer and leave the tree once gone; the panel's shape shrinks into the capsule around the search field.

use cupertino_kit::core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, CommandRegistry, Element, Event,
    EventCx, Key, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::render::{Effect, FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};

use super::paint::{self, Root};
use super::{kit_rect, IconSet, Input, GRID_APP_ID};
use crate::app_grid::{AppGrid, GridLayout, Pill, Zone};

/// What assistive technology calls the grid, and the search field.
pub const TITLE: &str = "Applications";
/// What assistive technology calls the recents row, the group its applications are in apart from the full list.
pub const RECENTS: &str = "Recently Used";
/// The search field's placeholder in the capsule (the panel's says [`TITLE`]), and the text shown when nothing
/// matches.
pub const PLACEHOLDER: &str = "Search";
pub const NO_RESULTS: &str = "No Results";

/// The panel's shadow: offset down, blur and opacity, and how far past the panel it is drawn.
const SHADOW_OFFSET: f64 = 8.0;
const SHADOW_BLUR: f64 = 24.0;
const SHADOW_ALPHA: f32 = 0.28;
const SHADOW_REACH: f64 = 3.0 * SHADOW_BLUR + SHADOW_OFFSET;
/// Below this opacity the panel changes no pixel by a whole level, so it paints no shadow: the shadow is a blur of the
/// panel's whole size, the dearest part of a frame, and the entry's first frame starts from nothing.
const INVISIBLE_ALPHA: f64 = 0.5 / 255.0;
/// The panel's outline: the separator colour at this share, one point wide.
const OUTLINE_SHARE: f32 = 0.6;
/// A pill that is off: the label colour at this share; the scroll indicator's thumb the same way.
const PILL_FILL_SHARE: f32 = 0.08;
const THUMB_SHARE: f32 = 0.3;
/// An application whose icon is still loading: a rounded square of the label colour at this share, its corners this
/// share of its side, where the icon will be.
const ICON_PLACEHOLDER_SHARE: f32 = 0.08;
const ICON_PLACEHOLDER_ROUNDING: f64 = 0.22;
/// The magnifying glass: its lens's share of the symbol's size and its stroke width; the caret's width.
const LENS_SHARE: f64 = 0.62;
const SYMBOL_STROKE: f64 = 2.0;
const CARET_WIDTH: f64 = 2.0;

pub fn key() -> WindowKey {
    WindowKey(Key::from("app-grid"))
}

/// What the grid shows this frame.
pub struct GridView<'a> {
    pub output: u32,
    pub grid: &'a AppGrid,
    pub now: f64,
    /// Icons by their theme name.
    pub icons: &'a dyn Fn(&str) -> IconSet,
    /// The window's size before the server sized it: the output's.
    pub size: Size,
    /// The grid moves by itself: the window sends [`Input::Frame`] at every frame.
    pub ticking: bool,
}

/// Scales about the panel's centre.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Zoom {
    centre: Point,
    scale: f64,
}

impl Zoom {
    fn rect(&self, r: &cupertino_wm::geometry::Rect) -> Rect {
        let r = kit_rect(r);
        let s = self.scale;
        Rect::new(
            self.centre.x + (r.x() - self.centre.x) * s,
            self.centre.y + (r.y() - self.centre.y) * s,
            r.width() * s,
            r.height() * s,
        )
    }
}

/// A theme colour, else `fallback`.
fn role(cx: &PaintCx<'_>, role: ColorRole, fallback: Color) -> Color {
    cx.env()
        .theme
        .colors
        .get(role, StateVariant::NORMAL)
        .unwrap_or(fallback)
}

fn share(c: Color, a: f32) -> Color {
    Color { a: c.a * a, ..c }
}

fn label_color(cx: &PaintCx<'_>) -> Color {
    role(cx, ColorRole::Text, Color::new(0.0, 0.0, 0.0, 1.0))
}

pub fn window<M: Clone + 'static>(view: GridView<'_>, input: fn(Input) -> M) -> Window<M> {
    let grid = view.grid;
    let visual = grid.visual(view.now);
    let mut children: Vec<Element<M>> = Vec::new();
    if let (Some(layout), Some((shape, radius))) = (grid.layout().copied(), grid.shape(view.now)) {
        let zoom = Zoom {
            centre: Point::new(layout.panel.cx(), layout.panel.cy()),
            scale: visual.scale,
        };
        let s = zoom.scale;
        let content_alpha = grid.content_alpha(view.now);
        children.push(
            Element::new(PanelNode {
                rect: zoom.rect(&shape),
                radius: radius * s,
                alpha: visual.alpha,
            })
            .key("panel"),
        );
        // the keys are on the focused pill, on the selected application, else on the search field
        let search_focus = match grid.zone() {
            Zone::Pills => grid.pill_focus().is_none(),
            Zone::Grid => grid.selected().is_none(),
        };
        // the search row stays at the top of the shape as it folds
        let search = cupertino_wm::geometry::Rect::new(
            shape.x,
            shape.y,
            shape.w,
            GridLayout::SEARCH_HEIGHT.min(shape.h),
        );
        children.push(
            Element::new(SearchNode {
                rect: zoom.rect(&search),
                query: grid.query().to_owned(),
                placeholder: if grid.is_folded() { PLACEHOLDER } else { TITLE },
                scale: s,
                focus: search_focus,
            })
            .key("search"),
        );
        // what folds away: the separators, the pills, the rows and the scroll indicator, faded as one layer
        let mut faded: Vec<Element<M>> = Vec::new();
        if content_alpha > 0.0 {
            faded.push(
                Element::new(SeparatorNode {
                    rect: zoom.rect(&layout.search_separator()),
                    announced: false,
                })
                .key("search-separator"),
            );
        }
        for (i, (pill, rect)) in grid
            .pill_rects()
            .into_iter()
            .enumerate()
            .filter(|_| content_alpha > 0.0)
        {
            faded.push(
                Element::new(PillNode {
                    pill,
                    rect: zoom.rect(&rect),
                    scale: s,
                    on: grid.pill() == Some(pill),
                    focus: grid.zone() == Zone::Pills && grid.pill_focus() == Some(i),
                    input,
                })
                .key(Key::from(format!("pill-{}", pill.name()))),
            );
        }
        if content_alpha > 0.0 {
            let content = zoom.rect(&layout.content);
            let mut rows: Vec<Element<M>> = Vec::new();
            let mut recents: Vec<Element<M>> = Vec::new();
            let selected = grid.selected().filter(|_| grid.zone() == Zone::Grid);
            for placed in grid.visible() {
                let Some(app) = grid.item(placed.index) else {
                    continue;
                };
                let cell = zoom.rect(&placed.cell);
                let item = zoom.rect(&layout.item_rect(&placed.cell));
                let icon = zoom.rect(&layout.icon_rect(&placed.cell));
                let row = if placed.index < grid.recents_shown() {
                    &mut recents
                } else {
                    &mut rows
                };
                row.push(
                    Element::new(ItemNode {
                        index: placed.index,
                        name: app.name.clone(),
                        cell: cell.translate(-content.x(), -content.y()),
                        item: item.translate(-cell.x(), -cell.y()),
                        icon: icon.translate(-cell.x(), -cell.y()),
                        label_cy: GridLayout::LABEL_CY * s,
                        scale: s,
                        focus: selected == Some(placed.index),
                        image: (view.icons)(&app.icon),
                        input,
                    })
                    // the recents and the list may hold the same application
                    .key(Key::from(format!("item-{}-{}", placed.index, app.key))),
                );
            }
            // the recents are a group of their own, so the applications they repeat from the list are told apart
            if !recents.is_empty() {
                rows.insert(
                    0,
                    Element::new(ClipNode {
                        rect: Rect::new(0.0, 0.0, content.width(), content.height()),
                        name: RECENTS,
                        children: recents,
                    })
                    .key("recents"),
                );
            }
            if let Some(y) = grid.separator_y() {
                let line = y - grid.scroll();
                if (0.0..=layout.content.h).contains(&line) {
                    rows.push(
                        Element::new(SeparatorNode {
                            rect: Rect::new(0.0, line * s, content.width(), 1.0),
                            announced: true,
                        })
                        .key("separator"),
                    );
                }
            }
            if grid.item_count() == 0 {
                rows.push(
                    Element::new(NoResultsNode {
                        rect: Rect::new(0.0, 0.0, content.width(), content.height()),
                        scale: s,
                    })
                    .key("no-results"),
                );
            }
            faded.push(
                Element::new(ClipNode {
                    rect: content,
                    name: TITLE,
                    children: rows,
                })
                .key("content"),
            );
            let thumb_alpha = grid.scroller_alpha(view.now);
            if let Some(thumb) = grid.scroller_thumb().filter(|_| thumb_alpha > 0.0) {
                faded.push(
                    Element::new(ThumbNode {
                        rect: zoom.rect(&thumb),
                        alpha: thumb_alpha,
                    })
                    .key("scroller"),
                );
            }
        }
        if !faded.is_empty() {
            children.push(
                Element::new(FadeNode {
                    size: Size::new(layout.width, layout.height),
                    alpha: content_alpha,
                    children: faded,
                })
                .key("folding"),
            );
        }
    }
    let size = grid
        .layout()
        .map_or(view.size, |l| Size::new(l.width, l.height));
    Window {
        key: key(),
        spec: WindowSpec {
            title: TITLE.to_owned(),
            app_id: format!("{GRID_APP_ID}@{}", view.output),
            size,
            min_size: None,
            max_size: None,
            decorations: DecorationMode::None,
            foreign_parent: None,
            input_region: None,
        },
        shortcuts: CommandRegistry::new(),
        on_close: Some(input(Input::Closed(key()))),
        background: WindowBackground::None,
        content: Element::new(Root {
            role: Role::Dialog,
            name: TITLE.to_owned(),
            pointer: Some(Box::new(move |p| input(Input::Grid(p)))),
            key: Some(Box::new(move |k| Some(input(Input::GridKey(k))))),
            roving: false,
            on_blur: Some(input(Input::GridLeft)),
            alpha: visual.alpha,
            // repaint the root once the surface is sized: a frame asked for before the compositor configured it is
            // not given, and the entry runs on the frames the root asks for
            revision: u64::from(grid.layout().is_some()),
            under: None,
            frame: view.ticking.then(|| input(Input::Frame)),
            children,
        }),
        popup: None,
        menus: None,
    }
}

/// The panel: its shadow, the application grid's material under it (or the window background's opaque fill where the
/// session shows none) and a fine outline.
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
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = self.radius.min(size.height / 2.0);
        let theme = cx.env().theme.clone();
        let fill = theme
            .surface(Surface::AppGrid)
            .fill
            .unwrap_or(Color::new(0.5, 0.5, 0.5, 1.0));
        let shape = Shape::RoundedRect { rect, radius };
        let shadow = Effect::DropShadow {
            offset: Point::new(0.0, SHADOW_OFFSET),
            blur: SHADOW_BLUR,
            color: Color::new(0.0, 0.0, 0.0, SHADOW_ALPHA),
        };
        let material = cx.material(Surface::AppGrid, radius, self.alpha);
        if self.alpha < INVISIBLE_ALPHA {
            if !material {
                cx.push(PaintOp::Fill {
                    shape: shape.clone(),
                    paint: Paint::Solid(fill),
                });
            }
        } else if material {
            // the compositor draws the material under the panel: the shadow is drawn only outside it, from an opaque
            // copy of its shape the clip hides
            let mut ring = rounded_path(rect.outset(SHADOW_REACH), 0.0);
            ring.extend(rounded_path(rect, radius));
            cx.push(PaintOp::PushClip(Shape::Path {
                verbs: ring,
                fill_rule: FillRule::EvenOdd,
            }));
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![shadow],
            });
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(fill),
            });
            cx.push(PaintOp::PopLayer);
            cx.push(PaintOp::PopClip);
        } else {
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![shadow],
            });
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(fill),
            });
            cx.push(PaintOp::PopLayer);
        }
        let outline = share(
            role(cx, ColorRole::Separator, Color::new(0.0, 0.0, 0.0, 0.1)),
            OUTLINE_SHARE,
        );
        cx.push(PaintOp::Stroke {
            shape: Shape::RoundedRect {
                rect: rect.outset(-0.5),
                radius: (radius - 0.5).max(0.0),
            },
            width: 1.0,
            paint: Paint::Solid(outline),
        });
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}

/// A rectangle with rounded corners as a path, clockwise.
fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

/// Follow the model's keys with the toolkit's focus, so a screen reader reads what they are on: a node the keys came to
/// asks for a frame ([`PaintCx::request_animation_frame`]) and takes the focus on it, once each time they come.
fn follow_focus<M>(
    focus: bool,
    took: &mut bool,
    event: &Event,
    cx: &mut EventCx<'_, M>,
) -> Propagation {
    match event {
        Event::AnimationFrame(_) if focus && !*took => {
            *took = true;
            cx.request_focus();
            Propagation::Continue
        }
        Event::Action(request) if request.action == Action::Focus => {
            cx.request_focus();
            Propagation::Stop
        }
        _ => Propagation::Continue,
    }
}

/// The search field: a magnifying glass, then what was typed with the caret after it, or the placeholder. The model
/// types into it: the keys go to the window.
struct SearchNode {
    rect: Rect,
    query: String,
    /// What the empty field says.
    placeholder: &'static str,
    scale: f64,
    /// The keys are on the field: neither a pill nor an application has them.
    focus: bool,
}

impl<M: 'static> Widget<M> for SearchNode {
    /// The node took the focus since the keys came to it: it takes it once each time, so the pointer can move it.
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, took: &mut bool) -> Change {
        if !self.focus {
            *took = false;
        }
        if self.rect != old.rect {
            Change::Layout
        } else if self.query != old.query
            || self.placeholder != old.placeholder
            || self.scale != old.scale
            || self.focus != old.focus
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.rect.width(), self.rect.height()),
            baseline: None,
        }
    }

    fn event(&self, took: &mut bool, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        follow_focus(self.focus, took, event, cx)
    }

    fn hit(&self, _: &bool, _: Point) -> bool {
        false
    }

    fn paint(&self, took: &bool, cx: &mut PaintCx<'_>) {
        if self.focus && !*took {
            cx.request_animation_frame();
        }
        let size = cx.size();
        let s = self.scale;
        let cy = size.height / 2.0;
        let secondary = role(cx, ColorRole::SecondaryText, Color::new(0.0, 0.0, 0.0, 0.5));
        // the magnifying glass: a lens up and to the left, its handle down to the right
        let side = GridLayout::SEARCH_SYMBOL * s;
        let centre = Point::new(GridLayout::SEARCH_SYMBOL_X * s, cy);
        let lens = side * LENS_SHARE;
        let lens_rect = Rect::new(centre.x - side / 2.0, centre.y - side / 2.0, lens, lens);
        paint::stroke(cx, lens_rect, lens / 2.0, SYMBOL_STROKE * s, secondary);
        let from = Point::new(lens_rect.x() + lens * 0.85, lens_rect.y() + lens * 0.85);
        let to = Point::new(centre.x + side / 2.0, centre.y + side / 2.0);
        cx.push(PaintOp::Stroke {
            shape: Shape::Path {
                verbs: vec![PathVerb::MoveTo(from), PathVerb::LineTo(to)],
                fill_rule: FillRule::NonZero,
            },
            width: SYMBOL_STROKE * s,
            paint: Paint::Solid(secondary),
        });
        let x = GridLayout::SEARCH_TEXT_X * s;
        let max = (size.width - x - GridLayout::INSET * s).max(0.0);
        let font = GridLayout::SEARCH_FONT_PT * s;
        let caret_h = font * 1.1;
        let caret_x = if self.query.is_empty() {
            let placeholder = role(cx, ColorRole::PlaceholderText, secondary);
            paint::text(
                cx,
                self.placeholder,
                font,
                GridLayout::SEARCH_WEIGHT,
                x + CARET_WIDTH * s + 1.0,
                cy,
                Some(max),
                placeholder,
            );
            x
        } else {
            let text = label_color(cx);
            x + paint::text(
                cx,
                &self.query,
                font,
                GridLayout::SEARCH_WEIGHT,
                x,
                cy,
                Some(max),
                text,
            ) + 1.0
        };
        let caret = role(cx, ColorRole::KeyboardFocusIndicator, label_color(cx));
        paint::fill(
            cx,
            Rect::new(caret_x, cy - caret_h / 2.0, CARET_WIDTH * s, caret_h),
            0.0,
            caret,
        );
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        if self.focus {
            cx.focusable();
        }
        cx.role(Role::TextInput);
        cx.name(TITLE.to_owned());
        cx.description(self.placeholder.to_owned());
        cx.value(AccessibleValue::Text {
            value: self.query.clone(),
            editing: None,
        });
    }
}

/// A category pill: its name on a faint capsule, the selected one on the accent, a focus ring while the keys are on it.
struct PillNode<M> {
    pill: Pill,
    rect: Rect,
    scale: f64,
    on: bool,
    /// The keys are on the pill.
    focus: bool,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for PillNode<M> {
    /// The node took the focus since the keys came to it: it takes it once each time, so the pointer can move it.
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, took: &mut bool) -> Change {
        if !self.focus {
            *took = false;
        }
        if self.rect != old.rect {
            Change::Layout
        } else if self.on != old.on || self.focus != old.focus || self.scale != old.scale {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, took: &mut bool, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::GridPill(self.pill)));
                Propagation::Stop
            }
            _ => follow_focus(self.focus, took, event, cx),
        }
    }

    fn paint(&self, took: &bool, cx: &mut PaintCx<'_>) {
        if self.focus && !*took {
            cx.request_animation_frame();
        }
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = GridLayout::PILL_RADIUS * self.scale;
        let label = label_color(cx);
        let (fill, text) = if self.on {
            (
                role(cx, ColorRole::SelectedControlFill, label),
                role(
                    cx,
                    ColorRole::SelectedControlText,
                    Color::new(1.0, 1.0, 1.0, 1.0),
                ),
            )
        } else {
            (share(label, PILL_FILL_SHARE), label)
        };
        paint::fill(cx, rect, radius, fill);
        let env = cx.env().clone();
        let font = GridLayout::PILL_FONT_PT * self.scale;
        let max = (size.width - 2.0 * GridLayout::PILL_PAD * self.scale).max(0.0);
        if let Some(line) = paint::line(
            &env,
            self.pill.name(),
            font,
            GridLayout::PILL_WEIGHT,
            Some(max),
        ) {
            let (w, h) = (line.size().width, line.size().height);
            paint::glyphs(
                cx,
                &line,
                Point::new((size.width - w) / 2.0, (size.height - h) / 2.0),
                text,
            );
        }
        if self.focus {
            paint::focus_ring(cx, rect, radius);
        }
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        if self.focus {
            cx.focusable();
        }
        cx.role(Role::CheckBox);
        cx.name(self.pill.name().to_owned());
        cx.value(AccessibleValue::Toggle(if self.on {
            Checked::True
        } else {
            Checked::False
        }));
        cx.action(Action::Click);
    }
}

/// The scrolled area, or the recents row's group in it: its rows are placed in it and cut at its edges.
struct ClipNode<M> {
    rect: Rect,
    /// What assistive technology calls the group.
    name: &'static str,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for ClipNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width().max(0.0)),
            height: SizeSpec::Fixed(self.rect.height().max(0.0)),
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
        if self.rect != old.rect {
            Change::Layout
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

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(self.name.to_owned());
    }
}

struct ItemNode<M> {
    index: usize,
    name: String,
    /// The cell in the scrolled area.
    cell: Rect,
    /// The selection's rectangle and the icon's, in the cell; the label's centre line.
    item: Rect,
    icon: Rect,
    label_cy: f64,
    scale: f64,
    /// The selection, with the keys on it.
    focus: bool,
    image: IconSet,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for ItemNode<M> {
    /// The node took the focus since the keys came to it: it takes it once each time, so the pointer can move it.
    type State = bool;

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.cell)
    }

    fn state(&self) -> bool {
        false
    }

    fn update(&self, old: &Self, took: &mut bool) -> Change {
        if !self.focus {
            *took = false;
        }
        if self.cell != old.cell {
            Change::Layout
        } else if self.name != old.name
            || self.focus != old.focus
            || self.image != old.image
            || self.scale != old.scale
            || self.icon != old.icon
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &bool, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.cell.size,
            baseline: None,
        }
    }

    fn hit(&self, _: &bool, point: Point) -> bool {
        self.item.contains(point)
    }

    fn event(&self, took: &mut bool, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::GridItem(self.index)));
                Propagation::Stop
            }
            _ => follow_focus(self.focus, took, event, cx),
        }
    }

    fn paint(&self, took: &bool, cx: &mut PaintCx<'_>) {
        if self.focus && !*took {
            cx.request_animation_frame();
        }
        if self.focus {
            let fill = role(
                cx,
                ColorRole::SecondarySelectedControlFill,
                Color::new(0.5, 0.5, 0.5, 0.25),
            );
            paint::fill(
                cx,
                self.item,
                GridLayout::SELECTION_RADIUS * self.scale,
                fill,
            );
        }
        if self.image.is_empty() {
            let side = self.icon.width().min(self.icon.height());
            paint::fill(
                cx,
                self.icon,
                side * ICON_PLACEHOLDER_ROUNDING,
                share(label_color(cx), ICON_PLACEHOLDER_SHARE),
            );
        } else {
            paint::image(cx, &self.image, self.icon);
        }
        let env = cx.env().clone();
        let size = GridLayout::LABEL_FONT_PT * self.scale;
        let max = self.cell.width() - 2.0 * GridLayout::LABEL_INSET * self.scale;
        let Some(text) = paint::line(&env, &self.name, size, GridLayout::LABEL_WEIGHT, Some(max))
        else {
            return;
        };
        let (w, h) = (text.size().width, text.size().height);
        let origin = Point::new((self.cell.width() - w) / 2.0, self.label_cy - h / 2.0);
        let color = label_color(cx);
        paint::glyphs(cx, &text, origin, color);
    }

    fn semantics(&self, _: &bool, cx: &mut SemanticsCx<'_>) {
        if self.focus {
            cx.focusable();
        }
        cx.role(Role::Button);
        cx.name(self.name.clone());
        cx.bounds(self.item);
        cx.states(AccessibleStates {
            selected: self.focus,
            ..AccessibleStates::default()
        });
        cx.action(Action::Click);
    }
}

/// What fades out when the panel folds into its capsule, painted as one layer at `alpha` while it fades; it covers the
/// window, so its children keep the window's coordinates.
struct FadeNode<M> {
    size: Size,
    alpha: f64,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for FadeNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width.max(0.0)),
            height: SizeSpec::Fixed(self.size.height.max(0.0)),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self.alpha != old.alpha {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
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
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A separator: under the search row, and between the recents row and the rest. Only the one that splits the rows is
/// `announced` to assistive technology; the one under the search row is decoration.
struct SeparatorNode {
    rect: Rect,
    announced: bool,
}

impl<M: 'static> Widget<M> for SeparatorNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
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
        let c = role(cx, ColorRole::Separator, Color::new(0.0, 0.0, 0.0, 0.1));
        paint::fill(cx, Rect::new(0.0, 0.0, size.width, size.height), 0.0, c);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.announced {
            cx.role(Role::Separator);
        }
    }
}

/// "No Results", centred in the scrolled area.
struct NoResultsNode {
    rect: Rect,
    scale: f64,
}

impl<M: 'static> Widget<M> for NoResultsNode {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let font = GridLayout::NO_RESULTS_FONT_PT * self.scale;
        let env = cx.env().clone();
        let w = paint::width(&env, NO_RESULTS, font, GridLayout::SEARCH_WEIGHT);
        let c = role(cx, ColorRole::SecondaryText, Color::new(0.0, 0.0, 0.0, 0.5));
        paint::text(
            cx,
            NO_RESULTS,
            font,
            GridLayout::SEARCH_WEIGHT,
            (size.width - w) / 2.0,
            size.height / 2.0,
            None,
            c,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(NO_RESULTS.to_owned());
    }
}

/// The scroll indicator's thumb, at `alpha` while it fades out.
struct ThumbNode {
    rect: Rect,
    alpha: f64,
}

impl<M: 'static> Widget<M> for ThumbNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.alpha != old.alpha {
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
        let c = share(label_color(cx), THUMB_SHARE * self.alpha as f32);
        paint::fill(
            cx,
            Rect::new(0.0, 0.0, size.width, size.height),
            size.width / 2.0,
            c,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.hidden();
    }
}
