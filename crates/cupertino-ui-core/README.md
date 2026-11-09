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
| `EventCx` | `emit`, `request_focus`, `capture_pointer` / `release_pointer`, `request_paint` / `request_layout`, `set_cursor`, `set_text_input` (when the focus moves from one text field to another, the window system is told to turn the input method off for the first before the second takes it), `request_animation_frame`, `input_time(at)` (when an input event stamped `at` happened, on the clock of `now()`: its own time, or now for one stamped zero or later than now), `wake_at(time)` (one `Event::Wake` once the clock reaches it; a new request replaces the node's last), `opening_press()` (when the pointer press or the finger that opened this popup went down, on the clock of `now()`: a popup opened while a button or a finger is held, such as a menu opened by a press, times press-and-hold from it; `None` in a toplevel and in a popup no held press opened), `popup_placement(key)` (where the compositor placed this window's open popup `key`, from `WindowEvent::PopupPlaced`, in window coordinates; `None` before it is placed and while it is asked to move), `move_window` (an interactive move answering the pointer press being handled, sent as `WindowRequest::Move` while the press is held; the compositor then takes the pointer, whose leave revokes the node's capture), `zoom_window` (`WindowRequest::Maximize` with the opposite of the maximized state the last configure gave; neither request is sent while the last configure says fullscreen), plus the node's focus, capture and phase |
| `PaintCx`, `SemanticsCx`, `MeasureCx` | painting into the node's `PaintOp`s in its own coordinates, and `set_group` to fade or shadow the whole subtree as one layer; `overlay()`, `fill_pixels` and `clip_pixels` (*Overlays*); `on_material()` (*On a material*); `material(surface, radius, alpha)` and `glass(Glass)` (*Window materials*); role, name, value, states, actions, `focusable`, `focus_scope`, `protected` |
| `Glass`, `GlassCorners`, `GlassStyle` | what `PaintCx::glass` puts under a node: `Glass::regular()`, `Glass::clear()` or `Glass::large_panel()` (regular glass with its tones pressed together and a deeper shadow, for a large floating panel), then `.radius(points)` or `.capsule()` (round ends at any size), `.container(id, spacing)` (nodes of a window in the same nonzero container join into one shape, blending over `spacing` points, 0 for a hard join) and `.alpha(a)` |
| `Command` | `Perform(future)`, `Focus(key)` (with its ring), `MoveFocus(key)` (its ring showing only if it showed: the keyboard following a click), `CloseWindow`, `Window(key, request)`, `SetClipboard`, `ActivationToken` (a token for launching another application, with its app id; the message always arrives, with `None` when the window is gone or the system has none), `FinishDrop(token, action)` (what the update that handles a `Dropped` really did with it: `Some(action)` or `None` when it took nothing, told to the source instead of the agreed action), `HoldDrop(token)` (given by that update, keeps the drop open past its turn until a later `FinishDrop(token, action)`, for work that ends later), `StartDrag(Drag)` (data dragged out of a window from the held button, with an optional `DragPreview`: an `Element` laid out at its size rounded up to whole points and drawn once, over nothing, at the window's scale rounded up to whole pixels per point, with the pointer at its hotspot; every `DragProgress` step reaches `on_progress`, which hears `Finished` or `Cancelled` last, `Cancelled` at once when the drag cannot start), `OnPresented { window, message }` (`message` once the window has presented its first frame: after its first configure, at the frame callback of its first committed frame, never at the commit alone; at once when it already has; never for a window that closes first or that the next view lacks), `After { delay, message }` (`message` once `delay` has passed on the runtime's clock, those due together in the order asked; not cancellable, so the message says what it is for), `Exit` |
| `DropTarget` | an element taking data dragged onto its `content` from any application: the MIME types it reads (best first), the `DropAction`s it can do, `place` (where a drop would land, from the pointer, the keys of the keyed nodes under it, innermost first, the offered types, the source's actions and the modifiers: a `DropPlace` with the keyed node to outline, or `None` for the whole target, and the preferred action; `None` takes no drop there), an optional `on_hover` (the place as the drag enters or moves to another, `None` as it leaves or drops), an optional `on_leave` (sent by every target of the window that has one when a drag that was over the window leaves it or ends there without a drop a target took: released where nothing takes it, refused, its data failed or cancelled) and `on_drop` (a `Dropped` with its `DropToken`, the place, the position, the chosen action, the modifiers held and the data). The innermost target under the pointer that takes one of the drag's types and a drop there is asked; the runtime answers the window system with the type and actions, outlines the place (the accent, translucent, with a text-colour outline in a high-contrast appearance, cut by the node's clip), asks again after each layout while the drag stands still, reads the data at the drop and, once the turn has updated the model with it (so a source told the data moved lets it go only after the application has it), finishes it with the action the update gave in `Command::FinishDrop`, else the chosen action, or without the window system's word with the preferred action when the source allows it; a drop nothing takes, a failed read or a target gone in the meantime is refused. It lays out as its content and has no semantics: keyboard alternatives stay the application's |
| `Runtime` | `new(window system, RuntimeOptions)`, `with_theme_for(hook)` (a new theme for each `PlatformEvent::Appearance`), `theme()`, `set_theme(theme)` (every window measures and paints again, focus and compositions stay; an equal theme changes nothing), `step(max_turns)` (non-blocking, never moves time, returns `StepStatus`), `run`, `protected_focus(window)`, `snapshot(window)`, `find`, `node_at`; `run` waits on the window system no longer than the earliest wake, and a test reads that time from `Pending::next_wake`. A focus change to the same node (only the keyboard ring shown or hidden) sends no blur or focus, only a repaint |
| `RuntimeOptions` | theme, layout direction, node ceiling, a renderer factory (one renderer per window), an optional `AccessibilityAdapter` (told each `PlatformEvent::SessionLock` through `session_locked`), the `Clock`, an optional `on_window_error` handler |
| `MonotonicClock` | `CLOCK_MONOTONIC`, the clock the window system stamps events with |
| `Icons` | the widgets' images: `symbol(name, px, color, mirrored)`, and `application(px)`, the running application's own icon (`None` by default; `PlaceholderIcons` draws a square outline for both) |

## How it works

- **Reconciliation.** A child keeps its node when its key matches, or, without keys, its position among the unkeyed
  siblings, and its widget type is the same; anything else unmounts with its subtree. Node ids carry their slot's
  generation, and a slot whose generation would wrap is retired. More nodes than the ceiling is `UiError::NodeLimit`.
- **Events.** An event goes down the target's path (capture), reaches the target and goes back up (bubble); a handler
  can stop it. Hit-testing takes the topmost node and respects every clip. Hover enter and leave reach each node the
  pointer crosses. A captured pointer goes to its node alone until it is released; leaving the surface or the window
  losing the focus cancels it with `Event::Cancel`. A scroll gesture keeps the path its first event hit: a finger's
  until its stop, a wheel's or another source's until it pauses for `SCROLL_IDLE_NS`, the source changes or the
  pointer moves, and any of them until the pointer leaves the surface. A cancelled touch reaches every node its path
  went through, the touched one first. A button let go anywhere but in a popup its press opened (on the opener, in
  the opener's window, in another window) also reaches the root of every popup that press opened, nested ones
  included, as a release at a point outside it, so a menu dragged through and let go elsewhere can close; one let go
  in such a popup goes only where it lands, and popups the press did not open hear nothing of it. The unit tests open
  popups through the fake window system and let go at each of those places.
- **Touch and popups.** Every event of a finger goes to the window it went down in. The first finger held is the one a
  popup opened meanwhile follows: in a window that is not a popup, what it went through hears `Event::Cancel` as the
  popup opens, so a button under it is not clicked as it lifts. Its moves reach the root of each popup it opened at
  that popup's own point; lifted over one of them, its lift is that popup's, and lifted anywhere else it goes where it
  lifts and each popup it opened hears it lift there, outside it, as a mouse release elsewhere does. A cancelled touch
  ends the interaction in those popups too, and another finger put down in one while it is held goes nowhere. The unit
  tests follow a finger through the same popups as the button.
- **Scroll timing.** `Runtime::with_scroll_timing(sink)` follows every pointer scroll event to the screen: the window
  system's timestamp, when its handlers ran, the model update and view it caused (`None` when it changed no model),
  when the first frame after it was handed to the window system and when the compositor showed that frame
  (`WindowEvent::Presented`; `None` for a `Discarded` one). The sink gets each `ScrollTiming` once its frame is shown
  or dropped; `ScrollTiming::latency` gives the stages from the input. Without a sink the runtime keeps nothing; at
  most 256 events wait at each stage.
- **Scroll motion.** `kinetic::ScrollMotion` is a pure model of a scroll view's content over the time it is given:
  a finger's drag, the kinetic motion its lift leaves (exponential decay), the stretch past an edge (a resistance
  curve that never passes the viewport's length) and the return to the edge (a critically damped spring). Its offset
  stays inside the range; the stretch and the velocity are separate. A wheel, a key or a continuous source steps it
  without stretch or inertia, so each input path has one owner of kinetic motion. A drag moves only the axis its first
  movement moved most unless the policy is `AxisPolicy::Free`.
- **Overlays.** A node that calls `PaintCx::overlay` while it paints draws `Widget::overlay` on every frame until it
  paints again without asking: a text caret. `OverlayCx::fill_pixels` fills a rect of the node with each edge moved to
  the nearest device pixel edge of the window (at least one pixel each way) and `OverlayCx::clip` clips the overlay to
  a rect widened to whole pixels, so what it draws is never blended at its edges whatever fraction of a pixel the node
  lies at, while the node's own paint stays independent of its position and keeps its cache. The overlay is a paint
  node of its own (`overlay_id(node)`), after the node's subtree, in window coordinates, under the clips and group
  around the node; its revision changes only when what it draws does. `EventCx::request_overlay` draws the window's
  overlays again on the next frame without painting any node or building the semantics again, as a caret's blink
  needs. What must lie under other paint, a text selection under its glyphs, stays in the node's paint:
  `PaintCx::fill_pixels` and `PaintCx::clip_pixels` mark a fill and a clip that each frame moves onto the window's
  device pixels where it draws the node (the fill to the nearest edges, the clip outward), so the node keeps its cache
  and revision as it moves; each stays on the pixels its rect touches, but a fill narrower than a pixel, which its
  extent counts as one.
- **Keys and focus.** Keys go to the focused node's path, then to the window's shortcuts, then Tab and Shift+Tab move
  the focus in reading order. A shortcut registered with `CommandRegistry::with_fresh` fires only on a fresh press:
  the repeats of a held key pass it by, so a key held since before a window opened cannot answer it. A focus scope (dialog, popover, menu) keeps Tab inside itself and gives the focus back
  when it closes, to the node focused before or its nearest surviving ancestor. A window paints as active
  (`PaintCx::window_active`) while it has the keyboard or a popup opened from it has, as a menu takes the keyboard
  while its window stays the one in use. It stays so after the popup closes, until the window hears of the keyboard
  itself or a configure says it is no longer activated, so it does not flash inactive while the keyboard comes back.
  A popup still open that loses the keyboard ends it, as the keyboard may have gone to another application without
  the compositor deactivating the window.
  Keys, focus and `EventCx::window_active` follow the keyboard alone.
- **Tasks.** `Command::Perform` futures run only inside a step, on the UI thread; a wake from any thread wakes the
  window system. Dropping the runtime cancels every task.
- **Frames.** A step lays out every changed window, rebuilds its accessibility tree and focus, and paints when the
  compositor is ready: before the first configure nothing is drawn, the first frame acknowledges it, and later frames
  wait for the frame callback. A window lays out again only when a node changed size (`Change::Layout`), it was
  resized or configured, or its environment changed; a focus move, a reveal, an animation frame or a paint-only
  change paints on the layout it has, and a layout that does run reuses the measures of the subtrees that did not
  change. A node repaints only when it, its size, its focus or the window's active state changed. Damage is where
  the changed nodes that draw were and are, a clip cutting it one point past its edge, whose anti-aliased pixels
  the content behind still reaches; a node with a group damages its whole subtree, and any node under a group with
  effects that group's whole reach. A node that draws nothing, before and after (no operation and no group,
  whose opacity reaches its children), damages nothing, so a container's repaint costs no pixels. A frame that
  repainted nothing sends no buffer; one that repainted a node but no pixel commits with no damage, as an
  animation's next frame and the state an application sets on its surface outside the frame wait for a commit. The
  CPU renderer's partial redraw can sample an anti-aliased curved or fractional edge cut by a damaged rectangle's
  border a few levels off a whole frame's, so the rectangles a change damages show in pixel fixtures. A window that
  cannot be laid out, rendered or presented goes to `on_window_error` and tries again, whole, on its next change or
  configure, whatever the cause: the renderer reports a target and content too large to draw with the same error. The same error
  again is not reported again, and the window's animations ask for no frames until a frame presents. An animation
  whose current value is what fails (a blur animating down from over the renderer's limit, say) therefore stays at
  that value until the application changes something or the compositor configures. Other windows go on, and only a
  lost connection ends `step`.
- **Windows of one colour.** A window whose background is `WindowBackground::Solid(color)` draws nothing: its view
  is laid out for focus, accessibility and hit testing, but no display list is rendered, and core asks the window
  system to show the colour (`WindowSystem::present_solid`), which the compositor stretches from a single pixel. It is
  shown again only for a configure, a new size or another colour. Glass its content had laid under itself goes with
  the colour, and is asked for again when the window is drawn. `WindowBackground::INPUT_ONLY` is the transparent
  one, for windows that only take input (outside-click catchers, edge strips). A window system that refuses it gets a
  drawn frame filled with the colour from then on, as a window of any other background would.
- **Window materials.** A window whose background is `WindowBackground::Surface` asks the window system for the
  surface's material (`WindowRequest::Backdrop`) when it reports `app_effects` and the theme asks for one; otherwise,
  or when the request fails, core paints the surface's opaque fill in the window's rounded shape. A theme change
  (Reduce Transparency, high contrast) takes the material away or asks for it again. Its `alpha` fades the material
  or the fill, so a window can fade its background with its content; a change of alpha alone keeps the material.
  `PaintCx::window_backdrop` says whether the window system shows the material and draws its edge: without it a
  panel draws its own edge over the opaque fill.
  A node can also ask for its surface's material under itself while painting (`PaintCx::material(surface, radius,
  alpha)`): core cuts it to the part of the node that shows (a scroll view's clip, say) and sends the window's list
  as `WindowRequest::Materials` when it changes. It is granted, and the call returns `true`, only with `app_effects`,
  when the theme gives the surface a material and while the window holds fewer than `MAX_MATERIALS` (32); otherwise,
  or after the window system refused the list, the node paints its own fill. The material lies under all of the
  window's content, so a node drawn over another node's material shows it; group opacity does not reach it.
- **Glass.** `PaintCx::glass(Glass)` makes the node glass: the compositor blurs what lies behind it, bends it along the
  edge and lights the rim, with the recipe of the node's size, the appearance and the display options. Reduce
  Transparency and Increase Contrast keep the glass with the compositor's recipes for them (the menu and popover
  surfaces of the theme are glass too, so `material` keeps them as well). Glass nodes of one window in the same
  container are drawn as one shape: their outlines blend into each other over the container's spacing, so two buttons
  close together merge. It is granted under the same limits as `material` and travels in the same
  `WindowRequest::Materials` list (`MaterialRegion::glass`).
- **Text on glass.** What a glass node paints after asking, its subtree and, for a node with no material above it,
  any later node of the window that its glass holds whole (a panel's content drawn beside the node of its glass) take
  the theme of text on glass (`ThemeTokens::on_glass`) through `PaintCx::env`; so does a window whose own material is
  glass and shown. A small shape follows the tone the compositor measured under it (`WindowEvent::GlassTone`, kept by
  region in the painter and reset when the material list changes); without one, and on a compositor before version 7
  of the effects, labels take the appearance's matrix. A node over a material other than glass keeps the window's
  colours. A tone that changes repaints only the nodes on that glass.
- **On a material.** `PaintCx::on_material` says whether the node lies on a material rather than the window's content:
  under glass, under a node that asked for a translucent surface's material (`Surface::WindowBackground` and
  `Surface::TextBackground` are content), or in a window whose background is such a surface. It follows what the
  node is drawn in, not whether the compositor shows the material, so a control keeps one look with and without desktop
  effects; it is part of what the painter keys a node's paint by, so a node paints again when it moves on or off one.

  ```rust
  use cupertino_ui_core::{Glass, PaintCx};

  fn paint_toolbar_button(cx: &mut PaintCx<'_>) {
      // the toolbar's buttons share container 1 and merge where they are less than 8 points apart
      if !cx.glass(Glass::regular().capsule().container(1, 8.0)) {
          // no glass here (no desktop effects, or too many materials): paint a fill instead
      }
  }
  ```
- **Menus.** A toplevel's `menus` go to the window system as `WindowRequest::Menus` when its bar or its disabled
  items change, and `None` withdraws them; an item chosen in the desktop's menu bar (`WindowEvent::MenuActivated`)
  sends `on_activate`'s message. A popup's menus are ignored.
- **Scrolling.** A scroll container's offset is model state, counted from the start edge; core moves the content
  by it when placing nodes, so painting and hit-testing agree. In right-to-left layout, where layout mirrors the
  content, a horizontal offset moves the content right.
- **Accessibility.** A node without a role stays out of the tree and its children move up. A protected node's value
  becomes `AccessibleValue::Protected`, here, before any adapter, snapshot or test sees it. An adapter gets each
  window's whole tree, marked `full`, when it becomes active or the window opens, then only diffs while
  `is_active`, and hears each window's title (`window_title`) before its first tree and whenever the title changes. A
  node holding the focus says it is focusable, whatever its semantics said, so a roving item reads as one that takes
  focus. A focus on a node without a role, such as one left on a container when the row it was on went away, is
  told to the adapter (and marked focused) on its nearest ancestor in the tree, so assistive technology is never told
  of a focus it cannot see; the keyboard focus, Tab order and key routing stay on the node itself
  (`FrameSnapshot::accessibility_focus` is the one told). An adapter wakes the runtime's waker when it becomes active,
  since `run` may be blocked. A protected field's input-method state goes out without its surrounding text. Each turn
  takes the adapter's actions and gives each one to its target as `Event::Action`, in the window whose published tree
  holds it; one for a node that is gone is dropped.

## Not yet

An application cannot read the clipboard (a paste): the runtime reads offered data only for a drop. Scrolling repaints everything the scroll view shows: the CPU renderer draws it again rather than moving
the pixels it has.
