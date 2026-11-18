//! The rubber band of the icon grid and the list: a press on empty space and a drag select the items the band
//! touches. Without a modifier the band's items replace the selection; with Shift or Primary held at the press they
//! toggle against the selection the press found. Near the top or bottom edge of the view the content scrolls under the
//! band, faster the deeper the pointer is in the edge's zone, and the band's far corner stays under the pointer.
//!
//! The view keeps the band in its node state and reports each change to the window, which shows it with [`BandView`]
//! over the items, so the band and the selection rebuild together.

use std::collections::BTreeSet;

use std::rc::Rc;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_theme::ui::{ColorRole, StateVariant};
use cupertino_ui_controls::Selection;
use cupertino_ui_core::layout::{
    Constraints, ContainerKind, LayoutSpec, Measured, ScrollState, SizeSpec,
};
use cupertino_ui_core::platform::{Mods, Timestamp};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};

use crate::metrics::marquee;
use crate::ViewEvent;

/// The time between two steps of the scroll at an edge, in nanoseconds.
pub(super) const AUTOSCROLL_NS: u64 = (marquee::AUTOSCROLL_INTERVAL * 1e9) as u64;

/// A band being drawn.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Band {
    /// Where the press was, in the content, so it stays on the items while they scroll.
    pub origin: Point,
    /// Where the pointer is, in the window.
    pub pointer: Point,
    /// The selection the press found, kept to toggle against when Shift or Primary was held.
    pub base: Option<Selection>,
}

impl Band {
    /// A band from a press at `pointer` in the window, at `origin` in the content, with `mods` held.
    pub fn start(origin: Point, pointer: Point, mods: Mods, selection: &Selection) -> Band {
        let toggles = mods.contains(Mods::SHIFT) || mods.contains(Mods::PRIMARY);
        Band {
            origin,
            pointer,
            base: toggles.then(|| selection.clone()),
        }
    }

    /// The band in the content whose top left corner is at `content` in the window.
    pub fn rect(&self, content: Point) -> Rect {
        let at = Point::new(self.pointer.x - content.x, self.pointer.y - content.y);
        span(self.origin, at)
    }

    /// The selection with the band over `hits`, in order: they replace the selection, or toggle against the one the
    /// press found. The cursor goes to the last item hit, and the anchor stays or comes there.
    pub fn select(&self, hits: &[usize]) -> Selection {
        let mut selected: BTreeSet<usize> = hits.iter().copied().collect();
        let (mut cursor, mut anchor) = (None, None);
        if let Some(base) = &self.base {
            let before: BTreeSet<usize> = base.ranges().iter().flat_map(Clone::clone).collect();
            selected = before.symmetric_difference(&selected).copied().collect();
            cursor = base.cursor();
            anchor = base.anchor();
        }
        if let Some(&last) = hits.last() {
            cursor = Some(last);
            anchor = anchor.or(Some(last));
        }
        Selection::new(selected.into_iter().map(|i| i..i + 1), cursor, anchor)
    }
}

/// The rectangle with corners `a` and `b`.
pub(super) fn span(a: Point, b: Point) -> Rect {
    let (x, y) = (a.x.min(b.x), a.y.min(b.y));
    Rect::new(x, y, (a.x - b.x).abs(), (a.y - b.y).abs())
}

/// How far the content scrolls in one step with the pointer at `y`, for a view showing `top` to `bottom` (all in the
/// window): up (negative) or down by up to [`marquee::MAX_STEP`], growing with how deep the pointer is in the zone of
/// [`marquee::EDGE_ZONE`] along that edge, the most beyond it.
pub(super) fn edge_step(y: f64, top: f64, bottom: f64) -> f64 {
    let zone = marquee::EDGE_ZONE.min((bottom - top) / 2.0).max(1.0);
    let depth = if y < top + zone {
        -(top + zone - y)
    } else if y > bottom - zone {
        y - (bottom - zone)
    } else {
        return 0.0;
    };
    marquee::MAX_STEP * depth.clamp(-zone, zone) / zone
}

/// `scroll` moved by `dy` within its content, or `None` when it cannot move that way.
pub(super) fn scrolled(scroll: ScrollState, dy: f64) -> Option<ScrollState> {
    let max = (scroll.content.height - scroll.viewport.height).max(0.0);
    let y = (scroll.offset.y + dy).clamp(0.0, max);
    (y != scroll.offset.y).then(|| ScrollState {
        offset: Point::new(scroll.offset.x, y),
        ..scroll
    })
}

/// What a view does when a band's pointer moves or its edge timer fires: the next scroll when the pointer is in an
/// edge's zone, and the wake for the step after it.
pub(super) fn follow<M>(
    on_event: &Rc<dyn Fn(ViewEvent) -> Option<M>>,
    scroll: ScrollState,
    pointer: Point,
    view: Rect,
    cx: &mut EventCx<'_, M>,
) -> Option<ScrollState> {
    let dy = edge_step(pointer.y, view.min_y(), view.max_y());
    if dy == 0.0 {
        return None;
    }
    let next = scrolled(scroll, dy)?;
    if let Some(message) = on_event(ViewEvent::Scroll(next)) {
        cx.emit(message);
    }
    cx.wake_at(Timestamp(cx.now().0 + AUTOSCROLL_NS));
    Some(next)
}

/// The band over a view's items: a quaternary label fill inside a border in the placeholder text colour.
pub(super) struct BandView {
    /// In the view's content.
    pub rect: Rect,
}

impl<M: 'static> Widget<M> for BandView {
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
        if self.rect != old.rect {
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        let colors = &cx.env().theme.colors;
        let fill = colors.get(ColorRole::QuaternaryLabel, StateVariant::NORMAL);
        let border = colors.get(ColorRole::PlaceholderText, StateVariant::NORMAL);
        if let Some(fill) = fill {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(area),
                paint: Paint::Solid(fill),
            });
        }
        let width = marquee::BORDER_WIDTH;
        if let Some(border) = border.filter(|_| size.width >= width && size.height >= width) {
            cx.push(PaintOp::Stroke {
                shape: Shape::Rect(Rect::new(
                    width / 2.0,
                    width / 2.0,
                    size.width - width,
                    size.height - width,
                )),
                paint: Paint::Solid(border),
                width,
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The band's element over a view's items.
pub(super) fn view<M: 'static>(rect: Rect) -> Element<M> {
    Element::new(BandView { rect }).key("band")
}
