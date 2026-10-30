//! Characters the run's font lacks are drawn by the next loaded font that has them.

use cupertino_ui_text::{
    AttributedText, FontId, FontIndex, FontStyle, FontWeight, ParagraphStyle, TextStyle, TextSystem,
};
use std::sync::Arc;

const INTER: &[u8] = include_bytes!("../../cupertino-assets/fonts/InterVariable-subset.ttf");
/// Boxes for 日, 本, ❤ and 😀 (tests/fixtures/make-fallback-font.py).
const BOXES: &[u8] = include_bytes!("fixtures/fallback-test.ttf");

fn system(faces: &[(&str, &[u8])]) -> TextSystem {
    let mut fonts = FontIndex::new();
    for (family, data) in faces {
        fonts
            .add_face(
                family,
                FontWeight::REGULAR,
                FontStyle::Normal,
                Arc::new(data.to_vec()),
                0,
            )
            .unwrap();
    }
    TextSystem::new(fonts, "Inter")
}

/// The font of every glyph of `text`, by the byte offset of its cluster, and whether any glyph is the missing glyph.
fn fonts_of(system: &TextSystem, text: &str) -> (Vec<(usize, FontId)>, bool) {
    let paragraph = system
        .paragraph(
            &AttributedText::plain(text, TextStyle::new(16.0)),
            &ParagraphStyle::default(),
        )
        .unwrap();
    let layout = paragraph.layout(None);
    let glyphs: Vec<(usize, FontId, u32)> = layout
        .lines()
        .iter()
        .flat_map(|line| &line.runs)
        .flat_map(|run| {
            run.run
                .glyphs
                .iter()
                .map(|g| (g.cluster, run.run.font, g.id.0))
        })
        .collect();
    let missing = glyphs.iter().any(|&(_, _, id)| id == 0);
    (
        glyphs.iter().map(|&(at, font, _)| (at, font)).collect(),
        missing,
    )
}

fn font_at(fonts: &[(usize, FontId)], text: &str, part: &str) -> FontId {
    let at = text.find(part).unwrap();
    let mut found = fonts.iter().filter(|(cluster, _)| *cluster == at);
    let font = found.next().unwrap().1;
    assert!(
        found.all(|(_, f)| *f == font),
        "{part} is drawn by one font"
    );
    font
}

#[test]
fn latin_cjk_and_emoji_each_find_a_font_that_has_them() {
    let text = "Hi 日本 ❤\u{FE0F} 😀 ok";
    let (fonts, missing) = fonts_of(&system(&[("Inter", INTER), ("Boxes", BOXES)]), text);
    assert!(!missing, "no missing glyph in {fonts:?}");
    let (latin, cjk) = (font_at(&fonts, text, "Hi"), font_at(&fonts, text, "日"));
    assert_ne!(latin, cjk);
    assert_eq!(font_at(&fonts, text, "本"), cjk);
    assert_eq!(
        font_at(&fonts, text, "❤"),
        cjk,
        "the heart and its variation selector stay one cluster in the font that has the heart"
    );
    assert_eq!(font_at(&fonts, text, "😀"), cjk);
    assert_eq!(font_at(&fonts, text, "ok"), latin);
}

#[test]
fn the_matched_family_goes_first_whatever_the_load_order() {
    let text = "Hi 日";
    let (fonts, missing) = fonts_of(&system(&[("Boxes", BOXES), ("Inter", INTER)]), text);
    assert!(!missing);
    assert_ne!(font_at(&fonts, text, "Hi"), font_at(&fonts, text, "日"));
}

#[test]
fn a_character_no_font_has_stays_in_the_runs_font() {
    let text = "Hi 日";
    let (fonts, missing) = fonts_of(&system(&[("Inter", INTER)]), text);
    assert!(missing, "the missing glyph shows");
    assert_eq!(font_at(&fonts, text, "Hi"), font_at(&fonts, text, "日"));
}
