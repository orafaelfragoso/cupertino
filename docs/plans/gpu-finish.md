# Plan: finishing the GPU rendering plan

What is left of `docs/plans/gpu-rendering.md` once the owner's checks are deferred and steps 12 and 13 are on hold.
Each task is one change with its own tests, in the crates it names; tasks that share a crate run one after the other.
The boxes here are ticked together with their items in the GPU plan.

## Rules

- One task per implementer, about an hour of work. Each implementer runs the tests of its own change only, plus
  `cargo fmt`, clippy on its crates, `ci/gate.py` and `ci/test_suite.py --check`; no benchmark, no `scripts/done`, no
  slow tier.
- At most three implementers at a time, never two in the same crate.
- A change that adds a test tier clause to `.config/nextest.toml` adds it as the first clause of both filters, and
  `scripts/test -p cupertino-ui-abi` proves the file still parses.
- No `sleep` or `pgrep` loops waiting on another process. A long command runs in the background, and its notice is
  waited for.
- The full gates (`scripts/done`, `scripts/test --slow`, the slow tier with `CUPERTINO_GPU_REQUIRED=1`) and every
  benchmark run once, in the last wave, on merged `main`.

## Wave 1: independent pieces (in parallel)

### 1a. 10 bits by default (`cupertino-server`, `cupertino-defaults`)

- [ ] `Depth::from_setting` gives 10 bits when the key is absent; `"output_depth": 8` keeps 8. A plane without a
      10-bit format still falls back to 8 bits.
- [ ] The settings table in `cupertino-defaults`, `docs/src/displays-and-gpus.md` and the GPU plan's step 11 item
      say so.
- [ ] Tests: absent key gives 10 bits, `8` gives 8, and a plane offering only 8-bit formats gets 8 bits.

### 1b. Docs sweep (`docs/`, crate READMEs)

- [ ] `docs/src/architecture.md` names `cupertino-gpu`, `cupertino-gpu-shaders` and `cupertino-gpu-slice` in its
      layer table.
- [ ] `docs/src/gpu-rendering.md` no longer says CI runs with `CUPERTINO_RENDERER=gpu` (or CI does, see 2c). Its
      section on allocation names the fallback when there is no allocator.
- [ ] The project's term is "control panel" on the lines the GPU work added.
- [ ] Each page the GPU plan's steps changed (`apps-on-kit.md`, `kit-getting-started.md`, `displays-and-gpus.md`,
      crate READMEs) matches the code. This ticks the plan's documentation item.

### 1c. The search field's open on the compositor's clock (`cupertino-launcher`)

- [x] The handed open, and the caret hold with it, ends on the layer's `finished` or `settled` event, not on the
      launcher's own timer. The window system tells a window its layers came to rest (`WindowEvent::LayersSettled`,
      once, when its last running animation ends), which the panel hears through `FrameClock::on_layers_settled`; a
      timer a second past the open's end (`OPEN_REST_FALLBACK`) takes it back if no word comes, and carries the open's
      number, so an earlier open's timer ends nothing
      (`a_handed_open_ends_on_the_compositor_s_word_or_on_the_timer_well_past_it`). The 4 stray buffers were one
      commit of the content layer about 700 ms in: an earlier open's timer, or any other message, rebuilt the view
      and the held caret was painted where its fade had come to, not as it was shown. A held caret now paints as it
      was last drawn (`a_held_caret_painted_again_part_way_through_its_fade_is_drawn_as_the_hold_began`).
