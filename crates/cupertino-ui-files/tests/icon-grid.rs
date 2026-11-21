//! The icon grid as a program uses it: what a press, a key, the band and typing select, what opens an item, renames
//! it, drags it and asks for its menu, each reported through the program's function.

use crate::support;

use std::cell::Cell;
use std::rc::Rc;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::Selection;
use cupertino_ui_core::layout::ScrollState;
use cupertino_ui_core::platform::{Mods, MouseButton};
use cupertino_ui_core::{Command, Element, Window};
use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::grid::{self, IconGrid, NameShape};
use cupertino_ui_files::layout::icon::{
    self, Flow, FontMetrics, IconLayout, IconParams, SectionSpec,
};
use cupertino_ui_files::ViewEvent;

use support::{View, DOWN, END, HOME, RETURN, RIGHT, TAB};

/// The window's size.
const SIZE: Size = Size::new(420.0, 320.0);
/// The items in the grid's order, which is not the order of their names.
const NAMES: [&str; 6] = ["delta", "alpha", "charlie", "bravo", "echo", "foxtrot"];

thread_local! {
    /// Whether the program has a message for a selection, for the grid the next application on the thread starts.
    static TAKES_SELECT: Cell<bool> = const { Cell::new(true) };
}

fn params() -> IconParams {
    IconParams::new(64, 54, 12, true, false, FontMetrics::estimate(12.0))
}

fn layout() -> IconLayout {
    icon::layout(
        &params(),
        SIZE.width,
        &[SectionSpec {
            title: None,
            count: NAMES.len(),
        }],
        Flow::Leading,
    )
}

struct Model {
    layout: Rc<IconLayout>,
    items: Rc<[Entry]>,
    selection: Selection,
    band: Option<Rect>,
    takes_select: bool,
    events: Vec<ViewEvent>,
}

struct Grid;

impl cupertino_ui_core::Application for Grid {
    type Model = Model;
    type Message = ViewEvent;

