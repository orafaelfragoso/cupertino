use crate::support;

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_controls::{Grid, Image};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_layout::Track;
use support::{window, Harness};

struct Cells;

fn cell(key: &'static str) -> Element<()> {
    Element::new(Image {
        sources: Vec::new(),
        size: Size::new(20.0, 20.0),
        fit: Default::default(),
        radius: 0.0,
        label: None,
    })
    .key(key)
}

impl Application for Cells {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        window(Element::new(Grid {
            columns: vec![Track::Fixed(50.0), Track::Fraction(1.0)],
            rows: vec![Track::Fixed(30.0)],
            padding: Insets::uniform(5.0),
            spacing: Size::new(10.0, 4.0),
            children: vec![cell("a"), cell("b"), cell("wide").span(2, 1)],
        }))
    }
}

#[test]
fn cells_flow_row_by_row_through_their_tracks_and_a_span_covers_several() {
    let h = Harness::<Cells>::new();
    assert_eq!(h.rect("a"), Rect::new(5.0, 5.0, 50.0, 30.0));
    assert_eq!(h.rect("b"), Rect::new(65.0, 5.0, 130.0, 30.0));
    assert_eq!(
        h.rect("wide"),
        Rect::new(5.0, 39.0, 190.0, 20.0),
        "an extra row fits its content"
    );
}
