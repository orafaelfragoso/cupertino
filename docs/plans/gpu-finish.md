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

- [x] `Depth::from_setting` gives 10 bits when the key is absent; `"output_depth": 8` keeps 8. A plane without a
      10-bit format still falls back to 8 bits.
- [x] The settings table in `cupertino-defaults`, `docs/src/displays-and-gpus.md` and the GPU plan's step 11 item
      say so.
- [x] Tests: absent key gives 10 bits, `8` gives 8, and a plane offering only 8-bit formats gets 8 bits.

### 1b. Docs sweep (`docs/`, crate READMEs)

- [x] `docs/src/architecture.md` names `cupertino-gpu`, `cupertino-gpu-shaders` and `cupertino-gpu-slice` in its
      layer table.
- [x] `docs/src/gpu-rendering.md` no longer says CI runs with `CUPERTINO_RENDERER=gpu` (or CI does, see 2c). Its
      section on allocation names the fallback when there is no allocator.
- [x] The project's term is "control panel" on the lines the GPU work added.
- [x] Each page the GPU plan's steps changed (`apps-on-kit.md`, `kit-getting-started.md`, `displays-and-gpus.md`,
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
- [x] The caret blinks through the open. It becomes a layer of its own whose opacity goes through samples of the
      panel's blink, so a blink makes no buffer (3d).
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

- [x] Content blur cropped to the visible output plus its reach and capped in size; past the cap the layer draws
      sharp. Test: a 1x1 buffer with a 20000 x 20000 destination allocates within the cap.
- [x] A layer is always clipped to its window, with or without `set_masks_to_bounds`, and with client-side
      decorations too. Test: a layer placed outside its window draws nothing there.
- [x] Layer requests validated: the gaps the review found, each rejected with a protocol error and tested.
- [x] A dismissed popup's linger ends on a timer of its own, so the frame that removes it comes without other damage.
- [x] The late frame start counts the GPU's time: the latch is fed the time the render fence signals (or a kept
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
- [x] The Files grid's idle work resumes where its last slice stopped, so a folder of long names ends its idle work.
      Test: on a clock that gives each slice too little time for the whole walk, the work still ends.
- [x] With two windows and idle work left, the run loop no longer waits with a zero timeout over and over
      (`runtime/idle.rs`).
- [x] A draw-into frame that finds no free buffer marks its damage again, so kept scroll images are repainted where
      that frame would have drawn. Test: a frame skipped for want of a buffer, then the next frame, shows the change.

### 2c. Tests and scripts (`scripts/`, the server's and `cupertino-ui-gpu`'s GPU tests)

- [x] `scripts/test --slow` and `scripts/done` set `CUPERTINO_GPU_REQUIRED=1` when the machine has a render node, so
      the render-node tests cannot skip there; without one they skip as before.
- [x] `gpu_slice`'s missed-refresh count leaves out the frames a synchronous screenshot stalls, and counts by frames,
      not a fixed three seconds.
- [x] A server test of the search field's open over `ToggleLauncher` (the window 760 points wide with 4 layers,
      `--no-files`, the field opened and closed once first) counts no buffer of the window or its layers while any
      layer still animates, in the way the scope's open test counts its own.
