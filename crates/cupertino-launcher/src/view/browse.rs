//! The application browser's part of the panel, below the shared query row: a separator, the category filters, the
//! applications (a grid or a list, the recents first) clipped to their scrolled area and the scroll indicator; the view
//! options open as a menu in a popup of the panel's window. Everything is placed where [`Browser`]'s layout puts it; only the rows on screen
//! are nodes. The whole part fades as one layer with the browser's content opacity, so folding into the search field
//! withdraws it before the enclosure contracts.
//!
//! The query field keeps the keyboard throughout: the selection is shown and announced (the selected application is
//! read out through a polite live region), never focused. Filters are check boxes, applications are buttons in the
//! grid and list items in the list, the view options are a menu of two checked items.

use super::{glyphs, Input};
use crate::browse::{Arrangement, BrowseLayout, Browser, Category, Zone};
use cupertino_kit::controls::{ImageSource, Menu, MenuAnchor, MenuItem};
use cupertino_kit::core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, Element, Event, EventCx, Key, Live,
    MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget, Window, WindowKey,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, Overflow, SizeSpec};
use cupertino_kit::platform::{MouseButton, PointerEvent};
use cupertino_kit::render::{Paint, PaintOp, Sampling, Shape};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant};
use std::collections::HashMap;

/// What assistive technology calls the applications, and the recents row's group.
pub const TITLE: &str = "Applications";
pub const RECENTS: &str = "Recently Used";
/// What the browser says when no application is left.
pub const NO_RESULTS: &str = "No Results";
/// The view options button and its menu.
pub const VIEW_OPTIONS: &str = "View Options";
/// The view options button's key in the panel's window, which its menu opens below.
pub const OPTIONS_BUTTON: &str = "view-options-button";

/// A filter that is off and the scroll indicator's thumb: the label colour at these shares.
const FILTER_FILL_SHARE: f32 = 0.08;
const THUMB_SHARE: f32 = 0.3;
/// An application whose icon is not loaded: a rounded square of the label colour at this share, its corners this share
/// of its side.
const ICON_PLACEHOLDER_SHARE: f32 = 0.08;
const ICON_PLACEHOLDER_ROUNDING: f64 = 0.22;
/// "No Results": its size.
const NO_RESULTS_FONT_PT: f64 = 22.0;
/// A list row's name and its category: their sizes.
const LIST_NAME_PT: f64 = 14.0;
const LIST_CATEGORY_PT: f64 = 12.0;
/// The view options button: its side and its dots' radius and spacing.
pub const OPTIONS_SIZE: f64 = 28.0;
const DOT_RADIUS: f64 = 1.75;
const DOT_SPACING: f64 = 6.0;

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

fn label(cx: &PaintCx<'_>) -> Color {
    role(cx, ColorRole::Text, Color::new(0.0, 0.0, 0.0, 1.0))
}

fn fill(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, c: Color) {
    cx.push(PaintOp::Fill {
        shape: if radius > 0.0 {
            Shape::RoundedRect { rect, radius }
        } else {
            Shape::Rect(rect)
        },
        paint: Paint::Solid(c),
    });
}

/// The keyboard focus ring around `rect`, in the theme's focus colour.
fn focus_ring(cx: &mut PaintCx<'_>, rect: Rect, radius: f64) {
    let Some(c) = cx
        .env()
        .theme
        .colors
        .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
    else {
        return;
    };
    let width = 3.0;
    cx.push(PaintOp::Stroke {
        shape: Shape::RoundedRect {
            rect: rect.outset(width / 2.0),
            radius: radius + width / 2.0,
        },
        width,
        paint: Paint::Solid(c),
    });
}

/// One line of text, cut with an ellipsis at `max_width`.
fn line(
    cx: &PaintCx<'_>,
    text: &str,
    size: f64,
    weight: u16,
    max_width: f64,
) -> Option<ParagraphLayout> {
    if text.is_empty() {
        return None;
    }
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size as f32)
    };
    let paragraph = ParagraphStyle {
        wrap: Wrap::None,
        max_lines: Some(1),
        ellipsis: true,
        ..ParagraphStyle::default()
    };
    let text = cx
        .env()
        .text
        .paragraph(&AttributedText::plain(text, style), &paragraph)
        .ok()?;
    Some(text.layout(Some(max_width.max(0.0) as f32)))
}

