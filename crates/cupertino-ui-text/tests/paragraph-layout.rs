//! Laying text out: where a paragraph breaks its lines, how a position and a point map to each other, and which
//! values the text system refuses.

use cupertino_assets::BUILTIN_FONT as FONT;
use cupertino_geometry::Point;
use cupertino_ui_text::{
    Affinity, AttributedText, Direction, FontIndex, FontStyle, FontWeight, ParagraphLayout,
    ParagraphStyle, TextError, TextPosition, TextStyle, TextSystem, Wrap,
};
use std::sync::Arc;
use unicode_segmentation::UnicodeSegmentation;

fn system() -> TextSystem {
    let mut fonts = FontIndex::new();
    fonts
        .add_face(
            "Inter",
            FontWeight::REGULAR,
            FontStyle::Normal,
            Arc::new(FONT.to_vec()),
            0,
        )
        .unwrap();
    TextSystem::new(fonts, "Inter")
}

fn laid_out(text: &str, style: ParagraphStyle, width: Option<f32>) -> ParagraphLayout {
    system()
        .paragraph(&AttributedText::plain(text, TextStyle::new(14.0)), &style)
        .unwrap()
        .layout(width)
}

/// The text of each line.
fn lines<'a>(text: &'a str, layout: &ParagraphLayout) -> Vec<&'a str> {
    layout
        .lines()
        .iter()
        .map(|l| &text[l.range.0.clone()])
        .collect()
}

fn width_of(text: &str) -> f64 {
    laid_out(text, ParagraphStyle::default(), None).size().width
}

#[test]
fn a_paragraph_breaks_lines_between_words_and_a_word_too_long_between_graphemes() {
    let text = "Open the quarterly report";
    let wide = width_of(text);
    let narrow = (wide * 0.6) as f32;
    let wrapped = laid_out(text, ParagraphStyle::default(), Some(narrow));
    let each = lines(text, &wrapped);
    assert!(each.len() >= 2, "{each:?}");
    assert_eq!(each.concat(), text, "the lines hold the text between them");
    assert!(
        each[..each.len() - 1].iter().all(|l| l.ends_with(' ')),
        "a line ends at a space that hangs past its edge: {each:?}"
    );
    assert!(wrapped.size().width <= f64::from(narrow) + 0.01);

    let unwrapped = laid_out(text, ParagraphStyle::default(), None);
    assert_eq!(lines(text, &unwrapped), [text]);
    let never = laid_out(
        text,
        ParagraphStyle {
            wrap: Wrap::None,
            ..Default::default()
        },
        Some(narrow),
    );
    assert_eq!(
        lines(text, &never),
        [text],
        "without wrapping the width is not a limit"
    );

    let word = "Supercalifragilistic\u{301}expialidocious";
    let broken = laid_out(word, ParagraphStyle::default(), Some(50.0));
    let pieces = lines(word, &broken);
    assert!(pieces.len() >= 3, "{pieces:?}");
    assert_eq!(pieces.concat(), word);
    for piece in &pieces {
        assert!(
            piece.graphemes(true).collect::<String>() == *piece && !piece.starts_with('\u{301}'),
            "a break never splits a grapheme: {piece:?}"
        );
    }

    let hard = "first\nsecond\n\nfourth";
    let by_line = laid_out(hard, ParagraphStyle::default(), Some(500.0));
    assert_eq!(lines(hard, &by_line), ["first", "second", "", "fourth"]);
}

#[test]
fn text_past_the_line_limit_ends_its_last_line_in_an_ellipsis_of_its_own() {
    let text = "one two three four five six seven eight nine ten";
    let plain = ParagraphStyle {
        max_lines: Some(2),
        ..Default::default()
    };
    let cut = laid_out(text, plain, Some(80.0));
    assert_eq!(cut.lines().len(), 2, "{:?}", lines(text, &cut));
    let none = laid_out(
        text,
        ParagraphStyle {
            max_lines: Some(0),
            ..Default::default()
        },
        Some(80.0),
    );
    assert_eq!(none.lines().len(), 1, "a limit of none keeps one line");

    let ellipsis = ParagraphStyle {
        max_lines: Some(2),
        ellipsis: true,
        ..Default::default()
    };
    let shown = laid_out(text, ellipsis, Some(80.0));
    assert_eq!(shown.lines().len(), 2);
    let last = shown.lines().last().unwrap();
    let mark = last.runs.last().unwrap();
    assert!(
        mark.range.0.is_empty(),
        "the ellipsis is a run with no text of its own"
    );
    assert!(
        mark.origin.x + 1.0 >= last.rect.x() && mark.origin.x <= 80.0,
        "it sits within the line: {} in 80",
        mark.origin.x
    );
    let fits = laid_out("short", ellipsis, Some(80.0));
    assert!(
        fits.lines()[0].runs.iter().all(|r| !r.range.0.is_empty()),
        "text that fits has none"
    );
}