- [x] `cupertino-ui-gpu`'s `a_large_windows_damaged_frame_stays_within_the_gpu_budget` times the GPU's own work (a
      timer query), not wall-clock time with other GPU users in it. The damaged frame takes about 0.05 ms on the GPU
      here. The test's other check, a whole frame recorded and issued within 4 ms of the thread's CPU time, holds an
      optimized build (2.4 to 2.6 ms of CPU time, at the commit that set the budget and now); the unoptimized build the
      test scripts run takes 5.6 to 6.2 ms at both, so it was never a slowdown, and that build is held to 10 ms instead,
      the optimized budget scaled by its measured cost. The damaged frame's figures do not depend on the build (0.05 ms
      on the GPU in both; without timer queries 0.18 ms of CPU time optimized and 0.5 ms not, well within 4 ms).

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
      off in the process until 3b, when the panel's edge followed the glass and it was turned on. Tests: the counts
      (`a_scope_folds_on_the_compositor_s_clock_drawing_its_row_once`, both scopes: no window frame, one row frame);
      at the same moments (`the_fold_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`, every other
      frame of 30 and the last, 1.5, both scopes and appearances) the glass is the panel's at every moment, every pixel
      away from the outline and the caret within 0 of 255; along the outline 178 of 255 in the dark and 52 in the light
      (the line's 237 and 69) while the corners are a scope's, within 1 from 300 ms, when the shape is the field's
      capsule (since 3b within 2 throughout); the caret, which the field holds, 255 where the panel's own blinks. Under Reduce Motion nothing is
      handed over. The panel's own fold now lies on whole device pixels and keeps the row's height where the shape
      springs shorter than the field (`PanelView::moving`).
- [ ] The unfold (the scope's shortcut from the field). Not done: its first frame needs the scope's body drawn into
      the body's layer, masked to the glass as it grows (`masks`, with the glass's samples), and the mode buttons'
      layers kept through their merge (as `layers::field_open_changes` carries them); the same-moment test's
      composition does not yet apply a layer's mask.

### 3b. The edge follows the glass

- [x] The panel strokes its own edge along the glass's continuous outline, using the shared distance of
      `cupertino-glass` and `ui_fill`, instead of circular arcs. This applies under Reduce Motion and without the
      capability too. The goldens that change at a scope's corners are re-recorded, named with their pixel counts,
      and shown as before and after pairs at 1.5 and 2 in both appearances. The toolkit gained the shape it needed:
      `Shape::Superellipse` (a corner's extent and exponent, and an outset along the distance, so a stroke lies just
      outside the outline), drawn by the CPU's analytic coverage and by `ui_fill` (the exponent it already took, the
      outset a new member of its block, `FillBlock::outline`, and a clip's in `ClipBlock::corner`), and
      `glass_outline` in `cupertino-ui-core`, a glass shape's continuous outline (a capsule's comes back as the same
      rounded rectangle as before). Tests: the shared coverage vectors of continuous corners, a stroke moved out by
      half its width covering the band outside the outline within 1 of 255 at 1, 1.5 and 2
      (`a_superellipse_stroke_moved_out_by_half_its_width_lies_just_outside_its_outline`), the GPU against the CPU on
      generated superellipse fills, strokes and clips (`parity::superellipses_match_the_cpu`); against the
      compositor's edge at rest the panel's is now within 1 of 255 along every side and corner, the corners within
      0.9 (`the_edge_the_compositor_draws_along_the_glass_is_the_edge_the_panel_draws`). No golden changes: the
      launcher's goldens draw without glass, where the edge keeps the opaque fill's circular corners, and the
      server's launcher golden is the search field, whose capsule is stroked as before. The before and after pairs of
      a scope's corner at rest are in the owner's sheet under "Edge follows the glass".
- [x] The scopes' rest, open and close use the compositor's edge (`layers::glass_edge`, within 4 of 255 of the
      panel's along every side and corner), and the scope's edge layer goes. A scope's glass layer carries the edge
      at rest, through its open from hidden and its close (`layers::compositor_edge`), and neither `open_changes` nor
      `close_changes` moves an edge layer for it; the search field keeps its edge layer at rest and through its close.
      Along the outline at rest, 1, 1.5 and 2, both appearances: within 1 of 255 (corners 0.6 to 0.9)
      (`the_edge_the_compositor_draws_along_the_glass_is_the_edge_the_panel_draws`, a scope's own edge drawn in
      place); the resting layers with that edge are the panel's pixels within 1
      (`a_resting_panel_s_layers_carry_the_pixels_and_the_glass_it_draws_in_place`). The open from hidden at the same
      moments: along the edge within 1 of 255 in both scopes and appearances, where the scaled edge layer parted by up
      to 90 (`the_open_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`, now bound at 4). Against
      the server the browser's open 0 buffers over 113 steps part way with 3 layers (the test now counts the glass
      growing in from where the open starts, as no layer scales), the field's close 0 over 15 with 6, the field's open
      0 over 181 with 5 (that test still counted 4 layers, from before the row and the body split; corrected). The
      scope's close is not compared at the same moments (that test is the search field's): the compositor's edge
      there follows the glass as it grows and fades, unblurred, where the panel's own close blurs its edge with the
      content.
