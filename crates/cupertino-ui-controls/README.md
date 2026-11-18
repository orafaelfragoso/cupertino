# cupertino-ui-controls

The standard CupertinoKit controls: `Text`, `Icon`, `Button`, `Toggle`, `TextField`, `SecureField`, `Stack`, `Spacer`,
`Separator`, the choice, value and picker controls and the settings form controls so far, with type-to-select (`typeahead`) and the model of a file chooser (`filepicker`). MPL-2.0. Each control reads every colour and metric from the theme tokens, takes its value
from the application's model and reports changes as messages.

- **Arming.** A pointer or touch press arms a button or switch, release over it activates once, release elsewhere
  disarms. Leaving the surface, losing the focus or a touch cancel disarms without activating. Touch has no hover.
  Space arms and activates on release; Return activates a button on press, never a switch; a key repeat never arms
  and Escape disarms Space.
- **Look.** Disabled overrides every other state and pressed overrides hover; a switch that is on resolves its
  colours as selected. Every control keeps its states apart: enabled or disabled, focused, selected, pressed or at
  rest, and in an active window or not. A disabled control is fainter (its bezel and its label); a control in a window
  that is not active keeps its look, but for a default button, which rests as an ordinary one there
 . An ordinary button, a pop-up or pull-down button, a segmented control and a stepper rest on
  a quiet bezel with no edge (white at 7.5 % over a dark window, black at 7.5 % over a light one, stronger under the
  pointer and pressed); a check box, a radio button, an off switch and the empty part of a slider or progress track
  on a slightly stronger well. A default button is the accent; a destructive one a red label over a faint red tint
  (a red fill with a light label under Increase Contrast). `ButtonStyle::Glass` is an action on glass of its own: a
  translucent body with a fine bright rim, toolkit-painted since the compositor's glass lies under the window's
  opaque content. `Toggle::header` gives a `HeaderToggle`, the switch of a pane's header (a 44.5 × 20 pt track, a 25.5 × 15 pt thumb). `Toggle::small` gives a `SmallToggle`, the switch of a settings form's rows: a 36 × 16 pt track
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
- **Overlay scroll bars.** An overlay bar flashes when its content first has something to scroll (shown 1 s), shows
  while the content moves and for 0.5 s after, then fades out over 0.25 s; a hide held back by the pointer on the thumb
  or a drag of it waits 0.3 s once the hold ends. The pointer on a shown thumb widens it after 0.05 s over 0.125 s.
  Under Reduce Motion the bar disappears without a fade or a widening.
- **Pop-ups in rows.** `InlinePopUpButton` wraps a `PopUpButton` for a grouped form's row: the value as plain text,
  then a 20 pt round grey disc with the up and down chevrons (darker under the pointer and pressed). `PopUpButton`
  keeps its bordered bezel elsewhere. `FullWidthSegmentedControl` stretches a segmented control's equal segments
  over its row. `AppIconRow` is a form row that starts with an application's icon (`AppIcon`, by the name of its
  desktop entry, through `Icons::named`), 20 or 22 pt in a 42 or 43 pt row (53 pt with a second line); `PaneHeader` is
  the compact header of a pane (26 pt tile, title, description and an optional master switch); `ThumbnailTile` is a
  picture of a grid of pictures with its caption and, when chosen, a neutral ring.
- **Targets.** Chrome is centred in a node padded out to the theme's minimum target, so the whole padded node takes
  the press and nothing overlaps a neighbour. The layout never follows the paint: a held thumb's lens, a press or a
  focus ring draws past the chrome without moving anything.
- **Sizes.** Each family takes its height, width, corners, label size and inset from `cupertino-theme`'s control
  size table (`control_geometry`) at the theme's `ControlMetrics::size` and `compact` setting: mini, small, regular,
  large and extra large. `ControlMetrics::at_size` gives the metrics at another size. At the regular size a button,
  a pop-up button, a field and a segmented control are 24 pt tall, a check box 16 pt square (a label longer than its
  column wraps beside it, the box on its first line) and a switch 54 × 24 pt
  with a 32 × 20 pt capsule thumb (38 × 20 pt in the compact setting). A tab view's segments are a segmented
  control's height and a disclosure button is the table's `ControlFamily::Disclosure` square (20 pt at regular);
  no control reads another table of sizes.
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
  `cupertino-keyboard`, no settings or file watching) with the platform's logical modifiers. Edits report the
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
 . The caret is 2 pt wide and blinks in a cycle of two `caret_phase_ms` phases less a fiftieth (about a second):
  it fades out over about an eighth of it, stays hidden, fades in and stays shown about half of it; each frame of a fade
  draws only the overlay again, no node, and the caret stays on with Reduce Motion or a phase of 0 (a
  session that asks for a steady caret). Shift-click and shift-drag extend the selection; a drag held past the field's
  edge (an editor's top or bottom) keeps scrolling and selecting every 50 ms by the distance past the edge until the
  text ends; the caret scrolls into view after every edit and move. The paste chord inserts the clipboard's text in
  place of the selection (`EventCx::request_paste`), line breaks as spaces in a single-line field.
- **A field's menu.** Every `TextField`, `SearchField`, `BareField`, `TextEditor` and `SecureField` opens a menu of its
  own (`src/text_menu.rs`, through `EventCx::open_popup`; the application declares nothing): Cut, Copy, Paste, a
  separator and Select All, at the pointer on a secondary click, under the caret on the Menu key or Shift+F10, and
  where a finger rests 0.5 s within 8 pt. It is a `Menu` like any other: the same panel, rows, keys and roles, named
  "Edit". A row is enabled only when it can act: Cut and Copy with a selection, Paste with text on the clipboard,
  Select All with text in the field. A secondary click inside the selection keeps it, one outside puts the caret there
  first; the field takes the keyboard as on any press, a composition stays as typed, and the selection outlives the
  menu. The rows run the field's editing commands, so a cut or a paste reports one edit and is one step to undo. A
  disabled field is read only: neither Cut, Paste nor Select All, and Copy copies all its text. A `SecureField`'s menu
  never enables Cut or Copy, and a `BareSecureField`, which stands on the surfaces that ask for the session's
  password, has no menu and takes no paste; a `BareField` has both unless its `menu` is off, as the login screen's
  name field's is.
- **Passwords.** `SecureField` keeps its text in a `SecureBuffer` only: no undo, copy, cut or other command chord but
  select all and paste, no surrounding text for the input method, a protected accessible value, and the secret reaches the application only
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
  place, standing still under Reduce Motion. In a window that is not active every row's icon and label, the selected
  row's too, and the headings and accessories take the inactive sidebar text colour (`ColorRole::InactiveSidebarText`)
  and the selection turns to the sidebar's faint neutral selection (`ColorRole::SidebarSelection` resolved inactive),
  laid over the material so its tint shows; the geometry and the text stay.
  `Sidebar::with_look(SidebarLook::Neutral { accent })` draws a list of places instead: taller rows with larger
  symbols, the selected row filled with the sidebar's neutral selection (`ColorRole::SidebarSelection`), its label
  semibold and its symbol in the accent colour; the
  other rows' symbols are in the accent colour in the sections `accent` marks and in the secondary label colour in the
  rest. In a window that is not active every symbol, the selected row's too, takes the inactive sidebar text colour. The default look (`SidebarLook::Accent`, Settings') fills the selected row
  with the accent.
