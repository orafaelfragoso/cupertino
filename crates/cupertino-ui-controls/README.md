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
- **Fields.** `TextField` edits through `cupertino-ui-text`'s chords (the binding table of
  `cupertino-key-bindings`, no settings or file watching) with the platform's logical modifiers. Edits report the
  committed text with a revision; the field takes the model's text only at or past its last reported revision, so a
  model that lags is not undone. An input method's composition never reaches the model, the accessible value or a
  snapshot. Return submits unless a composition holds it, and Escape cancels a composition first.
- **Passwords.** `SecureField` keeps its text in a `SecureBuffer` only: no undo, copy, cut or other command chord, no
  surrounding text for the input method, a protected accessible value, and the secret reaches the application only
  borrowed, in the submit handler. A composition is never held; only committed text goes in. `busy` and `invalid`
  give assistive technology the field's busy and invalid states, which the application sets while a password is checked
  and after it is refused. `max_len` caps the text in bytes: text past it is cut at a grapheme boundary and a key with
  no room left is refused without an edit. Build the field with `..SecureField::default()` for the fields left out.
- **Sidebar.** `Sidebar` lays out sections, each an optional heading over rows of an icon and a label, and keeps one
  row selected, counted over every section. The rows share one Tab stop; Up, Down, Home and End cross the sections,
  and a selection that does not change is not reported again. Assistive technology reads a list with a group per
  heading and a selected list item per row. A row is keyed by its item's key, so its focus and accessible object stay
  with it while rows come and go around it. A row's accessory (an eject button) offers a click only with
  `on_accessory`; without it the accessory reads as disabled. A busy row turns an activity indicator in the accessory's
  place, standing still under Reduce Motion. In a window that is not active the rows' icons take the secondary label
  colour.
- **Positions.** A menu item says its place among the menu's items and the menu says how many it holds, separators not
  counted; a tab and a page dot say their place and their tab list says its count.
- **Outline table.** `OutlineTable` is a `TableView` whose rows form an outline, as a `TreeView`'s do: `item` gives
  each row's level and whether it is open, its first cell comes after the indent and the disclosure triangle, and the
  triangle, Right and Left ask `on_toggle` to open or close a row (Right on an open row and Left on a child move to its
  first child and to its parent). `key` keys each row, so its focus and accessible object follow it while rows open
  above it; without it a row is keyed by its index. A plain `TableView` is unchanged. Assistive technology selects a
  row of a list, table or tree alone with its Click action, and opens and closes a tree or outline row with Expand and
  Collapse.
- **Row keys.** A `TableView`'s `on_row_key` hears each key that reached its rows and that the table did not take
  itself, with the row the keyboard is on; a message takes the key and `None` lets it go on, so Tab moves to the next
  control. A key on a column header never comes there, so an application can give Tab in the rows its own meaning while
  Tab and Shift+Tab still walk the headers in the order shown. `None` keeps the table as it was.
- **Truncation.** A `Text` that does not wrap, and a tooltip's tip past its 300 pt text width, end in an ellipsis
  where they are cut; the accessible name keeps the whole text.
- **Rich text.** `RichText` draws its links in the link colour over a 1 px underline 1.5 pt below the baseline. Each
  link is a tab stop in text order that Return, a click on its text or assistive technology opens; it reads as a link
  named by its text, with its URL as the description.
- **Images.** `Image` fits its picture whole (`Contain`, the default) or fills its box and crops (`Cover`), rounded only
  when given a radius, and draws the smallest rendition that covers its device pixels, else the largest. Without a
  label it is decorative and hidden from assistive technology.
- **Canvas and grid.** `Canvas` draws through the application's closure, fills the room offered unless it has a size,
  and reads as a group whose interactive parts are its children, layered over the drawing. `Grid` lays out fixed,
  auto and fraction tracks row by row; `Element::span` covers several cells, and rows past the declared ones fit
  their content.
- **Window drag regions.** `WindowDragArea` marks its content as a drag region of the window, and the toolbar row of
  `NavigationStack` is one, so every Kit app with a navigation toolbar gets it. A left press there that no control
  takes starts an interactive move (`WindowRequest::Move`, answering the press) once the pointer travels more than
  5 pt along either axis with the button held; a shorter wobble stays a click. A second press within 0.5 s of the
  first zooms the window (`WindowRequest::Maximize`, the opposite of the last configured state), the same action the
  compositor's own title bar takes on a double click; no setting in `cupertino-defaults` changes the double click
  today. A move ends a pair, so the press after it is a first click. Buttons, fields and other enabled controls stop
  their presses, so they keep them; a label, a spacer or a disabled control lets the press through to the region.
  Touch, right and middle presses start nothing, and a fullscreen window neither moves nor zooms. The region has no
  role and no Tab stop: keyboard users zoom with the compositor's Zoom shortcut (Super+Ctrl+Z by default) or the menu
  bar's Window menu, tile and fill with its shortcuts, and can drag the server's title bar with the pointer as always.
- **Menus.** `Menu` levels follow the pointer as the menu bar's do (`cupertino-menu-model`'s `tracking`): the row
  under the pointer takes the focus and the highlight, a submenu opens once the pointer rests 0.2 s on its row and
  closes once it rests 0.3 s on another, and while the pointer heads from a row into its open submenu (each move inside
  the triangle between the last point and the submenu's near edge, without a 0.3 s rest) the rows it crosses wait. A
  press that opened the menu (a secondary press, a press on a `PopUpButton`, which opens its menu on the press) and is
  let go over an item chooses it, once, if the pointer reached an item or the press was held 0.5 s; a quicker click
  leaves the menu open. The row a context menu opens under the pointer (one the compositor slid to fit) is not reached
  until the pointer goes on to another. Typed letters find a row as `cupertino-typeahead` does, per level, and Return chooses it;
  Escape closes one level and the focus goes back to its row, or to where the menu opened. The panel places the
  submenu's edge where it asked the compositor to put it, beside its row on the trailing side: a submenu the compositor
  flips to the other side keeps no triangle.
- **Sliders.** `on_change` reports every value a drag passes; `on_end` reports the value once the change ends, when
  the pointer lets go or the drag is cancelled, and at once for a key or assistive technology step, so an
  application can write a level live and commit it once. With Increase Contrast the knob's ring (the control border
  colour, 3:1 against the switch thumb's colour) is twice as wide, so the knob stands apart from a track of its own
  lightness. The empty part of the track is `SliderTrack` (a panel level slider's is `PanelTrack`), 3:1 or more
  against the surface under it in every palette; the filled part is the selected `ControlFill` (the accent).

## Not yet

Text shaping and glyph rasterization are not in `cupertino-ui-text` yet, so `Text`, `Icon`, the label of a button
or switch, a field's text, placeholder, caret and selection, pointer caret placement and paste are not drawn or
wired. A button's width holds only its insets until its label can be measured. A default button's Return comes from
the window's shortcuts for now.