- [x] The fold uses the compositor's edge too, and `Model::hand_folds` is on in the process: at the same moments
      (`the_fold_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`, now bound at 4 along the
      outline) along the outline 2 of 255 in the dark and 1 in the light, where it was 178 and 52.

### 3c. The fold and unfold on the compositor's clock

- [x] The unfold (the scope's shortcut from the field) handed over as the fold is: the scope's body drawn into the
      body layer at the first frame, that layer masked to the glass as it grows, the mode buttons' layers carried
      through their merge, and masks in the same-moment test's composition
      (`the_search_field_unfolds_on_the_compositor_s_clock_drawing_its_row_at_most_once`; under Reduce Motion nothing
      is handed over). The search field rests on the compositor's edge too, its edge layer gone (within 1 of 255 of
      the panel's own at 1, 1.5 and 2, both appearances), and the browser's view options button is a layer of its own
      (`layers::OPTIONS_LAYER`), which keeps to the row's end as the glass's width changes.
- [x] The fold and unfold carry the row, the body, the glass and the edge; `Model::hand_folds` and
      `Model::hand_unfolds` on in the process.
- [x] The same-moment test, as the open's (`open_moments` over `Escape` from a scope, and the scope's shortcut from
      the field). The unfold, 1.5, both scopes and appearances: the glass the panel's at every moment, the last frame
      within 1 of 255, apart from the outline and the caret within 1 at every frame; along the outline the browser
      parted by up to 20 of 255 (dark) and 18 (light) where its body's content crosses the moving edge, and the files
      scope by up to 18 and 14 where its shape waits as the field's capsule under the scope's header
      (`the_unfold_the_compositor_runs_is_the_panel_s_own_drawing_at_the_same_moments`). With the window held at one
      size through the unfold (both drawings in it, each panel on whole device pixels in it at 1.5) the bound is split
      by cause: `UNFOLD_BODY_CUT_ALONG_OUTLINE`, 47, the browser's body cut a fraction of a device pixel apart where it
      crosses the moving edge (47 in the dark and 37 in the light at one moment, 100 ms in, where its scroll indicator
      crosses the right edge; 4 at most at the others), and `UNFOLD_GROWING_CAPSULE_ALONG_OUTLINE`, 15, the files
      scope's edge stroked along the shape the panel draws and along the glass the compositor moves, apart along the
      capsule's ends while the shape waits (7) and along the bottom and the right edge as it grows out of the capsule
      (15 in the dark, 11 in the light).
      A scope's close at the same moments too (the browser and the files scope, dark): within 1 apart from the
      outline, along it up to 100 of 255 at its first frame (29 in the light for the field and the browser), as the
      compositor's edge is not blurred with the content.