- [ ] The caret blinks through the open. It becomes a layer of its own whose opacity goes through samples of the
      panel's blink, so a blink makes no buffer. Not done: the caret is drawn in the field's overlay at a place only
      its paint knows, and its blink starts when the keyboard focus comes, which the compositor gives after the window
      maps, later than the open's samples are sent. A caret layer needs the field to draw its caret into a layer of
      its own and tell when its blink started; meanwhile the held caret stays shown through the open (255 at the
      caret from 550 to 1000 ms, where the panel's own drawing fades it).
- [x] `Model::hand_field_opens` on. Against the server, the real process commits no buffer of its window or its
      layers while the open is part way, in 3 runs
      (`shell_processes::the_launcher_opens_a_scope_on_the_compositor_s_clock_without_a_new_buffer` over
      `ToggleLauncher`). Its warm-up wait reads the launcher's renderer record as `gpu`, instead of waiting for 500 ms
      of quiet. Measured with a variant of that test over `ToggleLauncher`, not kept in the suite (the window 760 points wide with 4 layers, part way
      while any layer still animates): 0 buffers in each of 8 opens over two runs (186 to 188 server steps part way); the
      only commits come after the layers rest, the field's glass dropping the edge and the button's face drawing it
      again. The scope's open as committed: 0 in 3 runs over 92 to 95 steps part way.

## Wave 2: stability fixes from the review (in parallel with wave 1's launcher chain)

### 2a. Compositor (`cupertino-server`)

- [ ] Content blur cropped to the visible output plus its reach and capped in size; past the cap the layer draws
      sharp. Test: a 1x1 buffer with a 20000 x 20000 destination allocates within the cap.
- [ ] A layer is always clipped to its window, with or without `set_masks_to_bounds`, and with client-side
      decorations too. Test: a layer placed outside its window draws nothing there.
- [ ] Layer requests validated: the gaps the review found, each rejected with a protocol error and tested.
- [ ] A dismissed popup's linger ends on a timer of its own, so the frame that removes it comes without other damage.
- [ ] The late frame start counts the GPU's time: the latch is fed the time the render fence signals (or a kept
      estimate from lateness), and backs off for longer after repeated misses. Test: a frame cheap on the CPU but
      heavy on the GPU gets no late start.

### 2b. GPU device and toolkit (`cupertino-gpu`, `cupertino-ui-wayland`, `cupertino-ui-gpu`, `cupertino-files`)

- [x] Device loss releases every image and buffer object (only the GL deletes are skipped), and nothing is deleted
      with another context current. Test: after a loss, `Device::live_textures()` is 0 and the buffer objects are
      freed.
- [x] Without explicit sync, a GPU window never stops drawing for good: the state the review traced recovers. Test:
      the stall it describes, forced, ends in a presented frame.
- [x] `take_turn` tells "cannot be held" from "timed out", and `make_ahead` gives up at once on the former. Test: an
      unwritable cache directory returns within one poll.
- [x] The client's warm-up thread is stopped and joined at exit, as the server's is.
- [ ] The Files grid's idle work resumes where its last slice stopped, so a folder of long names ends its idle work.
      Test: on a clock that gives each slice too little time for the whole walk, the work still ends.
- [ ] With two windows and idle work left, the run loop no longer waits with a zero timeout over and over
      (`runtime/idle.rs`).
- [ ] A draw-into frame that finds no free buffer marks its damage again, so kept scroll images are repainted where
      that frame would have drawn. Test: a frame skipped for want of a buffer, then the next frame, shows the change.

### 2c. Tests and scripts (`scripts/`, the server's and `cupertino-ui-gpu`'s GPU tests)

- [ ] `scripts/test --slow` and `scripts/done` set `CUPERTINO_GPU_REQUIRED=1` when the machine has a render node, so
      the render-node tests cannot skip there; without one they skip as before.
- [ ] `gpu_slice`'s missed-refresh count leaves out the frames a synchronous screenshot stalls, and counts by frames,
      not a fixed three seconds.
- [ ] A server test of the search field's open over `ToggleLauncher` (the window 760 points wide with 4 layers,
      `--no-files`, the field opened and closed once first) counts no buffer of the window or its layers while any
      layer still animates, in the way the scope's open test counts its own.
- [ ] `cupertino-ui-gpu`'s `a_large_windows_damaged_frame_stays_within_the_gpu_budget` times the GPU's own work (a
      timer query), not wall-clock time with other GPU users in it.

## Wave 3: the launcher's fold and unfold (`cupertino-launcher`, after 1c)

### 3a. Row and body as layers

