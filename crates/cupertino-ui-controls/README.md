# cupertino-ui-controls

The standard CupertinoKit controls: `Text`, `Icon`, `Button`, `Toggle`, `TextField`, `SecureField`, `Stack`, `Spacer`,
`Separator`, the choice, value and picker controls and the settings form controls so far. MPL-2.0. Each control reads every colour and metric from the theme tokens, takes its value
from the application's model and reports changes as messages.

- **Arming.** A pointer or touch press arms a button or switch, release over it activates once, release elsewhere
  disarms. Leaving the surface, losing the focus or a touch cancel disarms without activating. Touch has no hover.
  Space arms and activates on release; Return activates a button on press, never a switch; a key repeat never arms
  and Escape disarms Space.
- **Look.** Disabled overrides every other state and pressed overrides hover; a switch that is on resolves its
  colours as selected. Every control keeps its states apart: enabled or disabled, focused, selected, pressed or at
  rest, and in an active window or not. A disabled control is fainter (its bezel and its label); a control in a window
  that is not active keeps its look, but for a default button, which rests as an ordinary one there
  (`tests/contract.rs`). An ordinary button, a pop-up or pull-down button, a segmented control and a stepper rest on
  a quiet bezel with no edge (white at 7.5 % over a dark window, black at 7.5 % over a light one, stronger under the
  pointer and pressed); a check box, a radio button, an off switch and the empty part of a slider or progress track
  on a slightly stronger well. A default button is the accent; a destructive one a red label over a faint red tint
  (a red fill with a light label under Increase Contrast). `ButtonStyle::Glass` is an action on glass of its own: a
  translucent body with a fine bright rim, toolkit-painted since the compositor's glass lies under the window's
  opaque content. `Toggle::small` gives a `SmallToggle`, the switch of a settings form's rows: a 36 × 16 pt track
  and a 21 × 13 pt capsule thumb 1.5 pt inside it, with the toggle's behaviour and reading. In a high-contrast
  appearance every bezel and well strokes its border and every quiet fill is a little stronger. A theme without a colour for a role paints
  nothing rather than a guess. The keyboard focus
  ring is drawn only for a keyboard-visible focus, outside the chrome, without changing layout.
- **Scroll views.** `ScrollView` moves its content with `cupertino-ui-core`'s scroll motion. A touchpad finger or a
  touch drags it, stretches it past an edge and leaves kinetic motion at the lift (none under Reduce Motion; a stretch
  still returns). Each movement counts at the time its event happened, not when it is handled, so movements
  delivered together keep their spacing and the lift's speed is measured as the finger moved; an event without a time
  of its own counts as now. A wheel, a tilted wheel and a continuous source step it inside the range at once, with no stretch
  and no inertia of their own. The model's offset never leaves the range: the stretch is drawn on top of it. A gesture
  belongs to the view its first movement can move, or for a finger to the view it pulls past an edge along an axis
  that view scrolls; the content coasts on while the pointer goes elsewhere, and a new finger catches it where it is.
- **Targets.** Chrome is centred in a node padded out to the theme's minimum target, so the whole padded node takes
  the press and nothing overlaps a neighbour. The layout never follows the paint: a held thumb's lens, a press or a
  focus ring draws past the chrome without moving anything.
- **Sizes.** Each family takes its height, width, corners, label size and inset from `cupertino-theme`'s control
  size table (`control_geometry`) at the theme's `ControlMetrics::size` and `compact` setting: mini, small, regular,
  large and extra large. `ControlMetrics::at_size` gives the metrics at another size. At the regular size a button,
  a pop-up button, a field and a segmented control are 24 pt tall, a check box 16 pt square and a switch 54 × 24 pt
  with a 32 × 20 pt capsule thumb (38 × 20 pt in the compact setting).
- **Held thumbs.** While a switch is held or dragged its thumb becomes a lens about 1.6 times its width, and a dragged
  slider's knob one about 1.33 times its size: a capsule (a rounded rectangle for the knob) with a soft shade, a
  translucent body through which the track shows shrunk about the lens's centre, a light band along its top and
  bottom, a bright rim and a fine dark outline. At rest both thumbs are plain; buttons, check boxes and fields never
  take a lens. Increase Contrast strengthens the rim and outline, Reduce Transparency makes the body nearly opaque,
  and Reduce Motion leaves the lens as it is: it is a state, not a motion.