- [x] The close's edge blurs with the content: `cupertino_layers_v1` version 6 softens the line along a layer's glass
      by that layer's `content_blur`, with no new request (`layers_v1::SOFT_EDGE_VERSION`; a layer object below 6
      keeps it sharp). The glass draws the band blurred by a Gaussian in closed form, `σ·ψ(x/σ)` with
      `ψ(u) = u·Φ(u) + φ(u)` over the pixel and the band, so a deviation of 0 is the sharp band to the bit
      (`GlassEdge::soft`, `soft_edge_share`, the deviation in the uniform's spare `uv_x.w`, the glass reaching three
      deviations further), and the server passes the content's deviation as the content is drawn: scaled with the
      layer, none under a third of a device pixel (`render::layers::edge_softness`). `close_changes` gives the glass
      layer the content's `ContentBlur` change. Tests: the closed form against the sharp band convolved numerically,
      within 2·10⁻⁴, keeping its whole coverage, and 0 to the bit
      (`a_soft_edge_is_the_sharp_band_blurred_by_its_gaussian`); against the server a capsule's edge with a content
      blur of 2 points at 2 is the model's soft band within 1 of 255 (0.64 at most) where the sharp line does not
      reach, a version 5 layer's stays sharp to the bit, and with the blur back at 0 the frame is the sharp one to the
      bit (`a_glass_edge_softens_with_the_layer_s_content_blur_from_version_6`). The close at the same moments, 1.5,
      now the field, the browser and the files scope in both appearances (the scopes in the light in a slow test of
      their own, `a_scope_s_close_in_the_light_is_the_panel_s_own_drawing_at_the_same_moments`, as all six overran
      the runner's 120 s): along the outline 2 of 255 in the dark and 1 in the light at the first frame and 1 from
      33 ms on, where it was up to 100 and 29; elsewhere within 1 (bound along the outline now 4, was 104). The
      field's close is no worse than when it blurred its own edge layer, when the test held the whole frame within 8
      at the first frames. No golden changes.
- [x] The real process against the server: one buffer of the row's layer at the switch, and none other. The fold
      holds: 1 buffer of the row's layer and none other over 83 to 94 server steps animating, in 3 runs
      (`shell_processes::the_launcher_folds_a_scope_on_the_compositor_s_clock_drawing_its_row_at_most_once`). The
      unfold grew the window from the field's size to the scope's as it began, and the frame drawn again at the new
      size landed after the compositor started (2 buffers of the row's layer and 7 of the window and its other layers
      over 94 steps). Now, where the window system shows layers and Reduce Motion is off, the panel's window keeps the
      largest size the panel takes for as long as it shows (`Model::window_held`, `Model::largest_extent`: both scopes
      on the output known and the tallest list of results, each with its shape spring's overshoot from the field, on
      the shape grid and within the room the output leaves below the panel), from the first frame on, so a fold that
      settles no longer shrinks it; without layers or under Reduce Motion the window sizes as before. The input region
      stays the enclosure and the mode buttons, so a press elsewhere in the larger window reaches the catcher
      (`the_panel_s_window_keeps_the_largest_size_and_takes_input_on_the_panel_alone`). The unfold has its own flag,
      `Model::hand_unfolds`, on in the process: 0 buffers of the row's layer and 0 of the window and its other layers
      over 88 to 94 server steps animating, in 3 runs, the window at one size throughout and each of its 4 layers
      drawn once in the first frame
      (`shell_processes::the_launcher_unfolds_the_search_field_on_the_compositor_s_clock_drawing_its_row_at_most_once`;
      with the window sized as before it counts 1 buffer of the row's layer and 2 others, and the window at another
      size for 76 steps). The launcher's server tests tell the panels apart by their glass (the browser's 844 points
      wide, the field's capsule 57 tall) and the panel's close by its window going, since the window no longer
      changes size between panels.
- [ ] The GPU plan's open test and mid-way look items, which waited on these, ticked.

### 3d. The caret's blink through the field's open (`cupertino-ui-controls`, `cupertino-launcher`)

- [x] The field draws its caret into a layer of its own and tells when its blink started; the launcher sends the
      caret layer's opacity as samples of the blink through the open, so the caret blinks with no buffer. Test: the
      caret's pixels are the panel's own at the same moments. A node's overlay can be a surface layer of its own
      (`Widget::overlay_layer`, `cupertino-ui-core`), inside a marked subtree too, over the layer the node lies in and
      at that layer's rect, so the caret starts where the query row's layer would draw it and moves with the row's own
      samples. `BareField::caret_layer` draws the caret there fully shown, its blink drawing nothing;
      `BareField::blink_start` lets the owner start the blink: taken by the field's first focus gain, window
      activation or frame after the start, after which neither starts it over (an edit, a move or a press still
      does). The launcher starts it at each show from hidden, so the panel's own open and the compositor's blink from
      the same moment whenever the keyboard focus comes; sending the samples once focus comes was not sound, as the
      compositor starts a change when it first draws it, a frame or more after it is sent, and focus comes after the
      window maps. The caret layer's opacity is the content's fade times the field's blink (`CaretBlink::opacity`,
      the theme's blink, `CaretBlink::default` until the field tells its own through `BareField::on_blink`), at
      steps of about a millisecond to the first moment past the open from which the blink holds for 0.1 s
      (`CaretBlink::steady_from`, `CARET_HOLD`), so when the layers rest the caret goes back into the row's layer at
      the opacity the compositor shows. A key or a press part way (the field tells its blink started over), or a
      steady caret, takes the caret back into the row's layer at once; the open runs on
      (`a_key_during_the_search_field_s_open_shows_the_caret_at_once`). Tests: at the same moments, 1.5, both
      appearances, the caret within 1 of 255 at every moment from 117 ms on, through the whole blink (was 255 from
      550 to 1000 ms); 35 at 50 ms and 19 at 83 ms while the content fades in, where the caret's layer fades over the
      row's and the panel fades both as one group and the caret crosses the placeholder's ink (bound 40); the first
      and last frames within 1; elsewhere unchanged (17 in the dark, 7 in the light). Against the server the field's open makes 0 buffers of the window and its 6 layers over 183 server steps animating (`shell_processes::the_launcher_opens_the_search_field_on_the_compositor_s_clock_without_a_new_buffer`, its count of layers 6 with the caret's). At rest and
      under Reduce Motion nothing changes; the goldens pass unchanged. The blink a field tells is the one it draws, an
      owner's start holds through the window's activation, and a caret in its layer draws no frame as it blinks
      (`bare_field::`).
