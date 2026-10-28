# cupertino-ui-core

The CupertinoKit runtime. MPL-2.0. Depends on the other `cupertino-ui-*` type crates, `async-task` and `rustix`;
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
| `EventCx` | `emit`, `request_focus`, `capture_pointer` / `release_pointer`, `request_paint` / `request_layout`, `set_cursor`, `set_text_input`, `request_animation_frame`, plus the node's focus, capture and phase |
| `PaintCx`, `SemanticsCx`, `MeasureCx` | painting into the node's `PaintOp`s in its own coordinates, and `set_group` to fade or shadow the whole subtree as one layer; role, name, value, states, actions, `focusable`, `focus_scope`, `protected` |
| `Command` | `Perform(future)`, `Focus(key)`, `CloseWindow`, `Window(key, request)`, `SetClipboard`, `Exit` |
| `Runtime` | `new(window system, RuntimeOptions)`, `step(max_turns)` (non-blocking, never moves time, returns `StepStatus`), `run`, `snapshot(window)`, `find`, `node_at` |
| `RuntimeOptions` | theme, layout direction, node ceiling, a renderer factory (one renderer per window), an optional `AccessibilityAdapter`, the `Clock` |
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
  the focus in reading order. A focus scope (dialog, popover, menu) keeps Tab inside itself and gives the focus back
  when it closes, to the node focused before or its nearest surviving ancestor.
- **Tasks.** `Command::Perform` futures run only inside a step, on the UI thread; a wake from any thread wakes the
  window system. Dropping the runtime cancels every task.
- **Frames.** A step lays out every changed window, rebuilds its accessibility tree and focus, and paints when the
  compositor is ready: before the first configure nothing is drawn, the first frame acknowledges it, and later frames
  wait for the frame callback. A node repaints only when it, its size, its focus or the window's active state changed;
  damage is where changed nodes were and are, and a frame with no damage sends no buffer.
- **Scrolling.** A scroll container's offset is model state, counted from the start edge; core moves the content
  by it when placing nodes, so painting and hit-testing agree. In right-to-left layout, where layout mirrors the
  content, a horizontal offset moves the content right.
- **Accessibility.** A node without a role stays out of the tree and its children move up. A protected node's value
  becomes `AccessibleValue::Protected`, here, before any adapter, snapshot or test sees it. Adapters get diffs only
  while `is_active`.

## Not yet

Popups (`create_popup`), clipboard reads and drag and drop, and image resources in `PaintResources` are not wired
yet. Layout runs over the whole window on every change.
