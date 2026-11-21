//! The column view: a chain of folders side by side in a row that scrolls sideways ([`FileColumns`]), each column a
//! list of its folder's items, and what ends the chain, the program's preview of the selected file.
//!
//! The column of the folder the program shows has its selection, rename, drag, context menu and type-to-select as
//! the list's rows have them. The other columns are the folders before it, each with the next folder of the chain
//! highlighted, and the contents of the selected folder after it: choosing a row of one of those columns reports it
//! ([`ColumnEvent::Pick`]). Right on the column of the folder shown, and Left, report a step ([`ColumnEvent::Step`]).
//!
//! A column's trailing divider resizes it; with Option held it resizes every column ([`ColumnEvent::Resize`]).

use std::path::{Path, PathBuf};
use std::rc::Rc;

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_theme::ui::{ColorRole, StateVariant};
use cupertino_ui_controls::{
    Icon, IconDirection, Image, ImageFit, ImageSource, ListView, ScrollView, Scroller, Selection,
    Spacer, Stack, Text, TextRole,
};
use cupertino_ui_core::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    ScrollState, SizeSpec,
};
use cupertino_ui_core::platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent};
use cupertino_ui_core::{
    Change, Element, Event, EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, SemanticsCx,
    Widget,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, TextStyle};

use crate::entry::Entry;
use crate::grid::ItemIcon;
use crate::list::{row_key, Presses, RowCell, NAME_GAP};
use crate::metrics::column_view;
use crate::{ColumnEvent, ViewEvent};

/// The height of a row: the 16 pt icon and the row's padding.
pub const ROW_HEIGHT: f64 = 16.0 + column_view::ROW_EXTRA;
/// The side of a row's icon.
pub const ROW_ICON: f64 = 16.0;
/// From a row's selection to its icon, and from its chevron to the selection's far end.
const ROW_LEAD: f64 = 8.0;
/// The side of the chevron that ends a folder's row.
const CHEVRON: f64 = 9.0;
/// How far from a column's trailing edge a press resizes it.
const GRIP: f64 = 4.0;
/// The width of the line between two columns.
const DIVIDER: f64 = 1.0;
/// From a column's sides to a row's selection, which the highlight of a folder of the chain matches.
const SELECTION_INSET: f64 = 10.0;
/// From a list row's content box to the selection drawn around it: the list's content inset less
/// [`SELECTION_INSET`], none as the two are the same.
const HIGHLIGHT_OUTSET: f64 = 0.0;
/// The highlight's corner radius, the column rows' selection's.
const HIGHLIGHT_RADIUS: f64 = 8.0;
const LEFT: Keysym = Keysym(0xff51);
const RIGHT: Keysym = Keysym(0xff53);

/// What a column's list reports through a handler that must give a message, which a [`FileColumns`]' `on_event`
/// gives for each.
const LIST_EVENTS: &str =
    "a column view's on_event gives a message for Scroll, which its scroll view reports";

/// The program's message for each event a view reports, or none.
type OnEvent<M> = Rc<dyn Fn(ViewEvent) -> Option<M>>;

/// A chain of folders side by side in a row that scrolls sideways, a divider after each column that resizes it. It
/// becomes an element with `Element::from`.
pub struct FileColumns<M> {
    /// Tells the rows' keys from those of the program's other windows.
    pub window: u64,
    /// The columns in order, each as wide as the program lays it out.
    pub columns: Vec<(FileColumn<M>, f64)>,
    /// The width of the chain's content, which is the columns' widths together or more: a chain scrolled to a whole
    /// column keeps room after its last one.
    pub content: f64,
    /// The chain's sideways scroll, reported as `ViewEvent::Scroll`; a column's own is `ColumnEvent::Scroll`.
    pub scroll: ScrollState,
    /// How far down its pane each column's first row starts at rest: what a toolbar row covers.
    pub lead: f64,
    pub icon: ItemIcon,
    /// The program's message for each event the view reports, or none. Its lists report `Scroll`, `Select`, `Open`
    /// and a column's `Scroll` and `Pick` through handlers that must give one, so the function gives a message for
    /// each of those.
    pub on_event: Rc<dyn Fn(ViewEvent) -> Option<M>>,
}

