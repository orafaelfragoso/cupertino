# cupertino-ui-platform

CupertinoKit's window-system interface: the `WindowSystem` trait (windows, popups, presentation, compositor materials, cursors, clipboard,
input method, activation tokens, drags), its events and `Capabilities`. MPL-2.0. Implemented by `cupertino-ui-wayland` on the desktop and by
`cupertino-ui-test` headless.

`start_drag(window, DragRequest)` drags data out of a window from the pointer button held in it: the data in each of
its MIME types (`ClipboardData`), the `DropAction`s a drop may take, and an optional `DragImage` (premultiplied RGBA at a
whole number of pixels per point, both sides whole points, with the pointer at its `hotspot`). It returns a
`DragToken`; each step arrives as `PlatformEvent::Drag` with a `DragProgress` (the target's type, the action, the drop)
and the drag ends with `Finished` or `Cancelled`, the latter also when the compositor ignored it. The window gets a
pointer leave while the drag runs. The trait's default refuses with `PlatformError::Unsupported`, as does a system
without `Capabilities::drag_and_drop`.