/// The rendition of `icon` that best covers `rect` at the output's scale, drawn fitted in it.
fn image(cx: &mut PaintCx<'_>, icon: &[ImageSource], rect: Rect) {
    let px = rect.width().max(rect.height()) * cx.env().scale;
    let best = icon
        .iter()
        .filter(|s| f64::from(s.image.width.max(s.image.height)) >= px)
        .min_by_key(|s| s.image.width.max(s.image.height))
        .or_else(|| {
            icon.iter()
                .max_by_key(|s| s.image.width.max(s.image.height))
        });
    let Some(source) = best else {
        return;
    };
    let (w, h) = (
        f64::from(source.image.width),
        f64::from(source.image.height),
    );
    let k = (rect.width() / w).min(rect.height() / h);
    let dst = Rect::new(
        rect.x() + (rect.width() - w * k) / 2.0,
        rect.y() + (rect.height() - h * k) / 2.0,
        w * k,
        h * k,
    );
    cx.image(source.id, &source.image, dst, Sampling::Linear);
}

/// Places a node at `rect` of its absolute parent.
fn placed(rect: Rect, kind: ContainerKind) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fixed(rect.width().max(0.0)),
        height: SizeSpec::Fixed(rect.height().max(0.0)),
        position: rect.origin,
        ..LayoutSpec::new(kind)
    }
}

fn leaf_measure(rect: &Rect) -> Measured {
    Measured {
        size: rect.size,
        baseline: None,
    }
}

/// The browser's part of the panel, below the header row, laid out from the panel's top-left corner less the header.
pub fn body<M>(
    browser: &Browser,
    icons: &HashMap<String, Vec<ImageSource>>,
    alpha: f64,
    now: f64,
) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let layout = *browser.layout();
    let top = BrowseLayout::HEADER;
    let up = |r: Rect| r.translate(0.0, -top);
    let mut children: Vec<Element<M>> = vec![Element::new(SeparatorNode {
        rect: up(layout.header_separator()),
        announced: false,
    })
    .key("header-separator")];
    let filter_focus = browser.category_focus();
    for (i, (category, rect)) in browser.filter_rects().into_iter().enumerate() {
        children.push(
            Element::new(FilterNode {
                category,
                rect: up(rect),
                on: browser.category() == Some(category),
                focus: filter_focus == Some(i),
            })
            .key(format!("filter-{}", category.name())),
        );
    }
    let content = up(layout.content);
    let mut rows: Vec<Element<M>> = Vec::new();
    let mut recents: Vec<Element<M>> = Vec::new();
    let selected = browser
        .selected()
        .filter(|_| browser.zone() == Zone::Content);
    let list = browser.arrangement() == Arrangement::List;
    for placed in browser.visible() {
        let Some(app) = browser.item(placed.index) else {
            continue;
        };
        let cell = placed
            .cell
            .translate(-layout.content.x(), -layout.content.y());
        let recent = placed.index < browser.recents_shown();
        let node = ItemNode {
            index: placed.index,
            position: if recent {
                placed.index
            } else {
                placed.index - browser.recents_shown()
            },
            name: app.name.clone(),
            detail: list.then(|| app.first_category().name()),
            cell,
            list,
            selected: selected == Some(placed.index),
            image: icons.get(&app.id).cloned().unwrap_or_default(),
        };
        let row = if recent { &mut recents } else { &mut rows };
        // the recents and the list may hold the same application
        row.push(Element::new(node).key(format!("item-{}-{}", placed.index, app.id)));
    }
    let area = Rect::new(0.0, 0.0, content.width(), content.height());
    if !recents.is_empty() {
        rows.insert(
            0,
            Element::new(ClipNode {
                rect: area,
                name: RECENTS,
                role: if list { Role::List } else { Role::Group },
                count: browser.recents_shown(),
                children: recents,
            })
            .key("recents"),
        );
    }
    if let Some(y) = browser.separator_y() {
        let at = y - browser.scroll();
        if (0.0..=content.height()).contains(&at) {
            rows.push(
                Element::new(SeparatorNode {
                    rect: Rect::new(0.0, at, content.width(), 1.0),
                    announced: true,
                })
                .key("separator"),
            );
        }
    }
    if browser.item_count() == 0 {
        rows.push(Element::new(NoResultsNode { rect: area }).key("no-results"));
    }
    children.push(
        Element::new(ClipNode {
            rect: content,
            name: TITLE,
            role: if list { Role::List } else { Role::Group },
            count: browser.item_count() - browser.recents_shown(),
            children: rows,
        })
        .key("content"),
    );
    let thumb_alpha = browser.scroller_alpha(now);
    if let Some(thumb) = browser.scroller_thumb().filter(|_| thumb_alpha > 0.0) {
        children.push(
            Element::new(ThumbNode {
                rect: up(thumb),
                alpha: thumb_alpha,
            })
            .key("scroller"),
        );
    }
    // what the keys are on, read out as it changes
    let announced = match browser.zone() {
        Zone::Content if browser.item_count() == 0 => NO_RESULTS.to_owned(),
        Zone::Content => browser
            .selected_app()
            .map(|a| a.name.clone())
            .unwrap_or_default(),
        Zone::Categories => filter_focus
            .and_then(|i| browser.categories().get(i))
            .map(|c| c.name().to_owned())
            .unwrap_or_default(),
        Zone::Options => VIEW_OPTIONS.to_owned(),
    };
    children.push(Element::new(Announcer { text: announced }).key("announcer"));
    Element::new(BodyNode {
        size: Size::new(layout.panel.width(), layout.panel.height() - top),
        alpha,
        children,
    })
}