/// A column of a [`FileColumns`].
pub enum FileColumn<M> {
    /// The folder the program shows: its selection, its rename and its presses, as a list's rows have them.
    Shown {
        folder: PathBuf,
        label: String,
        items: Rc<[Entry]>,
        scroll: ScrollState,
        selection: Selection,
        /// The row whose name is being edited, and the field that stands in its place.
        #[allow(clippy::type_complexity)]
        name_field: Option<(usize, Rc<dyn Fn() -> Element<M>>)>,
        presses: Presses,
    },
    /// Another folder of the chain; the row of the chain's next folder is highlighted.
    Other {
        folder: PathBuf,
        label: String,
        items: Rc<[Entry]>,
        scroll: ScrollState,
        highlighted: Option<usize>,
    },
    /// A folder whose items have not come yet.
    Empty { folder: PathBuf },
    /// What ends the chain, built by the program: the preview of the selected file. It has no divider.
    Preview(Element<M>),
}

impl<M: Clone + 'static> From<FileColumns<M>> for Element<M> {
    fn from(view: FileColumns<M>) -> Element<M> {
        let FileColumns {
            window: id,
            columns,
            content,
            scroll,
            lead,
            icon,
            on_event,
        } = view;
        let mut children = Vec::new();
        for (place, (column, width)) in columns.into_iter().enumerate() {
            let (column, folder) = match column {
                FileColumn::Shown {
                    folder,
                    label,
                    items,
                    scroll,
                    selection,
                    name_field,
                    presses,
                } => {
                    let list = active_column(
                        id, &folder, label, items, scroll, selection, name_field, presses, lead,
                        &icon, &on_event,
                    );
                    (list, folder)
                }
                FileColumn::Other {
                    folder,
                    label,
                    items,
                    scroll,
                    highlighted,
                } => {
                    let list = other_column(
                        id,
                        &folder,
                        label,
                        items,
                        scroll,
                        highlighted,
                        lead,
                        &icon,
                        &on_event,
                    );
                    (list, folder)
                }
                FileColumn::Empty { folder } => (Element::new(Spacer { min: 0.0 }), folder),
                FileColumn::Preview(preview) => {
                    // the preview starts below the toolbar row
                    let pane = Element::new(Stack {
                        axis: Axis::Vertical,
                        padding: Insets {
                            top: lead,
                            ..Insets::ZERO
                        },
                        spacing: 0.0,
                        align: Align::Stretch,
                        children: vec![preview.fill()],
                    });
                    children.push(pane.key("preview").width(width));
                    continue;
                }
            };
            children.push(frame(&on_event, place, width, column, &folder));
        }
        let content = Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children,
        })
        .width(content);
        Element::from(ScrollView {
            content,
            state: scroll,
            horizontal: true,
            vertical: false,
            scroller: Scroller::Overlay,
            on_scroll: report(&on_event, ViewEvent::Scroll),
        })
        .key("columns")
    }
}

/// The scroll view's handler that reports its values as `event` through `on_event`.
fn report<T: 'static, M: 'static>(
    on_event: &OnEvent<M>,
    event: impl Fn(T) -> ViewEvent + 'static,
) -> Box<dyn Fn(T) -> M> {
    let on_event = Rc::clone(on_event);
    Box::new(move |value| on_event(event(value)).expect(LIST_EVENTS))
}

/// A list's handler that reports its values as `event` through `on_event`, sending nothing where it gives no
/// message.
fn relay<T: 'static, M: 'static>(
    on_event: &OnEvent<M>,
    event: impl Fn(T) -> ViewEvent + 'static,
) -> Box<dyn Fn(T) -> Option<M>> {
    let on_event = Rc::clone(on_event);
    Box::new(move |value| on_event(event(value)))
}

/// Hand `event` to the program through `on_event`, as its message if it has one for it.
fn emit<M>(on_event: &OnEvent<M>, event: ViewEvent, cx: &mut EventCx<'_, M>) {
    if let Some(message) = on_event(event) {
        cx.emit(message);
    }
}

