//! One text system serving several threads at once, as the Kit instances of one process share it: each gets what it
//! would alone, and a panic in one while it shapes leaves the system working for the others.

use cupertino_assets::BUILTIN_FONT as FONT;
use cupertino_ui_text::{
    AttributedText, FontFile, FontIndex, FontStyle, FontWeight, GlyphKey, GlyphRaster,
    GlyphRasterizer, Line, ParagraphStyle, SystemFonts, TextStyle, TextSystem,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

const TEXTS: [&str; 6] = [
    "Documents",
    "Résumé 2026.pdf",
    "Meeting minutes, 12 November",
    "untitled folder",
    "archive.tar.gz",
    "Café menu",
];

/// Installed fonts that panic the first time they are asked for anything, as a broken font configuration might.
struct PanicsOnce(AtomicBool);

impl SystemFonts for PanicsOnce {
    fn family(&self, _: &str, _: FontWeight, _: FontStyle) -> Option<FontFile> {
        self.ask()
    }

    fn covering(&self, _: char, _: Option<&str>, _: FontWeight, _: FontStyle) -> Option<FontFile> {
        self.ask()
    }
}

impl PanicsOnce {
    fn ask(&self) -> Option<FontFile> {
        assert!(
            self.0.swap(true, Ordering::Relaxed),
            "the installed fonts failed"
        );
        None
    }
}

fn system(installed: Option<Box<dyn SystemFonts>>) -> Arc<TextSystem> {
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
    if let Some(installed) = installed {
        fonts.set_system_fonts(installed);
    }
    Arc::new(TextSystem::new(fonts, "Inter"))
}

/// What `system` makes of `text`: its lines at 120 points wide, and the coverage of every glyph on them.
fn shaped(system: &Arc<TextSystem>, text: &str) -> (Vec<Line>, Vec<Option<Vec<u8>>>) {
    let paragraph = system
        .paragraph(
            &AttributedText::plain(text, TextStyle::new(13.0)),
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
fn threads_sharing_one_text_system_each_get_what_one_alone_gets() {
    let alone = system(None);
    let expected: Vec<_> = TEXTS.iter().map(|t| shaped(&alone, t)).collect();
    let shared = system(None);
    std::thread::scope(|threads| {
        for start in 0..8 {
            let (shared, expected) = (&shared, &expected);
            threads.spawn(move || {
                for round in 0..TEXTS.len() * 3 {
                    let i = (start + round) % TEXTS.len();
                    assert_eq!(&shaped(shared, TEXTS[i]), &expected[i], "{}", TEXTS[i]);
                }
            });
        }
    });
    assert!(
        shared.cached_paragraphs() <= TEXTS.len(),
        "{}",
        shared.cached_paragraphs()
    );
    let rasters = shared.shared_rasters();
    assert!(rasters.found > 0 && rasters.glyphs > 0, "{rasters:?}");
}

#[test]
fn a_panic_while_one_thread_shapes_leaves_the_text_system_working_for_the_others() {
    let shared = system(Some(Box::new(PanicsOnce(AtomicBool::new(false)))));
    let before = shaped(&shared, TEXTS[0]);
    let panicked = std::thread::scope(|threads| {
        threads
            .spawn(|| shaped(&shared, "a character no face has: \u{10FFFD}"))
            .join()
    });
    assert!(panicked.is_err());
    std::thread::scope(|threads| {
        threads.spawn(|| {
            assert_eq!(shaped(&shared, TEXTS[0]), before);
            assert_eq!(shaped(&shared, TEXTS[1]), shaped(&system(None), TEXTS[1]));
            shaped(&shared, "a character no face has: \u{10FFFD}");
        });
    });
}