    fn init() -> (Model, Vec<Command<ViewEvent>>) {
        let items: Vec<Entry> = NAMES
            .iter()
            .map(|name| Entry::synthetic(name, false, 1))
            .collect();
        let model = Model {
            layout: Rc::new(layout()),
            items: items.into(),
            selection: Selection::default(),
            band: None,
            takes_select: TAKES_SELECT.with(Cell::get),
            events: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, event: ViewEvent) -> Vec<Command<ViewEvent>> {
        match &event {
            ViewEvent::Select(selection) => model.selection = selection.clone(),
            ViewEvent::Band(band) => model.band = *band,
            _ => {}
        }
        model.events.push(event);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<ViewEvent>> {
        let params = params();
        let items = Rc::clone(&model.items);
        let takes_select = model.takes_select;
        let grid = IconGrid {
            window: 1,
            label: "Items".into(),
            layout: Rc::clone(&model.layout),
            params,
            items: Rc::clone(&model.items),
            icon: Rc::new(|_| Vec::new()),
            scroll: ScrollState {
                offset: Point::ZERO,
                content: model.layout.content,
                viewport: SIZE,
            },
            selection: model.selection.clone(),
            ringed: Selection::default(),
            band: model.band,
            lead: 0.0,
            name: Rc::new(move |index, selected| {
                grid::name(
                    items[index].name.clone(),
                    params.text_size,
                    selected,
                    Some(2),
                )
            }),
            names: Some(NameShape {
                text_size: params.text_size,
                max_lines: 2,
            }),
            revision: 0,
            strip: false,
            on_event: Rc::new(move |event| match event {
                ViewEvent::Select(_) if !takes_select => None,
                event => Some(event),
            }),
        };
        vec![support::window(SIZE, Element::new(grid))]
    }
}

/// The grid in its window, the program taking selections or not.
fn open(takes_select: bool) -> View<Grid> {
    TAKES_SELECT.with(|t| t.set(takes_select));
    View::open(SIZE)
}

/// The middle of item `index`'s icon, and of its name.
fn icon_of(index: usize) -> Point {
    layout().cells[index].icon_frame.center()
}

fn name_of(index: usize) -> Point {
    layout().cells[index].label_frame.center()
}

/// The items `selection` holds, in order.
fn selected(selection: &Selection) -> Vec<usize> {
    selection.ranges().iter().flat_map(Clone::clone).collect()
}

fn index_of(name: &str) -> usize {
    NAMES.iter().position(|n| *n == name).unwrap()
}

#[test]
fn a_press_selects_as_its_modifiers_say() {
    // a press on an item, then a press with the modifiers on another: the selection after the second
    let rows: [(Mods, Vec<usize>); 3] = [
        (Mods::NONE, vec![3]),
        (Mods::SHIFT, vec![1, 2, 3]),
        (Mods::PRIMARY, vec![1, 3]),
    ];
    for (mods, expected) in rows {
        let mut view = open(true);
        view.click(icon_of(1), Mods::NONE);
        view.wait(1.0);
        view.click(icon_of(3), mods);
        assert_eq!(selected(&view.model().selection), expected, "{mods:?}");
    }
    // a press on empty space clears the selection
    let mut view = open(true);
    view.click(icon_of(1), Mods::NONE);
    view.wait(1.0);
    view.click(Point::new(SIZE.width - 4.0, SIZE.height - 4.0), Mods::NONE);
    assert!(view.model().selection.is_empty());
}

#[test]
fn keys_move_the_selection_through_the_layout() {
    let columns = layout().columns;
    let rows = [
        (RIGHT, Mods::NONE, vec![1]),
        (DOWN, Mods::NONE, vec![columns]),
        (END, Mods::NONE, vec![NAMES.len() - 1]),
        (HOME, Mods::NONE, vec![0]),
        (RIGHT, Mods::SHIFT, vec![0, 1]),
    ];
    for (key, mods, expected) in rows {
        let mut view = open(true);
        view.click(icon_of(0), Mods::NONE);
        view.key(key, None, mods);
        assert_eq!(
            selected(&view.model().selection),
            expected,
            "{key:?} {mods:?}"
        );
    }
    let mut view = open(true);
    view.click(icon_of(0), Mods::NONE);
    view.key(
        cupertino_ui_core::platform::Keysym(0x61),
        None,
        Mods::PRIMARY,
    );
    assert_eq!(
        selected(&view.model().selection),
        (0..NAMES.len()).collect::<Vec<_>>()
    );
}

#[test]
fn the_band_selects_what_it_touches_until_it_is_let_go() {
    let mut view = open(true);
    let from = Point::new(SIZE.width - 4.0, SIZE.height - 4.0);
    let over = icon_of(NAMES.len() - 1);
    view.press(from, MouseButton::Left, Mods::NONE);
    view.move_to(over);
    let band = view.model().band.expect("a band while the press is held");
    assert!(band.contains(over) || band.max_x() >= over.x, "{band:?}");
    assert!(view.model().selection.contains(NAMES.len() - 1));
    view.release(over, MouseButton::Left);
    assert_eq!(view.model().band, None);
    assert!(matches!(
        view.model().events.last(),
        Some(ViewEvent::Band(None))
    ));
}

#[test]
fn typing_selects_by_name() {
    let rows = [("c", "charlie"), ("e", "echo"), ("fo", "foxtrot")];
    for (typed, expected) in rows {
        let mut view = open(true);
        view.click(icon_of(0), Mods::NONE);
        for c in typed.chars() {
            view.key(
                cupertino_ui_core::platform::Keysym(c as u32),
                Some(c),
                Mods::NONE,
            );
        }
        assert_eq!(
            selected(&view.model().selection),
            vec![index_of(expected)],
            "{typed}"
        );
    }
}

#[test]
fn a_double_click_and_return_open_an_item() {
    let mut view = open(true);
    view.click(icon_of(2), Mods::NONE);
    view.click(icon_of(2), Mods::NONE);
    assert!(view.model().events.contains(&ViewEvent::Open(2)));

    let mut view = open(true);
    view.click(icon_of(4), Mods::NONE);
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.model().events.last(), Some(&ViewEvent::Open(4)));
}

#[test]
fn a_press_on_the_only_selected_name_renames_once_the_interval_passes() {
    // after the press on the name: how long nothing else happens, and whether the name is edited then
    let rows = [(0.2, false), (0.6, true)];
    for (waited, renames) in rows {
        let mut view = open(true);
        view.click(icon_of(1), Mods::NONE);
        view.wait(1.0);
        view.click(name_of(1), Mods::NONE);
        view.wait(waited);
        assert_eq!(
            view.model().events.contains(&ViewEvent::Rename(1)),
            renames,
            "after {waited} s"
        );
    }
    // a key before the interval passes cancels it
    let mut view = open(true);
    view.click(icon_of(1), Mods::NONE);
    view.wait(1.0);
    view.click(name_of(1), Mods::NONE);
    view.key(RIGHT, None, Mods::NONE);
    view.wait(1.0);
    assert!(!view.model().events.contains(&ViewEvent::Rename(1)));
}

#[test]
fn tab_selects_the_next_item_by_name_and_passes_on_after_the_last() {
    let rows = [
        ("alpha", Mods::NONE, Some("bravo")),
        ("bravo", Mods::NONE, Some("charlie")),
        ("charlie", Mods::SHIFT, Some("bravo")),
        ("foxtrot", Mods::NONE, None),
    ];
    for (from, mods, to) in rows {
        let mut view = open(true);
        view.click(icon_of(index_of(from)), Mods::NONE);
        view.key(TAB, None, mods);
        let expected = vec![index_of(to.unwrap_or(from))];
        assert_eq!(
            selected(&view.model().selection),
            expected,
            "{from} {mods:?}"
        );
    }
}

#[test]
fn a_press_that_moves_drags_the_selection() {
    let mut view = open(true);
    view.click(icon_of(1), Mods::NONE);
    view.wait(1.0);
    view.click(icon_of(3), Mods::PRIMARY);
    view.wait(1.0);
    let at = icon_of(3);
    view.press(at, MouseButton::Left, Mods::NONE);
    view.move_to(Point::new(at.x + 2.0, at.y));
    assert!(!view
        .model()
        .events
        .iter()
        .any(|e| matches!(e, ViewEvent::DragOut(_))));
    view.move_to(Point::new(at.x + 8.0, at.y));
    let Some(ViewEvent::DragOut(grab)) = view.model().events.last() else {
        panic!("no drag: {:?}", view.model().events);
    };
    assert_eq!(grab.indices, vec![3, 1]);
    assert_eq!(grab.name, "bravo");
}

#[test]
fn a_secondary_press_says_which_item_before_the_menu_opens() {
    let at = icon_of(2);
    let rows: [(MouseButton, Mods, Vec<ViewEvent>); 2] = [
        (
            MouseButton::Right,
            Mods::NONE,
            vec![ViewEvent::SecondaryPress(Some(2))],
        ),
        (
            MouseButton::Left,
            Mods::CTRL,
            vec![
                ViewEvent::SecondaryPress(Some(2)),
                ViewEvent::SecondaryPress(None),
                ViewEvent::ContextMenu(at),
            ],
        ),
    ];
    for (button, mods, expected) in rows {
        let mut view = open(true);
        view.press(at, button, mods);
        assert_eq!(view.model().events, expected, "{button:?} {mods:?}");
    }
}

#[test]
fn an_event_the_program_has_no_message_for_reports_nothing() {
    let mut view = open(false);
    view.click(icon_of(2), Mods::NONE);
    assert!(view.model().events.is_empty());
    view.click(icon_of(2), Mods::NONE);
    assert_eq!(view.model().events, vec![ViewEvent::Open(2)]);
}
