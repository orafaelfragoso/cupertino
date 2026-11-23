# cupertino-ui-core

The CupertinoKit runtime, with its layout (the `layout` module, *Layout*) and its window-system interface (the
`platform` module, *Window-system interface*). MPL-2.0. Depends on the other `cupertino-ui-*` type crates,
`cupertino-theme` (its `ui` theme tokens), `cupertino-menu-model`, `cupertino-keyboard` (the modifiers of input events),
`async-task`, `rustix` and `tracing` (the spans of each frame's phases, which `cupertino-diagnostic-events`' `profile`
records when asked); never on a server, shell or private-protocol crate.

An `Application` keeps its state in a model, describes its windows as `Element` trees in `view`, and changes the model
only in `update`, from messages. The runtime owns the model, one node tree for every window, and the window system,
all on one thread. When a view opens a window or popup, the runtime gives its `WindowKey` to `WindowSystem::opening`
before creating it, so a backend can select its role even when several windows have identical specs.

## Public API

| Item | Contents |
|---|---|
| `Application` | `Model`, `Message`, `init`, `update(&mut Model, Message) -> Vec<Command>`, `view(&Model) -> Vec<Window>`; `RESIDENT`, and `RENDERER`, the `RendererClass` its windows belong on (the size and repaint rule, `ByUse`, unless it declares `Cpu` or `Gpu`) |
| `Window` | `key`, `spec`, `shortcuts: CommandRegistry`, `on_close` (without it a close request ends the application), `content` |
| `Element`, `Widget` | an element is a widget plus an optional `Key`; `Widget` has a typed `State`, `layout`, `take_children`, `update` (returns `Change`), `measure`, `event`, `paint`, `semantics`, `idle` (work done in the window's idle time, *Idle time*), and for scroll containers `scroll_offset` |
| `EventCx` | `emit`, `request_focus`, `capture_pointer` / `release_pointer`, `request_paint` / `request_layout`, `set_cursor`, `set_text_input` (when the focus moves from one text field to another, the window system is told to turn the input method off for the first before the second takes it), `request_animation_frame`, `input_time(at)` (when an input event stamped `at` happened, on the clock of `now()`: its own time, or now for one stamped zero or later than now), `wake_at(time)` (one `Event::Wake` once the clock reaches it; a new request replaces the node's last), `opening_press()` (when the pointer press or the finger that opened this popup went down, on the clock of `now()`: a popup opened while a button or a finger is held, such as a menu opened by a press, times press-and-hold from it; `None` in a toplevel and in a popup no held press opened), `popup_placement(key)` (where the compositor placed this window's open popup `key`, from `WindowEvent::PopupPlaced`, in window coordinates; `None` before it is placed and while it is asked to move), `move_window` (an interactive move answering the pointer press being handled, sent as `WindowRequest::Move` while the press is held; the compositor then takes the pointer, whose leave revokes the node's capture), `zoom_window` (`WindowRequest::Maximize` with the opposite of the maximized state the last configure gave; neither request is sent while the last configure says fullscreen), plus the node's focus, capture and phase |
| `IdleCx` | a slice of the window's idle time: `env()` (the window's, with its text system), `has_time()`, `until()` and `request_idle()` for the work left (*Idle time*) |
| `PaintCx`, `SemanticsCx`, `MeasureCx` | `request_idle()` on `PaintCx` and `MeasureCx` (*Idle time*); painting into the node's `PaintOp`s in its own coordinates, and `set_group` to fade or shadow the whole subtree as one layer; `overlay()`, `fill_pixels` and `clip_pixels` (*Overlays*); `on_material()` (*On a material*); `material(surface, radius, alpha)` and `glass(Glass)` (*Window materials*); role, name, value, states, actions, `focusable`, `focus_scope`, `protected` |
| `Glass`, `GlassCorners`, `GlassStyle` | what `PaintCx::glass` puts under a node: `Glass::regular()`, `Glass::clear()` or `Glass::large_panel()` (regular glass with its tones pressed together and a deeper shadow, for a large floating panel), then `.radius(points)` or `.capsule()` (round ends at any size), `.container(id, spacing)` (nodes of a window in the same nonzero container join into one shape, blending over `spacing` points, 0 for a hard join) and `.alpha(a)` |
| `Command` | `Perform(future)`, `Focus(key)` (with its ring), `MoveFocus(key)` (its ring showing only if it showed: the keyboard following a click), `CloseWindow`, `Window(key, request)`, `SetClipboard`, `SetClipboardData { data, replaced }` (the data in each of its MIME types, `WindowSystem::set_clipboard`; `replaced` once `PlatformEvent::ClipboardReplaced` says another client's selection took its place, and never after the application put something else on the clipboard first), `ActivationToken` (a token for launching another application, with its app id; the message always arrives, with `None` when the window is gone or the system has none), `FinishDrop(token, action)` (what the update that handles a `Dropped` really did with it: `Some(action)` or `None` when it took nothing, told to the source instead of the agreed action), `HoldDrop(token)` (given by that update, keeps the drop open past its turn until a later `FinishDrop(token, action)`, for work that ends later), `StartDrag(Drag)` (data dragged out of a window from the held button, with an optional `DragPreview`: an `Element` laid out at its size rounded up to whole points and drawn once, over nothing, at the window's scale rounded up to whole pixels per point, with the pointer at its hotspot; every `DragProgress` step reaches `on_progress`, which hears `Finished` or `Cancelled` last, `Cancelled` at once when the drag cannot start), `OnPresented { window, message }` (`message` once the window has presented its first frame: after its first configure, at the frame callback of its first committed frame, never at the commit alone; at once when it already has; never for a window that closes first or that the next view lacks), `After { delay, message }` (`message` once `delay` has passed on the runtime's clock, those due together in the order asked; not cancellable, so the message says what it is for), `Exit` |
| `DropTarget` | an element taking data dragged onto its `content` from any application: the MIME types it reads (best first), the `DropAction`s it can do, `place` (where a drop would land, from the pointer, the keys of the keyed nodes under it, innermost first, the offered types, the source's actions and the modifiers: a `DropPlace` with the keyed node to outline, or `None` for the whole target, and the preferred action; `None` takes no drop there), an optional `on_hover` (the place as the drag enters or moves to another, `None` as it leaves or drops), an optional `on_leave` (sent by every target of the window that has one when a drag that was over the window leaves it or ends there without a drop a target took: released where nothing takes it, refused, its data failed or cancelled) and `on_drop` (a `Dropped` with its `DropToken`, the place, the position, the chosen action, the modifiers held and the data). The innermost target under the pointer that takes one of the drag's types and a drop there is asked; the runtime answers the window system with the type and actions, outlines the place (the accent, translucent, with a text-colour outline in a high-contrast appearance, cut by the node's clip), asks again after each layout while the drag stands still, reads the data at the drop and, once the turn has updated the model with it (so a source told the data moved lets it go only after the application has it), finishes it with the action the update gave in `Command::FinishDrop`, else the chosen action, or without the window system's word with the preferred action when the source allows it; a drop nothing takes, a failed read or a target gone in the meantime is refused. It lays out as its content and has no semantics: keyboard alternatives stay the application's |
| `Runtime` | `new(window system, RuntimeOptions)`, `with_theme_for(hook)` (a new theme for each `PlatformEvent::Appearance`), `theme()`, `set_theme(theme)` (every window measures and paints again, focus and compositions stay; an equal theme changes nothing), `step(max_turns)` (non-blocking, never moves time, returns `StepStatus`), `run`, `run_idle()` and `has_idle_work()` (*Idle time*), `protected_focus(window)`, `snapshot(window)`, `find`, `node_at`; `run` waits on the window system no longer than the earliest wake, and `Pending::next_wake` tells that time. A focus change to the same node (only the keyboard ring shown or hidden) sends no blur or focus, only a repaint |
| `RuntimeOptions` | theme, layout direction, node ceiling, a renderer factory (one renderer per window), an optional `AccessibilityAdapter` (told each `PlatformEvent::SessionLock` through `session_locked`), the `Clock`, an optional `on_window_error` handler |
| `MonotonicClock` | `CLOCK_MONOTONIC`, the clock the window system stamps events with |
| `Icons` | the widgets' images: `symbol(name, px, color, mirrored)`, and `application(px)`, the running application's own icon (`None` by default; `PlaceholderIcons` draws a square outline for both) |

## How it works

- **Reconciliation.** A child keeps its node when its key matches, or, without keys, its position among the unkeyed
  siblings, and its widget type is the same; anything else unmounts with its subtree. Node ids carry their slot's
  generation, and a slot whose generation would wrap is retired. More nodes than the ceiling is `UiError::NodeLimit`.
  `Element::kept(key, mark, build)` is an element made only when its place needs it: when the node at its key was
  made by `kept` with the same mark, it and its subtree stay as they are (not built, compared or marked, their
  layout reused); otherwise `build` makes the element, which updates or replaces the node as any other. The mark stands for
  everything `build` reads. A long list or grid builds only the rows near its viewport this way and keeps each one
  while it only scrolls.
- **Kept placements.** The window keeps, for each kept node's subtree with no scroll container and no raised node,
  what its descendants were placed from (each node's parent in the run, its rect and clip from layout and its least
  hit size) with the subtree's revision and size. A kept node of the same revision and size next frame has its
  descendants placed from the run by the sums placing them from the layout makes, met with the clip the node gets
  from above, unwalked; the placements are those of placing every node (`layout_view.rs`).
- **Kept semantics.** The semantics pass keeps the same for each kept node's subtree with no scroll container: the
  size each node was asked at, the accessibility nodes as the walk left them with the bounds each declared, what the
  subtree handed the node above it (children, descriptions) and its Tab stops, fallbacks, scopes, roving groups and
  items, hidden roots and keyed relations, in order. A kept node of the same subtree revision and mark next pass, whose
  nodes have the same sizes, that holds no focus and is handed no description from above, lays them in unwalked with
  their bounds from where they lie now; the keyed relations, fallbacks and focus are worked out over the whole tree
  after, as before, so the semantics, the focus order and the accessibility diff are those of a full build
  (`semantics.rs`).
- **Kept paint.** The painter keeps the paint nodes of each kept node's subtree with the subtree's revision (which
  changes when it or any descendant is marked or built again) and the ground it was painted on (what lies under it,
  its layer, the window's state). A kept node whose subtree revision and ground are the same next frame has its paint
  nodes laid in as they were, each placed where layout puts it, without walking its subtree; anything else is painted
  as usual. The list is the one a full walk paints. The damage tracker keeps such a subtree as one run of its nodes:
  laid in again from the same root with the same nodes, they are compared in order with the run, none looked up
  apart, and any other run gives its nodes back to be compared one by one, so the damage is the one comparing every
  node apart finds.
- **Events.** An event goes down the target's path (capture), reaches the target and goes back up (bubble); a handler
  can stop it. Hit-testing takes the topmost node and respects every clip. Hover enter and leave reach each node the
  pointer crosses. A raised node (`Widget::raised`, a toolbar row over a page that scrolls under it) takes a point
  before the nodes it lies over, wherever it is in the tree, and only where its own `hit` takes it. A captured pointer goes to its node alone until it is released; leaving the surface or the window
  losing the focus cancels it with `Event::Cancel`. A scroll gesture keeps the path its first event hit: a finger's
  until its stop, a wheel's or another source's until it pauses for `SCROLL_IDLE_NS`, the source changes or the
  pointer moves, and any of them until the pointer leaves the surface. A cancelled touch reaches every node its path
  went through, the touched one first. A button let go anywhere but in a popup its press opened (on the opener, in
  the opener's window, in another window) also reaches the root of every popup that press opened, nested ones
  included, as a release at a point outside it, so a menu dragged through and let go elsewhere can close; one let go
  in such a popup goes only where it lands, and popups the press did not open hear nothing of it.
- **Touch and popups.** Every event of a finger goes to the window it went down in. The first finger held is the one a
  popup opened meanwhile follows: in a window that is not a popup, what it went through hears `Event::Cancel` as the
  popup opens, so a button under it is not clicked as it lifts. Its moves reach the root of each popup it opened at
  that popup's own point; lifted over one of them, its lift is that popup's, and lifted anywhere else it goes where it
  lifts and each popup it opened hears it lift there, outside it, as a mouse release elsewhere does. A cancelled touch
  ends the interaction in those popups too, and another finger put down in one while it is held goes nowhere. The unit
  tests follow a finger through the same popups as the button.
- **Closing menus.** A popup on the menu surface, or one whose root widget draws a menu's glass itself
  (`Widget::closes_as_menu`: a menu with a pointer), that the view stops declaring fades out instead of vanishing:
  the runtime keeps its window, clears its input region, takes it out of the accessibility tree and shows it at the
  opacity `(1 - t/0.236 s)³` (the compositor's material and the glass the content declares fade with it), then
  destroys it. It hears nothing meanwhile,
  and a popup declared under its key is a new window. Only a menu's first level fades, and only when no other menu
  opens in the same view; a submenu that closes under an open menu goes at once, and Reduce Motion closes at once.
- **Scroll timing.** `Runtime::with_scroll_timing(sink)` follows every pointer scroll event to the screen: the window
  system's timestamp, when its handlers ran, the model update and view it caused (`None` when it changed no model),
  when the first frame after it was handed to the window system and when the compositor showed that frame
  (`WindowEvent::Presented`; `None` for a `Discarded` one). The sink gets each `ScrollTiming` once its frame is shown
  or dropped; `ScrollTiming::latency` gives the stages from the input. Without a sink the runtime keeps nothing; at
  most 256 events wait at each stage.
- **Frame timing.** `Runtime::with_frame_timing(sink)` times every turn that presents a frame phase by phase: update,
  view, layout, paint, raster, the window system's copy into its buffer (`WindowSystem::time_copies`) and the commit.
  A renderer that draws into memory (`UiRenderer::draws_into_memory`) draws a window's frame straight into the buffer
  the window system lends (`WindowSystem::present_into`), so it has no copy and its raster paints what the buffer
  missed too; where none is lent the frame is drawn into the renderer's own image and presented with `present`.
  A window system that draws frames away from the UI thread takes them first (`WindowSystem::submit`: the display
  list, resources and damage with the window's shared renderer, and the changes of its raised layer and surface
  layers, `Submission::raised` and `Submission::surface_layers`, each layer's list with a renderer of its own shared
  the same way, which the window system draws and commits before the frame); the window then
  draws nothing until `WindowEvent::Shown` says how the frame went, and its layers count as shown only then, and the turn counts no raster for it. `pool::cpu`
  is the process's CPU pool (up to a thread per core, `kit-cpu-N`, each started when work finds the others busy) that
  such frames and the toolkit's CPU tasks run on; a window's frames go to one lane (`pool::cpu_for`), one thread
  for all of them, the lanes sharing at most `pool::FRAME_LANES` (4) threads.
  The sink gets one `FrameTiming` a turn that presented; without a sink no clock is read.
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
  A window that just became active crosses from its inactive colours to its active ones: the runtime keeps when it
  became active and paints it, until the crossing ends, with its theme and its pinned themes carrying the progress
  (`ColorTokens::set_activation`).
  A popup still open that loses the keyboard ends it, as the keyboard may have gone to another application without
  the compositor deactivating the window.
  Keys, focus and `EventCx::window_active` follow the keyboard alone.
- **Idle time.** A node that called `request_idle` in its measure, its paint or its last idle call has its widget's
  `idle` called once, between frames: `run` hands the time out (`run_idle`) when nothing is ready and it would wait
  on the window system, in slices of 1 ms at most, each ending 2 ms before a window waiting on its frame is due by its
  frame clock (the shortest of its last eight periods between frames; a frame a whole period late no longer counts)
  and before any wake. The window system is looked at before each slice and the first event that arrives ends the
  idle time; the work left waits for the next. A node does its work a piece at a time while `has_time()` and asks
  again for what is left. The work does now what a later frame would do (Files' icon grid shapes the names of the
  cells about to be built into the text system's cache, which holds two generations of 512 paragraphs, and keeps in
  a cell of its state where its last slice stopped, so a walk longer than a slice still ends) and changes nothing
  shown: the widget's state is only read. Once no node asks, the loop waits as it does without idle work, so the
  work adds no wakeup; with work left for a frame that is due, the wait ends when the first frame holding it back is
  a period late, a time `run_idle` returns as it stops, so it is never one already past (`runtime/idle.rs`).
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
  whose opacity reaches its children), damages nothing, so a container's repaint costs no pixels. A scroll container
  that clips marks its content as a scroll layer with its drawn offset, its scroll bars beside it; a node in a layer
  the renderer keeps is compared in the layer's content, so a scroll damages nothing it only moved, and what changed
  in it damages where it shows now. A window told it is suspended drops its renderers' kept layers. A toplevel
  declares its opaque region with the frame that changes it (`WindowRequest::SetOpaqueRegion`, `opaque.rs`): the
  large solid fills at full alpha its paint tree lays outside any translucent group, under clips that are rectangles,
  placed by a move alone, a rounded one less its corners, without the materials under its content, each pulled in by a
  point and rounded inwards; so a Kit window that covers another lets the compositor suspend it. A frame that
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
  `PaintCx::set_blur_ramp(BlurRamp)` after it varies the material's blur down the node (a variable blur, carried as
  `MaterialRegion::blur_ramp`): `top` times its blur along the node's top and `bottom` times it along its bottom; a
  compositor without blur ramps blurs evenly.
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
- **Raised nodes.** A node whose `Widget::raised` says so, with its subtree, draws in the window's raised layer
  when the window system has one (`Capabilities::raised_layer`; `PaintCx::raised_layer`): a frame over the rest of the
  window from its top edge, with the raised nodes' glass between the two (`MaterialRegion::raised`), so a page
  scrolled under a toolbar row shows through the row's glass and never covers it, and scrolling repaints the page
  alone. Content under the layer is not on a material (`on_material`). The part of the window the raised nodes cover
  is obscured for the rest of it (`PaintCx::obscured`): a scroll container sees it at its top and moves its bar,
  paging and focus scrolling below it, overlays such as a caret are cut from it (`OverlayCx::reach_obscured` lets one
  draw there), and a scroll container tells assistive technology only its part below it, while its items keep their
  true bounds. Without a layer the raised nodes draw in place, after the rest of the window, their materials after the rest's
  too (`paint_order`), and scroll containers outside them clip what they scroll to the part the raised nodes leave.
- **Surface layers.** A node whose `Widget::surface_layer` returns a `SurfaceLayerMark` (a key unique in its window
  and a look) draws its subtree into a surface layer of its own where the window system shows them
  (`Capabilities::surface_layers`): after each paint the runtime moves the marked subtrees out of the window's list
  into lists of their own at their layer's origin (`runtime/surface_layers.rs`; a mark inside a marked subtree draws in
  the outer layer, and a subtree's root leaves its ancestors' clips and groups to the layer's shape and properties),
  keeps a damage tracker and a renderer per layer, and hands a layer that changed to the window system with the
  window's frame, to be drawn only where it changed, at its layout size in whole points (`surface_layer_size`, the size
  the window system's buffers of the layer are) and the window's scale (`Submission::surface_layers`), or, where the
  window system declines the frame, draws it and hands it the frames (`present_surface_layers`). The application moves, fades, scales
  and blurs a layer with `WindowRequest::Layer`, which draws no frame; once the window's layers' last running
  animation ends, the window's root hears `Event::LayersSettled`. Layout, input and semantics keep the layout.
  Without the capability the subtree paints in place.
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

- **Personal choices.** `personal()` holds the choices the person made that the widgets read: what a click in a scroll
  bar's track does, the sidebar icon size and what a title-area double click does. `with_personal_refresh` lets the
  application hand the runtime a function that reads them again; the runtime calls it, and rebuilds its theme, when the
  application wakes it after `appearance.conf` changed.

## The clipboard and a control's own popup

A control reads the clipboard's text itself: `EventCx::clipboard_has_text()` says whether the clipboard offers text
(the runtime follows `PlatformEvent::ClipboardOffered` and what the application itself copied), and
`EventCx::request_paste()` reads it, as UTF-8 text in the first kind offered of `text/plain;charset=utf-8`,
`UTF8_STRING`, `text/plain`, `TEXT` and `STRING`; the control hears `Event::Paste` with the text, and nothing when
there is none or it or its window went meanwhile. What a paste reads may be a password on its way into its field, so
every paste is handled as one: the text's debug output is its length only, the bytes read are zeroed once they are
text, and the text is zeroed when the event drops; the runtime logs and keeps neither. A password field adds its own
part (no history, nothing read to assistive technology). `Runtime::paste_protected(token)` is for what records the
events it hands the runtime, as a test driver's trace does: it tells whether a clipboard read is such a paste and
whether the control it is for protects its input, so the record leaves the secret out.

A control opens a popup of its own with `EventCx::open_popup(build)`: `build` returns its windows (`OwnedWindow`: key,
spec, background, content, anchor and whether it grabs) now and at each view while it is open. The runtime declares
them after the application's windows, in the control's window, with the grab, placement, fade and dismissal of a
declared popup; the application declares nothing and hears nothing. Inside it `EventCx::answer_popup(n)` reaches the
control as `Event::PopupAnswer(n)` and `EventCx::close_popup()` ends it; it also ends when the compositor dismisses
it, when the window system refuses it, and with its control or the control's window. One is open at a time: another
control's replaces it. A text field's menu is one. `EventCx::holds_popup()` says whether a control of the window has
one open, so a window that closes when it loses the keyboard can tell that the keyboard went to that popup. When the
compositor dismisses that popup and the keyboard does not come back with it (another surface took it), the ancestors
of the window's focused node hear `Event::FocusWithin { gained: false, .. }` once more, now with no popup open: the
loss they took for the popup's is the window's own.

## Layout (`layout`)

Constraints, two-pass measure and place over a tree borrowed through `LayoutTree`, stacks, grids, absolute and overlay
containers, scroll math and virtualization windows. It uses nothing else of the runtime but `cupertino-geometry`: no
display, font or scale factor. Values are logical points.

### Use

```rust
let result = cupertino_ui_core::layout::layout(&tree, root, Constraints::loose(window_size)?, &options)?;
let node = result.nodes[&id]; // rect in the parent's coordinates, clip in the node's own, baseline from its top
```

Leaves (text, images, icons) are measured by `LayoutTree::measure_leaf`; the engine clamps the answer into the
constraints. Text wraps because a vertical stack hands a leaf the width it has; in a horizontal stack a leaf starts from
its unconstrained width and shrinks.

### Behaviour

- **Sizing.** `SizeSpec::Fixed` and `Fraction` (of the offered maximum) keep their size and are never stretched; `Fill`
  takes the offered maximum when it is finite and its content size otherwise; `Fit` takes its content size. A node never
  leaves the range its parent offers.
- **Stack.** A child starts at its `flex.basis`, its fixed size, a fraction, or its unconstrained size. Leftover room is
  shared by `grow`, a shortfall by `shrink` times basis, each child held at its bounds, in one pass however many children
  reach a bound (`LayoutStats::flex_passes` counts the passes). A `Fit` stack does not grow past its content. A shrinking child stops at its smallest size: for a leaf what
  `measure_leaf` returns when the axis maximum is zero (text answers with its longest word), for a container the sum or
  largest of its children's, and its fixed size when it has one; children whose floors exceed the room overflow.
  Cross-axis `Align`; `Align::Baseline` lines up first baselines in horizontal stacks and acts as `Start` in vertical
  ones, where the stack's own baseline is its first child's.
- **Grid.** Fixed, auto and fraction tracks; children fill cells row by row, skipping cells a spanning child covers
  (`LayoutSpec::span`, columns and rows; a span wider than the grid is cut to it). A spanning child that needs more than
  its tracks give widens its auto tracks equally (its fraction tracks when it covers none). Fraction tracks share the leftover room when
  the grid has a definite size, otherwise they are as large as their biggest content needs.
- **Overlay / Absolute.** Children share one box. Overlay children follow `align_self` and stretch by default; absolute
  children keep their size at `LayoutSpec::position` and the container grows to hold them.
- **Scroll.** Content is measured without a limit along the scrolled axes and stretches across the others. The content
  rect keeps its full size at the origin and the node's `clip` is the viewport; the offset belongs to the model, so core
  applies it with `ScrollState` (`clamped`, `scrolled_by`, `page_step`) when painting and hit-testing.
- **Right to left.** Children are mirrored inside their parent's box, so start-aligned content follows the reading
  direction. Insets are physical and do not mirror. A horizontal scroll view in right-to-left starts at
  its right edge: the content rect is mirrored (negative `x` when wider than the viewport), so core measures the scroll
  offset from the start edge and negates it for painting and hit-testing.
- **VirtualList.** Row windowing for 50k-row lists: constant time for fixed heights, `O(log n)` and 16 bytes per row for
  estimated ones, with `anchor_adjust` when rows above the top row are measured. `with_spacing` puts a gap between rows
  (none after the last); an offset inside a gap belongs to the row above. `first..last` is half-open.
- **Cache.** `layout_cached` keeps sizing results across calls in a `LayoutCache`. A node's entries are dropped when its
  `LayoutTree::generation` changed or any descendant's did, and when it left the tree or the direction changed; a cache
  past 16 entries per node is cleared. Output equals an uncached call. Placing every node and building the result still
  costs `O(nodes)`; a warm call with no changes does no sizing or leaf measuring (the bench shows about 3.5 times less
  time).
- **Stats.** `LayoutResult::stats` counts the sizing cache misses and leaf measurements of the call; `cargo bench -p
  cupertino-ui-core --bench layout` prints them next to the time for 1k and 10k node trees. `cargo bench -p
  cupertino-ui-core --bench instructions` counts the instructions of a file manager window's solve (about 480 nodes), cold, cached and resized.
- **Errors.** Non-finite or negative spec values, a node reached twice (a cycle), more than `node_limit` nodes or more
  than 256 levels return a `LayoutError`; leaf measurements that are not finite do too. The engine does not panic on them.

### Known limitations

- A grid child cannot be placed in a chosen cell; placement is automatic. Spanning children are left out of a grid's
  smallest size, and a span's extra width goes to auto tracks equally rather than by need.
- The smallest size of a container ignores what wrapping text would need once it is narrower than the width it was asked
  at: vertical queries use the width given, horizontal ones are asked without a height limit.
- A generation must change whenever the node's spec, its list of children or its leaf content change; a leaf whose
  measurement depends on something else (a font, the scale factor) needs its generation bumped when that changes.
- `LayoutCache` bounds itself by clearing, not by evicting the least recently used entry.

## Window-system interface (`platform`)

The `WindowSystem` trait (windows, popups, presentation, compositor materials under a window or under one rectangle of
it, cursors, clipboard, input method, activation tokens, drags), its events and `Capabilities`. Implemented by
`cupertino-ui-wayland` on the desktop and by `cupertino-ui-test` headless.

`present_solid(window, Color, configure)` shows a window as one colour without a frame (`Color` is the theme's,
re-exported): the compositor stretches a single pixel over the window, so a transparent window that only takes input,
or a uniform scrim, costs nothing per frame at any size or scale. It answers like `present`; the trait's default, and a
system that cannot, refuse with `PlatformError::Unsupported`, and the caller presents a drawn frame instead.

`present_into(window, size, configure, damage, draw)` lends the renderer a buffer of the window to draw the frame into,
with what the buffer misses (the frames shown in the window's other buffers since it held one, the damage of frames not
shown, or `None` when its pixels are unknown), and presents what `draw` drew with the damage it returns: the frame
reaches the buffer without a copy. With no free buffer, no configure yet or a buffer of another size, `draw` is not
called, `damage` carries over and the frame is `NotReady` (`Lent::Drawn`). `Lent::No`, the trait's default, asks for
the frame drawn into the renderer's own image and presented with `present`.

A popup asks for a place beside an anchor (`PopupSpec`); where the compositor put it, after flipping or sliding it to
fit, arrives as `WindowEvent::PopupPlaced(rect)`, in its parent's coordinates, before each configure that places it.

The compositor may tell whether the face of a glass material is dark or light: `WindowEvent::GlassTone { region, tone }`
names the region by its index in the last `WindowRequest::Materials` list, or the window's own backdrop with `None`,
and `GlassTone::{Appearance, Dark, Light}` picks the colour of the text over it. A system that cannot tell sends none.

With `Capabilities::raised_layer` a window may show a raised layer over its content (`WindowSystem::present_layer`, a
`RaisedLayer` frame from the window's top edge, as wide as the window): the materials marked `MaterialRegion::raised`
lie between the two and read the content below them, so content that scrolls under a toolbar row shows through the
row's glass without covering it. The layer is presented with the window's next `present` and `None` hides it; without
the capability a layer is `Unsupported`. A window system that takes frames (`WindowSystem::submit`) gets the layer with
the frame instead (`Submission::raised`, `RaisedChange`: kept, hidden, or drawn from a `LayerList`).

With `Capabilities::surface_layers` a window may show marked parts of it in surfaces of their own that the compositor
places and animates (`WindowSystem::present_surface_layers`): each `SurfaceLayer` has a key, its rect in the window,
its look (`SurfaceLayerLook`: glass style, the glass's lens magnification about its centre (1 to `MAX_LAYER_LENS`) and
its `BlurRamp` (the blur's weight from 0 to 1 at its top and its bottom edge), glass group and spacing, corner radius,
masks to its shape, and a
`LayerEdge`, a line the compositor draws along the outside of the glass's outline, in a group each shape's outline cut by the others, at
a width in points that stays as the layer scales, where `Capabilities::surface_layer_edges` says it can) and its frame at
its rect's size in whole points (`surface_layer_size`: each side rounded, at least one point), or `None` when unchanged; the list is shown with the window's next `present`, back to front over its
content, and a key left out goes. A window system that takes frames gets the list with the frame instead
(`Submission::surface_layers`, each a `SubmittedLayer` whose `LayerList` it draws). `WindowRequest::Layer { key, changes }` sets or animates a layer's properties
(`LayerChange`, `LayerProperty`: x, y, width, height, opacity, scale, corner radius, content blur, along a
`LayerCurve`, or with `LayerChange::through` through values the application works out itself at even steps of a
duration) on the compositor's clock without a frame. Without the capability an empty list is all a window system
takes.

`RendererClass` is the renderer a process's windows belong on by what they draw (`Cpu`, `Gpu`, or `ByUse`: the CPU until a
window is large or busy); `PlatformEvent::RendererChanged(RendererKind)` says the process now draws with the other one.

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

## Not yet

An application's `update` cannot read the clipboard: a paste goes to the control that asks for it, and the runtime
reads other offered data only for a drop.
