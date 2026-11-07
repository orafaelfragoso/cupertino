# cupertino-ui-core

The CupertinoKit runtime. MPL-2.0. Depends on the other `cupertino-ui-*` type crates, `cupertino-menu-model`, `async-task` and
`rustix`;
never on a server, shell or private-protocol crate.

An `Application` keeps its state in a model, describes its windows as `Element` trees in `view`, and changes the model
only in `update`, from messages. The runtime owns the model, one node tree for every window, and the window system,
all on one thread.

## Public API

| Item | Contents |
|---|---|
| `Application` | `Model`, `Message`, `init`, `update(&mut Model, Message) -> Vec<Command>`, `view(&Model) -> Vec<Window>` |
| `Window` | `key`, `spec`, `shortcuts: CommandRegistry`, `on_close` (without it a close request ends the application), `content` |
| `Element`, `Widget` | an element is a widget plus an optional `Key`; `Widget` has a typed `State`, `layout`, `take_children`, `update` (returns `Change`), `measure`, `event`, `paint`, `semantics`, and for scroll containers `scroll_offset` |
| `EventCx` | `emit`, `request_focus`, `capture_pointer` / `release_pointer`, `request_paint` / `request_layout`, `set_cursor`, `set_text_input`, `request_animation_frame`, `wake_at(time)` (one `Event::Wake` once the clock reaches it; a new request replaces the node's last), `move_window` (an interactive move answering the pointer press being handled, sent as `WindowRequest::Move` while the press is held; the compositor then takes the pointer, whose leave revokes the node's capture), `zoom_window` (`WindowRequest::Maximize` with the opposite of the maximized state the last configure gave; neither request is sent while the last configure says fullscreen), plus the node's focus, capture and phase |
| `PaintCx`, `SemanticsCx`, `MeasureCx` | painting into the node's `PaintOp`s in its own coordinates, and `set_group` to fade or shadow the whole subtree as one layer; role, name, value, states, actions, `focusable`, `focus_scope`, `protected` |
| `Command` | `Perform(future)`, `Focus(key)`, `CloseWindow`, `Window(key, request)`, `SetClipboard`, `ActivationToken` (a token for launching another application, with its app id; the message always arrives, with `None` when the window is gone or the system has none), `StartDrag(Drag)` (data dragged out of a window from the held button, with an optional `DragPreview`: an `Element` laid out at its size rounded up to whole points and drawn once, over nothing, at the window's scale rounded up to whole pixels per point, with the pointer at its hotspot; every `DragProgress` step reaches `on_progress`, which hears `Finished` or `Cancelled` last, `Cancelled` at once when the drag cannot start), `Exit` |
| `Runtime` | `new(window system, RuntimeOptions)`, `with_theme_for(hook)` (a new theme for each `PlatformEvent::Appearance`), `theme()`, `set_theme(theme)` (every window measures and paints again, focus and compositions stay; an equal theme changes nothing), `step(max_turns)` (non-blocking, never moves time, returns `StepStatus`), `run`, `protected_focus(window)`, `snapshot(window)`, `find`, `node_at`; `run` waits on the window system no longer than the earliest wake, and a test reads that time from `Pending::next_wake`. A focus change to the same node (only the keyboard ring shown or hidden) sends no blur or focus, only a repaint |
| `RuntimeOptions` | theme, layout direction, node ceiling, a renderer factory (one renderer per window), an optional `AccessibilityAdapter` (told each `PlatformEvent::SessionLock` through `session_locked`), the `Clock`, an optional `on_window_error` handler |
| `MonotonicClock` | `CLOCK_MONOTONIC`, the clock the window system stamps events with |

## How it works

- **Reconciliation.** A child keeps its node when its key matches, or, without keys, its position among the unkeyed
  siblings, and its widget type is the same; anything else unmounts with its subtree. Node ids carry their slot's
  generation, and a slot whose generation would wrap is retired. More nodes than the ceiling is `UiError::NodeLimit`.
- **Events.** An event goes down the target's path (capture), reaches the target and goes back up (bubble); a handler
  can stop it. Hit-testing takes the topmost node and respects every clip. Hover enter and leave reach each node the
  pointer crosses. A captured pointer goes to its node alone until it is released; leaving the surface or the window
  losing the focus cancels it with `Event::Cancel`.
- **Keys and focus.** Keys go to the focused node's path, then to the window's shortcuts, then Tab and Shift+Tab move
  the focus in reading order. A shortcut registered with `CommandRegistry::with_fresh` fires only on a fresh press:
  the repeats of a held key pass it by, so a key held since before a window opened cannot answer it. A focus scope (dialog, popover, menu) keeps Tab inside itself and gives the focus back
  when it closes, to the node focused before or its nearest surviving ancestor.
- **Tasks.** `Command::Perform` futures run only inside a step, on the UI thread; a wake from any thread wakes the
  window system. Dropping the runtime cancels every task.
- **Frames.** A step lays out every changed window, rebuilds its accessibility tree and focus, and paints when the
  compositor is ready: before the first configure nothing is drawn, the first frame acknowledges it, and later frames
  wait for the frame callback. A node repaints only when it, its size, its focus or the window's active state changed;
  damage is where changed nodes were and are, and a frame with no damage sends no buffer. A window that cannot be
  laid out, rendered or presented goes to `on_window_error` and tries again, whole, on its next change or configure,
  whatever the cause: the renderer reports a target and content too large to draw with the same error. The same error
  again is not reported again, and the window's animations ask for no frames until a frame presents. An animation
  whose current value is what fails (a blur animating down from over the renderer's limit, say) therefore stays at
  that value until the application changes something or the compositor configures. Other windows go on, and only a
  lost connection ends `step`.
- **Window materials.** A window whose background is `WindowBackground::Surface` asks the window system for the
  surface's material (`WindowRequest::Backdrop`) when it reports `app_effects` and the theme asks for one; otherwise,
  or when the request fails, core paints the surface's opaque fill in the window's rounded shape. A theme change
  (Reduce Transparency, high contrast) takes the material away or asks for it again. Its `alpha` fades the material
  or the fill, so a window can fade its background with its content; a change of alpha alone keeps the material.
  `WindowBackground::SurfaceRect` puts the material, or the fill, under one rounded rectangle of the window only
  (`Backdrop::bounds`), for a window larger than what it shows, such as a dock bar with room for its magnified tiles.
  A node can also ask for its surface's material under itself while painting (`PaintCx::material(surface, radius,
  alpha)`): core cuts it to the part of the node that shows (a scroll view's clip, say) and sends the window's list
  as `WindowRequest::Materials` when it changes. It is granted, and the call returns `true`, only with `app_effects`,
  when the theme gives the surface a material and while the window holds fewer than `MAX_MATERIALS` (32); otherwise,
  or after the window system refused the list, the node paints its own fill. The material lies under all of the
  window's content, so a node drawn over another node's material shows it; group opacity does not reach it.
- **Menus.** A toplevel's `menus` go to the window system as `WindowRequest::Menus` when its bar or its disabled
  items change, and `None` withdraws them; an item chosen in the desktop's menu bar (`WindowEvent::MenuActivated`)
  sends `on_activate`'s message. A popup's menus are ignored.
- **Scrolling.** A scroll container's offset is model state, counted from the start edge; core moves the content
  by it when placing nodes, so painting and hit-testing agree. In right-to-left layout, where layout mirrors the
  content, a horizontal offset moves the content right.
- **Accessibility.** A node without a role stays out of the tree and its children move up. A protected node's value
  becomes `AccessibleValue::Protected`, here, before any adapter, snapshot or test sees it. An adapter gets each
  window's whole tree, marked `full`, when it becomes active or the window opens, then only diffs while
  `is_active`. An adapter wakes the runtime's waker when it becomes active, since `run` may be blocked. A protected
  field's input-method state goes out without its surrounding text. Each turn takes the adapter's actions and gives
  each one to its target as `Event::Action`, in the window whose published tree holds it; one for a node that is gone
  is dropped.

## Not yet

Popups (`create_popup`), clipboard reads and drops onto a window, and image resources in `PaintResources` are not
wired yet. Layout runs over the whole window on every change.