/// The column at `place` of the chain, `width` wide, with the divider after it that resizes it.
fn frame<M: 'static>(
    on_event: &OnEvent<M>,
    place: usize,
    width: f64,
    column: Element<M>,
    folder: &Path,
) -> Element<M> {
    Element::new(ColumnFrame {
        index: place,
        width,
        children: vec![column.fill(), Element::new(Divider).width(DIVIDER)],
        on_event: Rc::clone(on_event),
    })
    .key(column_key(folder))
    .width(width)
}

/// The key of the column of `folder`.
pub fn column_key(folder: &Path) -> Key {
    Key::from(format!("column/{}", folder.display()))
}

/// The column of the folder the program shows: its items, selection and rename.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn active_column<M: Clone + 'static>(
    id: u64,
    folder: &Path,
    label: String,
    entries: Rc<[Entry]>,
    scroll: ScrollState,
    selection: Selection,
    name_field: Option<(usize, Rc<dyn Fn() -> Element<M>>)>,
    presses: Presses,
    lead: f64,
    icons: &ItemIcon,
    on_event: &OnEvent<M>,
) -> Element<M> {
    let names: Rc<[String]> = entries.iter().map(|e| e.name.clone()).collect();
    let cell_selection = selection.clone();
    let icons = Rc::clone(icons);
    let keys = Rc::clone(&entries);
    let folder = folder.to_path_buf();
    let cell_events = Rc::clone(on_event);
    let list = ListView {
        label,
        count: entries.len(),
        row_height: ROW_HEIGHT,
        scroll,
        selection,
        multiple: true,
        initial: false,
        row: Box::new(move |row, selected| {
            let entry = &entries[row];
            let name = match name_field.as_ref().filter(|r| r.0 == row) {
                Some((_, field)) => field(),
                None => name_text(entry.name.clone(), selected),
            };
            let named = Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: NAME_GAP,
                align: Align::Center,
                children: vec![icon(icons(entry)), name.fill()],
            })
            .fill();
            let cell = Element::new(RowCell {
                index: row,
                name: true,
                label: Some(entry.name.clone()),
                selection: cell_selection.clone(),
                presses: presses.clone(),
                children: vec![named],
                on_event: Rc::clone(&cell_events),
            })
            .fill();
            row_content(cell, entry.is_dir, selected)
        }),
        key: Some(Box::new(move |row| row_key(id, &keys[row].path))),
        type_select: Some(Box::new(move |row| names[row].clone())),
        on_scroll: relay(on_event, move |s| {
            ViewEvent::Column(ColumnEvent::Scroll(folder.clone(), s))
        }),
        on_select: Some(relay(on_event, ViewEvent::Select)),
        double_click_activates: false,
        on_activate: Some(relay(on_event, ViewEvent::Open)),
    };
    Element::new(ColumnKeys {
        children: vec![list
            .rounded_element(SELECTION_INSET, HIGHLIGHT_RADIUS, lead)
            .fill()],
        on_event: Rc::clone(on_event),
    })
}

