//! A font the program holds, a built-in one, registered from its own bytes: the index keeps no copy of them, and text
//! lays out and draws in it as in the same font read from a file.

use cupertino_assets::BUILTIN_FONT as FONT;
use cupertino_ui_text::{
    AttributedText, FontData, FontIndex, GlyphKey, GlyphRaster, GlyphRasterizer, Line,
    ParagraphStyle, TextStyle, TextSystem,
};
use std::sync::Arc;

/// A text system on `data` alone.
fn system(data: FontData) -> Arc<TextSystem> {
    let mut fonts = FontIndex::new();
    fonts.add_font(data).unwrap();
    Arc::new(TextSystem::new(fonts, "Inter Variable"))
}

/// The lines of "Café menu" at 120 points wide, and the coverage of every glyph on them.
fn shaped(system: &Arc<TextSystem>) -> (Vec<Line>, Vec<Option<Vec<u8>>>) {
    let paragraph = system
        .paragraph(
            &AttributedText::plain("Café menu", TextStyle::new(13.0)),
            &ParagraphStyle::default(),
        )
        .unwrap();
    let lines = paragraph.layout(Some(120.0)).lines().to_vec();
    let mut raster = GlyphRaster::new(Arc::clone(system));
    let glyphs = lines
        .iter()
        .flat_map(|line| &line.runs)
        .flat_map(|run| run.run.glyphs.iter().map(|g| (run.run.font, g.id)))
        .map(|(font, glyph)| {
            let key = GlyphKey::for_draw(font, glyph, 26.0, 0.0, true, [0.0; 3]);
            raster.rasterize(&key).unwrap().map(|g| g.pixels.to_vec())
        })
        .collect();
    (lines, glyphs)
}

#[test]
fn a_font_the_program_holds_is_kept_as_its_own_bytes_and_sets_text_as_one_read_from_a_file() {
    let held = system(Arc::new(FONT));
    let read = system(Arc::new(FONT.to_vec()));
    let (lines, glyphs) = shaped(&held);
    assert_eq!((lines.clone(), glyphs.clone()), shaped(&read));
    assert!(glyphs.iter().any(Option::is_some), "the text has ink");
    let font = lines[0].runs[0].run.font;
    let source = held.font_source(font).unwrap();
    assert_eq!(
        (*source.data).as_ref().as_ptr(),
        FONT.as_ptr(),
        "the bytes are the program's, not a copy"
    );
}
