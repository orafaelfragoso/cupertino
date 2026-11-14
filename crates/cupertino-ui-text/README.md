# cupertino-ui-text

CupertinoKit text: fonts and fallback, paragraph layout, grapheme caret and selection as UTF-8 byte offsets, the
editing buffer, the zeroized `SecureBuffer` and the input-method `TextInputSession`. MPL-2.0. Today the crate declares the
public types and implements the password buffer and the input-method rules below; shaping, layout and the editing
buffer follow.

- `SecureBuffer` keeps a password in one fixed block that is zeroed on drop, on every deletion or replacement, on
  `clear` and when it grows (the text is copied to a larger block and the old one is zeroed). It has no undo history,
  no clipboard access and no accessor except `with_secret`; the mask shows one bullet per grapheme. `with_max_len` and
  `set_max_len` cap it in bytes: an insertion past the cap is cut at a grapheme boundary, and text already past a new,
  lower cap stays.
- `Preedit`, `ImeBatch` and `Surrounding` print lengths in `Debug`, never the text.
- `TextInputSession::state` sends a protected field's own rectangle and no text; `Surrounding::around` cuts the
  bounded window around the caret on grapheme boundaries. `TextInputSession::protected_hints` are a protected field's
  hints: hidden, sensitive text with completion, spell checking and capitalisation off. `TextInputState::reset` asks
  the window system to enable the input method afresh, so it drops a composition the field ended itself.
- `TextBuffer::apply_ime` applies one input-method batch in the protocol's order: the composition is taken away, the
  bytes around the selection are deleted (cut inward to character boundaries, the selection kept, caret and anchor
  then on grapheme boundaries), the commit replaces the selection as typing (a commit or a kept composition that joins marks after it leaves the caret
  after the whole cluster), and the new composition shows in its
  place with its own cursor (`composition_cursor`, moved back to a character boundary inside the composition, hidden
  when the input method hides it). A batch that answers an older state still applies; the field just does not answer
  it. `TextBuffer::surrounding` is the committed text around the caret, without the composition, with the caret where
  the composition starts. A press while composing keeps the composition as typed; a shift press extends from the
  anchor; a third press selects the paragraph with its line break in an editor, everything in a field.
  `SecureBuffer::select_masked` selects by grapheme index, so a click lands between bullets.
- `TextBuffer` is the single-line editing model of Kit's fields and the apps' text fields: grapheme caret and selection,
  undo, kill buffer, composition range, click counting, caret blink and scroll. `Keymap` resolves a key (evdev code, modifiers
  physical through a profile or already logical, and the character) through `cupertino-key-bindings`' chords, and
  `TextFieldKeys` carries the result out on the buffer; neither reads settings, which `cupertino-keyboard` does. Left
  and Right move to the next caret stop on screen through text of either direction, each hard line in the direction it
  is laid out in, and past a line's edge on in that line's reading order (`TextBuffer::caret_position` keeps
  the side of a direction change the caret is on, and `TextBuffer::set_direction` takes the base direction the field
  lays the text out in). Option+Left and Option+Right move by word the same way, to the drawn edge of the next word on
  that side; Backward, Forward, the word moves by those names and the deletes keep the text's order.
- `Surrounding::around` costs the window, not the document; a text that is one huge run of regional indicators (flags)
  is the exception, because the boundary rule scans back over the run.
- `FontIndex` holds the faces the app registers from font bytes and resolves a family, weight and variation set to a
  stable `FontId`; `FontIndex::shape` shapes one run with `harfrust` (visual order, byte clusters, advances in points).
  `set_system_fonts` gives it the installed fonts: `InstalledFonts` asks Fontconfig, loaded at run time, so the
  font configuration picks the face per script and language; without the library only the registered faces draw.
  A run that must not depend on the machine's fonts sets `FONTCONFIG_FILE` to a configuration of its own, as
  `tests/fontconfig.rs` does; Kit's golden tests build their own font index and never see installed fonts.
  Kit apps set it. `tests/installed.rs` shapes Arabic, Devanagari and Hebrew with Noto Sans subsets (OFL-1.1).
