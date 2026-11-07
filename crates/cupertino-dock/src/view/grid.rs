//! The application grid: one window over the output with the keyboard and the app grid material. The pages, the
//! selection, the search and the motion are the model's ([`AppGrid`]); the parts are where [`GridLayout`] puts them,
//! scaled about the window's centre by the entry's and the exit's zoom, and the whole content fades with them.
//! Every shown application is a button for assistive technology, which can click it; so is every page dot.

use cupertino_kit::core::{
    AccessibleStates, AccessibleValue, Action, Change, CommandRegistry, Element, Event, EventCx,
    Key, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{DecorationMode, WindowSpec};
use cupertino_kit::theme::{Color, ColorRole, Surface};

use super::paint::{self, Root};
use super::{kit_rect, IconSet, Input, GRID_APP_ID};
use crate::app_grid::{AppGrid, GridLayout, PER_PAGE};

/// What assistive technology calls the grid.
pub const TITLE: &str = "Applications";
/// The search field's placeholder and the text shown when nothing matches.
pub const PLACEHOLDER: &str = "Search";
pub const NO_RESULTS: &str = "No Results";

/// Colours over the material: labels white with a black shadow one point down, the current dot white and the others
/// white at 0.2, the keyboard selection white at 0.2, the search field's fill and stroke, its placeholder, and "No
/// Results".
const LABEL: paint::Rgba = [1.0, 1.0, 1.0, 1.0];
const LABEL_SHADOW: paint::Rgba = [0.0, 0.0, 0.0, 0.2];
const DOT_CURRENT: paint::Rgba = [1.0, 1.0, 1.0, 1.0];
const DOT_OTHER: paint::Rgba = [1.0, 1.0, 1.0, 0.2];
const SELECTION: paint::Rgba = [1.0, 1.0, 1.0, 0.2];
const SEARCH_FILL: paint::Rgba = [1.0, 1.0, 1.0, 0.12];
const SEARCH_STROKE: paint::Rgba = [1.0, 1.0, 1.0, 0.3];
const PLACEHOLDER_ALPHA: f64 = 0.5;
const NO_RESULTS_ALPHA: f64 = 0.20392;
/// The selection's and the search field's corner radii, and the field's text inset.
const SELECTION_RADIUS: f64 = 8.0;
const SEARCH_RADIUS: f64 = 6.0;
const SEARCH_INSET: f64 = 8.0;
/// Over an opaque background (Reduce Transparency, Increase Contrast) the white parts take the theme's text colour,
/// the faint ones this share of it, and no shadow.
const OPAQUE_FAINT: f64 = 0.25;

pub fn key() -> WindowKey {
    WindowKey(Key::from("app-grid"))
}

/// What the grid shows this frame.
pub struct GridView<'a> {
    pub output: u32,
    pub grid: &'a AppGrid,
    pub layout: Option<GridLayout>,
    pub now: f64,
    /// Icons by their theme name.
    pub icons: &'a dyn Fn(&str) -> IconSet,
    /// The window's size before the server sized it: the output's.
    pub size: Size,
}

/// Scales about the window's centre.
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

/// The colours the grid draws with: over the material, or over the opaque fill.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Ink {
    label: Color,
    shadow: Option<Color>,
    faint: Color,
    selection: Color,
    dot_other: Color,
    search_fill: Color,
    search_stroke: Color,
}

impl Ink {
    fn of(cx: &PaintCx<'_>) -> Self {
        let env = cx.env();
        if paint::is_opaque(env) {
            let text = env
                .theme
                .colors
                .get(ColorRole::Text, cupertino_kit::theme::StateVariant::NORMAL)
                .unwrap_or_else(|| paint::color(LABEL));
            let share = |a: f64| Color {
                a: text.a * a as f32,
                ..text
            };
            Ink {
                label: text,
                shadow: None,
                faint: share(PLACEHOLDER_ALPHA),
                selection: share(OPAQUE_FAINT),
                dot_other: share(OPAQUE_FAINT),
                search_fill: share(OPAQUE_FAINT / 2.0),
                search_stroke: share(PLACEHOLDER_ALPHA),
            }
        } else {
            Ink {
                label: paint::color(LABEL),
                shadow: Some(paint::color(LABEL_SHADOW)),
                faint: paint::color(paint::faded(LABEL, PLACEHOLDER_ALPHA)),
                selection: paint::color(SELECTION),
                dot_other: paint::color(DOT_OTHER),
                search_fill: paint::color(SEARCH_FILL),
                search_stroke: paint::color(SEARCH_STROKE),
            }
        }
    }
}