/// The column of a folder other than the one shown, `entries` its items; row `highlighted`, the folder of the next
/// column, is highlighted.
#[allow(clippy::too_many_arguments)]
fn other_column<M: Clone + 'static>(
    id: u64,
    folder: &Path,
    label: String,
    entries: Rc<[Entry]>,
    scroll: ScrollState,
    highlighted: Option<usize>,
    under: f64,
    icons: &ItemIcon,
    on_event: &OnEvent<M>,
) -> Element<M> {
    let names: Rc<[String]> = entries.iter().map(|e| e.name.clone()).collect();
    let keys = Rc::clone(&entries);
    let icons = Rc::clone(icons);
    let picked = folder.to_path_buf();
    let pressed = folder.to_path_buf();
    let scrolled = folder.to_path_buf();
    let cell_events = Rc::clone(on_event);
    let list = ListView {
        label,
        count: entries.len(),
        row_height: ROW_HEIGHT,
        scroll,
        selection: Selection::new([], highlighted, highlighted),
        multiple: false,
        initial: false,
        row: Box::new(move |row, _| {
            let entry = &entries[row];
            let named = Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets::ZERO,
                spacing: NAME_GAP,
                align: Align::Center,
                children: vec![
                    icon(icons(entry)),
                    name_text(entry.name.clone(), false).fill(),
                ],
            })
            .fill();
            let content = row_content(named, entry.is_dir, false);
            Element::new(PickCell {
                pick: ViewEvent::Column(ColumnEvent::Pick {
                    folder: pressed.clone(),
                    index: row,
                    secondary: true,
                }),
                highlighted: highlighted == Some(row),
                children: vec![content],
                on_event: Rc::clone(&cell_events),
            })
            .fill()
        }),
        key: Some(Box::new(move |row| row_key(id, &keys[row].path))),
        type_select: Some(Box::new(move |row| names[row].clone())),
        on_scroll: relay(on_event, move |s| {
            ViewEvent::Column(ColumnEvent::Scroll(scrolled.clone(), s))
        }),
        on_select: Some(relay(on_event, move |s: Selection| {
            ViewEvent::Column(ColumnEvent::Pick {
                folder: picked.clone(),
                index: s.cursor().unwrap_or(0),
                secondary: false,
            })
        })),
        double_click_activates: false,
        on_activate: None,
    };
    list.rounded_element(SELECTION_INSET, HIGHLIGHT_RADIUS, under)
}

/// A row: its icon and name, then a folder's chevron at the far end.
fn row_content<M: 'static>(named: Element<M>, folder: bool, selected: bool) -> Element<M> {
    let mut children = vec![named];
    if folder {
        children.push(Element::new(Icon {
            name: "symbol.chevron.right".into(),
            size: CHEVRON,
            direction: IconDirection::Semantic,
            label: None,
            tint: Some(if selected {
                ColorRole::SelectedControlText
            } else {
                ColorRole::SecondaryText
            }),
        }));
    }
    Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: 0.0,
            left: ROW_LEAD,
            bottom: 0.0,
            right: ROW_LEAD,
        },
        spacing: NAME_GAP,
        align: Align::Center,
        children,
    })
    .fill()
}

fn icon<M: 'static>(sources: Vec<ImageSource>) -> Element<M> {
    Element::new(Image {
        sources,
        size: Size::new(ROW_ICON, ROW_ICON),
        fit: ImageFit::Contain,
        radius: 0.0,
        label: None,
    })
}

fn name_text<M: 'static>(name: String, selected: bool) -> Element<M> {
    Element::new(Text {
        content: AttributedText::plain(name, TextStyle::new(13.0)),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected,
    })
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn filling(axis: Axis) -> LayoutSpec {
    LayoutSpec {
        width: SizeSpec::Fill,
        height: SizeSpec::Fill,
        flex: Flex::grow(1.0),
        ..LayoutSpec::new(ContainerKind::Stack {
            axis,
            spacing: 0.0,
            align: Align::Stretch,
            justify: Justify::Start,
        })
    }
}

/// Left and Right in the column of the folder shown, which the list leaves alone.
struct ColumnKeys<M> {
    children: Vec<Element<M>>,
    on_event: OnEvent<M>,
}

impl<M: 'static> Widget<M> for ColumnKeys<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling(Axis::Vertical)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let Event::Key(
            KeyEvent {
                keysym,
                mods,
                pressed: true,
                ..
            },
            _,
        ) = event
        else {
            return Propagation::Continue;
        };
        if !mods.is_empty() {
            return Propagation::Continue;
        }
        let (forward, back) = match cx.env().direction {
            LayoutDirection::Ltr => (RIGHT, LEFT),
            LayoutDirection::Rtl => (LEFT, RIGHT),
        };
        let by = match *keysym {
            k if k == forward => 1,
            k if k == back => -1,
            _ => return Propagation::Continue,
        };
        emit(&self.on_event, ViewEvent::Column(ColumnEvent::Step(by)), cx);
        Propagation::Stop
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A column and its divider; a press on the divider, or just before it, drags it to resize the column.
struct ColumnFrame<M> {
    index: usize,
    /// The column's width as shown.
    width: f64,
    children: Vec<Element<M>>,
    on_event: OnEvent<M>,
}

