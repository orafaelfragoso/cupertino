//! What the list view does beyond the toolkit's outline table: a press on empty space under the rows, or on a row's
//! blank part (anywhere but its icon and the text of its name) that then moves, draws a rubber band ([`super::band`]), which scrolls the rows at
//! the view's top and bottom edges; a double click opens a row, a press on the name of the only selected row renames it after
//! the double-click interval ([`super::clicks`]), a press with Ctrl held opens the context menu, a press that moves
//! drags the selected rows out of the window, and Tab and Shift+Tab on a row select the next and the previous item by
//! name ([`tab`]); past the last (or before the first) they move the keyboard on to the next control, so the list never
//! holds it. Tab on a column header goes to the next header, or into the rows from the last, as any control's does.
//!
//! [`RowCell`] wraps each cell of a row and takes the presses on it; the triangle before the name takes its own, so a
//! press on it only opens or closes the folder. [`Gestures`] holds the whole table and takes what no row took: the
//! presses under the rows, the band's pointer and the timers. The presses' state is shared between them ([`Presses`]),
//! as a press on one row and the next press on another count together.

use std::cell::RefCell;
use std::rc::Rc;

use cupertino_kit::controls::Selection;
use cupertino_kit::core::{
    Change, Element, Event, EventCx, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx, Widget,
};
use cupertino_kit::geometry::{Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    ScrollState, SizeSpec,
};
use cupertino_kit::platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent};
use cupertino_kit::text::{AttributedText, FontWeight, ParagraphStyle, TextStyle};

use super::band::{self, Band};
use super::clicks::Clicks;
use super::grid::by_name;
use super::{Event as WindowEvent, Message};
use crate::metrics::drag;

const TAB: Keysym = Keysym(0xff09);
const LEFT_TAB: Keysym = Keysym(0xfe20);

/// A press on a row that may become a drag.
struct RowPress {
    index: usize,
    /// In window coordinates.
    at: Point,
    /// The rows a drag carries, the pressed one first.
    dragged: Vec<usize>,
    /// The selection a plain press on a selected row found, given back when the press drags: the table selects the
    /// pressed row alone at once.
    restore: Option<Selection>,
    /// Where the press was from the top left corner of the row's first cell, and that cell's size.
    offset: Point,
    cell: Size,
}

/// A press on a row's blank part, which becomes a band once it moves.
#[derive(Clone)]
struct BlankPress {
    /// In window coordinates.
    at: Point,
    mods: Mods,
    /// The selection the press found, which a band with Shift or Primary toggles against.
    selection: Selection,
    /// The press moved far enough: the frame starts the band at the next move it sees.
    moved: bool,
}

/// The state of the list's presses, shared by its cells and its frame.
#[derive(Default)]
pub(super) struct Presses {
    clicks: Clicks,
    press: Option<RowPress>,
    blank: Option<BlankPress>,
    band: Option<Band>,
}

pub(super) type SharedPresses = Rc<RefCell<Presses>>;

/// A row dragged out of the list: what [`super::dragged::rows`] draws.
#[derive(Clone, Debug, PartialEq)]
pub struct RowGrab {
    /// The dragged rows, the pressed one first.
    pub indices: Vec<usize>,
    /// Where the press was from the top left corner of the pressed row's name cell, and that cell's size.
    pub offset: Point,
    pub cell: Size,
    /// The window reads right to left.
    pub rtl: bool,
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// The selection the table makes of a press on row `index` with `mods` held.
fn pressed(selection: &Selection, index: usize, mods: Mods) -> Selection {
    let mut next = selection.clone();
    if mods.contains(Mods::SHIFT) {
        next.extend_to(index);
    } else if mods.contains(Mods::PRIMARY) {
        next.toggle(index);
    } else {
        next.select_only(index);
    }
    next
}

/// One cell of row `index`: its presses.
pub(super) struct RowCell {
    pub window: u64,
    pub index: usize,
    /// The row's first cell, which holds its name.
    pub name: bool,
    /// The name the first cell shows, which tells where its text ends; `None` in the other cells.
    pub label: Option<String>,
    pub selection: Selection,
    pub presses: SharedPresses,
    pub children: Vec<Element<Message>>,
}

impl RowCell {
    fn emit(&self, event: WindowEvent, cx: &mut EventCx<'_, Message>) {
        cx.emit(Message::Window(self.window, event));
    }

