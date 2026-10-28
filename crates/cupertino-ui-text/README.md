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
  undo, kill buffer, composition range, click counting, caret blink and scroll. It takes no key codes; the apps' adapter
  maps keys onto its methods.
- `Surrounding::around` costs the window, not the document; a text that is one huge run of regional indicators (flags)
  is the exception, because the boundary rule scans back over the run.
