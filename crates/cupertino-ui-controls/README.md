# cupertino-ui-controls

The standard CupertinoKit controls: `Text`, `Icon`, `Button`, `Toggle`, `TextField`, `SecureField`, `Stack`, `Spacer`,
`Separator` and the settings form controls so far. MPL-2.0. Each control reads every colour and metric from the theme tokens, takes its value
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
  `Sidebar::with_look(SidebarLook::Neutral { accent })` draws a list of places instead: taller rows with larger
  symbols, the selected row filled with a neutral tint, its label semibold and its symbol in the accent colour; the
  other rows' symbols are in the accent colour in the sections `accent` marks and in the secondary label colour in the
  rest. The default look (`SidebarLook::Accent`, Settings') fills the selected row with the accent.
- **Settings form.** `FormGroup` sets rows on a rounded fill with a hairline between each, inset from both edges;
  `FormRow` is a row of it, at least `FORM_ROW_HEIGHT` tall (`FORM_TILE_ROW_HEIGHT` with an `IconTile`, a white
  symbol on a rounded square of a `TileColor`), and its content takes the rest of the row. `NavigationRow` is one Tab
  stop that opens a page: a tile, the label and a trailing chevron; a click, Return, Space or assistive technology's
  Click sends `on_open`, and without it the row reads as disabled. `FormHeader` is the card at the top of a page, its
  large tile over a bold title and a centred description, read as the title described by the description.
  `FormBackground` fills its room with the colour behind the groups. `SearchField` is a `TextField` drawn as a capsule
  with a magnifying glass; it edits and reads as its field does. `BareField` is a `TextField` with no chrome, its
  text and placeholder at its own size (whole points), for a field standing on a surface drawn around it such as a
  launcher capsule; it edits and reads as its field does (`tests/bare_field.rs`).
- **Tiled sidebar.** `TileSidebar` lists a settings window's pages: rows of `TILE_SIDEBAR_ROW_HEIGHT`, a tile before
  each label, in groups set apart by space, under an optional header (a `SearchField`) that does not scroll. The
  selected row fills with the selected control colour, its label bold. Keys, Tab stop, reporting and assistive
  technology behave as `Sidebar`'s.
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
- **Window shell and toolbar.** `SidebarWindow` lays out a window opened with `DecorationMode::Sidebar`: the sidebar
  panel from the window's top edge to its bottom edge, its 52 pt top (`SIDEBAR_TOP`) left free for the compositor's
  window control buttons and a drag region of the window as a toolbar is, then an optional header slot
  (`sidebar_header`, for a search field) and the sidebar's content; beside it, past a `SplitView` divider drawn as a rim one device pixel wide on the sidebar's
  side over the content's colour (no separator line), the content
  pane paints the window background up to the window's top edge, so the window paints none. The panel meets the
  window's edges (`SIDEBAR_PANEL_INSET` 0); an inset panel would take the window's 16 pt corner less the inset. The
  floating toolbar controls are 36 pt tall in the middle of the 52 pt row, 8 pt from its edges and 8 pt apart:
  `ToolbarGroup` is a capsule of segments 36.5 pt wide (40 pt for a choice, whose chosen segment shows a pill 4 pt
  inside it, its radius the capsule's less that inset by `nested_radius`), with 20 pt dividers between segments that
  ask for them except beside the chosen one; a choice reads as a radio group of radio buttons, other segments as
  buttons, a segment with a menu as a pop-up button that opens on the press; a group with an empty label adds no
  group to the tree. `ToolbarMenuButton` is a 47 pt capsule of a symbol and a chevron, `ToolbarButton` a 36 pt circle,
  `ToolbarSearch` a search field that rests as such a circle and passes the press to the field under it, and
  `ToolbarTitle` the 15 pt bold title that takes the room the controls leave. `NavigationStack` puts its back and
  forward buttons in such a group. Glyphs recede to the secondary label colour in an inactive window and to the
  disabled colour when disabled.
- **Glass.** Each glass surface names a `GlassRole` (`ToolbarGroup`, `SidebarPanel`) and `GlassRole::material` says
  what draws it: the sidebar panel asks for the compositor's sidebar material under the node, with the surface's
  opaque fill where the window cannot show it; a toolbar group lies over the window's own content, which a material
  under the window cannot show through, so it paints a translucent body with a one-pixel rim (and a soft shade in the
  light appearance; opaque with Reduce Transparency or Increase Contrast). Moving a role to another material is its
  one arm in `GlassRole::material`.
- **Menus.** `Menu` levels follow the pointer as the menu bar's do (`cupertino-menu-model`'s `tracking`): the row
  under the pointer takes the focus and the highlight, a submenu opens once the pointer rests 0.2 s on its row and
  closes once it rests 0.3 s on another, and while the pointer heads from a row into its open submenu (each move inside
  the triangle between the last point and the submenu's near edge, without a 0.3 s rest) the rows it crosses wait. A
  press that opened the menu (a secondary press, a press on a `PopUpButton`, which opens its menu on the press) and is
  let go over an item chooses it, once, if the pointer reached an item or the press was held 0.5 s; a quicker click
  leaves the menu open. The row a context menu opens under the pointer (one the compositor slid to fit) is not reached
  until the pointer goes on to another. Typed letters find a row as `cupertino-typeahead` does, per level, and Return chooses it;
  Escape closes one level and the focus goes back to its row, or to where the menu opened. The panel takes the
  submenu's near edge from where the compositor placed its window (`EventCx::popup_placement`), so a submenu flipped to
  the leading side, or slid, keeps its triangle; until the compositor has placed it, the edge is where the panel
  asked for it, beside its row on the trailing side. The 0.5 s of a held press count from the press
  (`EventCx::opening_press`), however late after it the application opens the menu, and from when the panel first
  hears of anything only for a menu no held press opened.
  `MenuItem::Shortcut` is an item that shows the keys that choose it ("⇧⌘N") at its trailing edge in the secondary
  label colour (the row's colour when it is highlighted or disabled), which assistive technology reads as its
  description; the application binds the keys itself.
  The panel has a 10 pt corner. A row is 24 pt tall and a separator 11 pt. Labels start 15 pt in, or 31 pt when any row of the menu is checked
  (the check mark is centred 17 pt in); the keys and a submenu's chevron end 20 pt from the trailing edge. A
  separator runs from the label column to 15 pt short of the trailing edge, translucent over the glass and the
  theme's separator colour over the opaque fill and with Increase Contrast. Over the glass the compositor draws the
  panel's edge; over the opaque fill the panel strokes its own.
- **Popovers.** A popover is glass with a 16 pt corner, like a menu: the compositor draws its body and edge, and
  over the opaque fill (no material) it strokes its own edge.
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