- `TextSystem` is the shared entry point (`&self` methods, one `Arc` for measuring and painting): `paragraph` shapes an
  `AttributedText` per style run and hard line and caches the result, `measure` lays it out for a size. A family
  no face matches is looked up among the installed fonts, else falls back to the first registered face. A character
  the run's face lacks is drawn by the first other face, in load order, that has its whole grapheme cluster (joiners
  and variation selectors do not decide), then by the installed face the system picks for its first character and
  the style's language, loaded once; white space stays in the face before it, and a cluster no face has shows the
  missing glyph. A cluster with the
  emoji presentation selector (U+FE0F) takes the first colour face (COLR, CBDT or sbix) that has it, the run's face
  included, before any face without colour, so ❤️ is the colour heart even when the text face has a heart; without
  the selector the run's face keeps it. The cache keeps two generations of 512 paragraphs (`cached_paragraphs`
  counts them; text shaped ahead of the frame that lays it out cannot grow it past them), a size
  or spacing that is not finite (or a size of zero or less) is `TextError::InvalidValue`, and variation values are
  kept in quarter units so an animated axis shares instances.
- `Paragraph::layout(width)` re-wraps the shaped text without reshaping (UAX 14 breaks from `icu_segmenter`; a word
  wider than the width breaks between graphemes, never inside one even where shaping splits it, as in a conjunct; spaces at a soft wrap hang past the line). `ParagraphLayout` gives
  lines with glyph runs, `caret_rect`, `index_at` (always a grapheme boundary) and `selection_rects` (`max_lines` of
  zero keeps one line, a width that is not finite means no limit). With `ParagraphStyle::ellipsis`, text past
  `max_lines` or wider than the width ends its last line in an ellipsis (U+2026, or three full stops where the font
  lacks it) at the end edge, in the font of the last character kept, after the characters that fit; the ellipsis is a
  run of its own with an empty range at the cut. Bidi follows UAX 9 (`unicode-bidi`): the base
  direction is `ParagraphStyle::direction` or the first strong character's (`Paragraph::direction`); without a set
  direction each hard line takes its own first strong character's (UAX 9 P1–P3), and a line without one keeps the
  text's. Each run of one
  embedding level is shaped in its direction, and each line is reordered for display, with the spaces that end it at
  the paragraph's level so they hang past its end edge. `Start` and `End` follow the base direction; `Left` and
  `Right` hold in either. Positions stay logical: a downstream caret sits at the start edge of the character after it and an upstream one at the end edge of
  the character before it, `index_at` returns the side a click lands on, and a selection across a direction change can
  be several rectangles on one line. `tests/corpus.rs` pins the caret stops,
  glyph order, wrap points and hit testing of Latin, combining marks, Hebrew, Arabic, Devanagari, ZWJ emoji, flags and
  digits inside right-to-left text.
- `GlyphRaster` implements `GlyphRasterizer` over a `TextSystem`'s fonts with `swash` and `cupertino-glyph`: unhinted
  coverage with the stem darkening that `GlyphKey::dilation_q` names (0 is font smoothing off, 1 to 5 is the fill's
  luminance in quarters; build keys with `GlyphKey::for_draw`, and the size the dilation reads is floored to a power of
  1.25), and colour glyphs as premultiplied RGBA without dilation.
- Timing, release, medians of 15 (`cargo test --release -p cupertino-ui-text -- --ignored --nocapture timing`; the
  shape figure is cold and includes loading the font and the break data, and the grouping of glyphs into clusters
  that every layout reuses): 200 characters shape 85-105 us, reflow 4-5 us; 5000 characters shape 940-980 us, reflow
  87-90 us; a cache hit costs under 1 us. A caret move lays out only the caret's hard line and the lines on either
  side: in a 10,000-line text an arrow costs 11-14 us and a word move 17-18 us, and on one 92,000-character line an
  arrow costs 3.2 ms and a word move 10.5 ms.