pub fn window<M: Clone + 'static>(view: GridView<'_>, input: fn(Input) -> M) -> Window<M> {
    let visual = view.grid.visual(view.now);
    let mut children: Vec<Element<M>> = Vec::new();
    let mut no_results = None;
    if let Some(layout) = view.layout {
        let zoom = Zoom {
            centre: Point::new(layout.width / 2.0, layout.height / 2.0),
            scale: visual.scale,
        };
        let grid = view.grid;
        let position = grid.page_position(view.now);
        for page in 0..grid.page_count() {
            let frame = layout.page_frame(page, position);
            if frame.right() <= 0.0 || frame.x >= layout.width {
                continue;
            }
            let first = page * PER_PAGE;
            for slot in 0..PER_PAGE {
                let index = first + slot;
                let Some(app) = grid.result(index) else {
                    break;
                };
                let cell = layout.cell_rect(&frame, slot);
                let item = zoom.rect(&layout.item_rect(&cell));
                let image = zoom.rect(&layout.image_rect(&cell));
                let label = zoom.rect(&layout.label_rect(&cell, layout.cell.0));
                let cell = zoom.rect(&cell);
                children.push(
                    Element::new(ItemNode {
                        index,
                        name: app.name.clone(),
                        cell,
                        item: item.translate(-cell.x(), -cell.y()),
                        image: image.translate(-cell.x(), -cell.y()),
                        label_cy: label.y() - cell.y() + label.height() / 2.0,
                        label_max: (layout.cell.0
                            - GridLayout::LABEL_CELL_INSET
                            - GridLayout::LABEL_PAD)
                            * zoom.scale,
                        scale: zoom.scale,
                        selected: grid.selected() == Some(index),
                        icon: (view.icons)(&app.icon),
                        input,
                    })
                    .key(Key::from(format!("app-{}", app.key))),
                );
            }
        }
        let count = grid.page_count();
        for i in 0..count {
            children.push(
                Element::new(DotNode {
                    index: i,
                    count,
                    rect: zoom.rect(&layout.dot_rect(i, count)),
                    scale: zoom.scale,
                    current: i == grid.page(),
                    input,
                })
                .key(Key::from(format!("dot-{i}"))),
            );
        }
        children.push(
            Element::new(SearchNode {
                rect: zoom.rect(&layout.search),
                query: grid.query().to_owned(),
                scale: zoom.scale,
            })
            .key("search"),
        );
        if grid.result_count() == 0 && !grid.query().is_empty() {
            no_results = Some((zoom.rect(&layout.page), zoom.scale));
        }
    }
    let size = view
        .layout
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
        background: WindowBackground::Surface {
            surface: Surface::AppGrid,
            radius: 0.0,
            alpha: visual.alpha,
        },
        content: Element::new(Root {
            role: Role::Dialog,
            name: TITLE.to_owned(),
            pointer: Some(Box::new(move |p| input(Input::Grid(p)))),
            key: Some(Box::new(move |k| Some(input(Input::GridKey(k))))),
            roving: false,
            on_blur: Some(input(Input::GridLeft)),
            alpha: visual.alpha,
            revision: u64::from(no_results.is_some()),
            under: Some(Box::new(move |cx| {
                if let Some((page, scale)) = no_results {
                    let ink = Ink::of(cx);
                    let size = GridLayout::NO_RESULTS_FONT_PT * scale;
                    let env = cx.env().clone();
                    let w = paint::width(&env, NO_RESULTS, size, GridLayout::SEARCH_WEIGHT);
                    let c = Color {
                        a: ink.label.a * NO_RESULTS_ALPHA as f32,
                        ..ink.label
                    };
                    paint::text(
                        cx,
                        NO_RESULTS,
                        size,
                        GridLayout::SEARCH_WEIGHT,
                        page.x() + (page.width() - w) / 2.0,
                        page.y() + page.height() / 2.0,
                        None,
                        c,
                    );
                }
            })),
            children,
        }),
        popup: None,
        menus: None,
    }
}

