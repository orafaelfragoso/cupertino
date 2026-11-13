//! The files scope's part of the panel, below the shared query row: a separator, the three filters, the suggested
//! files, a heading and the grid of files (a thumbnail over a name of up to two lines), clipped to their scrolled
//! area, and the scroll indicator. Everything is placed where [`FilesScope`]'s layout puts it; only the cells on
//! screen are nodes. The whole part fades as one layer with the scope's content opacity, so folding into the search
//! field withdraws it before the enclosure contracts.
//!
//! The query field keeps the keyboard throughout: the selection is shown and announced, never focused. Filters are
//! check boxes, files are buttons.

use super::browse::{
    fill, focus_ring, image, label, leaf_measure, line, placed, role, share, Announcer, BodyNode,
    ClipNode, SeparatorNode, ThumbNode,
};
use super::{glyphs, Input};
use crate::scope::{FilesScope, Filter, ScopeLayout, Zone, NO_RESULTS, SUGGESTED, TITLE};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, Element, Event, EventCx, MeasureCx,
    PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{Constraints, LayoutSpec, Measured};
use cupertino_kit::platform::{MouseButton, PointerEvent};
use cupertino_kit::text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole};
use std::collections::HashMap;
use std::path::PathBuf;

/// A filter that is off: the label colour at this share.
const FILTER_FILL_SHARE: f32 = 0.08;
/// A file whose picture is not loaded: a rounded square of the label colour at this share, its corners this share of
/// its side.
const THUMB_PLACEHOLDER_SHARE: f64 = 0.08;
const THUMB_PLACEHOLDER_ROUNDING: f64 = 0.22;
/// "No Results": its size.
const NO_RESULTS_FONT_PT: f64 = 22.0;
/// What the filter row's group is called to assistive technology.
const FILTERS: &str = "Filters";

