# cupertino-ui-controls

The standard CupertinoKit controls: `Text`, `Icon`, `Button`, `Toggle`, `TextField`, `SecureField`, `Stack`, `Spacer`
and `Separator` so far. MPL-2.0. Each control reads every colour and metric from the theme tokens, takes its value
from the application's model and reports changes as messages.

- **Arming.** A pointer or touch press arms a button or switch, release over it activates once, release elsewhere
  disarms. Leaving the surface, losing the focus or a touch cancel disarms without activating. Touch has no hover.
  Space arms and activates on release; Return activates a button on press, never a switch; a key repeat never arms
  and Escape disarms Space.
- **Look.** Disabled overrides every other state and pressed overrides hover; a switch that is on resolves its
  colours as selected. In a high-contrast appearance every button that fills its chrome also strokes its border. A
  theme without a colour for a role paints nothing rather than a guess. The keyboard focus
  ring is drawn only for a keyboard-visible focus, outside the chrome, without changing layout.
- **Targets.** Chrome is centred in a node padded out to the theme's minimum target, so the whole padded node takes
  the press and nothing overlaps a neighbour.
- **Fields.** `TextField` edits through `cupertino-text-field`'s chords (the binding table of
  `cupertino-key-bindings`, no settings or file watching) with the platform's logical modifiers. Edits report the
  committed text with a revision; the field takes the model's text only at or past its last reported revision, so a
  model that lags is not undone. An input method's composition never reaches the model, the accessible value or a
  snapshot. Return submits unless a composition holds it, and Escape cancels a composition first.
- **Passwords.** `SecureField` keeps its text in a `SecureBuffer` only: no undo, copy, cut or other command chord, no
  surrounding text for the input method, a protected accessible value, and the secret reaches the application only
  borrowed, in the submit handler. A composition is never held; only committed text goes in.
- **Sidebar.** `Sidebar` lays out sections, each an optional heading over rows of an icon and a label, and keeps one
  row selected, counted over every section. The rows share one Tab stop; Up, Down, Home and End cross the sections,
  and a selection that does not change is not reported again. Assistive technology reads a list with a group per
  heading and a selected list item per row. A row is keyed by its item's key, so its focus and accessible object stay
  with it while rows come and go around it.

## Not yet

Text shaping and glyph rasterization are not in `cupertino-ui-text` yet, so `Text`, `Icon`, the label of a button
or switch, a field's text, placeholder, caret and selection, pointer caret placement and paste are not drawn or
wired. A button's width holds only its insets until its label can be measured. A default button's Return comes from
the window's shortcuts for now.
