//! Instruction counts of the text a file manager shows: 64 file names in 13-point text, as a list shows them (one line
//! each, cut with an ellipsis to a 220-point column) and as an icon grid does (up to two lines wrapped at 84 points).
//! Shaped and laid out (none of them in the paragraph cache), and laid out again at another width from the shaping kept.

use cupertino_ui_text::{
    AttributedText, FontIndex, FontStyle, FontWeight, Paragraph, ParagraphLayout, ParagraphStyle,
    TextStyle, TextSystem, Wrap,
};
use gungraun::prelude::*;
use std::hint::black_box;
use std::sync::Arc;

const FONT: &[u8] = include_bytes!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cupertino-assets/fonts/InterVariable-subset.ttf"
));
/// The text size, in points.
const SIZE: f32 = 13.0;
const NAMES: usize = 64;

const STEMS: [&str; 16] = [
    "Quarterly report",
    "IMG_4821",
    "Holiday plans (final)",
    "notes",
    "Invoice 2026-031",
    "build-output",
    "Screenshot at 09.41.12",
    "Project Proposal — draft 3",
    "README",
    "budget_spreadsheet_with_a_rather_long_name",
    "Recording 17",
    "Café menu",
    "untitled folder",
    "Résumé",
    "archive",
    "Meeting minutes, 12 November",
];
const EXTENSIONS: [&str; 8] = [
    ".pdf", ".jpeg", ".txt", ".md", "", ".tar.gz", ".png", ".odt",
];

/// The 64 names: each stem with several extensions, numbered past the first round so that no two are equal.
fn names() -> Vec<String> {
    (0..NAMES)
        .map(|i| {
            let stem = STEMS[i % STEMS.len()];
            let ext = EXTENSIONS[(i * 3) % EXTENSIONS.len()];
            match i / STEMS.len() {
                0 => format!("{stem}{ext}"),
                n => format!("{stem} {n}{ext}"),
            }
        })
        .collect()
}

/// How the names are shown.
#[derive(Clone, Copy)]
enum View {
    /// One line in a list's name column.
    List,
    /// Up to two lines under an icon.
    Icons,
}

impl View {
    fn style(self) -> ParagraphStyle {
        let (max_lines, wrap) = match self {
            View::List => (1, Wrap::None),
            View::Icons => (2, Wrap::Word),
        };
        ParagraphStyle {
            max_lines: Some(max_lines),
            wrap,
            ellipsis: true,
            ..ParagraphStyle::default()
        }
    }

    /// The width the names are laid out at, in points.
    fn width(self) -> f32 {
        match self {
            View::List => 220.0,
            View::Icons => 84.0,
        }
    }
}

/// A text system on the built-in font that has shaped and laid out another line in both views already, so that what
/// is set up once per process (the segmenters' data) is not counted.
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
    let system = TextSystem::new(fonts, "Inter");
    for view in [View::List, View::Icons] {
        let warm = AttributedText::plain("Warm up 0123 and wrap", TextStyle::new(SIZE));
        let paragraph = system.paragraph(&warm, &view.style()).unwrap();
        paragraph.layout(Some(view.width()));
    }
    system
}

fn setup_cold(view: View) -> (TextSystem, Vec<AttributedText>, View) {
    let texts = names()
        .iter()
        .map(|n| AttributedText::plain(n, TextStyle::new(SIZE)))
        .collect();
    (system(), texts, view)
}

fn setup_shaped(view: View) -> (Vec<Paragraph>, View) {
    let (system, texts, view) = setup_cold(view);
    let paragraphs = texts
        .iter()
        .map(|t| system.paragraph(t, &view.style()).unwrap())
        .collect();
    (paragraphs, view)
}

#[library_benchmark]
#[bench::list_64(args = (View::List), setup = setup_cold)]
#[bench::icons_64(args = (View::Icons), setup = setup_cold)]
fn shape_and_lay_out(
    (system, texts, view): (TextSystem, Vec<AttributedText>, View),
) -> Vec<ParagraphLayout> {
    let style = view.style();
    black_box(
        texts
            .iter()
            .map(|t| {
                let paragraph = system.paragraph(black_box(t), &style).unwrap();
                paragraph.layout(Some(view.width()))
            })
            .collect(),
    )
}

// Laid out narrower than they were shaped for, as when the column or the window is resized.
#[library_benchmark]
#[bench::list_64(args = (View::List), setup = setup_shaped)]
#[bench::icons_64(args = (View::Icons), setup = setup_shaped)]
fn lay_out_shaped((paragraphs, view): (Vec<Paragraph>, View)) -> Vec<ParagraphLayout> {
    let width = (view.width() - 60.0).max(view.width() * 0.75);
    black_box(
        paragraphs
            .iter()
            .map(|p| black_box(p).layout(Some(width)))
            .collect(),
    )
}

library_benchmark_group!(
    name = file_names,
    benchmarks = [shape_and_lay_out, lay_out_shaped]
);
main!(library_benchmark_groups = file_names);