/// The scope's part of the panel, below the header row, laid out from the panel's top-left corner less the header.
/// `alpha` is the content's opacity (what input and assistive technology follow); `panel` the enclosure's, which the
/// body is drawn at too when the enclosure leaves its fade to its parts.
pub fn body<M>(
    scope: &FilesScope,
    images: &HashMap<PathBuf, Vec<ImageSource>>,
    alpha: f64,
    panel: f64,
    now: f64,
) -> Element<M>
where
    M: From<Input> + Clone + 'static,
{
    let layout = *scope.layout();
    let top = ScopeLayout::HEADER;
    let up = |r: Rect| r.translate(0.0, -top);
    let mut children: Vec<Element<M>> = vec![
        Element::new(SeparatorNode {
            rect: up(layout.header_separator()),
            announced: false,
        })
        .key("header-separator"),
        Element::new(SeparatorNode {
            rect: up(layout.filters_separator()),
            announced: false,
        })
        .key("filters-separator"),
    ];
    let focus = scope.filter_focus();
    for (i, (filter, rect)) in scope.filter_rects().into_iter().enumerate() {
        children.push(
            Element::new(PillNode {
                filter,
                rect: up(rect),
                on: scope.filter() == Some(filter),
                focus: focus == Some(i),
            })
            .key(format!("filter-{}", filter.name())),
        );
    }
    let content = up(layout.content);
    let mut rows: Vec<Element<M>> = Vec::new();
    let mut suggested: Vec<Element<M>> = Vec::new();
    let selected = scope.selected().filter(|_| scope.zone() == Zone::Content);
    for placed in scope.visible() {
        let Some(file) = scope.item(placed.index) else {
            continue;
        };
        let cell = placed
            .cell
            .translate(-layout.content.x(), -layout.content.y());
        let is_suggestion = placed.index < scope.suggested();
        let node = CellNode {
            index: placed.index,
            position: if is_suggestion {
                placed.index
            } else {
                placed.index - scope.suggested()
            },
            name: file.name.clone(),
            cell,
            selected: selected == Some(placed.index),
            image: images.get(&file.path).cloned().unwrap_or_default(),
        };
        let row = if is_suggestion {
            &mut suggested
        } else {
            &mut rows
        };
        row.push(Element::new(node).key(format!("cell-{}", file.path.display())));
    }
    let area = Rect::new(0.0, 0.0, content.width(), content.height());
    if !suggested.is_empty() {
        rows.insert(
            0,
            Element::new(ClipNode {
                rect: area,
                name: SUGGESTED,
                role: Role::Group,
                count: scope.suggested(),
                children: suggested,
            })
            .key("suggested"),
        );
    }
    if let Some(y) = scope.separator_y() {
        let at = y - scope.scroll();
        if (0.0..=content.height()).contains(&at) {
            rows.push(
                Element::new(SeparatorNode {
                    rect: Rect::new(
                        ScopeLayout::INSET,
                        at,
                        content.width() - 2.0 * ScopeLayout::INSET,
                        1.0,
                    ),
                    announced: true,
                })
                .key("separator"),
            );
        }
    }
    let heading = scope
        .heading_rect()
        .translate(-layout.content.x(), -layout.content.y());
    if heading.max_y() > 0.0 && heading.y() < content.height() {
        rows.push(
            Element::new(HeadingNode {
                rect: heading,
                text: scope.heading(),
            })
            .key("heading"),
        );
    }
    if scope.item_count() == 0 && scope.answered() {
        rows.push(Element::new(NoResultsNode { rect: area }).key("no-results"));
    }
    children.push(
        Element::new(ClipNode {
            rect: content,
            name: TITLE,
            role: Role::Group,
            count: scope.item_count() - scope.suggested(),
            children: rows,
        })
        .key("content"),
    );
    let thumb_alpha = scope.scroller_alpha(now);
    if let Some(thumb) = scope.scroller_thumb().filter(|_| thumb_alpha > 0.0) {
        children.push(
            Element::new(ThumbNode {
                rect: up(thumb),
                alpha: thumb_alpha,
            })
            .key("scroller"),
        );
    }
    // what the keys are on, read out as it changes
    let announced = match scope.zone() {
        Zone::Content if scope.item_count() == 0 && scope.answered() => NO_RESULTS.to_owned(),
        Zone::Content => scope
            .selected_item()
            .map(|f| f.name.clone())
            .unwrap_or_default(),
        Zone::Filters => focus
            .and_then(|i| Filter::ALL.get(i))
            .map(|f| f.name().to_owned())
            .unwrap_or_default(),
    };
    children.push(Element::new(Announcer { text: announced }).key("announcer"));
    Element::new(BodyNode {
        size: Size::new(layout.panel.width(), layout.panel.height() - top),
        alpha,
        opacity: alpha * panel.clamp(0.0, 1.0),
        children,
    })
}

/// One line or two of text, cut with an ellipsis at `max_width`, centred.
fn lines(cx: &PaintCx<'_>, text: &str, max_width: f64) -> Option<ParagraphLayout> {
    if text.is_empty() {
        return None;
    }
    let style = TextStyle {
        weight: FontWeight(ScopeLayout::LABEL_WEIGHT),
        ..TextStyle::new(ScopeLayout::LABEL_FONT_PT as f32)
    };
    let paragraph = ParagraphStyle {
        wrap: Wrap::Word,
        max_lines: Some(2),
        ellipsis: true,
        align: TextAlign::Center,
        ..ParagraphStyle::default()
    };
    let text = cx
        .env()
        .text
        .paragraph(&AttributedText::plain(text, style), &paragraph)
        .ok()?;
    Some(text.layout(Some(max_width.max(0.0) as f32)))
}

