# cupertino-ui-platform

CupertinoKit's window-system interface: the `WindowSystem` trait (windows, popups, presentation, compositor materials under a window or under one rectangle of it, cursors, clipboard,
input method, activation tokens, drags), its events and `Capabilities`. MPL-2.0. Implemented by `cupertino-ui-wayland` on the desktop and by
`cupertino-ui-test` headless.

`present_solid(window, Color, configure)` shows a window as one colour without a frame (`Color` is the theme's,
re-exported): the compositor stretches a single pixel over the window, so a transparent window that only takes input,
or a uniform scrim, costs nothing per frame at any size or scale. It answers like `present`; the trait's default, and a
system that cannot, refuse with `PlatformError::Unsupported`, and the caller presents a drawn frame instead.

A popup asks for a place beside an anchor (`PopupSpec`); where the compositor put it, after flipping or sliding it to
fit, arrives as `WindowEvent::PopupPlaced(rect)`, in its parent's coordinates, before each configure that places it.

The compositor may tell whether the face of a glass material is dark or light: `WindowEvent::GlassTone { region, tone }`
names the region by its index in the last `WindowRequest::Materials` list, or the window's own backdrop with `None`,
and `GlassTone::{Appearance, Dark, Light}` picks the colour of the text over it. A system that cannot tell sends none.

`PlatformEvent::ScrollBars` tells how the session wants scroll views to show their bars: `ScrollBars::WhenScrolling`
(thin bars over the content while it scrolls) or `ScrollBars::Always` (tracks beside it that stay shown). A system
that cannot tell never sends it, and each view keeps its own style.

`SystemAppearance::steady_caret` says the session wants text carets shown without blinking; a system that cannot tell
leaves it off.

A pointer scroll (`PointerEvent::Scroll`) keeps its provenance: the source (`ScrollSource::{Wheel, Finger,
Continuous, WheelTilt}`), the high-resolution wheel steps (`value120`, 120 a detent), the stop that ends a finger or
continuous scroll, and `inverted`, whether each axis is in natural (inverted) direction.

`WindowSpec::decorations` chooses the window's style: `Preferred` (titled: the compositor's title bar over the
content), `Client`, `None`, and the styles whose content fills the window and draws the row the compositor's window
control buttons lie in (`DecorationMode::content_fills_window`): `Unified` (the 52 pt toolbar row), `UnifiedCompact`
(the 40 pt one) and `Sidebar` (the buttons at the top of a full-height sidebar). Without the desktop's effects those
three are titled.

`start_drag(window, DragRequest)` drags data out of a window from the pointer button held in it: the data in each of
its MIME types (`ClipboardData`), the `DropAction`s a drop may take, and an optional `DragImage` (premultiplied RGBA at a
whole number of pixels per point, both sides whole points, with the pointer at its `hotspot`). It returns a
`DragToken`; each step arrives as `PlatformEvent::Drag` with a `DragProgress` (the target's type, the action, the drop)
and the drag ends with `Finished` or `Cancelled`, the latter also when the compositor ignored it. The window gets a
pointer leave while the drag runs. The trait's default refuses with `PlatformError::Unsupported`, as does a system
without `Capabilities::drag_and_drop`.

A drag from any application, this one included, over a window arrives as `WindowEvent::Drop(DropEvent)`: `Enter` (a
`DropToken`, the position, the MIME types offered, the `DropAction`s the source allows, a copy alone when its protocol
has none, and the modifiers held), `Motion`, `Action` (what a drop would do now), then `Leave` or `Drop`.
`answer_drop(token, DropAnswer)` tells the source the type the window would read there (`None` refuses), the actions it
can do and the one it prefers; after the drop `read_drop(token, mime)` reads the data, which arrives as
`PlatformEvent::Clipboard` with the returned `ClipboardToken`, and `finish_drop(token, Some(action))` takes it (the
source removes its copy after a move) or `finish_drop(token, None)` refuses it. The defaults refuse with
`Unsupported` and finish nothing.