- **Dragging.** A switch's thumb follows a drag along its track once the pointer moves 3 pt; let go over the switch,
  it takes the side the thumb is nearer, and let go away from it the drag is cancelled. A segmented control dragged
  from one segment across the others keeps the chosen segment marked and shows the one under the pointer pressed;
  the release chooses that one, and a release off the control chooses nothing.
- **Fields.** `TextField` edits through `cupertino-ui-text`'s chords (the binding table of
  `cupertino-key-bindings`, no settings or file watching) with the platform's logical modifiers. Edits report the
  committed text with a revision; the field takes the model's text only at or past its last reported revision, so a
  model that lags is not undone. An input method's composition never reaches the model, the accessible value or a
  snapshot. Return submits unless a composition holds it, and Escape cancels a composition first and asks the input
  method to start afresh. A click, losing the focus or the window, and the input method leaving keep the composition
  as typed (committed and reported) and reset the input method. The composition draws a thin underline at the same
  distance below the baseline at every scale, the clause the input method is converting a heavier one and no caret,
  and otherwise a caret inside the composition where the input method puts it. The input method learns the committed
  text around the caret and the caret's rectangle. The caret and the composition's underlines are the field's overlay
  (`Widget::overlay`): drawn over its paint with every edge on a device pixel of the window wherever the field lies,
  so they are crisp at every scale, fractional ones included, and the caret a whole number of device pixels wide. The
  selection lies under the text on whole device pixels of the window too (`PaintCx::fill_pixels`), as does the
  field's interior clip, so its edges meet the caret and the field without a blended row or column
  (`tests/caret-overlay.rs`). The caret blinks in `caret_phase_ms` phases, each blink
  drawing only the overlay again, no node, and stays on with Reduce Motion or a phase of 0 (a
  session that asks for a steady caret). Shift-click and shift-drag extend the selection; a drag held past the field's
  edge (an editor's top or bottom) keeps scrolling and selecting every 50 ms by the distance past the edge until the
  text ends; the caret scrolls into view after every edit and move.
- **Passwords.** `SecureField` keeps its text in a `SecureBuffer` only: no undo, copy, cut or other command chord, no
  surrounding text for the input method, a protected accessible value, and the secret reaches the application only
  borrowed, in the submit handler. A composition is never held; only committed text goes in. The input method is
  asked for hidden, sensitive text with completion and spell checking off and is given the field's rectangle, never
  the caret's. A click puts the caret between bullets, shift-click and dragging select, and a double click selects all. `busy` and `invalid`
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
  rest. In a window that is not active the selected row's symbol takes the secondary label colour as well, so it still
  stands out from the neutral selection. The default look (`SidebarLook::Accent`, Settings') fills the selected row
  with the accent.
- **Settings form.** `FormGroup` sets rows on a rounded fill with a hairline between each, inset from both edges;
  `FormRow` is a row of it, at least `FORM_ROW_HEIGHT` tall (`FORM_TILE_ROW_HEIGHT` with an `IconTile`, a white
  symbol on a rounded square of a `TileColor`), and its content takes the rest of the row. `NavigationRow` is one Tab
  stop that opens a page: a tile, the label and a trailing chevron; a click, Return, Space or assistive technology's
  Click sends `on_open`, and without it the row reads as disabled. `FormHeader` is the card at the top of a page, its
  large tile over a bold title and a centred description, read as the title described by the description.
  `FormBackground` fills its room with the colour behind the groups: the compositor draws it
  (`Surface::TextBackground`) where the desktop can, below the window's glass, so a toolbar's capsules over a form
  show, and it paints it where the desktop cannot. A `TextField`'s chrome is a recessed well: darker than what it
  sits on in the dark appearance (black at 25 %), the text background in the light one, with a faint edge (the
  theme's stronger border under Increase Contrast). `SearchField` is a `TextField` with a magnifying glass
  about 12.5 pt across, 10.5 pt from its leading edge, and its text 29.5 pt in; on the window's content it is a text field's recessed well, on a
  material (`PaintCx::on_material`: a sidebar, a popover, glass) a light capsule (white at 23 % in the dark
  appearance, 45 % in the light one, with a faint rim; the theme's border under Increase Contrast). It edits and reads
  as its field does. Its height is the size table's for a search field at its `size`, the window's when `None`
  (24 pt at the regular size), 28 pt at `ControlSize::Large`, the size a sidebar's header uses
  (`tests/search-field.rs`). The other fields (plain, secure, combo box, date picker) keep their well on a material. `BareField` is a `TextField` with no chrome, its
  text and placeholder at its own size (whole points), for a field standing on a surface drawn around it such as a
  launcher capsule; it edits and reads as its field does (`tests/bare_field.rs`).
- **Tiled sidebar.** `TileSidebar` lists a settings window's pages: rows of `TILE_SIDEBAR_ROW_HEIGHT`, a tile before
  each label, in groups set apart by space, under an optional header (a `SearchField`) that does not scroll. The
  selected row fills with the selected control colour, its label bold. Keys, Tab stop, reporting and assistive
  technology behave as `Sidebar`'s, except that a press or click on the selected row reports it again, so that a
  window showing a page below that row's page can go back to it.
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
- **Selection inset.** A list's selection and focus ring sit 4 pt in from a row's sides; `ListView::inset_element`
  builds the list with another inset, for a list whose selection spans more of its column.
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
- **Dialogs.** `Modal` shows its content and, while it has a `Dialog`, dims the content with the scrim, takes every
  press meant for it, hides it from assistive technology and the Tab order, and hangs the dialog 52 pt below the
  window's top (under the toolbar row), centred across. Return answers with the default button and Escape with the
  cancel one once the focused control passes them on; Tab and Shift+Tab stay inside; every other key stops there. The
  first control of the body takes the focus, else the least destructive button (Cancel, then the default, then
  another). A `DialogKind::Dialog` is a sheet: 340 pt wide, solid in the window colour with a 16 pt radius, a soft
  shadow, a dark hairline outside its edge and, in the dark appearance, a faint light rim inside it; the title, the
  message, the body and a trailing row of buttons, 20 pt in. A `DialogKind::Alert` is 260 pt wide, a nearly opaque
  grey (97 %, opaque under Reduce Transparency and Increase Contrast) with a light rim, a 26 pt radius and the
  hairline: the application's icon (`Icons::application`, 64 pt)
  22 pt in from the top and the leading side, the title in bold body text below it, the message 10 pt under that, then
  28 pt capsule buttons 16 pt from the sides and the bottom. Two buttons whose labels fit half the row (reckoned from
  the character count) go side by side at equal widths in their given order; otherwise they are stacked full width,
  8 pt apart, in the reverse order, so the default is on top. The default capsule takes the accent, the others a quiet
  fill; a destructive answer has a red label (a red capsule in high contrast). The alert is not glass: `PaintCx::glass`
  is drawn by the compositor under the window's surface, so over the window's own opaque content it would not show,
  and the alert is part of that surface. Glass would need the alert on a surface of its own above the window, which
  Kit's popups are not fit for: a popup without a grab gets no keyboard, one with a grab needs the serial of a press
  (an alert raised by a timer or a task has none) and is dismissed by a press outside, and no anchor hangs one centred
  under the toolbar row. Dialogs show and go at once: the window draws no motion for them, since a fade keyed to the
  clock would leave them unseen in every test harness whose clock stands still.
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
  pane's window background reaches the window's top edge, so the window paints none; the compositor draws it
  (`Surface::WindowBackground`) where the desktop can, so the toolbar's glass over it reads the window, and the pane
  paints it where the desktop cannot. The panel meets the
  window's edges (`SIDEBAR_PANEL_INSET` 0); an inset panel would take the window's 16 pt corner less the inset. The
  floating toolbar controls are 36 pt tall in the middle of the 52 pt row, 8 pt from its edges and 8 pt apart:
  `ToolbarGroup` is a capsule of segments 36.5 pt wide (40 pt for a choice, whose chosen segment shows a pill 4 pt
  inside it, its radius the capsule's less that inset by `nested_radius`), with 20 pt dividers between segments that
  ask for them except beside the chosen one; a choice reads as a radio group of radio buttons, other segments as
  buttons, a segment with a menu as a pop-up button that opens on the press; a group with an empty label adds no
  group to the tree. `ToolbarMenuButton` is a 47 pt capsule of a symbol and a chevron, `ToolbarButton` a 36 pt circle,
  `ToolbarSearch` a search field that rests as such a circle and passes the press to the field under it, and
  `ToolbarTitle` the 15 pt bold title that takes the room the controls leave. `NavigationStack` puts its back and
  forward buttons in such a group. With `under_toolbar` its page fills the whole pane and the row lies over it (the
  row takes the pointer first); the page lays out below the row with `unobscured_insets(mode)`, the top the window's
  chrome covers (52 pt for the unified and sidebar modes, 40 pt compact, none for a title bar above the content), so a
  `ScrollView` whose content starts that far down scrolls behind the row. The compositor draws a window's glass and
  materials below everything the window paints, so opaque content scrolled under the row covers the row's glass:
  Files and Settings keep the page below the row. Glyphs recede to the secondary label colour in an inactive window and to the
  disabled colour when disabled. Besides symbols a toolbar item may draw a glyph of its own (`ToolbarGlyph`): the
  back and forward chevrons, `More`'s three dots and `Tag`, a tag outline pointing up and to the trailing side with its
  hole by the point, mirrored right to left.
- **Glass.** Each glass surface names a `GlassRole` (`ToolbarGroup`, `SidebarPanel`, `GlassButton`, `ThumbLens`) and `GlassRole::material` says
t; a toolbar group is regular glass (`PaintCx::glass`) in one container
  per window (`GlassRole::container`, blending only when two controls come closer than the 8 pt gap between them),
  kept by the compositor with Reduce Transparency and Increase Contrast, over the content pane's background, which the
  compositor draws below it; where the desktop shows no glass it paints a translucent body with a one-pixel rim (and a
  soft shade in the light appearance). The field under a resting `ToolbarSearch` button is not drawn, so the button's
  glass shows what lies below the field. Moving a role to another material is its one arm in `GlassRole::material`.
  `GlassRole::floating` tells the floating controls (toolbar groups, glass buttons, a held thumb) from the content
  surfaces (the sidebar panel). Glass is never laid on glass: a node on glass the window shows
  (`PaintCx::on_glass`) draws none of its own, so a glass button on a toolbar group shows its label alone.
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
  hears of anything only for a menu no held press opened. A press dragged over the menus and let go away from all of
  them chooses nothing and closes them; one that never came over them leaves them open. Escape closes them at once,
  mid-drag too, and a pop-up button keeps its value. None of this waits on how a menu appears: it tracks alike with
  Reduce Motion (`tests/menu-tracking.rs`).
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
- **Pickers.** `PullDownButton` is titled with what it does and opens a menu of actions below itself on the press,
  with a single downward chevron where a `PopUpButton` has two opposing ones; its menu marks nothing.
  `ColorWell` shows a colour in a capsule set in a quiet rim, sends `on_open` on a press, Space or Return, and reads as
  its colour (`#RRGGBB`). `DatePicker` shows a `CalendarDate` as `YYYY-MM-DD` in a field's well with a stepper
  beside it; a click or Left and Right pick the year, month or day, and Up, Down and the stepper step that part,
  wrapping within its range and fitting the day to the month. `ComboBox` is an editable field in a well with a small
  bezel button at its end that opens a menu of choices below it (built with `Element::from`); the field takes any
  text and its value is marked in the menu (`tests/pickers.rs`).
- **Sliders.** `on_change` reports every value a drag passes; `on_end` reports the value once the change ends, when
  the pointer lets go or the drag is cancelled, and at once for a key or assistive technology step, so an
  application can write a level live and commit it once. A regular knob is a white capsule 1.25 times as wide as
  it is high with a soft shade; with Increase Contrast it is ringed in the control border colour, so it stands apart
  from a track of its own lightness. The empty part of the track is a quiet well, and `SliderTrack` (3:1 or more
  against the surface under it) under Increase Contrast; a panel level slider's is `PanelTrack`; the filled part is
  the selected `ControlFill` (the accent).

- **File preview.** `FilePreview` draws a file without opening it (`PreviewContent`): a picture whole and as large as
  fits, the start of a text file in a fixed-pitch face (`text_family`, the theme's face without one) on the text
  background, scrolled by the application's `ScrollState`, or the file's icon. `PreviewStyle::Pane` is the column a
  file browser puts after its lists: the content in a 200 pt box, then the name, the summary (kind and size) and the
  labelled details, clipped above an optional footer pinned to the bottom edge behind a hairline. `PreviewStyle::Window`
  fills a preview window with the content alone, a file shown by its icon keeping its name, summary and details beside
  it. It reads as a group named "Preview of <name>"; a picture reads by the file's name and text by its text.

## Not yet

Text shaping and glyph rasterization are not in `cupertino-ui-text` yet, so `Text`, `Icon`, the label of a button
or switch, a field's text, placeholder, caret and selection, pointer caret placement and paste are not drawn or
wired. A button's width holds only its insets until its label can be measured. A default button's Return comes from
the window's shortcuts for now.
