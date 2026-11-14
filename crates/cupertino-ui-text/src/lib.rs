//! CupertinoKit text. Paragraphs are laid out from [`AttributedText`] into lines of [`GlyphRun`]s, positions are UTF-8
//! byte offsets on grapheme boundaries, and editing goes through an [`TextBuffer`] or, for passwords, a
//! [`SecureBuffer`]. A [`TextInputSession`] keeps the input method's composition consistent with the buffer by
//! revision.

use cupertino_geometry::Rect;
use std::ops::Range;
use std::sync::Arc;

mod bidi;
mod buffer;
mod font;
mod input;
mod installed;
mod keymap;
mod keys;
mod paragraph;
mod raster;
mod secure;
mod shape;
mod strike;
mod system;

/// The text work this thread has done: paragraphs shaped (not found in the shaping cache) and laid out. For tests
/// that hold the work of an edit to what it changed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TextWork {
    pub shaped: u64,
    pub laid_out: u64,
}

thread_local!(static TEXT_WORK: std::cell::Cell<TextWork> = const {
    std::cell::Cell::new(TextWork { shaped: 0, laid_out: 0 })
});

impl TextWork {
    pub fn current() -> TextWork {
        TEXT_WORK.with(std::cell::Cell::get)
    }

    fn record(change: impl FnOnce(&mut TextWork)) {
        TEXT_WORK.with(|work| {
            let mut now = work.get();
            change(&mut now);
            work.set(now);
        });
    }
}

pub use buffer::{LineLayout, TextBuffer, CARET_BLINK_OFF, CARET_BLINK_ON, MULTI_CLICK_INTERVAL};
pub use font::{FontIndex, FontMetrics, FontSource};
pub use input::SURROUNDING_MAX_BYTES;
pub use installed::{FontFile, InstalledFonts, SystemFonts};
pub use keymap::{KeyInput, Keymap};
pub use keys::{Response, TextEditorKeys, TextFieldKeys};
pub use paragraph::{Line, LineRun, Paragraph, ParagraphLayout, CARET_WIDTH};
pub use raster::{GlyphRaster, SharedRasterStats, SHARED_RASTER_BYTES};
pub use secure::SecureBuffer;
pub use shape::{ShapeRequest, ShapedGlyph, ShapedRun};
pub use system::TextSystem;

/// One face of a font file: its file and collection index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FaceId(pub u32);

/// A face instance: a face with its variation coordinates (`wght`, `opsz`, ...) applied, as the font index resolved
/// them. Glyph runs and raster keys name fonts by this id, so they carry no coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct GlyphId(pub u32);

/// OpenType weight, 1 to 1000.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FontWeight(pub u16);

impl FontWeight {
    pub const REGULAR: FontWeight = FontWeight(400);
    pub const SEMIBOLD: FontWeight = FontWeight(600);
    pub const BOLD: FontWeight = FontWeight(700);
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum FontStyle {
    #[default]
    Normal,
    Italic,
}

/// An OpenType feature setting, such as `tnum` = 1.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FontFeature {
    pub tag: [u8; 4],
    pub value: u32,
}

/// A variable-font axis setting, such as `wght` = 650.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FontVariation {
    pub tag: [u8; 4],
    pub value: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `None` uses the theme's font.
    pub family: Option<Arc<str>>,
    pub size: f32,
    pub weight: FontWeight,
    pub style: FontStyle,
    pub features: Vec<FontFeature>,
    pub variations: Vec<FontVariation>,
    /// A BCP 47 language tag; it picks fallback fonts and localized glyph forms.
    pub lang: Option<Arc<str>>,
    pub letter_spacing: f32,
}

/// A range of UTF-8 bytes in a text, on grapheme boundaries.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextRange(pub Range<usize>);

/// Text with styled byte ranges.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributedText {
    pub text: String,
    pub runs: Vec<(TextRange, TextStyle)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
    /// The left edge in either direction, as a layout that runs left to right places its text.
    Left,
    Right,
}

/// The paragraph's base direction; `None` takes it from the first strong character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Direction {
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Wrap {
    #[default]
    Word,
    None,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParagraphStyle {
    pub align: TextAlign,
    pub direction: Option<Direction>,
    pub max_lines: Option<u32>,
    pub wrap: Wrap,
    /// Text that does not fit, past `max_lines` or wider than the width a line is laid out at, ends its last line in an
    /// ellipsis at the end edge, in place of the characters it hides.
    pub ellipsis: bool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionedGlyph {
    pub id: GlyphId,
    pub x: f32,
    pub y: f32,
    /// Byte offset in the paragraph's text of the cluster this glyph belongs to.
    pub cluster: usize,
}

/// Glyphs of one face instance at one size, positioned in logical points relative to the run's origin on the
/// baseline. Render applies the scale and derives each glyph's subpixel phase from its physical position.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub font: FontId,
    /// Logical points per em.
    pub size: f32,
    pub glyphs: Vec<PositionedGlyph>,
}

/// Which side of a line break or direction change a position belongs to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Affinity {
    #[default]
    Downstream,
    Upstream,
}

/// A position between graphemes, as a byte offset.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TextPosition {
    pub byte: usize,
    pub affinity: Affinity,
}

/// A selection in logical order; `anchor == focus` is a caret.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: TextPosition,
    pub focus: TextPosition,
}

