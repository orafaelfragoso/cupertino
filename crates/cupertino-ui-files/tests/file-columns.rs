//! The column view as a program uses it: a row of another folder's column picked, a step from the column of the
//! folder shown, and a column resized by its divider, each reported through the program's function.

use crate::support;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use cupertino_geometry::{Point, Size};
use cupertino_ui_controls::{Selection, Text, TextRole};
use cupertino_ui_core::layout::ScrollState;
use cupertino_ui_core::platform::{Mods, MouseButton};
use cupertino_ui_core::{Command, Element, Role, Window};
use cupertino_ui_files::columns::{FileColumn, FileColumns};
use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::list::Presses;
use cupertino_ui_files::{ColumnEvent, ViewEvent};
use cupertino_ui_text::{AttributedText, TextStyle};

use support::{View, LEFT, RIGHT};

/// The window's size.
const SIZE: Size = Size::new(420.0, 200.0);
/// The folder before the one shown, its items, and the folder shown, its items.
const PARENT: &str = "/home";
const PARENT_ITEMS: [&str; 3] = ["docs", "music", "zebra"];
const SHOWN: &str = "/home/music";
const SHOWN_ITEMS: [&str; 2] = ["one", "two"];
/// Each column's width, and the preview's at the end.
const COLUMN: f64 = 150.0;
const PREVIEW: f64 = 100.0;

fn entries(folder: &str, names: &[&str]) -> Rc<[Entry]> {
    names
        .iter()
        .map(|name| {
            let mut entry = Entry::synthetic(name, true, 1);
            entry.path = Path::new(folder).join(name);
            entry
        })
        .collect()
}

fn no_scroll() -> ScrollState {
    ScrollState {
        offset: Point::ZERO,
        content: Size::ZERO,
        viewport: Size::ZERO,
    }
}

struct Model {
    selection: Selection,
    presses: Presses,
    scroll: ScrollState,
    scrolls: HashMap<PathBuf, ScrollState>,
    events: Vec<ViewEvent>,
}

struct Columns;

impl cupertino_ui_core::Application for Columns {
    type Model = Model;
    type Message = ViewEvent;

    fn init() -> (Model, Vec<Command<ViewEvent>>) {
        let model = Model {
            selection: Selection::default(),
            presses: Presses::default(),
            scroll: no_scroll(),
            scrolls: HashMap::new(),
            events: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, event: ViewEvent) -> Vec<Command<ViewEvent>> {
        // the scroll views report their scroll as they lay out: kept, and not among the events a test looks at
        match event {
            ViewEvent::Scroll(scroll) => model.scroll = scroll,
            ViewEvent::Column(ColumnEvent::Scroll(folder, scroll)) => {
                model.scrolls.insert(folder, scroll);
            }
            ViewEvent::Select(selection) => {
                model.selection = selection.clone();
                model.events.push(ViewEvent::Select(selection));
            }
            event => model.events.push(event),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<ViewEvent>> {
        let scroll = |folder: &str| {
            model
                .scrolls
                .get(Path::new(folder))
                .copied()
                .unwrap_or_else(no_scroll)
        };
        let preview = Element::new(Text {
            content: AttributedText::plain("preview", TextStyle::new(13.0)),
            role: TextRole::Body,
            wrap: false,
            secondary: false,
            selected: false,
        });
        let view = FileColumns {
            window: 1,
            columns: vec![
                (
                    FileColumn::Other {
                        folder: PARENT.into(),
                        label: "home".into(),
                        items: entries(PARENT, &PARENT_ITEMS),
                        scroll: scroll(PARENT),
                        highlighted: Some(1),
                    },
                    COLUMN,
                ),
                (
                    FileColumn::Shown {
                        folder: SHOWN.into(),
                        label: "music".into(),
                        items: entries(SHOWN, &SHOWN_ITEMS),
                        scroll: scroll(SHOWN),
                        selection: model.selection.clone(),
                        name_field: None,
                        presses: model.presses.clone(),
                    },
                    COLUMN,
                ),
                (FileColumn::Preview(preview), PREVIEW),
            ],
            content: 2.0 * COLUMN + PREVIEW,
            scroll: model.scroll,
            lead: 0.0,
            icon: Rc::new(|_| Vec::new()),
            on_event: Rc::new(Some),
        };
        vec![support::window(SIZE, Element::from(view))]
    }
}

/// A point on the name of the item named `name`.
fn name_of(view: &View<Columns>, name: &str) -> Point {
    let text = view
        .bounds_of(Some(Role::StaticText), name)
        .expect("the item is shown");
    Point::new(text.min_x() + 4.0, text.center().y)
}

#[test]
fn a_row_of_another_column_is_picked() {
    let pick = |index, secondary| {
        ViewEvent::Column(ColumnEvent::Pick {
            folder: PARENT.into(),
            index,
            secondary,
        })
    };
    // a click picks the row; a secondary press picks it first, and Ctrl with a press opens the menu after
    let mut view = View::<Columns>::open(SIZE);
    let at = name_of(&view, "zebra");
    view.click(at, Mods::NONE);
    assert_eq!(view.model().events, vec![pick(2, false)]);

    let mut view = View::<Columns>::open(SIZE);
    let at = name_of(&view, "docs");
    view.press(at, MouseButton::Right, Mods::NONE);
    assert_eq!(view.model().events.first(), Some(&pick(0, true)));

    let mut view = View::<Columns>::open(SIZE);
    let at = name_of(&view, "docs");
    view.press(at, MouseButton::Left, Mods::CTRL);
    assert_eq!(
        view.model().events,
        vec![pick(0, true), ViewEvent::ContextMenu(at)]
    );
}

#[test]
fn right_and_left_in_the_column_shown_step_along_the_chain() {
    let rows = [(RIGHT, 1), (LEFT, -1)];
    for (key, by) in rows {
        let mut view = View::<Columns>::open(SIZE);
        let at = name_of(&view, "one");
        view.click(at, Mods::NONE);
        view.key(key, None, Mods::NONE);
        assert_eq!(
            view.model().events.last(),
            Some(&ViewEvent::Column(ColumnEvent::Step(by))),
            "{key:?}"
        );
    }
}

#[test]
fn a_dragged_divider_resizes_its_column_until_it_is_let_go() {
    // without and with Option held: every column with it
    for (mods, all) in [(Mods::NONE, false), (Mods::ALT, true)] {
        let mut view = View::<Columns>::open(SIZE);
        let y = name_of(&view, "music").y;
        let edge = Point::new(COLUMN - 1.0, y);
        view.press(edge, MouseButton::Left, mods);
        let to = Point::new(edge.x + 30.0, y);
        view.move_to(to);
        assert_eq!(
            view.model().events.last(),
            Some(&ViewEvent::Column(ColumnEvent::Resize {
                index: 0,
                width: COLUMN + 30.0,
                all,
            })),
            "{mods:?}"
        );
        view.release(to, MouseButton::Left);
        assert_eq!(
            view.model().events.last(),
            Some(&ViewEvent::Column(ColumnEvent::Resized)),
            "{mods:?}"
        );
    }
}
