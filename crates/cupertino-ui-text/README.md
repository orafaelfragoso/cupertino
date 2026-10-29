# cupertino-ui-text

CupertinoKit text: fonts and fallback, paragraph layout, grapheme caret and selection as UTF-8 byte offsets, the
editing buffer, the zeroized `SecureBuffer` and the input-method `TextInputSession`. MPL-2.0. Today the crate declares the
public types and implements the password buffer and the input-method rules below; shaping, layout and the editing
buffer follow.

- `SecureBuffer` keeps a password in one fixed block that is zeroed on drop, on every deletion or replacement, on
  `clear` and when it grows (the text is copied to a larger block and the old one is zeroed). It has no undo history,
  no clipboard access and no accessor except `with_secret`; the mask shows one bullet per grapheme.
- `Preedit`, `ImeBatch` and `Surrounding` print lengths in `Debug`, never the text.
- `TextInputSession::state` sends a protected field's own rectangle and no text; `Surrounding::around` cuts the
  bounded window around the caret on grapheme boundaries.
- `TextBuffer` is the single-line editing model that used to be `cupertino-text-field`'s: grapheme caret and selection,
  undo, kill buffer, composition range, click counting, caret blink and scroll. `Keymap` resolves a key (evdev code, modifiers
  physical through a profile or already logical, and the character) through `cupertino-key-bindings`' chords, and
  `TextFieldKeys` carries the result out on the buffer; neither reads settings, which `cupertino-keyboard` does.
- `Surrounding::around` costs the window, not the document; a text that is one huge run of regional indicators (flags)
  is the exception, because the boundary rule scans back over the run.
- `FontIndex` holds the faces the app registers from font bytes and resolves a family, weight and variation set to a
  stable `FontId`; `FontIndex::shape` shapes one run with `harfrust` (visual order, byte clusters, advances in points).
- `TextSystem` is the shared entry point (`&self` methods, one `Arc` for measuring and painting): `paragraph` shapes an
  `AttributedText` per style run and hard line and caches the result, `measure` lays it out for a size. A family
  no face matches falls back to the first registered face. The cache keeps two generations of 512 paragraphs, a size
  or spacing that is not finite (or a size of zero or less) is `TextError::InvalidValue`, and variation values are
  kept in quarter units so an animated axis shares instances.
- `Paragraph::layout(width)` re-wraps the shaped text without reshaping (UAX 14 breaks from `icu_segmenter`; a word
  wider than the width breaks between clusters; spaces at a soft wrap hang past the line). `ParagraphLayout` gives
  lines with glyph runs, `caret_rect`, `index_at` (always a grapheme boundary) and `selection_rects`. Layout is
  logical-order left to right for now (`max_lines` of zero keeps one line, a width that is not finite means no limit); bidi, script fallback and system fonts follow.
- Timing, release, medians of 15 (`cargo test --release -p cupertino-ui-text -- --ignored --nocapture timing`; the
  shape figure is cold and includes loading the font and the break data): 200 characters shape 66 us, reflow 4 us;
  5000 characters shape 530 us, reflow 125-150 us; a cache hit costs under 1 us.
