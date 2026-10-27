# cupertino-text-field

The single-line text field model of the Cupertino apps (the Settings search field, the launcher's query, the file
manager's rename editor). Pure logic: no drawing and no Wayland. The app feeds key and pointer presses with
`Instant`s, draws with `cupertino-decorations::field`, and carries out the clipboard requests.

## Public API

| Item | Contents |
|---|---|
| `TextField` | The text, the caret and the selection anchor as byte offsets on grapheme boundaries; `key`, `press` / `drag_to` / `release`, `insert`, `paste`, `select_all`, `blur`, `undo` / `redo`, the composition range of an input method (`set_composition`, `commit_composition`, `cancel_composition`), `caret_visible` / `next_blink`, `boundaries` / `index_at` for hit-testing, `follow_caret` / `scroll` for text wider than the field |
| `Keymap` | The user's modifier profile (`keyboard.conf`, `modifier_profile`) and the standard editing chords with the user's overrides (`cupertino-keyboard`, `keybindings.conf`); `Keymap::load` in an app, `Keymap::standard` in tests |
| `KeyInput` | A key press: evdev code, physical modifiers, the character it types |
| `Response` | What a key did: `Ignored` (the window handles it: Return, Escape, Tab, a window shortcut), `Moved`, `Edited`, `Copy(text)`, `Cut(text)`, `Paste` |

## Behaviour

- **Keys.** The primary modifier is Super with the `cupertino` profile and Ctrl with `classic-pc`. Primary+A selects
  all, Primary+C / X / V copy, cut and paste, Primary+Z undoes and Primary+Shift+Z redoes. Every other chord goes
  through the standard binding table: Left / Right by grapheme, Alt+Left / Right by word, Primary+Left / Right and
  Home / End to the line ends, Shift extends, Backspace / Delete with Alt (word) and Primary (to the line start),
  and with the `cupertino` profile the Ctrl chords (Ctrl+A / E line ends, Ctrl+K kill, Ctrl+Y yank, Ctrl+T transpose).
  Up / Down go to the start / end; an app that uses them for a list handles them first. Home and End move the
  caret (the table's binding only scrolls, which a single line cannot).
- **Pointer.** A press places the caret (Shift extends from the anchor), a second press within 0.5 s at the same
  place selects the word (or the run of spaces or punctuation) under it, a third selects everything; a drag extends
  by characters or, after a double press, by words.
- **Undo.** Consecutive typing undoes as one step, consecutive deletion as another; a move, click, paste or cut
  starts a new step. Undo brings back the selection an edit replaced.
- **Caret.** It shows for 0.5 s after every edit, move or press, then blinks 0.5 s off and 0.5 s on (approximated: a
  design value, not measured); it hides while text is selected.
- **Length.** `TextField::with_max_len` caps the text in bytes; typing, a paste or a composition past the cap is cut at
  a grapheme boundary (the apps: 1 KiB for the search fields, 255 bytes for a file name).
- **Single line.** Pasted or composed line breaks and tabs become spaces; other control characters and the bidi
  embedding, override and isolate controls are dropped, so a paste holds only what typing could.
- **Other layouts.** With Ctrl or Super held, a key whose layout letter is not Latin is named by the key's Latin letter,
  so Super+A selects all on a Cyrillic layout.
- **Composition.** A composition is an undo step of its own; a cancelled one leaves no step.

Out of scope: the arrows move in logical order; visual-order movement for right-to-left text is not done.
