use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{Stack, Text, TextRole};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{AttributedText, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{ColorRole, InteractionVariant};
use support::{color, label_width, text, window, Harness};

const LONG: &str = "A sentence long enough to need more than one line in a narrow window";

struct Texts;

impl Application for Texts {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let text = |key: &'static str, content: &str, role, wrap| {
            Element::new(Text {
                content: AttributedText::plain(content, TextStyle::new(13.0)),
                role,
                wrap,
                secondary: false,
                selected: false,
            })
            .key(key)
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![
                text("body", "Wi-Fi", TextRole::Body, false),
                text("title", "Wi-Fi", TextRole::Title, false),
                text("one-line", LONG, TextRole::Body, false),
                text("wrapped", LONG, TextRole::Body, true),
            ],
        }))
    }
}

#[test]
fn text_takes_its_shaped_size_in_its_role() {
    let h = Harness::<Texts>::new();
    assert_eq!(h.rect("body").width(), label_width("Wi-Fi"));
    assert!(h.rect("title").width() > h.rect("body").width());
    assert!(h.rect("title").height() > h.rect("body").height());
}

#[test]
fn wrapping_text_breaks_within_the_width_it_is_offered() {
    let h = Harness::<Texts>::new();
    assert!(h.rect("wrapped").width() <= 200.0);
    assert!(h.rect("wrapped").height() > h.rect("one-line").height());
}

/// Where each line of the wrapped text starts and ends, from the left of its box.
fn wrapped_lines(direction: LayoutDirection) -> (f64, Vec<(f64, f64)>) {
    let h = Harness::<Texts>::with_direction(direction);
    let width = h.rect("wrapped").width();
    let starts = h.ops("wrapped").into_iter().filter_map(|op| match op {
        PaintOp::Glyphs { origin, .. } => Some(origin.x),
        _ => None,
    });
    let layout = text()
        .paragraph(
            &AttributedText::plain(LONG, TextStyle::new(13.0)),
            &ParagraphStyle::default(),
        )
        .unwrap()
        .layout(Some(width as f32));
    let lines = layout
        .lines()
        .iter()
        .zip(starts)
        .map(|(line, x)| (x, x + label_width(LONG[line.range.0.clone()].trim_end())))
        .collect();
    (width, lines)
}

#[test]
fn wrapped_text_lines_up_with_the_leading_edge_of_its_layout() {
    let (width, ltr) = wrapped_lines(LayoutDirection::Ltr);
    assert!(ltr.len() > 1);
    assert!(ltr.iter().all(|&(start, _)| start == 0.0), "{ltr:?}");
    let (_, rtl) = wrapped_lines(LayoutDirection::Rtl);
    assert_eq!(rtl.len(), ltr.len());
    assert!(
        rtl.iter().all(|&(_, end)| (end - width).abs() < 1e-3),
        "{width} {rtl:?}"
    );
}

#[test]
fn text_is_static_in_the_text_colour_and_never_a_tab_stop() {
    let h = Harness::<Texts>::new();
    let colors: Vec<_> = h
        .ops("body")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { color, .. } => Some(color),
            _ => None,
        })
        .collect();
    assert!(!colors.is_empty());
    let text = color(ColorRole::Text, InteractionVariant::Normal, false);
    assert!(colors.iter().all(|&c| c == text));
    let node = h.accessible("body");
    assert_eq!(node.role, Role::StaticText);
    assert_eq!(node.name.as_deref(), Some("Wi-Fi"));
    assert!(!node.states.focusable);
}

#[test]
fn the_window_paints_its_background_under_the_content() {
    let h = Harness::<Texts>::new();
    let snapshot = h.snapshot();
    let root = snapshot
        .display_list
        .nodes
        .iter()
        .find(|n| n.parent.is_none())
        .unwrap();
    assert_eq!(
        root.ops.first(),
        Some(&PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 200.0, 100.0)),
            paint: Paint::Solid(color(
                ColorRole::WindowBackground,
                InteractionVariant::Normal,
                false
            )),
        })
    );
}

struct Padded;

impl Application for Padded {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets {
                top: 4.0,
                left: 12.0,
                bottom: 0.0,
                right: 0.0,
            },
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Text {
                content: AttributedText::plain("Wi-Fi", TextStyle::new(13.0)),
                role: TextRole::Body,
                wrap: false,
                secondary: false,
                selected: false,
            })
            .key("body")],
        }))
    }
}

#[test]
fn a_stack_insets_its_children_by_its_padding() {
    let h = Harness::<Padded>::new();
    let rect = h.rect("body");
    assert_eq!((rect.min_x(), rect.min_y()), (12.0, 4.0));
}