    /// Whether `position` lands on the row's icon or the text of its name, rather than on the blank part of the name
    /// cell past them (a cell other than the name's is all blank).
    fn on_name(&self, position: Point, cx: &EventCx<'_, Message>) -> bool {
        if !self.name {
            return false;
        }
        let (Some(label), Some(bounds)) = (self.label.as_deref(), cx.bounds()) else {
            return true;
        };
        // the name in the body role, as the cell shows it
        let body = cx.env().theme.type_scale.body;
        let style = TextStyle {
            weight: FontWeight(body.weight),
            ..TextStyle::new(body.size)
        };
        let Ok(paragraph) = cx.env().text.paragraph(
            &AttributedText::plain(label, style),
            &ParagraphStyle::default(),
        ) else {
            return true;
        };
        let end =
            (super::view::NAME_TEXT_X + paragraph.layout(None).size().width).min(bounds.width());
        let from_leading = match cx.env().direction {
            LayoutDirection::Ltr => position.x - bounds.min_x(),
            LayoutDirection::Rtl => bounds.max_x() - position.x,
        };
        from_leading <= end
    }

    fn press(&self, position: Point, mods: Mods, cx: &mut EventCx<'_, Message>) -> Propagation {
        if mods.contains(Mods::CTRL) {
            // Ctrl with a press is the secondary click
            self.presses.borrow_mut().clicks.cancel();
            self.emit(WindowEvent::SecondaryPress(Some(self.index)), cx);
            self.emit(WindowEvent::ContextMenu(position), cx);
            return Propagation::Stop;
        }
        let index = self.index;
        let sole = self.selection.len() == 1 && self.selection.contains(index);
        let plain = !mods.contains(Mods::SHIFT) && !mods.contains(Mods::PRIMARY);
        let on_name = self.on_name(position, cx);
        let mut presses = self.presses.borrow_mut();
        if presses.clicks.press(index, on_name, sole, plain, cx.now()) {
            presses.press = None;
            drop(presses);
            self.emit(WindowEvent::Open(index), cx);
            return Propagation::Stop;
        }
        presses.clicks.arm(cx);
        if !on_name {
            // the row's blank part: the table selects the row, and a move draws a band from here
            presses.press = None;
            presses.blank = Some(BlankPress {
                at: position,
                mods,
                selection: self.selection.clone(),
                moved: false,
            });
            cx.capture_pointer();
            return Propagation::Continue;
        }
        let keep = plain && self.selection.contains(index) && self.selection.len() > 1;
        let next = if keep {
            self.selection.clone()
        } else {
            pressed(&self.selection, index, mods)
        };
        presses.press = next.contains(index).then(|| {
            let rest = next.ranges().iter().flat_map(Clone::clone);
            let bounds = cx.bounds().unwrap_or_default();
            RowPress {
                index,
                at: position,
                dragged: std::iter::once(index)
                    .chain(rest.filter(|&i| i != index))
                    .collect(),
                restore: keep.then(|| self.selection.clone()),
                offset: Point::new(position.x - bounds.min_x(), position.y - bounds.min_y()),
                cell: bounds.size,
            }
        });
        if presses.press.is_some() {
            cx.capture_pointer();
        }
        // the table selects the row and the row takes the focus
        Propagation::Continue
    }