/// Where the view options button sits in a panel `width` points wide: at the header's end, centred on its row.
pub fn options_rect(width: f64) -> Rect {
    Rect::new(
        width - super::SIDE_PADDING - OPTIONS_SIZE,
        ((BrowseLayout::HEADER - OPTIONS_SIZE) / 2.0).round(),
        OPTIONS_SIZE,
        OPTIONS_SIZE,
    )
}

/// The browser's part: placed below the header, faded as one layer; the wheel scrolls the applications.
struct BodyNode<M> {
    size: Size,
    alpha: f64,
    children: Vec<Element<M>>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for BodyNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width.max(0.0)),
            height: SizeSpec::Fixed(self.size.height.max(0.0)),
            overflow: Overflow::Clip,
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

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(PointerEvent::Scroll { delta, .. }, _)
                if cx.phase() != Phase::Capture && self.alpha > 0.5 =>
            {
                cx.emit(M::from(Input::BrowseScroll(delta.y)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.alpha < 1.0 {
            cx.set_group(self.alpha.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.alpha <= 0.0 {
            cx.hidden();
        }
    }
}

/// A category filter: its name on a faint capsule, the one that is on in the selected-control colours, a focus ring
/// while the keys are on it.
struct FilterNode {
    category: Category,
    rect: Rect,
    on: bool,
    focus: bool,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for FilterNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if (self.on, self.focus, self.category) != (old.on, old.focus, old.category) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf_measure(&self.rect)
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::BrowseCategory(self.category)));
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(M::from(Input::BrowseCategory(self.category)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = BrowseLayout::FILTER_RADIUS;
        let text = label(cx);
        let (back, fore) = if self.on {
            (
                role(cx, ColorRole::SelectedControlFill, text),
                role(
                    cx,
                    ColorRole::SelectedControlText,
                    Color::new(1.0, 1.0, 1.0, 1.0),
                ),
            )
        } else {
            (share(text, FILTER_FILL_SHARE), text)
        };
        fill(cx, rect, radius, back);
        let max = size.width - 2.0 * BrowseLayout::FILTER_PAD;
        if let Some(l) = line(
            cx,
            self.category.name(),
            BrowseLayout::FILTER_FONT_PT,
            BrowseLayout::FILTER_WEIGHT,
            max,
        ) {
            let (w, h) = (l.size().width, l.size().height);
            glyphs(
                cx,
                &l,
                Point::new((size.width - w) / 2.0, (size.height - h) / 2.0),
                fore,
            );
        }
        if self.focus {
            focus_ring(cx, rect, radius);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::CheckBox);
        cx.name(self.category.name().to_owned());
        cx.value(AccessibleValue::Toggle(if self.on {
            Checked::True
        } else {
            Checked::False
        }));
        cx.states(AccessibleStates {
            selected: self.focus,
            ..AccessibleStates::default()
        });
        cx.action(Action::Click);
    }
}

/// The scrolled area, or the recents' group in it: its rows are placed in it and cut at its edges.
struct ClipNode<M> {
    rect: Rect,
    name: &'static str,
    role: Role,
    /// How many items it holds, shown or not.
    count: usize,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for ClipNode<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            overflow: Overflow::Clip,
            ..placed(self.rect, ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if (self.name, self.role) != (old.name, old.role) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf_measure(&self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(self.role);
        cx.name(self.name.to_owned());
        if self.role == Role::List {
            cx.item_count(self.count);
        }
    }
}

/// One application: its icon and name (a grid cell: the icon over the name; a list row: the icon, the name and its
/// category), the selection behind them. Hover selects it and a press opens it.
struct ItemNode {
    index: usize,
    /// Its place in its group (the recents, or every application).
    position: usize,
    name: String,
    /// The list row's category.
    detail: Option<&'static str>,
    cell: Rect,
    list: bool,
    selected: bool,
    image: Vec<ImageSource>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for ItemNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.cell, ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.cell != old.cell {
            Change::Layout
        } else if (
            self.index,
            self.position,
            &self.name,
            self.detail,
            self.list,
            self.selected,
        ) != (
            old.index,
            old.position,
            &old.name,
            old.detail,
            old.list,
            old.selected,
        ) || self.image != old.image
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf_measure(&self.cell)
    }

    fn hit(&self, _: &(), point: Point) -> bool {
        let local = Rect::new(0.0, 0.0, self.cell.width(), self.cell.height());
        BrowseLayout::item_rect(&local).contains(point)
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Motion { .. }, _) if !self.selected => {
                cx.emit(M::from(Input::BrowseHover(self.index)));
                Propagation::Continue
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::BrowseActivate(self.index)));
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(M::from(Input::BrowseActivate(self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let local = Rect::new(0.0, 0.0, self.cell.width(), self.cell.height());
        let item = BrowseLayout::item_rect(&local);
        if self.selected {
            let c = role(
                cx,
                ColorRole::SecondarySelectedControlFill,
                Color::new(0.5, 0.5, 0.5, 0.25),
            );
            fill(cx, item, BrowseLayout::SELECTION_RADIUS, c);
        }
        let icon = if self.list {
            BrowseLayout::list_icon(&local)
        } else {
            BrowseLayout::grid_icon(&local)
        };
        if self.image.is_empty() {
            let side = icon.width().min(icon.height());
            let c = share(label(cx), ICON_PLACEHOLDER_SHARE);
            fill(cx, icon, side * ICON_PLACEHOLDER_ROUNDING, c);
        } else {
            image(cx, &self.image, icon);
        }
        let text = label(cx);
        if self.list {
            let x = icon.max_x() + BrowseLayout::LIST_GAP;
            let end = item.max_x() - BrowseLayout::LIST_INSET;
            let detail = self
                .detail
                .and_then(|d| line(cx, d, LIST_CATEGORY_PT, 400, 160.0));
            let detail_w = detail.as_ref().map_or(0.0, |d| d.size().width);
            if let Some(name) = line(cx, &self.name, LIST_NAME_PT, 400, end - x - detail_w - 12.0) {
                let h = name.size().height;
                glyphs(cx, &name, Point::new(x, (local.height() - h) / 2.0), text);
            }
            if let Some(d) = detail {
                let c = role(cx, ColorRole::SecondaryText, share(text, 0.6));
                let (w, h) = (d.size().width, d.size().height);
                glyphs(cx, &d, Point::new(end - w, (local.height() - h) / 2.0), c);
            }
        } else {
            let max = local.width() - 2.0 * BrowseLayout::LABEL_INSET;
            if let Some(name) = line(
                cx,
                &self.name,
                BrowseLayout::LABEL_FONT_PT,
                BrowseLayout::LABEL_WEIGHT,
                max,
            ) {
                let (w, h) = (name.size().width, name.size().height);
                let origin =
                    Point::new((local.width() - w) / 2.0, BrowseLayout::LABEL_CY - h / 2.0);
                glyphs(cx, &name, origin, text);
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(if self.list {
            Role::ListItem
        } else {
            Role::Button
        });
        cx.name(self.name.clone());
        if let Some(detail) = self.detail {
            cx.description(detail.to_owned());
        }
        let local = Rect::new(0.0, 0.0, self.cell.width(), self.cell.height());
        cx.bounds(BrowseLayout::item_rect(&local));
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.item_index(self.position);
        cx.action(Action::Click);
    }
}

/// A separator: under the header, and between the recents and the rest. Only the one between the rows is announced.
struct SeparatorNode {
    rect: Rect,
    announced: bool,
}

impl<M: 'static> Widget<M> for SeparatorNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, ContainerKind::Leaf)
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
        leaf_measure(&self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        if let Some(c) = super::separator_color(cx) {
            fill(cx, Rect::new(0.0, 0.0, size.width, size.height), 0.0, c);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.announced {
            cx.role(Role::Separator);
        } else {
            cx.hidden();
        }
    }
}

/// "No Results", centred in the scrolled area.
struct NoResultsNode {
    rect: Rect,
}

impl<M: 'static> Widget<M> for NoResultsNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, ContainerKind::Leaf)
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
        leaf_measure(&self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let c = role(cx, ColorRole::SecondaryText, Color::new(0.0, 0.0, 0.0, 0.5));
        if let Some(l) = line(cx, NO_RESULTS, NO_RESULTS_FONT_PT, 400, size.width) {
            let (w, h) = (l.size().width, l.size().height);
            glyphs(
                cx,
                &l,
                Point::new((size.width - w) / 2.0, (size.height - h) / 2.0),
                c,
            );
        }
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
        placed(self.rect, ContainerKind::Leaf)
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
        leaf_measure(&self.rect)
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let c = share(label(cx), THUMB_SHARE * self.alpha as f32);
        fill(
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

/// What the keys are on, read out by assistive technology as it changes; it draws nothing.
struct Announcer {
    text: String,
}

impl<M: 'static> Widget<M> for Announcer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(Rect::new(0.0, 0.0, 1.0, 1.0), ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.text != old.text {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        leaf_measure(&Rect::new(0.0, 0.0, 1.0, 1.0))
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.text.is_empty() {
            cx.hidden();
            return;
        }
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
        cx.live(Live::Polite);
    }
}

/// The view options: a menu in a popup of the panel's window `parent`, below the button and free of the panel's
/// bounds, on the menu material every menu of the shell uses. One item per arrangement, the current one checked and
/// focused when it opens; choosing one sends [`Input::Arrange`], and Escape or a press outside [`Input::CloseMenu`].
pub fn options_menu<M>(parent: WindowKey, current: Arrangement, key: WindowKey) -> Vec<Window<M>>
where
    M: From<Input> + Clone + 'static,
{
    let arrangements = [Arrangement::Grid, Arrangement::List];
    let items = arrangements
        .into_iter()
        .map(|choice| MenuItem::Item {
            label: choice.name().to_owned(),
            enabled: true,
            checked: choice == current,
            on_select: M::from(Input::Arrange(choice)),
        })
        .collect();
    Menu {
        parent,
        anchor: MenuAnchor::Below(Key::from(OPTIONS_BUTTON)),
        label: VIEW_OPTIONS.to_owned(),
        items,
        current: arrangements.iter().position(|&a| a == current),
        open: Vec::new(),
        on_open: Box::new(|_| M::from(Input::CloseMenu)),
        on_close: M::from(Input::CloseMenu),
    }
    .windows(key)
}

/// The view options button at the header's end: three dots, a ring while the keys are on it.
pub(super) struct OptionsButton {
    pub focus: bool,
    pub expanded: bool,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for OptionsButton {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(OPTIONS_SIZE),
            height: SizeSpec::Fixed(OPTIONS_SIZE),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.focus, self.expanded) != (old.focus, old.expanded) {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(OPTIONS_SIZE, OPTIONS_SIZE),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(M::from(Input::ViewOptions));
                Propagation::Stop
            }
            Event::Action(request)
                if matches!(request.action, Action::Click | Action::ShowMenu) =>
            {
                cx.emit(M::from(Input::ViewOptions));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let c = role(cx, ColorRole::SecondaryText, Color::new(0.0, 0.0, 0.0, 0.5));
        if self.expanded {
            let back = share(label(cx), FILTER_FILL_SHARE);
            fill(cx, rect, size.width / 2.0, back);
        }
        let centre = rect.center();
        for k in [-1.0, 0.0, 1.0] {
            let p = Point::new(centre.x + k * DOT_SPACING, centre.y);
            let dot = Rect::new(
                p.x - DOT_RADIUS,
                p.y - DOT_RADIUS,
                2.0 * DOT_RADIUS,
                2.0 * DOT_RADIUS,
            );
            fill(cx, dot, DOT_RADIUS, c);
        }
        if self.focus {
            focus_ring(cx, rect, size.width / 2.0);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(VIEW_OPTIONS.to_owned());
        cx.states(AccessibleStates {
            expanded: Some(self.expanded),
            selected: self.focus,
            ..AccessibleStates::default()
        });
        cx.action(Action::ShowMenu);
    }
}
