//! A Unicode corpus: for each script and sequence, where the caret stops, the order its glyphs are drawn in, and where it
//! wraps. Scripts the test font lacks are drawn with its missing glyph, which still has an advance.

use cupertino_geometry::Point;
use cupertino_ui_text::{
    AttributedText, FontIndex, FontStyle, FontWeight, Paragraph, ParagraphStyle, TextBuffer,
    TextStyle, TextSystem,
};
use std::ops::Range;
use std::sync::Arc;

const INTER: &[u8] = include_bytes!("../../cupertino-assets/fonts/InterVariable-subset.ttf");

struct Case {
    name: &'static str,
    text: &'static str,
    /// Every caret position, start to end: the grapheme boundaries.
    stops: &'static [usize],
    /// The cluster of every glyph, left to right.
    glyphs: &'static [usize],
    width: f32,
    /// The lines at `width`.
    lines: &'static [Range<usize>],
}

const CORPUS: &[Case] = &[
    Case {
        name: "Latin",
        text: "Hello, world",
        stops: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        glyphs: &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11],
        width: 50.0,
        lines: &[0..7, 7..12],
    },
    Case {
        name: "combining marks",
        text: "e\u{301}te\u{302}",
        stops: &[0, 3, 4, 7],
        glyphs: &[0, 3, 4],
        width: 20.0,
        lines: &[0..4, 4..7],
    },
    Case {
        name: "Hebrew",
        text: "\u{5E9}\u{5DC}\u{5D5}\u{5DD} \u{5E2}\u{5D5}\u{5DC}\u{5DD}",
        stops: &[0, 2, 4, 6, 8, 9, 11, 13, 15, 17],
        glyphs: &[15, 13, 11, 9, 8, 6, 4, 2, 0],
        width: 50.0,
        lines: &[0..9, 9..17],
    },
    Case {
        name: "Arabic with vowel marks",
        text: "\u{645}\u{64E}\u{631}\u{652}\u{62D}\u{64E}\u{628}\u{64B}\u{627} \u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}",
        stops: &[0, 4, 8, 12, 16, 18, 19, 21, 23, 25, 27, 29, 31, 33],
        glyphs: &[31, 29, 27, 25, 23, 21, 19, 18, 16, 12, 12, 8, 8, 4, 4, 0, 0],
        width: 100.0,
        lines: &[0..19, 19..33],
    },
    Case {
        name: "Devanagari conjunct",
        text: "\u{928}\u{92E}\u{938}\u{94D}\u{924}\u{947}",
        stops: &[0, 3, 6, 18],
        glyphs: &[0, 3, 6, 6, 12, 12],
        width: 20.0,
        lines: &[0..3, 3..6, 6..18],
    },
    Case {
        name: "ZWJ emoji",
        text: "\u{1F469}\u{200D}\u{1F4BB} ok",
        stops: &[0, 11, 12, 13, 14],
        glyphs: &[0, 0, 0, 11, 12, 13],
        width: 40.0,
        lines: &[0..12, 12..14],
    },
    Case {
        name: "flags",
        text: "\u{1F1E7}\u{1F1F7}\u{1F1EF}\u{1F1F5}",
        stops: &[0, 8, 16],
        glyphs: &[0, 0, 8, 8],
        width: 40.0,
        lines: &[0..8, 8..16],
    },
    Case {
        name: "Hebrew with digits inside Latin",
        text: "abc \u{5D0}\u{5D1}\u{5D2} 123",
        stops: &[0, 1, 2, 3, 4, 6, 8, 10, 11, 12, 13, 14],
        glyphs: &[0, 1, 2, 3, 11, 12, 13, 10, 8, 6, 4],
        width: 60.0,
        lines: &[0..4, 4..11, 11..14],
    },
];

fn paragraph(text: &str) -> Paragraph {
    let mut fonts = FontIndex::new();
    fonts
        .add_face(
            "Inter",
            FontWeight::REGULAR,
            FontStyle::Normal,
            Arc::new(INTER.to_vec()),
            0,
        )
        .unwrap();
    TextSystem::new(fonts, "Inter")
        .paragraph(
            &AttributedText::plain(text, TextStyle::new(16.0)),
            &ParagraphStyle::default(),
        )
        .unwrap()
}

#[test]
fn the_caret_steps_over_whole_graphemes_both_ways() {
    for case in CORPUS {
        let mut buffer = TextBuffer::new();
        buffer.set_text(case.text);
        buffer.move_to_start(false);
        let mut forward = vec![buffer.caret()];
        while buffer.caret() < case.text.len() {
            buffer.move_char(true, false);
            forward.push(buffer.caret());
        }
        assert_eq!(forward, case.stops, "{}", case.name);
        let mut backward = vec![buffer.caret()];
        while buffer.caret() > 0 {
            buffer.move_char(false, false);
            backward.push(buffer.caret());
        }
        backward.reverse();
        assert_eq!(backward, case.stops, "{}", case.name);
    }
}

#[test]
fn glyphs_are_drawn_in_display_order() {
    for case in CORPUS {
        let layout = paragraph(case.text).layout(None);
        let glyphs: Vec<usize> = layout.lines()[0]
            .runs
            .iter()
            .flat_map(|run| run.run.glyphs.iter().map(|glyph| glyph.cluster))
            .collect();
        assert_eq!(glyphs, case.glyphs, "{}", case.name);
    }
}

#[test]
fn lines_wrap_between_words_and_never_inside_a_grapheme() {
    for case in CORPUS {
        let layout = paragraph(case.text).layout(Some(case.width));
        let lines: Vec<Range<usize>> = layout
            .lines()
            .iter()
            .map(|line| line.range.0.clone())
            .collect();
        assert_eq!(lines, case.lines, "{}", case.name);
    }
}

#[test]
fn every_click_lands_on_a_caret_stop() {
    for case in CORPUS {
        let layout = paragraph(case.text).layout(None);
        let y = layout.lines()[0].rect.y() + 1.0;
        let mut x = -2.0;
        while x < layout.size().width + 2.0 {
            let hit = layout.index_at(Point::new(x, y)).byte;
            assert!(case.stops.contains(&hit), "{}: x {x} hit {hit}", case.name);
            x += 0.25;
        }
    }
}
