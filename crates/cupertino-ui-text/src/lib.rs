//! CupertinoKit text. Paragraphs are laid out from [`AttributedText`] into lines of [`GlyphRun`]s, positions are UTF-8
//! byte offsets on grapheme boundaries, and editing goes through an [`EditBuffer`] or, for passwords, a
//! [`SecureBuffer`]. A [`TextInputSession`] keeps the input method's composition consistent with the buffer by
//! revision.

use cupertino_geometry::Rect;
use std::fmt;
use std::ops::Range;
use std::sync::Arc;

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

#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    /// `None` uses the theme's font.
    pub family: Option<Arc<str>>,
    pub size: f32,
    pub weight: FontWeight,
    pub style: FontStyle,
    pub features: Vec<FontFeature>,
}

/// Text with styled byte ranges.
#[derive(Clone, Debug, PartialEq)]
pub struct AttributedText {
    pub text: String,
    pub runs: Vec<(Range<usize>, TextStyle)>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TextAlign {
    #[default]
    Start,
    Center,
    End,
    Justify,
}

/// The paragraph's base direction; `None` takes it from the first strong character.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextDirection {
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
    pub direction: Option<TextDirection>,
    pub max_lines: Option<u32>,
    pub wrap: Wrap,
}

/// A laid-out paragraph.
#[derive(Clone, Debug)]
pub struct Paragraph {
    _private: (),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PositionedGlyph {
    pub id: GlyphId,
    pub x: f32,
    pub y: f32,
    /// Byte offset in the source text of the cluster this glyph belongs to.
    pub cluster: usize,
}

/// Glyphs of one face at one size, positioned relative to the run's origin on the baseline.
#[derive(Clone, Debug, PartialEq)]
pub struct GlyphRun {
    pub font: FontId,
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

/// The editable text of a field, with its selection and undo history.
#[derive(Clone, Debug)]
pub struct EditBuffer {
    _private: (),
}

/// The editable text of a password field: zeroed when dropped, never printed, copied or exposed to accessibility.
pub struct SecureBuffer {
    _private: (),
}

impl fmt::Debug for SecureBuffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecureBuffer(..)")
    }
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
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Preedit {
    pub text: String,
    pub cursor: Option<Range<usize>>,
}

/// One focused field's conversation with the input method. Every buffer change bumps `revision`; an input method
/// event that answers an older revision resets the composition instead of applying.
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

/// What the window system sends to the compositor's input method.
#[derive(Clone, Debug, PartialEq)]
pub struct TextInputState {
    /// The text around the caret; `None` for a protected field.
    pub surrounding: Option<String>,
    pub cursor: usize,
    pub anchor: usize,
    pub purpose: ContentPurpose,
    pub hints: ContentHints,
    /// The caret in window coordinates, so the candidate window can sit next to it.
    pub cursor_rect: Rect,
}

/// What the input method sends back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImeEvent {
    Preedit {
        text: String,
        cursor: Option<Range<usize>>,
    },
    Commit(String),
    DeleteSurrounding {
        before: usize,
        after: usize,
    },
    /// Applies the events since the previous `Done`.
    Done {
        serial: u32,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextError {
    NoFont,
    NotACharBoundary(usize),
}
