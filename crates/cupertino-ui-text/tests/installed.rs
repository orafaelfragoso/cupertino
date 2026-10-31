//! Installed fonts fill in what the registered faces lack, and the scripts they bring shape with their own rules.

use cupertino_ui_text::{
    AttributedText, FontFile, FontIndex, FontStyle, FontWeight, ParagraphLayout, ParagraphStyle,
    SystemFonts, TextBuffer, TextStyle, TextSystem,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

const INTER: &[u8] = include_bytes!("../../cupertino-assets/fonts/InterVariable-subset.ttf");

fn fixture(name: &str) -> FontFile {
    FontFile {
        path: PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(format!("tests/fixtures/{name}")),
        index: 0,
    }
}

/// The system of the tests: Noto subsets for Arabic, Devanagari and Hebrew, counting what it is asked.
#[derive(Clone, Default)]
struct Installed {
    asked: Arc<AtomicUsize>,
}

impl SystemFonts for Installed {
    fn family(&self, family: &str, _: FontWeight, _: FontStyle) -> Option<FontFile> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        (family == "Noto Sans Hebrew").then(|| fixture("noto-hebrew-subset.ttf"))
    }

    fn covering(&self, ch: char, _: Option<&str>, _: FontWeight, _: FontStyle) -> Option<FontFile> {
        self.asked.fetch_add(1, Ordering::Relaxed);
        match ch {
            '\u{590}'..='\u{5FF}' => Some(fixture("noto-hebrew-subset.ttf")),
            '\u{600}'..='\u{6FF}' => Some(fixture("noto-arabic-subset.ttf")),
            '\u{900}'..='\u{97F}' => Some(fixture("noto-devanagari-subset.ttf")),
            _ => None,
        }
    }
}

fn system(installed: Option<Installed>) -> TextSystem {
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
    if let Some(installed) = installed {
        fonts.set_system_fonts(Box::new(installed));
    }
    TextSystem::new(fonts, "Inter")
}

fn layout_in(system: &TextSystem, text: &str, style: TextStyle) -> ParagraphLayout {
    system
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle::default(),
        )
        .unwrap()
        .layout(None)
}

/// The font file a run is drawn from, by its bytes.
fn file_of(system: &TextSystem, font: cupertino_ui_text::FontId) -> String {
    let data = system.font_source(font).unwrap().data;
    if data.as_slice() == INTER {
        return "Inter".to_owned();
    }
    [
        "noto-arabic-subset.ttf",
        "noto-devanagari-subset.ttf",
        "noto-hebrew-subset.ttf",
    ]
    .into_iter()
    .find(|name| std::fs::read(fixture(name).path).unwrap() == *data)
    .unwrap_or("other")
    .to_owned()
}

/// Every glyph of the first line, left to right: its cluster, its glyph id and its font file.
fn glyphs(system: &TextSystem, layout: &ParagraphLayout) -> Vec<(usize, u32, String)> {
    layout.lines()[0]
        .runs
        .iter()
        .flat_map(|run| {
            let family = file_of(system, run.run.font);
            run.run
                .glyphs
                .iter()
                .map(move |glyph| (glyph.cluster, glyph.id.0, family.clone()))
        })
        .collect()
}

#[test]
fn a_script_the_registered_faces_lack_is_drawn_by_the_installed_face_for_it_loaded_once() {
    let installed = Installed::default();
    let system = system(Some(installed.clone()));
    let text = "abc \u{5E9}\u{5DC}\u{5D5}\u{5DD}";
    let drawn = glyphs(&system, &layout_in(&system, text, TextStyle::new(16.0)));
    assert!(drawn.iter().all(|(_, id, _)| *id != 0), "{drawn:?}");
    let families: Vec<&str> = drawn.iter().map(|(_, _, family)| family.as_str()).collect();
    assert_eq!(
        families,
        [
            "Inter",
            "Inter",
            "Inter",
            "Inter",
            "noto-hebrew-subset.ttf",
            "noto-hebrew-subset.ttf",
            "noto-hebrew-subset.ttf",
            "noto-hebrew-subset.ttf"
        ]
    );
    assert_eq!(
        installed.asked.load(Ordering::Relaxed),
        1,
        "the first letter loads the face for all four"
    );

    layout_in(
        &system,
        "\u{5E2}\u{5D5}\u{5DC}\u{5DD} \u{2603}",
        TextStyle::new(17.0),
    );
    layout_in(&system, "\u{2603}", TextStyle::new(18.0));
    assert_eq!(
        installed.asked.load(Ordering::Relaxed),
        2,
        "loaded letters ask nothing, and a character no installed face has is asked for once"
    );
}

#[test]
fn without_installed_fonts_a_missing_script_stays_the_missing_glyph() {
    let system = system(None);
    let drawn = glyphs(
        &system,
        &layout_in(&system, "\u{5E9}", TextStyle::new(16.0)),
    );
    assert_eq!(drawn, [(0, 0, "Inter".to_owned())]);
}

#[test]
fn a_family_no_registered_face_has_is_looked_up_among_the_installed_fonts() {
    let system = system(Some(Installed::default()));
    let style = TextStyle {
        family: Some("Noto Sans Hebrew".into()),
        ..TextStyle::new(16.0)
    };
    let drawn = glyphs(&system, &layout_in(&system, " ", style));
    assert_eq!(drawn[0].2, "noto-hebrew-subset.ttf");
}

#[test]
fn a_pre_base_vowel_sign_is_drawn_before_its_consonant_and_the_caret_steps_over_both() {
    let system = system(Some(Installed::default()));
    let text = "\u{915}\u{93F}";
    let layout = layout_in(&system, text, TextStyle::new(16.0));
    let drawn = glyphs(&system, &layout);
    assert_eq!(drawn.len(), 2, "{drawn:?}");
    assert!(drawn.iter().all(|(cluster, id, family)| *cluster == 0
        && *id != 0
        && family == "noto-devanagari-subset.ttf"));
    let alone = |text: &str| glyphs(&system, &layout_in(&system, text, TextStyle::new(16.0)))[0].1;
    assert_eq!(
        drawn[1].1,
        alone("\u{915}"),
        "the consonant comes second on screen, after the sign of i"
    );
    let mut buffer = TextBuffer::new();
    buffer.set_text(text);
    buffer.move_to_start(false);
    buffer.move_char(true, false);
    assert_eq!(buffer.caret(), text.len());
}

#[test]
fn arabic_letters_take_their_joined_forms() {
    let system = system(Some(Installed::default()));
    let alone = glyphs(
        &system,
        &layout_in(&system, "\u{639}", TextStyle::new(16.0)),
    )[0]
    .1;
    let word = "\u{628}\u{627}\u{644}\u{639}\u{627}\u{644}\u{645}";
    let drawn = glyphs(&system, &layout_in(&system, word, TextStyle::new(16.0)));
    let ain = drawn
        .iter()
        .find(|(cluster, _, _)| *cluster == word.find('\u{639}').unwrap())
        .unwrap();
    assert_ne!(ain.1, alone, "ain between two letters is its medial form");
    assert_eq!(ain.2, "noto-arabic-subset.ttf");
}