    fn moved(&self, position: Point, cx: &mut EventCx<'_, Message>) -> Propagation {
        let mut presses = self.presses.borrow_mut();
        if let Some(blank) = presses.blank.as_mut().filter(|b| !b.moved) {
            if (position.x - blank.at.x).hypot(position.y - blank.at.y) < drag::START_DISTANCE {
                return Propagation::Stop;
            }
            // the frame around the table takes the pointer and draws the band
            blank.moved = true;
            presses.clicks.cancel();
            cx.release_pointer();
            return Propagation::Continue;
        }
        let Some(press) = presses.press.as_ref().filter(|p| p.index == self.index) else {
            return Propagation::Continue;
        };
        let moved = (position.x - press.at.x).hypot(position.y - press.at.y);
        if moved < drag::START_DISTANCE {
            return Propagation::Stop;
        }
        let press = presses.press.take().expect("held above");
        presses.clicks.cancel();
        drop(presses);
        cx.release_pointer();
        if let Some(selection) = press.restore {
            self.emit(WindowEvent::Select(selection), cx);
        }
        let grab = RowGrab {
            indices: press.dragged,
            offset: press.offset,
            cell: press.cell,
            rtl: cx.env().direction == LayoutDirection::Rtl,
        };
        self.emit(WindowEvent::DragRows(grab), cx);
        Propagation::Stop
    }
}

impl Widget<Message> for RowCell {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) => self.press(position, mods, cx),
            // the context menu opens from what holds the list; it learns which row was pressed first
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                self.presses.borrow_mut().clicks.cancel();
                self.emit(WindowEvent::SecondaryPress(Some(self.index)), cx);
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => self.moved(position, cx),
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) => {
                let mut presses = self.presses.borrow_mut();
                if presses.blank.as_ref().is_some_and(|b| !b.moved) {
                    presses.blank = None;
                    cx.release_pointer();
                    presses.clicks.arm(cx);
                    return Propagation::Stop;
                }
                if presses
                    .press
                    .as_ref()
                    .is_some_and(|p| p.index == self.index)
                {
                    presses.press = None;
                    cx.release_pointer();
                    // a rename that came due while the button was held starts now
                    presses.clicks.arm(cx);
                    return Propagation::Stop;
                }
                Propagation::Continue
            }
            Event::Wake(now) => {
                let mut presses = self.presses.borrow_mut();
                let held = presses.press.is_some();
                presses.clicks.wake(self.window, now, held, cx);
                Propagation::Continue
            }
            Event::Cancel => {
                let mut presses = self.presses.borrow_mut();
                presses.press = None;
                presses.blank = None;
                presses.clicks.cancel();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The list's table with what no row takes: presses under the rows, the band and the timers.
pub(super) struct Gestures {
    pub window: u64,
    pub count: usize,
    pub row_height: f64,
    pub scroll: ScrollState,
    pub selection: Selection,
    pub presses: SharedPresses,
    /// The band being drawn, in the rows' content.
    pub band: Option<Rect>,
    /// From the frame's top to the first row's at rest: the header, and the toolbar row over it when the list fills
    /// the pane under the row.
    pub lead: f64,
    pub children: Vec<Element<Message>>,
}

impl Gestures {
    fn emit(&self, event: WindowEvent, cx: &mut EventCx<'_, Message>) {
        cx.emit(Message::Window(self.window, event));
    }

    /// Where the rows' content starts in the window: under the header (and the row over it), moved up by the scroll.
    fn content_origin(&self, cx: &EventCx<'_, Message>) -> Option<Point> {
        let bounds = cx.bounds()?;
        Some(Point::new(
            bounds.min_x(),
            bounds.min_y() + self.lead - self.scroll.offset.y,
        ))
    }

    /// The rows' part of the frame, in the window.
    fn shown(&self, cx: &EventCx<'_, Message>) -> Option<Rect> {
        let bounds = cx.bounds()?;
        Some(Rect::new(
            bounds.min_x(),
            bounds.min_y() + self.lead,
            bounds.size.width,
            (bounds.size.height - self.lead).max(0.0),
        ))
    }

    /// Show `band` and select the rows it crosses: the window hears of the band before its selection, so it holds the
    /// selection's count until the band is let go.
    fn draw_band(&self, band: &Band, cx: &mut EventCx<'_, Message>) {
        let Some(origin) = self.content_origin(cx) else {
            return;
        };
        let rect = band.rect(origin);
        let first = (rect.min_y() / self.row_height).floor().max(0.0) as usize;
        let past = ((rect.max_y() / self.row_height).ceil().max(0.0) as usize).min(self.count);
        let hits: Vec<usize> = (first..past).collect();
        let next = band.select(&hits);
        self.emit(WindowEvent::Band(Some(rect)), cx);
        if next != self.selection {
            self.emit(WindowEvent::Select(next), cx);
        }
    }

    /// What the list sees before the table and its rows do, which take keys and presses for themselves: a key, a scroll
    /// or a press anywhere (a triangle's too) cancels a pending rename, and Ctrl with a press on the rows is the
    /// secondary click wherever it lands in a row, its triangle and margins included.
    fn capture(&self, event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        match *event {
            Event::Key(ref key, _) if key.pressed => self.presses.borrow_mut().clicks.cancel(),
            Event::Pointer(PointerEvent::Scroll { .. }, _) => {
                self.presses.borrow_mut().clicks.cancel()
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) => {
                // a press on a row's name arms the rename again as it lands; a double click is still told
                self.presses.borrow_mut().clicks.cancel();
                if mods.contains(Mods::CTRL) && self.shown(cx).is_some_and(|s| s.contains(position))
                {
                    let row = self.content_origin(cx).and_then(|origin| {
                        let row = ((position.y - origin.y) / self.row_height).floor();
                        (row >= 0.0 && (row as usize) < self.count).then_some(row as usize)
                    });
                    self.emit(WindowEvent::SecondaryPress(row), cx);
                    self.emit(WindowEvent::ContextMenu(position), cx);
                    return Propagation::Stop;
                }
            }
            _ => {}
        }
        Propagation::Continue
    }

    fn follow(&self, pointer: Point, cx: &mut EventCx<'_, Message>) {
        if let Some(shown) = self.shown(cx) {
            band::follow(self.window, self.scroll, pointer, shown, cx);
        }
    }
}

impl Widget<Message> for Gestures {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        let mut children = std::mem::take(&mut self.children);
        if let Some(rect) = self.band {
            // the band moves with the rows, under the header
            let top = self.lead - self.scroll.offset.y;
            let shown = Rect::new(
                rect.min_x(),
                rect.min_y() + top,
                rect.width(),
                rect.height(),
            );
            let clipped_top = shown.min_y().max(self.lead);
            let clipped = Rect::new(
                shown.min_x(),
                clipped_top,
                shown.width(),
                (shown.max_y() - clipped_top).max(0.0),
            );
            children.push(
                Element::new(BandLayer {
                    children: vec![band::view(clipped)],
                })
                .key("band-layer"),
            );
        }
        children
    }

    fn state(&self) {}

    // it paints nothing, and its children and its spec tell the tree their own changes
    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, Message>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return self.capture(event, cx);
        }
        match *event {
            // what reaches here is under the rows: no row took it
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) => {
                if !self.shown(cx).is_some_and(|s| s.contains(position)) {
                    return Propagation::Continue;
                }
                if mods.contains(Mods::CTRL) {
                    self.emit(WindowEvent::SecondaryPress(None), cx);
                    self.emit(WindowEvent::ContextMenu(position), cx);
                    return Propagation::Stop;
                }
                let Some(origin) = self.content_origin(cx) else {
                    return Propagation::Continue;
                };
                let mut presses = self.presses.borrow_mut();
                presses.clicks.press_empty();
                let toggles = mods.contains(Mods::SHIFT) || mods.contains(Mods::PRIMARY);
                if !toggles && !self.selection.is_empty() {
                    self.emit(WindowEvent::Select(Selection::new([], None, None)), cx);
                }
                let base = if toggles {
                    self.selection.clone()
                } else {
                    Selection::new([], None, None)
                };
                let start = Point::new(position.x - origin.x, position.y - origin.y);
                presses.band = Some(Band::start(start, position, mods, &base));
                cx.capture_pointer();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let band = {
                    let mut presses = self.presses.borrow_mut();
                    if presses.band.is_none() {
                        // a press on a row's blank part that moved: the band starts where it was
                        let blank = presses.blank.take_if(|b| b.moved);
                        let origin = self.content_origin(cx);
                        if let (Some(blank), Some(origin)) = (blank, origin) {
                            let start = Point::new(blank.at.x - origin.x, blank.at.y - origin.y);
                            presses.band =
                                Some(Band::start(start, position, blank.mods, &blank.selection));
                            cx.capture_pointer();
                        }
                    }
                    let Some(band) = presses.band.as_mut() else {
                        return Propagation::Continue;
                    };
                    band.pointer = position;
                    band.clone()
                };
                self.draw_band(&band, cx);
                self.follow(position, cx);
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) => {
                if self.presses.borrow_mut().band.take().is_none() {
                    return Propagation::Continue;
                }
                cx.release_pointer();
                self.emit(WindowEvent::Band(None), cx);
                Propagation::Stop
            }
            Event::Wake(now) => {
                {
                    let mut presses = self.presses.borrow_mut();
                    let held = presses.press.is_some();
                    presses.clicks.wake(self.window, now, held, cx);
                }
                let band = self.presses.borrow().band.clone();
                if let Some(band) = band {
                    // the rows moved under a still pointer: the band follows, and the scroll goes on
                    self.draw_band(&band, cx);
                    self.follow(band.pointer, cx);
                }
                Propagation::Continue
            }
            Event::Cancel => {
                let mut presses = self.presses.borrow_mut();
                presses.clicks.cancel();
                presses.blank = None;
                if presses.band.take().is_some() {
                    self.emit(WindowEvent::Band(None), cx);
                }
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// What Tab (or Shift+Tab) on a row of the list of window `window` does: it selects the next (or previous) item by
/// `names` and gives it the keyboard ([`WindowEvent::FocusRow`]); past the last (or before the first) it leaves the
/// key to go on to the next control. The table offers only its rows' keys, so a column header's Tab never comes here.
pub(super) fn tab(
    window: u64,
    names: Rc<[String]>,
    cursor: Option<usize>,
) -> impl Fn(usize, &KeyEvent) -> Option<Message> {
    move |at, key| {
        let mods = key.mods;
        let plain = !mods.contains(Mods::CTRL)
            && !mods.contains(Mods::ALT)
            && !mods.contains(Mods::PRIMARY);
        if !(plain && (key.keysym == TAB || key.keysym == LEFT_TAB)) {
            return None;
        }
        let back = key.keysym == LEFT_TAB || mods.contains(Mods::SHIFT);
        let to = tab_to(&names, cursor, at, back)?;
        Some(Message::Window(window, WindowEvent::FocusRow(to)))
    }
}

/// The item Tab (or Shift+Tab, `back`) goes to by `names` from the row the keyboard is on, `at`: the one after (or
/// before) the selection's `cursor`; with nothing selected, the first (or last) by name, or the one after (or before)
/// it when the keyboard is on it already. `None` past the last (or before the first).
fn tab_to(names: &[String], cursor: Option<usize>, at: usize, back: bool) -> Option<usize> {
    let to = by_name(names, cursor, back)?;
    let to = match cursor {
        None if to == at => by_name(names, Some(at), back)?,
        _ => to,
    };
    (to != cursor.unwrap_or(at)).then_some(to)
}

/// The band's layer over the table: it places the band, and lets every press through.
struct BandLayer {
    children: Vec<Element<Message>>,
}

impl Widget<Message> for BandLayer {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Absolute)
    }

    fn take_children(&mut self) -> Vec<Element<Message>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    // it paints nothing, and its children and its spec tell the tree their own changes
    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, Message>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_press_selects_as_the_table_does() {
        let selection = Selection::new(std::iter::once(2..4), Some(3), Some(2));
        let picked = |s: Selection| s.ranges().iter().flat_map(Clone::clone).collect::<Vec<_>>();
        assert_eq!(picked(pressed(&selection, 6, Mods::NONE)), [6]);
        assert_eq!(picked(pressed(&selection, 5, Mods::SHIFT)), [2, 3, 4, 5]);
        assert_eq!(picked(pressed(&selection, 3, Mods::PRIMARY)), [2]);
    }

    #[test]
    fn tab_goes_on_by_name_from_the_selection_or_the_row_the_keyboard_is_on() {
        // shown by size: b.txt, a.txt, Notes; by name: a.txt, b.txt, Notes
        let names: Vec<String> = ["b.txt", "a.txt", "Notes"].map(str::to_owned).to_vec();
        assert_eq!(tab_to(&names, Some(1), 1, false), Some(0), "a.txt to b.txt");
        assert_eq!(
            tab_to(&names, Some(0), 0, true),
            Some(1),
            "b.txt back to a.txt"
        );
        assert_eq!(tab_to(&names, Some(2), 2, false), None, "past the last");
        assert_eq!(tab_to(&names, Some(1), 1, true), None, "before the first");
        // nothing selected: the first by name, unless the keyboard is on it already
        assert_eq!(tab_to(&names, None, 0, false), Some(1));
        assert_eq!(tab_to(&names, None, 1, false), Some(0));
        assert_eq!(tab_to(&names, None, 2, true), Some(0));
        let one = vec!["a.txt".to_owned()];
        assert_eq!(tab_to(&one, None, 0, false), None, "the only item is left");
    }
}