/// What a field holds, as the input method is told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ContentPurpose {
    #[default]
    Normal,
    Alpha,
    Digits,
    Number,
    Phone,
    Url,
    Email,
    Name,
    Password,
    Pin,
    Date,
    Time,
    DateTime,
    Terminal,
}

/// Input method hints, as a set.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ContentHints {
    pub completion: bool,
    pub spellcheck: bool,
    pub auto_capitalization: bool,
    pub lowercase: bool,
    pub uppercase: bool,
    pub titlecase: bool,
    pub hidden_text: bool,
    pub sensitive_data: bool,
    pub latin: bool,
    pub multiline: bool,
}

/// Text the input method is composing, not yet part of the buffer.
#[derive(Clone, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    /// The input method's cursor, in bytes of `text`: a caret when empty, the clause being converted otherwise;
    /// `None` hides it.
    pub cursor: Option<Range<usize>>,
}

/// One focused field's conversation with the input method. The field's owner bumps `revision` on every buffer change
/// ([`TextInputSession::select`] does it for a selection change).
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputSession {
    pub revision: u64,
    pub purpose: ContentPurpose,
    pub hints: ContentHints,
    pub selection: Selection,
    pub preedit: Option<Preedit>,
    /// A protected session never sends its text to the input method.
    pub protected: bool,
}

/// Why the text around the caret changed, as the input method is told.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ChangeCause {
    /// The change applied the input method's own batch.
    InputMethod,
    #[default]
    Other,
}

/// A bounded window of the text around the caret, with the caret and anchor as byte offsets into it.
#[derive(Clone, PartialEq, Eq)]
pub struct Surrounding {
    pub text: String,
    pub cursor: usize,
    pub anchor: usize,
}

/// What the window system sends to the compositor's input method.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputState {
    /// `None` for a protected field, so the input method learns neither its text nor where the caret is.
    pub surrounding: Option<Surrounding>,
    pub purpose: ContentPurpose,
    pub hints: ContentHints,
    pub cause: ChangeCause,
    /// The caret in window coordinates, so the candidate window can sit next to it; `None` before the first layout.
    pub cursor_rect: Option<Rect>,
    /// The field dropped, or kept as typed, a composition the input method still holds (a click, Escape): the input
    /// method is to start afresh. The window system acts on it once.
    pub reset: bool,
}

/// One batch from the input method, applied whole and in this order ([`TextBuffer::apply_ime`]): remove the shown
/// composition, delete around the selection, insert `commit`, then show `preedit`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct ImeBatch {
    pub preedit: Option<Preedit>,
    pub commit: Option<String>,
    /// Bytes to delete before and after the selection (the selection itself stays).
    pub delete: (u32, u32),
    /// Whether the input method had seen the field's latest state. A stale batch applies all the same, but the field
    /// does not answer it with its new state: the input method answers the newer one next.
    pub current: bool,
}

/// What the input method sends back. A batch cut short by `Leave` never arrives.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    Enter,
    Leave,
    Done(ImeBatch),
}

/// The raster identity of one glyph: everything that changes its pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct GlyphKey {
    pub font: FontId,
    pub glyph: GlyphId,
    /// Physical pixels per em, in 1/64 steps.
    pub size_q: u32,
    /// Horizontal subpixel phase, in 1/4 pixel steps.
    pub subpixel_x: u8,
    /// The stem darkening: 0 when font smoothing is off, otherwise 1 plus the fill's luminance in quarters (1 to 5).
    /// Build keys with [`GlyphKey::for_draw`], so equal draws share one key.
    pub dilation_q: u8,
}

impl GlyphKey {
    /// The key of a glyph drawn at `size_px` physical pixels per em, `x_fraction` pixels right of a whole pixel, in
    /// the sRGB fill `fill` (0 to 1, alpha ignored).
    pub fn for_draw(
        font: FontId,
        glyph: GlyphId,
        size_px: f64,
        x_fraction: f64,
        smoothing: bool,
        fill: [f32; 3],
    ) -> Self {
        let dilation_q = if smoothing {
            1 + (cupertino_glyph::quantized_luminance(fill) * 4.0).round() as u8
        } else {
            0
        };
        Self {
            font,
            glyph,
            size_q: (size_px * 64.0).round() as u32,
            subpixel_x: ((x_fraction * 4.0) as u8).min(3),
            dilation_q,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum GlyphFormat {
    /// One coverage byte per pixel, tinted by the run's colour.
    Coverage,
    /// Premultiplied RGBA, drawn as is (colour emoji, bitmap glyphs).
    ColorPremultiplied,
}

/// A rasterized glyph; immutable once made, so caches can share it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RasterGlyph {
    pub format: GlyphFormat,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// From the pen position on the baseline to the image's top-left corner, in physical pixels, y up.
    pub bearing_x: i32,
    pub bearing_y: i32,
    pub pixels: Arc<[u8]>,
}

pub trait GlyphRasterizer {
    /// `Ok(None)` for a glyph with no pixels, such as a space.
    fn rasterize(&mut self, key: &GlyphKey) -> Result<Option<RasterGlyph>, TextError>;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    NoFont,
    UnknownFont(FontId),
    /// Font data that is not a font, or has no face at the requested index; names the family.
    InvalidFont(Box<str>),
    ShapeFailed,
    /// A size or spacing that is not a finite number, or a size that is not positive.
    InvalidValue,
    NotACharBoundary(usize),
    /// Text with control characters where a password field takes none.
    InvalidText,
}