/// A divider being dragged: where the press was, the column's width then, and whether Option was held.
#[derive(Default)]
struct Resizing {
    from: Option<(f64, f64, bool)>,
}

impl<M: 'static> Widget<M> for ColumnFrame<M> {
    type State = Resizing;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            ..filling(Axis::Horizontal)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Resizing {
        Resizing::default()
    }

    fn update(&self, old: &Self, _: &mut Resizing) -> Change {
        if self.width != old.width {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Resizing, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, state: &mut Resizing, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let rtl = cx.env().direction == LayoutDirection::Rtl;
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
            ) if cx.phase() == Phase::Capture => {
                let Some(bounds) = cx.bounds() else {
                    return Propagation::Continue;
                };
                let edge = if rtl { bounds.min_x() } else { bounds.max_x() };
                if (position.x - edge).abs() > GRIP {
                    return Propagation::Continue;
                }
                state.from = Some((position.x, self.width, mods.contains(Mods::ALT)));
                cx.capture_pointer();
                Propagation::Stop
            }
            Event::Pointer(PointerEvent::Motion { position }, _) => {
                let Some((from, width, all)) = state.from else {
                    return Propagation::Continue;
                };
                let moved = if rtl {
                    from - position.x
                } else {
                    position.x - from
                };
                emit(
                    &self.on_event,
                    ViewEvent::Column(ColumnEvent::Resize {
                        index: self.index,
                        width: width + moved,
                        all,
                    }),
                    cx,
                );
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.from.is_some() => {
                state.from = None;
                cx.release_pointer();
                emit(&self.on_event, ViewEvent::Column(ColumnEvent::Resized), cx);
                Propagation::Stop
            }
            Event::Cancel => {
                if state.from.take().is_some() {
                    emit(&self.on_event, ViewEvent::Column(ColumnEvent::Resized), cx);
                }
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &Resizing, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Resizing, _: &mut SemanticsCx<'_>) {}
}

/// The line between two columns.
struct Divider;

impl<M: 'static> Widget<M> for Divider {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(DIVIDER),
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(DIVIDER, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let color = cx
            .env()
            .theme
            .colors
            .get(ColorRole::Separator, StateVariant::NORMAL);
        if let Some(color) = color {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// A row of a column other than the folder shown's: a secondary press on it shows its folder with it selected before
/// the context menu opens, and the row of the next folder of the chain is highlighted.
struct PickCell<M> {
    /// The row chosen by a secondary press.
    pick: ViewEvent,
    highlighted: bool,
    children: Vec<Element<M>>,
    on_event: OnEvent<M>,
}

impl<M: 'static> Widget<M> for PickCell<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        filling(Axis::Horizontal)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.highlighted != old.highlighted {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let secondary = match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => true,
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    mods,
                    ..
                },
                _,
            ) => mods.contains(Mods::CTRL),
            _ => false,
        };
        if secondary {
            emit(&self.on_event, self.pick.clone(), cx);
            if let Event::Pointer(PointerEvent::Button { position, .. }, _) = *event {
                if !matches!(
                    event,
                    Event::Pointer(
                        PointerEvent::Button {
                            button: MouseButton::Right,
                            ..
                        },
                        _
                    )
                ) {
                    // Ctrl with a press is the secondary click
                    emit(&self.on_event, ViewEvent::ContextMenu(position), cx);
                    return Propagation::Stop;
                }
            }
        }
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.highlighted {
            return;
        }
        let size = cx.size();
        let color = cx.env().theme.colors.get(
            ColorRole::SecondarySelectedControlFill,
            StateVariant::NORMAL,
        );
        if let Some(color) = color {
            cx.push(PaintOp::Fill {
                shape: Shape::RoundedRect {
                    rect: Rect::new(
                        -HIGHLIGHT_OUTSET,
                        0.0,
                        size.width + 2.0 * HIGHLIGHT_OUTSET,
                        size.height,
                    ),
                    radius: HIGHLIGHT_RADIUS,
                },
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