#[test]
fn a_position_and_the_point_of_its_caret_name_each_other_and_a_selection_is_a_rectangle_a_line() {
    let text = "Wrapped te\u{301}xt in two lines";
    let layout = laid_out(text, ParagraphStyle::default(), Some(90.0));
    assert!(layout.lines().len() >= 2);
    let mut x_before = f64::MIN;
    let mut line_before = 0.0;
    for (byte, _) in text.grapheme_indices(true) {
        let at = TextPosition {
            byte,
            affinity: Affinity::Downstream,
        };
        let caret = layout.caret_rect(at);
        assert!(caret.height() > 0.0);
        if caret.y() == line_before {
            assert!(
                caret.x() >= x_before,
                "carets go right along a line: {byte}"
            );
        }
        (x_before, line_before) = (caret.x(), caret.y());
        let hit = layout.index_at(Point::new(
            caret.x() + 0.1,
            caret.y() + caret.height() / 2.0,
        ));
        assert_eq!(
            hit.byte, byte,
            "a click on the caret's own place lands on its position"
        );
    }

    let first = layout.lines().first().unwrap();
    let before = layout.index_at(Point::new(-30.0, first.rect.y() + 1.0));
    assert_eq!(before.byte, 0, "left of the text is its start");
    let below = layout.index_at(Point::new(1000.0, 1000.0));
    assert_eq!(below.byte, text.len(), "past the end is the end");

    let rects = layout.selection_rects(2..text.len() - 2);
    assert_eq!(
        rects.len(),
        layout.lines().len(),
        "a rectangle on each line it touches"
    );
    assert!(
        rects.windows(2).all(|w| w[0].y() < w[1].y()),
        "top to bottom"
    );
    assert!(layout.selection_rects(3..3).is_empty());
}

#[test]
fn the_base_direction_is_the_style_s_or_else_the_first_strong_character_s() {
    let system = system();
    let direction = |text: &str, set: Option<Direction>| {
        let style = ParagraphStyle {
            direction: set,
            ..Default::default()
        };
        system
            .paragraph(&AttributedText::plain(text, TextStyle::new(14.0)), &style)
            .unwrap()
            .direction()
    };
    assert_eq!(direction("hello", None), Direction::Ltr);
    assert_eq!(
        direction("\u{5E9}\u{5DC}\u{5D5}\u{5DD} hello", None),
        Direction::Rtl
    );
    assert_eq!(
        direction("123 \u{5E9}\u{5DC}\u{5D5}\u{5DD}", None),
        Direction::Rtl,
        "digits are not strong"
    );
    assert_eq!(
        direction("\u{5E9}\u{5DC}\u{5D5}\u{5DD}", Some(Direction::Ltr)),
        Direction::Ltr
    );
}

#[test]
fn a_size_or_spacing_that_is_not_a_usable_number_is_refused() {
    let system = system();
    let with = |change: fn(&mut TextStyle)| {
        let mut style = TextStyle::new(14.0);
        change(&mut style);
        system
            .paragraph(
                &AttributedText::plain("text", style),
                &ParagraphStyle::default(),
            )
            .map(|_| ())
    };
    type Change = fn(&mut TextStyle);
    let rows: [(&str, Change); 6] = [
        ("a size of zero", |s| s.size = 0.0),
        ("a negative size", |s| s.size = -3.0),
        ("a size that is not a number", |s| s.size = f32::NAN),
        ("an infinite size", |s| s.size = f32::INFINITY),
        ("spacing that is not a number", |s| {
            s.letter_spacing = f32::NAN
        }),
        ("infinite spacing", |s| s.letter_spacing = f32::NEG_INFINITY),
    ];
    for (what, change) in rows {
        assert_eq!(with(change), Err(TextError::InvalidValue), "{what}");
    }
    assert_eq!(
        with(|s| s.letter_spacing = -0.5),
        Ok(()),
        "negative spacing is allowed"
    );
}