- **Settings form.** `FormGroup` sets rows on a rounded fill with a hairline between each, inset from both edges;
  `FormRow` is a row of it, at least `FORM_ROW_HEIGHT` tall (`FORM_TILE_ROW_HEIGHT` with an `IconTile`, a white
  symbol on a rounded square of a `TileColor`), and its content takes the rest of the row. `NavigationRow` is one Tab
  stop that opens a page: a tile, the label and a trailing chevron; a click, Return, Space or assistive technology's
  Click sends `on_open`, and without it the row reads as disabled. `FormHeader` is the card at the top of a page, its
  large tile over a bold title and a centred description, read as the title described by the description.
  `FormBackground` fills its room with the colour behind the groups, the window background (the groups keep their own
  fill): the compositor draws it (`Surface::WindowBackground`) where the desktop can, below the window's glass, so a
  toolbar's capsules over a form show, and it paints it where the desktop cannot. A `TextField`'s chrome is a recessed well: darker than what it
  sits on in the dark appearance (black at 25 %), the text background in the light one, with a faint edge (the
  theme's stronger border under Increase Contrast). `SearchField` is a `TextField` with a magnifying glass
  about 12.5 pt across, 10.5 pt from its leading edge, and its text 29.5 pt in; on the window's content it is a text field's recessed well, on a
  material (`PaintCx::on_material`: a sidebar, a popover, glass) a light capsule (white at 23 % in the dark
  appearance, 45 % in the light one, with a faint rim; the theme's border under Increase Contrast). It edits and reads
  as its field does. Its height is the size table's for a search field at its `size`, the window's when `None`
  (24 pt at the regular size), 28 pt at `ControlSize::Large`, the size a sidebar's header uses
 . The other fields (plain, secure, combo box, date picker) keep their well on a material. `BareField` is a `TextField` with no chrome, its
  text and placeholder at its own size (whole points), for a field standing on a surface drawn around it such as a
  launcher capsule; it edits and reads as its field does. With `hold_caret` its caret holds
  what it shows: the blink's cycle goes on but draws nothing, and a paint for any other reason draws the caret as it
  was last drawn, for a field whose surface the compositor is moving.
- **Tiled sidebar.** `TileSidebar` lists a settings window's pages: rows of `TILE_SIDEBAR_ROW_HEIGHT`, a tile before
  each label, in groups set apart by space, under an optional header (a `SearchField`) that does not scroll. The
  selected row fills with the selected control colour, its label bold. In a window that is not active every label, the
  selected row's too, takes the inactive sidebar text colour and the selection turns to the sidebar's faint neutral
  selection; the tiles keep their colours. Keys, Tab stop, reporting and assistive
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
- **Press catchers.** `PressCatcher` is the window a shell panel puts under itself, over a whole output, to hear a press
  outside the panel: it asks for no size (its surface's role gives it one), is shown as one transparent colour without
  a frame (`WindowBackground::INPUT_ONLY`) and sends its message for a left or right press anywhere on it. The press
  stops there, so nothing under the window hears it; other buttons, motion and scrolling say nothing, and it has no
  node for assistive technology. The control panel and the notification centre build theirs with `Window::from`.
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
  side over the content's own background, which the content pane lays under the line so the strip takes the content's
  tint (no separator line and no fill of its own), the content
  pane's window background reaches the window's top edge, so the window paints none; the compositor draws it
  (`Surface::WindowBackground`) where the desktop can, so the toolbar's glass over it reads the window, and the pane
  paints it where the desktop cannot. A drag on the divider keeps the sidebar between `min_sidebar` and `max_sidebar`
  and never leaves the content under `min_content`, which wins over the sidebar's maximum in a narrow window; dragged
  below half its minimum the sidebar collapses, and a drag back out from the edge or Return on the divider restores
  it. The panel meets the
  window's edges (`SIDEBAR_PANEL_INSET` 0); an inset panel would take the window's 16 pt corner less the inset. The
  floating toolbar controls are 36 pt tall in the middle of the 52 pt row, 8 pt from its edges and 8 pt apart:
  `ToolbarGroup` is a capsule of segments 36.5 pt wide (40 pt for a choice, whose chosen segment shows a pill 4 pt
  inside it, its radius the capsule's less that inset by `nested_radius`), with 20 pt dividers between segments that
  ask for them except beside the chosen one; a choice reads as a radio group of radio buttons, other segments as
  buttons, a segment with a menu as a pop-up button that opens on the press; a group with an empty label adds no
  group to the tree. `ToolbarMenuButton` is a 47 pt capsule of a symbol and a chevron, `ToolbarButton` a 36 pt circle,
  `ToolbarSearch` a search field that rests as such a circle and passes the press to the field under it, and
  `ToolbarTitle` the 15 pt bold title that takes the room the controls leave, on one line that ends in an ellipsis
  when it does not fit (nothing at all when not even the ellipsis does), so the row keeps its height. Each control's glass gets an
  outline from the toolkit: a grey rim on its outermost device pixel (26 % black at the sides, 7 % at the top and bottom)
  and a faint shade below it in the light appearance, a dark rim along its sides (60 % black) in the dark one. `NavigationStack` puts its back and
  forward buttons in such a group. With `under_toolbar` its page fills the whole pane and the row lies over it (the
  row takes the pointer first); the page lays out below the row with `unobscured_insets(mode)`, the top the window's
  chrome covers (52 pt for the unified and sidebar modes, 40 pt compact, none for a title bar above the content), so a
  `ScrollView` whose content starts that far down scrolls behind the row. The row is a raised node
  (`Widget::raised`): where the window system has a raised layer it draws over the page with its glass between the two,
  so the page shows through the glass and never covers it, and a scroll view under it draws the scroll edge over
  what it covers (`scroll_edge`): a soft veil of the window background, shown only once the page is scrolled, 96 % at the top of the
  band thinning in a straight line to 72 % at its bottom, with a hairline of 0.5 pt along the bottom, or, with Reduce Transparency and in high contrast, the band filled with
  the window background and a separator along its bottom. In a window that is not active the band is the inactive
  title bar colour (`ColorRole::InactiveTitlebar`), solid with a hard edge and no veil, with the separator under it
  where the active band has one. Its bar, paging and focus scrolling keep below the row.
  Without a layer the row draws in place, the page is clipped below it and there is no edge. A `TableView` with
  `header_top` puts its header that far down, raised with the row, and starts its rows below both; the rows scroll
  under the two, and since the header's titles are text outside glass the band under them is always the solid one,
  the header drawing its line. `ListView::under_element(inset, lead)` starts a list's rows `lead` points down so they
  scroll under the row. Files and Settings fill their panes this way. Glyphs and the title recede to the tertiary label colour (`ColorRole::TertiaryText`) in an inactive window and to the
  disabled colour when disabled. Besides symbols a toolbar item may draw a glyph of its own (`ToolbarGlyph`): the
  back and forward chevrons, `More`'s three dots and `Tag`, a tag outline pointing up and to the trailing side with its
  hole by the point, mirrored right to left.
- **Glass.** Each glass surface names a `GlassRole` (`ToolbarGroup`, `SidebarPanel`, `GlassButton`, `ThumbLens`) and
  `GlassRole::material` says what draws it; a toolbar group is toolbar glass (`Glass::toolbar`, through `GlassRole::glass`) in one container
  per window (`GlassRole::container`, blending only when two controls come closer than the 8 pt gap between them),
  kept by the compositor with Reduce Transparency and Increase Contrast, over the content pane's background, which the
  compositor draws below it; where the desktop shows no glass it paints a translucent body with a one-pixel rim (and a
  soft shade in the light appearance). At rest the toolkit outlines a toolbar group's glass (above); with Increase Contrast
  in the light appearance it lays a firmer rim over any floating control's glass: a dark rim on the outermost device
  pixel, 50 % black at the sides and 30 % at the top and bottom. A glass button is regular glass of its own (container 0) where that glass can show,
  its body filling its node, without Reduce Transparency: on a material the window shows (a sidebar, a translucent
  window background), and over the window's own content, as the button is a raised node (`Widget::raised`) whose glass
  lies between the window's raised layer and its content, so the content shows through it; its label then takes the
  colours of text on glass. Where the window system has no raised layer, with Reduce Transparency and where the
  desktop shows no glass it paints its body and rim. A held thumb's lens lies over the
  track the window paints, so it stays painted. A light divider is 40 % black, as strong over
  the glass as the disabled glyphs beside it. The field under a resting `ToolbarSearch` button is not drawn, so the button's
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
  until the pointer goes on to another. Typed letters find a row as `typeahead` does (*Type-to-select*), per level, and Return chooses it;
  Escape closes one level and the focus goes back to its row, or to where the menu opened. The panel takes the
  submenu's near edge from where the compositor placed its window (`EventCx::popup_placement`), so a submenu flipped to
  the leading side, or slid, keeps its triangle; until the compositor has placed it, the edge is where the panel
  asked for it, beside its row on the trailing side. The 0.5 s of a held press count from the press
  (`EventCx::opening_press`), however late after it the application opens the menu, and from when the panel first
  hears of anything only for a menu no held press opened. A press dragged over the menus and let go away from all of
  them chooses nothing and closes them; one that never came over them leaves them open. Escape closes them at once,
  mid-drag too, and a pop-up button keeps its value. None of this waits on how a menu appears: it tracks alike with
  Reduce Motion. A finger does the same: put down on a `PopUpButton` it opens the menu,
  slid onto an item and lifted it chooses it, tapped it leaves the menu open, and a later tap on an item chooses. A
  finger that rests 0.5 s within 8 pt of where it went down on a `ContextMenu`'s region opens its menu there, and the
  content under it is not clicked. A second finger chooses nothing while the first holds the menu, and a cancelled
  touch chooses nothing and leaves the menu open.
  `MenuItem::Shortcut` is an item that shows the keys that choose it ("⇧⌘N") at its trailing edge in the secondary
  label colour (the row's colour when it is highlighted or disabled), which assistive technology reads as its
  description; the application binds the keys itself.
  The panel has a 10 pt corner. A row is 24 pt tall and a separator 11 pt. Labels start 15 pt in, or 31 pt when any row of the menu is checked
  (the check mark is centred 17 pt in); the keys and a submenu's chevron end 20 pt from the trailing edge. A
  separator runs from the label column to 16.5 pt short of the trailing edge, translucent over the glass and the
  theme's separator colour over the opaque fill and with Increase Contrast. Over the glass the compositor draws the
  panel's edge; over the opaque fill the panel strokes its own.
  A chosen row's highlight goes off for 0.034 s and on for 0.067 s before its messages are sent (at once with Reduce
  Motion), and the menu takes no input meanwhile; the menu then fades out when the application stops declaring it
  (`cupertino-ui-core`'s runtime). Rows are 24 pt, labels 16.5 pt in, the highlight inset 5.5 pt with a 6.75 pt corner,
  the panel's corner 11.5 pt, a submenu overlaps its menu by 5.5 pt, and shortcuts take 0.28 of the label's strength.
- **Popovers.** A popover is glass with a 16 pt corner, like a menu: the compositor draws its body and edge, and
  over the opaque fill (no material) it strokes its own edge.
- **Pickers.** `PullDownButton` is titled with what it does and opens a menu of actions below itself on the press,
  with a single downward chevron where a `PopUpButton` has two opposing ones; its menu marks nothing.
  `ColorWell` shows a colour in a capsule set in a quiet rim, sends `on_open` on a press, Space or Return, and reads as
  its colour (`#RRGGBB`). `DatePicker` shows a `CalendarDate` as `YYYY-MM-DD` in a field's well with a stepper
  beside it; a click or Left and Right pick the year, month or day, and Up, Down and the stepper step that part,
  wrapping within its range and fitting the day to the month. `ComboBox` is an editable field in a well with a small
  bezel button at its end that opens a menu of choices below it (built with `Element::from`); the field takes any
  text and its value is marked in the menu.
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

`BarChart` draws a series of vertical bars (a recorded level over time) with value labels and guide lines at its right and time labels along its bottom; bars can be set apart by a dimmer shade. A `Swatch` can also mock a window at one step of a row from larger text to more room. `Swatch` is a choice drawn as a picture: a 66 by 45 point mock of the light, dark or automatic look with its name under it, or a 22 point dot of a colour (or of every accent colour in turn), with a ring in the accent colour when chosen. A scroll bar with `ScrollClick::JumpToSpot` moves the knob's middle to a click in the track and carries on as a drag. A `TileSidebar` row can be an account row (a 64 point row with the person's picture or initials and a second line), and its icon size follows the sidebar icon size choice (rows 28, 32 or 36 points, tiles 16, 20 or 24).

## Type-to-select

`typeahead::TypeAhead` keeps the text typed so far and finds the next item whose title starts with it, ignoring case.

- A pause of one second (`typeahead::RESET_AFTER`) starts a new search.
- One typed character looks after the highlighted item, so repeating it steps through the items that share it.
- Two or more characters search from the highlighted item, wrapping. When no title starts with the same letter typed
  again (`cc`), it steps on to the next item that letter starts, as one character does.
- Items that cannot be chosen (separators, disabled rows) are passed as `None` and never match.
- A space is part of the text while a search is under way (`typing`); the caller treats it as a choice otherwise.

Titles and the typed text are compared as they are, lowercased: there is no Unicode normalization, so a decomposed accent
(`e` and a combining acute) does not match a precomposed one (`é`); a caller that needs the match normalizes the titles
and the text first.

Used by `Menu`, the file chooser model below, Settings' choice menus and the menu bar (through `cupertino-kit`).

## File chooser model

`filepicker` is the model of a file chooser, with no drawing, for every window that lets the user pick a folder or a
picture: the Settings wallpaper pane and the file-chooser portal.

- The picker never reads the filesystem itself, so a folder on a hung mount cannot freeze a window. `Picker::new(start, mode)`
  lists nothing; a change of folder makes a `Request` (`take_request`) that a worker runs (`Request::run`, the only place
  the disk is read) and whose `Listing` goes back to `apply`; until then the picker shows what it had and `loading()` is
  true. An answer for an older request is dropped. `Picker::open(start, mode)` is the same with the answers read in place
  (`settle`), for callers with nothing to keep responsive.
- `Mode::Folder` shows folders, `Mode::Photo` shows folders and PNG and JPEG
  files, `Mode::Program` shows folders and the files someone may execute, `Mode::File` shows folders and every file (the file-chooser portal filters them itself). Folders come first, then files, each by name without regard to case. Names that start with a dot are hidden
  until `set_hidden(true)`.
- `Picker::key(key, now)` takes the keys of the chooser: Up, Down, Home and End move the highlight (no wrap); Return
  enters the highlighted folder or chooses the highlighted file; Super+Return (`Key::Choose`) chooses (in `Folder`
  mode the highlighted folder, else the open one); Super+Up (`Key::Parent`) opens the parent folder; Escape cancels; a
  typed character jumps to the next entry whose name starts with the text typed so far (*Type-to-select*).
  `Picker::go(dir)` opens a folder of a sidebar; `filepicker::places::session_places(home)` lists the home folder and the
  user directories of `user-dirs.dirs` that exist.
- A folder that cannot be read leaves the picker where it was and sets `unreadable`, which the view words plainly.
- The result is `Outcome::Choose(path)`, `Cancel`, `Changed` (redraw) or `Ignore`.

## Not yet

Text shaping and glyph rasterization are not in `cupertino-ui-text` yet, so `Text`, `Icon`, the label of a button
or switch, a field's text, placeholder, caret and selection and pointer caret placement are not drawn or
wired. A button's width holds only its insets until its label can be measured. A default button's Return comes from
the window's shortcuts for now.

## File views (proposal)

The design of W4.8 (D8 in `docs/plans/layered-architecture.md`), for the owner's approval: what the file family of
this crate is, what stays in Files, how its slow work runs, and the steps that get there. Nothing here is in the code
until its step lands. [Decisions for approval](#decisions-for-approval) is the short list.

### What shapes it

- Files' views emit Files' own message (`Message::Window(id, Event)`, 62 events) and its column view reads the app's
  session and window. A view of this crate knows neither, so the views do not come over with their bodies as they are.
- A `shell` crate cannot depend on `cupertino-files`. What the desktop component (W6.10) and the dialogs component
  (W6.11) share with Files is in Kit before they start, and it starts no thread: in the shell host a thread is the
  host's to start, so that its panic is the component's.
- `kit::task` and its pools are in `cupertino-kit`, above this crate, so nothing here starts a task. The family gives
  blocking functions and futures, and the program runs them (`kit::task::io`, `Command::Perform`), as it runs a
  `filepicker::Request`.
- An application's message is `Clone + Send`, so it cannot carry an open folder or a watch. A model keeps the queue
  its work fills, and the message only says there is something to `poll`.

### The family

`cupertino_ui_controls::files` holds the views, the values they show and the operations behind them. Kit names it as
`kit::files`, item by item, under the rules of its *Public API*: what Files, the desktop code and the portal name,
and the types those signatures name. `kit::filepicker` and `kit::file_icons` keep their paths.

### The views

A view is a widget with public fields, generic over the program's message `M`, as every control of this crate is. It
takes plain values (the items, the selection, the scroll state, a function for an item's icon and one for the field a
name is edited in), never a model, a session or a window. The three report through one event type and one function.

```rust
/// What a file view reports. The program gives each view one function that turns an event into its message.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewEvent {
    /// The selection a press, a key, the band or typing made.
    Select(Selection),
    /// Open item `index`: a double click or Return.
    Open(usize),
    /// A secondary press on item `index`, or on empty space.
    SecondaryPress(Option<usize>),
    /// Open the context menu at this point of the window.
    ContextMenu(Point),
    /// A press on items of the grid went far enough to drag them.
    DragOut(Grab),
    /// A press on rows of the list, or of the shown folder's column, went far enough to drag them.
    DragRows(RowGrab),
    /// The rubber band being drawn, in the view's content; `None` once it is let go.
    Band(Option<Rect>),
    /// The view scrolled: its scroll view, or a band held past its edge.
    Scroll(ScrollState),
    /// A press on the name of the only selected item had no second press: edit the name of item `index`.
    Rename(usize),
    /// Tab chose row `index` of the list.
    FocusRow(usize),
    /// A column header of the list was chosen.
    Sort((usize, SortOrder)),
    /// A folder's row of the list opens or closes.
    Toggle(TreeToggle),
    /// The column view's own events.
    Column(ColumnEvent),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ColumnEvent {
    /// The column of this folder scrolled.
    Scroll(PathBuf, ScrollState),
    /// Row `index` of the column of `folder`, a folder other than the one shown, was chosen; `secondary` when a
    /// secondary press chose it, whose context menu follows.
    Pick { folder: PathBuf, index: usize, secondary: bool },
    /// Right (1) or Left (-1) in the column of the folder shown.
    Step(isize),
    /// A divider was dragged to make column `index` `width` wide; `all` with Option held.
    Resize { index: usize, width: f64, all: bool },
    /// The divider was let go.
    Resized,
}

/// The folder as a grid of icons with their names: the content of the program's scroll view. Its fields are the ones
/// Files' grid has, and `on_event`; its body differs only where it emits.
pub struct IconGrid<M> {
    /// Tells this view's keys from those of the program's other windows.
    pub window: u64,
    pub label: String,
    pub layout: Rc<IconLayout>,
    pub params: IconParams,
    pub items: Rc<[Entry]>,
    pub icon: ItemIcon, // Rc<dyn Fn(&Entry) -> Vec<ImageSource>>
    pub scroll: ScrollState,
    pub selection: Selection,
    /// The items a context menu is open on, which wear a ring.
    pub ringed: Selection,
    pub band: Option<Rect>,
    /// How far down its pane the first item starts at rest: what a toolbar row covers.
    pub lead: f64,
    /// What sits under item `index`'s icon, given `(index, selected)`: its `name`, or the field it is renamed in.
    pub name: Rc<dyn Fn(usize, bool) -> Element<M>>,
    pub names: Option<NameShape>,
    pub revision: u64,
    pub strip: bool,
    pub on_event: Rc<dyn Fn(ViewEvent) -> M>,
}

/// The folder as an outline table: a header, a row an item with its icon and name first, folders that open in place,
/// the band and the presses of its rows.
pub struct FileList<M> {
    pub window: u64,
    pub label: String,
    /// The rows' items in order: an open folder's items follow its row.
    pub items: Rc<[Entry]>,
    /// Each row's place in the outline; a row past its end is a top-level row without children.
    pub outline: Rc<[TreeItem]>,
    /// The columns, the name's first: a table's `Column`, a title and a width.
    pub columns: Vec<Column>,
    /// The text of an item in column `column`, from 1; the name's cell is the item's icon and name.
    pub text: Rc<dyn Fn(&Entry, usize) -> String>,
    pub sort: Option<(usize, SortOrder)>,
    pub icon: ItemIcon,
    pub multiple: bool,
    pub scroll: ScrollState,
    pub selection: Selection,
    pub ringed: Selection,
    pub band: Option<Rect>,
    /// From the pane's top to the header at rest.
    pub lead: f64,
    /// The row whose name is being edited, and the field that stands in its place; the field's messages are the
    /// program's own.
    pub name_field: Option<(usize, Rc<dyn Fn() -> Element<M>>)>,
    /// The presses its rows and its frame share: the program keeps one a window between views.
    pub presses: SharedPresses,
    pub on_event: Rc<dyn Fn(ViewEvent) -> M>,
}

/// A chain of folders side by side in a row that scrolls sideways, a divider after each column that resizes it.
pub struct FileColumns<M> {
    pub window: u64,
    /// The columns in order, each as wide as the program lays it out.
    pub columns: Vec<(FileColumn<M>, f64)>,
    /// The chain's sideways scroll, reported as `ViewEvent::Scroll`; a column's own is `ColumnEvent::Scroll`.
    pub scroll: ScrollState,
    pub lead: f64,
    pub icon: ItemIcon,
    pub on_event: Rc<dyn Fn(ViewEvent) -> M>,
}

pub enum FileColumn<M> {
    /// The folder the program shows: its selection, its rename and its presses, as a list's rows have them.
    Shown {
        folder: PathBuf,
        label: String,
        items: Rc<[Entry]>,
        scroll: ScrollState,
        selection: Selection,
        name_field: Option<(usize, Rc<dyn Fn() -> Element<M>>)>,
        presses: SharedPresses,
    },
    /// Another folder of the chain; the row of the chain's next folder is highlighted.
    Other { folder: PathBuf, label: String, items: Rc<[Entry]>, scroll: ScrollState, highlighted: Option<usize> },
    /// A folder whose items have not come yet.
    Empty { folder: PathBuf },
    /// What ends the chain, built by the program: the preview of the selected file. It has no divider.
    Preview(Element<M>),
}
```

`FileList` and `FileColumns` become elements with `Element::from`. Public beside them: `name` and `laid_out_name` (an
item's name as the grid draws and lays it out, which the desktop's items use), `NameShape`, `ItemIcon`, `Grab`,
`RowGrab`, `Presses` and `SharedPresses` (opaque), and the keys `cell_key`, `cell_index`, `row_key` and `column_key`.
The widgets the views are made of (cells, name boxes, row cells, the band, the click and rename timing, the column
frame and its divider) are private.

Files keeps what is its own around them: the scroll view and the kept layouts around the grid, the gallery, the drag
images and commands (`dragged`), the drop targets, the chain's listings and navigation (`ColumnsView`, the `columns`
chain), the preview pane and window, the name field. Its `Event` and its `update` do not change: one function maps
the family's events onto the events it has.

```rust
// cupertino-files
fn view_events(id: u64) -> Rc<dyn Fn(ViewEvent) -> Message> {
    Rc::new(move |event| {
        let event = match event {
            ViewEvent::Select(selection) => Event::Select(selection),
            ViewEvent::Rename(index) => Event::RenameItem(index),
            ViewEvent::Column(ColumnEvent::Step(by)) => Event::ColumnStep(by),
            // one arm an event, each to the `Event` Files has for it
        };
        Message::Window(id, event)
    })
}

// the chooser, once its rows are a `FileList` (W6.11): the four messages it has
Rc::new(move |event| match event {
    ViewEvent::Select(rows) => Message::Select(id, rows),
    ViewEvent::Open(row) => Message::Open(id, row),
    ViewEvent::Scroll(scroll) => Message::Scroll(id, scroll),
    _ => Message::Ignore,
})
```

### Where Files' modules go

To the family goes what a view names, what the desktop component needs, and what those two stand on. What only Files
uses stays a module of Files (D32).

| Files' module | Goes to `files` as | Because |
|---|---|---|
| `entry`, `natural` | the same | the views' item and the order of names |
| `sort`, `mime`, `tags`, `applications`, `recents` | the same | the listing sorts, types and tags its items and lists the two virtual folders |
| `model` | the same, without threads ([Slow work](#slow-work)) | Files and the desktop list folders |
| `layout::icon`; `metrics::{icon_view, list_view, column_view, timing, selection, marquee, drag}` | the same | the views' layout and design values |
| `geom` | `Direction` alone | its `Point`, `Size` and `Rect` are `cupertino-geometry`'s, which this crate draws with |
| `kit::{grid, list, band, clicks}`; the widgets and assembly of `kit::columns`; the list's assembly of `kit::view`; `Grab` of `kit::dragged` | `grid`, `list`, `columns` | the views |
| `rename`, `name_edit`, `open`, `operations` | the same | D8's operations; blocking functions as they are |
| `places` | the same, the user directories read through `cupertino-defaults` ([Places](#places-and-user-directories)) | Files' sidebar, the chooser's, the desktop's folder |
| `notice` | the same, on the program's bus connection | Files and the desktop post failures |
| `thumbnail_service`, `item_art`, `kit::item_icons` | the same, as jobs; `file_icons` comes down from `cupertino-kit` beside them | Files and the desktop draw the same icons and thumbnails |

Stay in Files: `browser`, `history`, `selection`, `typeselect`, `settings`, `layout::list`, `columns` (the chain),
`preview`, `format`, `group`, `window`, `window_prefs`, `tab_strip`, `search_field`, `spring`, `context_menu`,
`menu_spec`, `menu_bar`, `instance`, `udisks`, the rest of `metrics` and of `kit`. `udisks` is the sidebar's removable
media, which no other program lists. `desktop`, `kit::desktop` and `metrics::desktop` stay until W6.10 takes them to
`cupertino-shell-desktop`. There is no `trash` module to bring: `cupertino-trash` is a `system` crate, and the
family's operations and places call it by its name.

The family is about 8 800 lines; this crate is 28 005 without it. It brings these dependencies: `cupertino-apps`,
`cupertino-assets`, `cupertino-icons`, `cupertino-trash`, `cupertino-defaults`, `zbus`, `inotify`, `async-io`,
`async-channel`, `serde`, `libc`, `xattr`, `tree_magic_mini`, `mime_guess`. Every Kit program links all of them but
`cupertino-trash` and the last three through `cupertino-kit` already.

### Slow work

The family starts no thread and its values on the UI thread read no file. Work is one of three things: a blocking
function the program runs with `kit::task::io`; a job a value hands out (`take_work`, `take_job`) that the program
runs the same way and that leaves its result in the value's queue; or a future that waits without reading a file,
which the program runs with `Command::Perform` on its UI loop (a folder's watch, the notices on the bus connection).

```rust
impl DirectoryModel {
    /// A model of `dir`. It reads nothing: `notify` wakes the program whenever there is something to `poll`.
    pub fn open(dir: impl Into<PathBuf>, opts: ModelOptions, notify: Notify) -> DirectoryModel;
    /// The blocking work the model has waiting: listing the folder in batches, reading the items a change named,
    /// listing a virtual folder again.
    pub fn take_work(&mut self) -> Option<ModelWork>;
    /// The folder's watch, once; `None` without `opts.watch`.
    pub fn take_watch(&mut self) -> Option<ModelWatch>;
}
impl ModelWork { pub fn run(self); }
impl ModelWatch {
    /// Hands each burst of changed names to the model and wakes it, until the model is dropped.
    pub async fn follow(self);
}

impl ThumbnailService {
    /// The next thumbnail to make, while fewer than `JOBS` (two, the count `WORKERS` holds) are being made.
    pub fn take_job(&self) -> Option<ThumbnailJob>;
}
impl ThumbnailJob { pub fn run(self); }

impl Notices {
    /// Notices that are logged, not posted: the session has no bus.
    pub fn logged() -> Notices;
    /// Notices posted on `session` in order, and the future that posts them and hears their clicks.
    pub fn on(session: zbus::Connection) -> (Notices, impl Future<Output = ()> + Send + 'static);
    /// Ends once every notice posted so far is sent.
    pub fn sent(&self) -> impl Future<Output = ()> + Send + 'static;
}
```

`open_recents` and `open_applications` have the same form; `poll`, `entries`, `set_sort`, `want`, `forget`, `get`,
`post`, `post_clickable` and `clicks` keep their signatures.

| Now, in Files | In the family | What changes for the person |
|---|---|---|
| `model`: a thread a folder that lists it; the `notify` crate's thread for changes; the items a change names are read in `poll`, on the UI thread; `open` reads the folder's metadata there; dropping a model waits for its thread | `take_work`, run as `kit::task::io`; `ModelWatch::follow` waits on an inotify descriptor through the reactor the bus connection waits on, as the settings watch does | A folder that cannot be opened is the model's first `ModelDelta::Error`, not an error of `open`: Files goes back to the folder it showed and posts the notice it posts now. A change shows one turn of the loop later. Closing a window never waits for a listing. |
| `thumbnail_service`: two worker threads | `take_job`, run as `kit::task::io`, two at once | None at rest. A decoder's panic still marks the file as failed. |
| `notice`: one thread that posts with blocking calls, one that hears clicks; `flush` blocks the exit | one future on the UI loop, on the connection the program gives; `sent` instead of `flush` | Files exits when `sent` ends or after `EXIT_NOTICES`, without blocking. |
| `operations`, `open`, `places`: blocking functions | as they are | None. |

One instance runs at most `task::IO_SHARE` (four) of these at once and its others wait their turn, where each has a
thread of its own now: a folder on a mount that does not answer holds its turn, and Kit reports it once
(`toolkit.task.overdue`). The deadlines are named constants beside the work: 10 s for a model's work, 5 s for a
thumbnail.

Not in this task, because their threads are their callers': a transfer's thread (`kit/transfer.rs`), the preview
window's read, the look-ups, the single-instance service, the places watch and `udisks` are Files' until W7.1; the
desktop's `desktop-files` and `trash-sizes` threads are W6.10's; the chooser's `read` and `arm` threads are W6.11's.

### The open and save panel

The panel keeps its rows in wave 4. The chooser draws a plain list of 24 pt rows in `dialog.rs`, which it shares
with the application chooser, the sharing picker and two consent dialogs, and which W6.4, W6.11 and W6.14 take apart;
the portal has no capture example. Rows that are a `FileList` are another look (a header, Files' row height and
selection, perhaps its columns) and another order (the picker lists folders first by lower-cased name, the family by
natural order with the sort the program gives), and both need the owner's eye on a sheet.

- Wave 4 shares what is the same already: the places (below) and the row icons (`file_icons`).
- W6.11, which moves the chooser into `cupertino-shell-dialogs` and gives it a capture example, gains a step: the
  panel's rows are a `FileList`, the picker lists the family's `Entry` in the family's order, and `filepicker::Entry`
  and its reader go (the second pair of copies, which meets only then). It is a look and behaviour change, shown on a
  review sheet of the panel before and after, open and save, light and dark, for the owner.
- What a single-selection list does with a band, and whether the panel's sidebar is Files' sidebar, are settled on
  that sheet.

### Places and user directories

`user-dirs.dirs` has four parsers: Files', this crate's `filepicker::places` (which the portal's chooser and its
screenshot directory name through Kit), `cupertino-screenshot`'s and the dock's. The portal becomes a service that
never uses Kit (W6.11), so the one reader is below Kit, beside `config_home`:

```rust
// cupertino-defaults
pub mod user_dirs {
    /// The `XDG_<NAME>_DIR` assignments of a `user-dirs.dirs` text as `(NAME, path)`, `$HOME` expanded against `home`.
    pub fn parse(text: &str, home: &Path) -> Vec<(String, PathBuf)>;
    /// The assignments of `user-dirs.dirs` in `config_home()`: none when the file is missing, an error when it cannot
    /// be read or neither `XDG_CONFIG_HOME` nor `HOME` is set. It reads a file.
    pub fn read(home: &Path) -> io::Result<Vec<(String, PathBuf)>>;
}
```

Files' `places` stays, as `files::places`: its `Place` (name, path, group, kind), `standard_places`, `folder_icons`,
`desktop_folder`, `session_places`. `filepicker::places` goes, with its `Place`, `places` and `session_places`. The
two lists differ and both stay, over one table of user directories: `standard_places` is Files' sidebar, and
`chooser_places(home, &assigned) -> Vec<Place>` is the chooser's (Home, then Desktop, Documents, Downloads, Music,
Pictures and Videos where they exist), with the names and symbols its rows have now.

| Caller | Effect |
|---|---|
| Files | None: its rules are the ones kept (a missing file assigns nothing, an unreadable one is an error at start). |
| The portal's chooser | The same rows. `XDG_CONFIG_HOME` follows `config_home()`, the rule of every settings file: a relative value is used, which `filepicker::places` ignores. A file that cannot be read is one line on standard error and the sidebar of a missing file; `filepicker::places` says nothing. |
| The portal's screenshot directory | Reads `cupertino_defaults::user_dirs`; the same two changes; `~/Pictures` when nothing is assigned or readable. |
| Settings' sheet | None: it names `filepicker::Picker`, not the places. |
| `cupertino-screenshot`, the dock | None now. Their copies go in W6.4 and W6.8, which own those crates. |

### Steps of W4.8

**Owns** `ui-controls` (`files`); `files`; `kit` (the `files` and `filepicker` lists of `lib.rs`, `file_icons.rs`,
the README's module table); `defaults` (`user_dirs`); `portal` (where `dialog.rs` takes its places from, `pictures`
in `screenshot.rs`). **Touches** the manifests of `ui-controls files kit portal`. **After** W4.7, W4.11. Size L, nine
steps, one commit each.

The proof of a step that changes no behaviour is the same pixels: every sheet of `files_capture` (`files/window`,
`files/sidebar`, `files/preview`, `files/popover`, `files/tabs`) and `files/window --view columns --selected
Documents/notes.txt`, at 1× and 2×, written with `--out` before and after, and `diff -r` of the two folders is empty.
No step of this task changes a look, so none needs a review sheet; the panel's is W6.11's.

| # | Step | Kind, size | Done when |
|---|---|---|---|
| 1 | **One geometry.** In Files, `geom`'s `Point`, `Size` and `Rect` go: the layouts, the desktop arrangement and their callers use `kit::geometry`. `Direction` stays. | no behaviour change, S | `geom` holds `Direction` alone and no function turns one rectangle type into the other; same pixels. |
| 2 | **Values and operations.** `entry natural sort mime tags applications recents geom layout::icon rename name_edit open operations item_art` and the views' `metrics` are modules of `files`; `kit::files` lists what callers name; Files, its example and its bench name them from Kit. | move, M | `git diff -M` shows the bodies unchanged; no glob in Kit's lists; the remaining-work line `cupertino-files -> cupertino-ui-text` leaves the list; same pixels. |
| 3 | **Places and user directories: one copy.** `cupertino_defaults::user_dirs`; Files' `places` is `files::places` with `chooser_places`; `filepicker::places` goes; the portal's chooser and `pictures` use them. | behaviour change (the portal: [Places](#places-and-user-directories)), S | `rg parse_user_dirs crates` finds only `cupertino-screenshot`; a test of `user_dirs` in `crates/cupertino-defaults/tests/`; a test that `chooser_places` gives the rows the chooser has now; the portal's diff holds no drawing code; same pixels in Files. |
| 4 | **Icon grid.** `grid`, `band`, `clicks` and `Grab` are `files::IconGrid<M>` with `ViewEvent`; Files maps the events. | no behaviour change, M | Files' `Event` and `update` are untouched; same pixels. |
| 5 | **List.** `list` and the list's assembly are `files::FileList<M>`. | no behaviour change, M | as 4. |
| 6 | **Columns.** The column widgets and assembly are `files::FileColumns<M>`; the chain and its listings stay in Files. | no behaviour change, M | as 4; `rg 'Session\|BrowserWindow' crates/cupertino-ui-controls/src` finds nothing. |
| 7 | **Listings as work.** `model` is `files::model` in the form above; the `directory` bench comes with it; Files runs the work and the watch, and goes back from a folder that does not open. | behaviour change, M | no `std::thread` and no `notify` in `files::model`; `tests/files-model.rs`: batches, a created, renamed and removed item seen through the watch, a folder that cannot be read as the first delta, a dropped model that ends its work; same pixels. |
| 8 | **Thumbnails and item icons.** `thumbnail_service` and `item_icons` are in `files` as jobs; `file_icons.rs` comes down from `cupertino-kit`, whose `kit::file_icons` names the same items. | behaviour change, M | no `std::thread` in them; `tests/files-thumbnails.rs`: wanted first, two at once, the budget, a failure kept; same pixels, thumbnails included. |
| 9 | **Notices.** `notice` is `files::notice` on the program's connection. | behaviour change, S | no `std::thread` and no `zbus::blocking` in it; `tests/files-notices.rs` against a stand-in service on a peer connection: order kept, a click back with its tag and token, `sent` ends. |

Steps 1 to 6 change no behaviour and 7 to 9 do; the lead may run 7 to 9 as a task of its own after 1 to 6. Every
step ends with `cargo xtask ci --profile agent --base <B>`, the layers check and the graph and remaining-work list
written again. From step 2 `cupertino-files` depends on `cupertino-kit` whether or not its `app` feature is on; the
feature stays until W7.1. Until W7.1, too, Files makes its session connection before its loop starts and hands its
async half to `Notices`.

### What the later tasks take

- **W6.10** (`cupertino-shell-desktop`): `desktop`, `kit::desktop` and `metrics::desktop` leave Files for the
  component, which names `kit::files` for its listing, item icons, names, rename, operations, open, notices and
  `places::desktop_folder`, and runs its file work as tasks. It keeps its own item view: "on Kit's file views" reads
  "on `kit::files`".
- **W6.11** (`cupertino-shell-dialogs`): the panel step above; the chooser reads its places and its folders as tasks.
- **W7.1** (Files through Kit): Files' own threads become tasks, `udisks` and the session connection go through
  `kit::bus::connect`, its `notify` watch of the places goes, its `clippy.toml` arrives. A transfer run as one
  `kit::task::io` holds one of the instance's four turns for as long as it copies; W7.1 says how many run at once.
- **W6.4**, **W6.8**: `cupertino-screenshot` and the dock read `cupertino_defaults::user_dirs`.

### What W4.T tests

Once step 9 is merged, the `ui` part of W4.T writes this crate's tests with the family in them, one a behaviour:
each view driven through `kit::test` (selection by press, keys, band and typing; open; the rename press; Tab by
name; a drag starting; the events of a column), `layout::icon`, natural order and sort, name checks and free names,
the operations on a temporary folder (copy and move with progress and a stop, duplicate, alias, a folder of items),
`open` against a fixture of applications, the two lists of places, and a notice's words. The tests of steps 3 and 7
to 9 are there already. Files' own modules are W7.T's.

### Decisions for approval

1. **The views' boundary (A).** The three views are widgets generic over the program's message, reporting through
   `ViewEvent` and one function; the list and the columns come assembled, not as their parts; the chain's listings,
   the preview pane and the name field stay with the program. Default: yes. The types above are the whole of it.
2. **Where the family is (B).** A module of `cupertino-ui-controls`, as D8 and the crate map say, with the lines and
   dependencies named above. The other course is a crate of its own in `ui` (`cupertino-ui-files`, D32's fourth
   reason: this crate is past 25 000 lines and the views already name the controls only by their public items), which
   needs a row in the crate map and a line in the decision log. Default: the module.
3. **What stays in Files (B).** Everything one program uses, `udisks` included. Default: yes.
4. **Threads (C).** The family starts none and reads no file on the UI thread; the behaviour changes of
   [Slow work](#slow-work) are accepted, the four turns an instance has among them. Default: yes.
5. **The panel's look (D).** Unchanged in wave 4; its rows become a `FileList` in W6.11, on a review sheet.
   Default: yes. The other course builds it in wave 4, in a file three later tasks take apart, with no capture to
   show it.
6. **Places (E).** One reader of the user directories in `cupertino-defaults`; Files' places and rules stay; the
   portal takes Files' `XDG_CONFIG_HOME` rule and reports an unreadable file. Default: yes. The other course keeps
   the reader in Kit, and the portal copies it again when it stops using Kit.
7. **Wave 4 holds all nine steps**, with the Owns list above (`kit`, `defaults` and all of `files` beside the plan's),
   so Kit's file API is settled and tested once; only the callers' own work is left to W6.10, W6.11 and W7.1.
   Default: yes.