- [x] The query row and the body are layers of their own instead of the one content layer (`layers::ROW_LAYER`, the
      query bar's own mark, and `layers::BODY_LAYER`, a `layers::ContentPart` from the content's top with room for
      the row, so both lie on the content's device pixels). At rest they are the panel's pixels within 1 of 255 at 1,
      1.5 and 2 in both appearances (`a_resting_panel_s_layers_carry_the_pixels_and_the_glass_it_draws_in_place` over
      the field, its results, the browser and the files scope). The opens and the close are as before at the same
      moments; against the server the close 0 buffers over 14 to 16 steps part way with 6 layers, the browser's open 0 over
      91 to 96 with 4. The pixel goldens are unchanged; the tree goldens gain the body's node.
- [x] The fold (`Escape` from a scope): the glass's place, size and corner go through samples of the panel's own values
      on the shape spring (`layers::fold_changes` over `Motion::shaping_left`, the enclosure's motion without the
      buttons' later bud), with the corner held to half the height. The body's opacity goes through samples of its
      own curve and the row's does not. The row's frame is drawn again once, at the header's switch
      (`Message::HeaderSwitch`), the only buffer the fold makes part way; the fold's first frame commits the glass's
      look once (its edge, no pixels), and the edge is the compositor's along the glass. Behind `Model::hand_folds`,
      off in the process: the compositor's edge follows a scope's continuous corners where the panel strokes circular
      arcs, so until 3b the fold's first frames part at the corners. Tests: the counts
      (`a_scope_folds_on_the_compositor_s_clock_drawing_its_row_once`, both scopes: no window frame, one row frame);
      at the same moments (`the_fold_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`, every other
      frame of 30 and the last, 1.5, both scopes and appearances) the glass is the panel's at every moment, every pixel
      away from the outline and the caret within 0 of 255; along the outline 178 of 255 in the dark and 52 in the light
      (the line's 237 and 69) while the corners are a scope's, within 1 from 300 ms, when the shape is the field's
      capsule; the caret, which the field holds, 255 where the panel's own blinks. Under Reduce Motion nothing is
      handed over. The panel's own fold now lies on whole device pixels and keeps the row's height where the shape
      springs shorter than the field (`PanelView::moving`).
- [ ] The unfold (the scope's shortcut from the field). Not done: its first frame needs the scope's body drawn into
      the body's layer, masked to the glass as it grows (`masks`, with the glass's samples), and the mode buttons'
      layers kept through their merge (as `layers::field_open_changes` carries them); the same-moment test's
      composition does not yet apply a layer's mask.

### 3b. The edge follows the glass

- [ ] The panel strokes its own edge along the glass's continuous outline, using the shared distance of
      `cupertino-glass` and `ui_fill`, instead of circular arcs. This applies under Reduce Motion and without the
      capability too. The goldens that change at a scope's corners are re-recorded, named with their pixel counts,
      and shown as before and after pairs at 1.5 and 2 in both appearances.
- [ ] The scopes' rest, open and close use the compositor's edge (`layers::glass_edge`, within 4 of 255 of the
      panel's along every side and corner), and the scope's edge layer goes.

### 3c. The fold and unfold on the compositor's clock

- [ ] The fold and unfold carry the row, the body, the glass and the edge.
- [ ] The same-moment test, as the open's (`open_moments` over `Escape` from a scope, and the scope's shortcut from
      the field).
- [ ] The real process against the server: one buffer of the row's layer at the switch, and none other.
- [ ] The GPU plan's open test and mid-way look items, which waited on these, ticked.

### 3d. The caret's blink through the field's open (`cupertino-ui-controls`, `cupertino-launcher`)

- [ ] The field draws its caret into a layer of its own and tells when its blink started; the launcher sends the
      caret layer's opacity as samples of the blink through the open, so the caret blinks with no buffer. Test: the
      caret's pixels are the panel's own at the same moments.

## Wave 4: the lead's gates and measurements (merged `main`)

- [ ] `scripts/done`, `scripts/test --slow` and the slow tier with `CUPERTINO_GPU_REQUIRED=1`.
- [ ] The benchmarks, run once, each of the GPU plan's "measured at the end" figures recorded where it stands:
  - Files' scroll and layout (step 4);
  - the control panel's opening frames and the memory per process (step 7);
  - the window drag over glass, its p99 taken over a span long enough to show the once-a-second driver stall as
    such (step 2).
  A budget still missed is recorded with its figures and its cause, not hidden.
- [ ] Packages built, and the owner's before and after sheet for the fold and unfold.
