//! A table that fills a pane under a toolbar row: its header sits below the row and lies over the rows with it, the
//! rows start below the header and scroll under both, and the header keeps its clicks. Over a raised layer the rows
//! under the row and the header sit on a solid band, so the header's titles stay legible.

mod support;

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_controls::scroll_edge::SOFT_EDGE_STOPS;
use cupertino_ui_controls::{
    unobscured_insets, Column, ColumnWidth, NavigationStack, Selection, SortOrder, TableView, Text,
    TextRole,
};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::ScrollState;
use cupertino_ui_platform::{DecorationMode, PointerEvent, ScrollSource};
use cupertino_ui_render::Paint;
use cupertino_ui_text::{AttributedText, TextStyle};
use support::{window, Harness};

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Scroll(ScrollState),
    Sort((usize, SortOrder)),
}

struct Model {
    scroll: ScrollState,
    sorted: Vec<(usize, SortOrder)>,
}

struct Under;

const ROW: f64 = 20.0;

impl Application for Under {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let model = Model {
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            sorted: Vec::new(),
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Scroll(scroll) => model.scroll = scroll,
            Message::Sort(sort) => model.sorted.push(sort),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let column = |title: &str, width| Column {
            title: title.into(),
            width,
        };
        let table = Element::from(TableView {
            label: "Files".into(),
            columns: vec![
                column("Name", ColumnWidth::Fill),
                column("Size", ColumnWidth::Fixed(60.0)),
            ],
            sort: Some((0, SortOrder::Ascending)),
            on_sort: Some(Box::new(Message::Sort)),
            count: 30,
            row_height: ROW,
            scroll: model.scroll,
            selection: Selection::default(),
            multiple: false,
            cell: Box::new(|row, column, selected| {
                Element::new(Text {
                    content: AttributedText::plain(format!("{row}.{column}"), TextStyle::new(13.0)),
                    role: TextRole::Body,
                    wrap: false,
                    secondary: false,
                    selected,
                })
            }),
            type_select: None,
            on_scroll: Box::new(Message::Scroll),
            on_select: None,
            on_activate: None,
            on_row_key: None,
            header_top: unobscured_insets(DecorationMode::Unified).top,
        });
        window(Element::from(NavigationStack {
            content: table,
            toolbar: Vec::new(),
            can_go_back: false,
            can_go_forward: false,
            on_navigate: None,
            under_toolbar: true,
        }))
    }
}

/// The bounds of the accessible node of `role` named `name`.
fn bounds(h: &Harness<Under>, role: Role, name: &str) -> Rect {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("{role:?} {name}"))
        .1
        .bounds
}

/// Where the text `name` of a cell is laid out: under the header, its accessible bounds are cut to what shows.
fn cell(h: &Harness<Under>, name: &str) -> Rect {
    let snapshot = h.snapshot();
    let id = snapshot
        .accessibility
        .iter()
        .find(|(_, n)| n.name.as_deref() == Some(name) && n.role != Role::ColumnHeader)
        .unwrap_or_else(|| panic!("the cell {name}"))
        .0;
    snapshot.nodes.iter().find(|n| n.id == id).unwrap().rect
}

fn scrolled(h: &mut Harness<Under>, dy: f64) {
    h.pointer(PointerEvent::Scroll {
        position: Point::new(200.0, 200.0),
        delta: Point::new(0.0, dy),
        value120: None,
        source: ScrollSource::Wheel,
        stop: false,
        inverted: (false, false),
    });
}

fn opened(layer: bool) -> Harness<Under> {
    let mut h = if layer {
        Harness::<Under>::with_layer(support::theme())
    } else {
        Harness::<Under>::with_effects(support::theme())
    };
    h.resize(Size::new(400.0, 300.0));
    h
}

