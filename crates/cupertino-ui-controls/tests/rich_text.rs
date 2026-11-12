use crate::support;

use cupertino_geometry::Point;
use cupertino_ui_controls::{RichText, TextLink, TextRole};
use cupertino_ui_core::{
    AccessibilityNode, Action, Application, Command, Element, NodeId, Role, Window,
};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, TextStyle};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, window, Harness, RETURN, SPACE, TAB};

const TEXT: &str = "See the guide or terms.";

struct Links;

impl Application for Links {
    type Model = Vec<&'static str>;
    type Message = &'static str;

    fn init() -> (Self::Model, Vec<Command<Self::Message>>) {
        (Vec::new(), Vec::new())
    }

    fn update(opened: &mut Self::Model, url: Self::Message) -> Vec<Command<Self::Message>> {
        opened.push(url);
        Vec::new()
    }

    fn view(_: &Self::Model) -> Vec<Window<Self::Message>> {
        rich_text(&[("guide", "https://g.test"), ("terms", "https://t.test")])
    }
}

/// [`TEXT`] with a link on each word, sending its URL.
fn rich_text(words: &[(&str, &'static str)]) -> Vec<Window<&'static str>> {
    let links = words
        .iter()
        .map(|&(word, url)| {
            let start = TEXT.find(word).unwrap();
            TextLink {
                range: start..start + word.len(),
                url: url.to_string(),
                on_open: Some(url),
            }
        })
        .collect();
    window(
        Element::new(RichText {
            content: AttributedText::plain(TEXT, TextStyle::new(13.0)),
            role: TextRole::Body,
            wrap: false,
            secondary: false,
            links,
        })
        .key("rich"),
    )
}

/// Two links that trade places once one is opened, so the paragraph's runs stay as they were.
struct Swapping;

impl Application for Swapping {
    type Model = Vec<&'static str>;
    type Message = &'static str;

    fn init() -> (Self::Model, Vec<Command<Self::Message>>) {
        (Vec::new(), Vec::new())
    }

    fn update(opened: &mut Self::Model, url: Self::Message) -> Vec<Command<Self::Message>> {
        opened.push(url);
        Vec::new()
    }

    fn view(opened: &Self::Model) -> Vec<Window<Self::Message>> {
        if opened.is_empty() {
            rich_text(&[("guide", "https://g.test"), ("terms", "https://t.test")])
        } else {
            rich_text(&[("terms", "https://t.test"), ("guide", "https://g.test")])
        }
    }
}

fn links(h: &Harness<Links>) -> Vec<(NodeId, AccessibilityNode)> {
    h.snapshot()
        .accessibility
        .into_iter()
        .filter(|(_, node)| node.role == Role::Link)
        .collect()
}

#[test]
fn links_are_tab_stops_in_text_order_that_return_opens_and_space_does_not() {
    let mut h = Harness::<Links>::new();
    let links = links(&h);
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(links[0].0));
    h.tap(SPACE);
    h.tap(RETURN);
    h.tap(TAB);
    assert_eq!(h.snapshot().focus, Some(links[1].0));
    h.tap(RETURN);
    assert_eq!(h.model(), &vec!["https://g.test", "https://t.test"]);
}

#[test]
fn a_click_opens_the_link_under_it_and_nowhere_else() {
    let mut h = Harness::<Links>::new();
    let terms = links(&h)[1].1.bounds;
    h.click(terms.center());
    h.click(Point::new(2.0, terms.center().y));
    assert_eq!(h.model(), &vec!["https://t.test"]);
}

#[test]
fn links_that_trade_words_take_the_pointer_over_their_new_words() {
    let both = Harness::<Links>::new();
    let (guide, terms) = (
        links(&both)[0].1.bounds.center(),
        links(&both)[1].1.bounds.center(),
    );
    let mut h = Harness::<Swapping>::new();
    h.click(guide);
    h.click(guide);
    h.click(terms);
    assert_eq!(
        h.model(),
        &vec!["https://g.test", "https://g.test", "https://t.test"]
    );
}

#[test]
fn assistive_technology_reads_each_link_with_its_url_inside_the_whole_text() {
    let mut h = Harness::<Links>::assistive();
    let read: Vec<_> = links(&h)
        .into_iter()
        .map(|(_, node)| {
            (
                node.name,
                node.description,
                node.actions,
                node.states.focusable,
            )
        })
        .collect();
    let link = |name: &str, url: &str| {
        (
            Some(name.to_string()),
            Some(url.to_string()),
            vec![Action::Click],
            true,
        )
    };
    assert_eq!(
        read,
        [
            link("guide", "https://g.test"),
            link("terms", "https://t.test")
        ]
    );
    let text = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, node)| node.role == Role::StaticText)
        .unwrap()
        .1;
    assert_eq!(text.name.as_deref(), Some(TEXT));
    let guide = links(&h)[0].0;
    h.ask_node(guide, Action::Click);
    assert_eq!(h.model(), &vec!["https://g.test"]);
}

#[test]
fn link_runs_take_the_link_colour_over_a_one_pixel_underline_on_the_pixel_grid_below_the_baseline()
{
    let h = Harness::<Links>::new();
    let ops: Vec<PaintOp> = h
        .snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|node| node.ops)
        .collect();
    let link = color(ColorRole::Link, InteractionVariant::Normal, false);
    let text = color(ColorRole::Text, InteractionVariant::Normal, false);
    let glyphs: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { color, origin, .. } => Some((*color, origin.y)),
            _ => None,
        })
        .collect();
    assert_eq!(
        glyphs.iter().map(|(c, _)| *c).collect::<Vec<_>>(),
        [text, link, text, link, text],
        "before, guide, between, terms, after"
    );
    let baseline = glyphs[0].1;
    let underlines: Vec<_> = ops
        .iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(rect),
                paint: Paint::Solid(c),
            } if *c == link => Some((rect.min_y(), rect.height())),
            _ => None,
        })
        .collect();
    let row = (baseline + 1.5).round();
    assert_eq!(
        underlines,
        [(row, 1.0), (row, 1.0)],
        "on the pixel row nearest 1.5 pt below the baseline"
    );
}