struct ItemNode<M> {
    index: usize,
    name: String,
    cell: Rect,
    /// The selection's rectangle, the image's and the label's centre line, in the cell.
    item: Rect,
    image: Rect,
    label_cy: f64,
    label_max: f64,
    scale: f64,
    selected: bool,
    icon: IconSet,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for ItemNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.cell)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.cell != old.cell {
            Change::Layout
        } else if self.name != old.name
            || self.selected != old.selected
            || self.icon != old.icon
            || self.scale != old.scale
            || self.image != old.image
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.cell.width(), self.cell.height()),
            baseline: None,
        }
    }

    fn hit(&self, _: &(), point: Point) -> bool {
        self.item.contains(point)
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::GridItem(self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let ink = Ink::of(cx);
        if self.selected {
            paint::fill(cx, self.item, SELECTION_RADIUS * self.scale, ink.selection);
        }
        paint::image(cx, &self.icon, self.image);
        let env = cx.env().clone();
        let size = GridLayout::LABEL_FONT_PT * self.scale;
        let Some(text) = paint::line(
            &env,
            &self.name,
            size,
            GridLayout::LABEL_WEIGHT,
            Some(self.label_max),
        ) else {
            return;
        };
        let (w, h) = (text.size().width, text.size().height);
        let origin = Point::new((self.cell.width() - w) / 2.0, self.label_cy - h / 2.0);
        if let Some(shadow) = ink.shadow {
            paint::glyphs(
                cx,
                &text,
                Point::new(origin.x, origin.y + self.scale),
                shadow,
            );
        }
        paint::glyphs(cx, &text, origin, ink.label);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.name.clone());
        cx.bounds(self.item);
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.action(Action::Click);
    }
}

struct DotNode<M> {
    index: usize,
    count: usize,
    rect: Rect,
    scale: f64,
    current: bool,
    input: fn(Input) -> M,
}

impl<M: Clone + 'static> Widget<M> for DotNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.current != old.current || self.count != old.count {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.rect.width(), self.rect.height()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Action(request) if request.action == Action::Click => {
                cx.emit((self.input)(Input::GridPage(self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let ink = Ink::of(cx);
        let s = self.scale;
        let dot = Rect::new(
            GridLayout::DOT_INSET * s,
            GridLayout::DOT_INSET * s,
            GridLayout::DOT_SIZE * s,
            GridLayout::DOT_SIZE * s,
        );
        let c = if self.current {
            if paint::is_opaque(cx.env()) {
                ink.label
            } else {
                paint::color(DOT_CURRENT)
            }
        } else {
            ink.dot_other
        };
        paint::fill(cx, dot, GridLayout::DOT_SIZE * s / 2.0, c);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(format!("Page {} of {}", self.index + 1, self.count));
        cx.states(AccessibleStates {
            selected: self.current,
            ..AccessibleStates::default()
        });
        cx.action(Action::Click);
    }
}

/// The search field: what was typed, or the placeholder. The model types into it: the keys go to the window.
struct SearchNode {
    rect: Rect,
    query: String,
    scale: f64,
}

impl<M: 'static> Widget<M> for SearchNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        paint::placed(self.rect)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.query != old.query || self.scale != old.scale {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.rect.width(), self.rect.height()),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let ink = Ink::of(cx);
        let size = cx.size();
        let s = self.scale;
        let bounds = Rect::new(0.0, 0.0, size.width, size.height);
        paint::fill(cx, bounds, SEARCH_RADIUS * s, ink.search_fill);
        let edge = Rect::new(0.5, 0.5, size.width - 1.0, size.height - 1.0);
        paint::stroke(cx, edge, SEARCH_RADIUS * s, 1.0, ink.search_stroke);
        let (text, c) = if self.query.is_empty() {
            (PLACEHOLDER, ink.faint)
        } else {
            (self.query.as_str(), ink.label)
        };
        paint::text(
            cx,
            text,
            GridLayout::SEARCH_FONT_PT * s,
            GridLayout::SEARCH_WEIGHT,
            SEARCH_INSET * s,
            size.height / 2.0,
            Some(size.width - 2.0 * SEARCH_INSET * s),
            c,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::TextInput);
        cx.name(PLACEHOLDER.to_owned());
        cx.value(AccessibleValue::Text {
            value: self.query.clone(),
            editing: None,
        });
        cx.states(AccessibleStates {
            focused: true,
            ..AccessibleStates::default()
        });
    }
}