/// A filter: its name on a faint capsule, the one that is on in the selected-control colours, a focus ring while the
/// keys are on it.
struct PillNode {
    filter: Filter,
    rect: Rect,
    on: bool,
    focus: bool,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for PillNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, cupertino_kit::layout::ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if (self.on, self.focus, self.filter) != (old.on, old.focus, old.filter) {
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
                cx.emit(M::from(Input::ScopeFilter(self.filter)));
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(M::from(Input::ScopeFilter(self.filter)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = ScopeLayout::FILTER_RADIUS;
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
            // a pill lighter than the panel, its name in the field's placeholder tone
            (
                super::lift_color(cx).unwrap_or(share(text, FILTER_FILL_SHARE)),
                role(cx, ColorRole::SecondaryText, share(text, 0.6)),
            )
        };
        fill(cx, rect, radius, back);
        if let Some(l) = line(
            cx,
            self.filter.name(),
            ScopeLayout::FILTER_FONT_PT,
            ScopeLayout::FILTER_WEIGHT,
            size.width - 16.0,
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
        cx.name(self.filter.name().to_owned());
        cx.description(FILTERS.to_owned());
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

/// One file: its thumbnail over its name, the selection behind them. Hover selects it and a press opens it.
struct CellNode {
    index: usize,
    /// Its place in its group (the suggestions, or every file).
    position: usize,
    name: String,
    cell: Rect,
    selected: bool,
    image: Vec<ImageSource>,
}

impl<M: From<Input> + Clone + 'static> Widget<M> for CellNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.cell, cupertino_kit::layout::ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.cell != old.cell {
            Change::Layout
        } else if (self.index, self.position, &self.name, self.selected)
            != (old.index, old.position, &old.name, old.selected)
            || self.image != old.image
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
        ScopeLayout::item_rect(&local).contains(point)
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match event {
            Event::Pointer(PointerEvent::Motion { .. }, _) if !self.selected => {
                cx.emit(M::from(Input::ScopeHover(self.index)));
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
                cx.emit(M::from(Input::ScopeActivate(self.index)));
                Propagation::Stop
            }
            Event::Action(request) if request.action == Action::Click => {
                cx.emit(M::from(Input::ScopeActivate(self.index)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let local = Rect::new(0.0, 0.0, self.cell.width(), self.cell.height());
        if self.selected {
            let c = super::selection_color(cx).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.25));
            fill(
                cx,
                ScopeLayout::item_rect(&local),
                ScopeLayout::SELECTION_RADIUS,
                c,
            );
        }
        let thumb = ScopeLayout::thumb(&local);
        if self.image.is_empty() {
            let c = share(label(cx), THUMB_PLACEHOLDER_SHARE as f32);
            fill(cx, thumb, thumb.width() * THUMB_PLACEHOLDER_ROUNDING, c);
        } else {
            image(cx, &self.image, thumb);
        }
        let max = local.width() - 2.0 * ScopeLayout::LABEL_INSET;
        if let Some(name) = lines(cx, &self.name, max) {
            let color = label(cx);
            glyphs(
                cx,
                &name,
                Point::new(ScopeLayout::LABEL_INSET, ScopeLayout::LABEL_TOP),
                color,
            );
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.name.clone());
        let local = Rect::new(0.0, 0.0, self.cell.width(), self.cell.height());
        cx.bounds(ScopeLayout::item_rect(&local));
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.item_index(self.position);
        cx.action(Action::Click);
    }
}

/// The heading over the grid, in bold.
struct HeadingNode {
    rect: Rect,
    text: &'static str,
}

impl<M: 'static> Widget<M> for HeadingNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, cupertino_kit::layout::ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.text != old.text {
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
        let color = label(cx);
        if let Some(l) = line(cx, self.text, ScopeLayout::HEADING_FONT_PT, 700, size.width) {
            let h = l.size().height;
            glyphs(cx, &l, Point::new(0.0, (size.height - h) / 2.0), color);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.to_owned());
    }
}

/// "No Results", centred in the scrolled area.
struct NoResultsNode {
    rect: Rect,
}

impl<M: 'static> Widget<M> for NoResultsNode {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        placed(self.rect, cupertino_kit::layout::ContainerKind::Leaf)
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
