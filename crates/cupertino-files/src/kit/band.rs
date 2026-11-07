//! The rubber band of the icon grid and the list: a press on empty space and a drag select the items the band
//! touches. Without a modifier the band's items replace the selection; with Shift or Primary held at the press they
//! toggle against the selection the press found. Near the top or bottom edge of the view the content scrolls under the
//! band, faster the deeper the pointer is in the edge's zone, and the band's far corner stays under the pointer.
//!
//! The view keeps the band in its node state and reports each change to the window, which shows it with [`BandView`]
//! over the items, so the band and the selection rebuild together.

use std::collections::BTreeSet;

use cupertino_kit::controls::Selection;
use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Constraints, ContainerKind, LayoutSpec, Measured, ScrollState, SizeSpec,
};
use cupertino_kit::platform::{Mods, Timestamp};
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{ColorRole, StateVariant};

use super::{Event as WindowEvent, Message};
use crate::metrics::marquee;

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
pub(super) fn follow(
    window: u64,
    scroll: ScrollState,
    pointer: Point,
    view: Rect,
    cx: &mut EventCx<'_, Message>,
) -> Option<ScrollState> {
    let dy = edge_step(pointer.y, view.min_y(), view.max_y());
    if dy == 0.0 {
        return None;
    }
    let next = scrolled(scroll, dy)?;
    cx.emit(Message::Window(window, WindowEvent::Scroll(next)));
    cx.wake_at(Timestamp(cx.now().0 + AUTOSCROLL_NS));
    Some(next)
}

/// The band over a view's items: a quaternary label fill inside a border in the placeholder text colour.
pub(super) struct BandView {
    /// In the view's content.
    pub rect: Rect,
}

impl Widget<Message> for BandView {
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

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
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
pub(super) fn view(rect: Rect) -> Element<Message> {
    Element::new(BandView { rect }).key("band")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn picked(selection: &Selection) -> Vec<usize> {
        selection.ranges().iter().flat_map(Clone::clone).collect()
    }

    #[test]
    fn a_plain_band_replaces_the_selection_and_a_modified_one_toggles_against_it() {
        let before = Selection::new(std::iter::once(0..2), Some(1), Some(0));
        let plain = Band::start(Point::ZERO, Point::ZERO, Mods::NONE, &before);
        assert_eq!(picked(&plain.select(&[1, 2])), [1, 2]);
        assert_eq!(plain.select(&[1, 2]).cursor(), Some(2));
        assert_eq!(picked(&plain.select(&[])), Vec::<usize>::new());
        for mods in [Mods::SHIFT, Mods::PRIMARY] {
            let toggled = Band::start(Point::ZERO, Point::ZERO, mods, &before);
            let next = toggled.select(&[1, 2]);
            assert_eq!(picked(&next), [0, 2], "{mods:?}");
            assert_eq!((next.cursor(), next.anchor()), (Some(2), Some(0)));
            assert_eq!(
                picked(&toggled.select(&[])),
                [0, 1],
                "a band that left every item gives the selection back"
            );
        }
    }

    #[test]
    fn the_band_spans_its_press_and_the_pointer_in_the_content() {
        let band = Band {
            origin: Point::new(50.0, 300.0),
            pointer: Point::new(20.0, 100.0),
            base: None,
        };
        // the content starts 40 pt above the window's top: scrolled down by 40
        let rect = band.rect(Point::new(0.0, -40.0));
        assert_eq!(rect, Rect::new(20.0, 140.0, 30.0, 160.0));
    }

    #[test]
    fn the_edge_scroll_grows_with_the_depth_in_the_zone_and_stops_at_the_ends() {
        let (top, bottom) = (100.0, 500.0);
        assert_eq!(edge_step(300.0, top, bottom), 0.0);
        assert_eq!(edge_step(125.0, top, bottom), -10.0);
        assert_eq!(edge_step(100.0, top, bottom), -20.0);
        assert_eq!(
            edge_step(0.0, top, bottom),
            -20.0,
            "beyond the edge, the most"
        );
        assert_eq!(edge_step(475.0, top, bottom), 10.0);
        assert_eq!(edge_step(900.0, top, bottom), 20.0);

        let scroll = ScrollState {
            offset: Point::new(0.0, 5.0),
            content: Size::new(100.0, 1000.0),
            viewport: Size::new(100.0, 400.0),
        };
        assert_eq!(scrolled(scroll, -20.0).unwrap().offset.y, 0.0);
        assert_eq!(scrolled(scrolled(scroll, -20.0).unwrap(), -20.0), None);
        assert_eq!(scrolled(scroll, 1000.0).unwrap().offset.y, 600.0);
    }
}