#[test]
fn the_header_sits_below_the_row_and_the_rows_below_it_scroll_under_both() {
    for layer in [true, false] {
        let mut h = opened(layer);
        let name = bounds(&h, Role::ColumnHeader, "Name");
        assert_eq!(name.min_y(), 52.0, "below the 52 pt row");
        assert!(name.max_y() <= 80.0);
        // the first row's text, in the first row below the header
        let first = cell(&h, "0.0").min_y();
        assert!((80.0..80.0 + ROW).contains(&first), "{first}");

        scrolled(&mut h, 30.0);
        assert_eq!(h.model().scroll.offset.y, 30.0);
        assert_eq!(
            cell(&h, "0.0").min_y(),
            first - 30.0,
            "the first row under the header"
        );
        assert_eq!(
            bounds(&h, Role::ColumnHeader, "Name").min_y(),
            52.0,
            "the header stays"
        );
        // to assistive technology, the rows' scroll view shows them below the header
        let view = h
            .snapshot()
            .accessibility
            .into_iter()
            .find(|(_, n)| n.role == Role::ScrollView)
            .unwrap()
            .1
            .bounds;
        assert_eq!(view, Rect::new(0.0, 80.0, 400.0, 220.0));

        // the header, not the row under it, takes a click on it
        h.click(Point::new(40.0, 66.0));
        assert_eq!(
            h.model().sorted,
            [(0, SortOrder::Descending)],
            "layer {layer}: the click sorted"
        );
        if layer {
            let system = h.rt.window_system();
            assert!(
                matches!(system.layers(h.window).unwrap().last(), Some(Some(l)) if (80..=81).contains(&l.height)),
                "the row and the header draw in the layer: {:?}",
                system
                    .layers(h.window)
                    .unwrap()
                    .iter()
                    .map(|l| l.as_ref().map(|l| l.height))
                    .collect::<Vec<_>>()
            );
        } else {
            // in place, the rows are cut below the header: none covers it
            let row = h.rt.find(h.window, &"0".into());
            let drawn = h
                .snapshot()
                .display_list
                .nodes
                .into_iter()
                .filter(|n| Some(n.id) == row.map(Into::into))
                .map(|n| n.bounds)
                .next();
            assert!(
                drawn.is_none_or(|b| b.is_empty() || b.min_y() >= 79.0),
                "{drawn:?}"
            );
        }
    }
}

/// The grey level of the window's last frame, with its raised layer over it, at `(x, y)`.
fn level(h: &Harness<Under>, x: usize, y: usize) -> u8 {
    let system = h.rt.window_system();
    let width = system.frames(h.window).unwrap().last().unwrap().width as usize;
    let pixels = system.composited(h.window).unwrap().expect("a frame");
    pixels[(y * width + x) * 4 + 1]
}

#[test]
fn over_a_raised_layer_the_rows_under_the_header_sit_on_a_solid_band() {
    let mut h = opened(true);
    scrolled(&mut h, 30.0);
    // no soft veil: it would leave the rows legible under the header's titles
    assert!(!h.snapshot().display_list.nodes.iter().any(|n| n
        .ops
        .iter()
        .any(|op| matches!(op, cupertino_ui_render::PaintOp::Fill { paint: Paint::Linear { stops, .. }, .. } if stops.len() == SOFT_EDGE_STOPS))));
    let background = support::color(
        cupertino_ui_theme::ColorRole::WindowBackground,
        cupertino_ui_theme::InteractionVariant::Normal,
        false,
    );
    let bg = (background.g * 255.0).round() as u8;
    // the rows scrolled 30 pt: the first two now lie under the header, the first's text at the header's titles
    let first = cell(&h, "0.0");
    assert!(first.max_y() > 52.0 && first.min_y() < 80.0, "{first:?}");
    // over its text, above the header's titles, only the band shows
    for y in 53..=57 {
        for x in first.min_x().floor() as usize..=first.max_x().ceil() as usize {
            assert_eq!(level(&h, x, y), bg, "the band hides the row at {x},{y}");
        }
    }
    for y in [4, 30, 60, 70] {
        assert_eq!(level(&h, 200, y), bg, "solid at {y}");
    }
    // below the header, the rows show
    let below = cell(&h, "3.0");
    assert!(below.min_y() >= 80.0, "{below:?}");
}