- [x] The same-moment tests (`view::tests::frames::layers::`) check every moment they did, to the same figures, in 5
      to 16 s each beside one another (11 to 29 s before; 40 to 90 s before the build optimised its drawing maths):
      the tests' own composition skips empty frames, empty texels and a layer's own-scale bilinear weights, and the edge
      model the pixels past the line's reach; the walks keep window pixels only for the frames they take.
      `CUPERTINO_LAUNCHER_QUICK=1` checks only the key moments (the first, the last, the header's switch, where a bound
      changes and where the figures come nearest their bounds), 2 to 10 s each.
- [x] A steady caret on the first open: the launcher follows the session's steady caret option from its start and at
      each change (`Message::SteadyCaret`, the option the field's theme follows), so the search field's very first
      open hands the compositor no blink and the caret stays in the query row's layer; a steady caret mid-open goes
      back into the row's layer at once. Test: `with_a_steady_caret_the_first_open_never_hides_the_caret`.

## Wave 4: the lead's gates and measurements (merged `main`)

- [ ] `scripts/done`, `scripts/test --slow` and the slow tier with `CUPERTINO_GPU_REQUIRED=1`.
- [ ] The benchmarks, run once, each of the GPU plan's "measured at the end" figures recorded where it stands:
  - Files' scroll and layout (step 4);
  - the control panel's opening frames and the memory per process (step 7);
  - the window drag over glass, its p99 taken over a span long enough to show the once-a-second driver stall as
    such (step 2).
  A budget still missed is recorded with its figures and its cause, not hidden.
- [ ] Packages built, and the owner's before and after sheet for the fold and unfold.
