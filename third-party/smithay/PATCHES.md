# Smithay: upstream release and patches

This tree is the published `smithay` 0.7.0 package from crates.io with the patches below applied in order. Every
change to a file of the release is one of these patches, and each one has its entry in the Unreleased section of
`CHANGELOG.md`.

`cargo xtask vendored` (also run by `cargo xtask ci` whenever this tree changes, and in every profile with the gate)
reads the tree as git tracks it, reverses the patches from the last to the first, and compares every file with
`UPSTREAM.sha256`. It fails on a file that is missing, extra or different, and on a patch that does not reverse
exactly. It fetches nothing.

## Upstream release

- `smithay` 0.7.0, the package `https://crates.io/api/v1/crates/smithay/0.7.0/download`, sha256
  `740cea6927892bc182d5bf70c8f79806c8bc9f68f2fb96e55a30be171b63af98`; its `.cargo_vcs_info.json` names the source
  commit `a166cf4c94b5aedc332a65aa1dd753e8148829c3`.
- `UPSTREAM.sha256` lists all 270 files of the package, `sha256sum` format, sorted by path. It is checked with
  `sha256sum -c UPSTREAM.sha256` in an unpacked copy of the package.
- The files the packaging writes are kept exactly as published and are covered by the hashes like any other file:
  `Cargo.toml` is the normalised manifest the build reads, `Cargo.toml.orig` the manifest as written in the source
  repository, `.cargo_vcs_info.json` the source commit. No patch changes them.
- `PATCHES.md`, `UPSTREAM.sha256` and `patches/` are this record, not part of the release; the check leaves them out.

## Patches

The order matters: a later patch may change lines an earlier one added (the changelog in particular), so the check
reverses them from the last to the first. Each is a unified diff with paths relative to this directory, `a/` and `b/`
prefixed, applied without fuzz.

1. `patches/0001-selection-refusal.patch`
   - What: `SelectionHandler::allow_selection` is asked before a client's set_selection replaces the clipboard or the
     primary selection (wl_data_device, primary selection, wlr and ext data control), and
     `SelectionHandler::allow_receive` before a client reads an offered selection or the data of a drag, during it or
     after its drop. A refused source is cancelled; a refused read closes the client's pipe; a read whose seat is gone
     is refused. `SelectionSource::provider` names the protocol object that set a selection.
   - Why: the compositor keeps clients from setting or reading the clipboard while the session is locked, and decides
     per client who may read it.
   - Files: `CHANGELOG.md`, `src/wayland/selection/mod.rs`, `src/wayland/selection/offer.rs`,
     `src/wayland/selection/source.rs`, `src/wayland/selection/data_device/device.rs`,
     `src/wayland/selection/data_device/dnd_grab.rs`, `src/wayland/selection/primary_selection/device.rs`,
     `src/wayland/selection/wlr_data_control/device.rs`, `src/wayland/selection/ext_data_control/device.rs`.
2. `patches/0002-gles-reset-notification.patch`
   - What: `EGLContext::new_with_reset_notification` creates a context that loses itself on a GPU reset where
     `EGL_EXT_create_context_robustness` is offered, and also on a purge of video memory where
     `EGL_NV_robustness_video_memory_purge` is offered; `EGLContext::loses_context_on_reset` says whether it does.
     `GlesRenderer::graphics_reset_status` reports a reset (`GraphicsResetStatus`). The build generates the bindings
     of both extensions and of `GL_EXT_robustness`.
   - Why: without a reset notification a GPU reset, or video memory purged across a suspend or a console switch,
     leaves the compositor drawing with a broken context and lost buffers; with it the compositor recreates them.
   - Files: `CHANGELOG.md`, `build.rs`, `src/backend/egl/context.rs`, `src/backend/renderer/gles/mod.rs`.
3. `patches/0003-shm-destruction-hook-removal.patch`
   - What: removing a `wl_shm` buffer destruction hook removes that hook, not another hook of the same buffer.
   - Why: a client that destroyed its `xdg_toplevel_icon_v1` and then a buffer it had added to it got the `no_buffer`
     protocol error and was disconnected.
   - Files: `CHANGELOG.md`, `src/wayland/shm/mod.rs`.
4. `patches/0004-compiler-warnings.patch`
   - What: removes four unused tracing imports, casts the SIGBUS handler through a pointer before its integer
     representation, and advances swapchain buffer ages with compare/exchange instead of the deprecated
     `fetch_update`, keeping age zero, overflow handling and the ordering.
   - Why: the workspace builds with warnings denied, on the toolchain it pins, without raising the declared Rust
     version.
   - Files: `CHANGELOG.md`, `src/backend/allocator/swapchain.rs`, `src/backend/drm/surface/gbm.rs`,
     `src/backend/winit/mod.rs`, `src/input/keyboard/mod.rs`, `src/wayland/seat/keyboard.rs`,
     `src/wayland/shm/pool.rs`.
5. `patches/0005-shm-keep-texture-without-damage.patch`
   - What: a `wl_shm` buffer committed without damage keeps the texture its surface already holds instead of being
     uploaded whole again.
   - Why: a client presenting an unchanged frame cost a full upload every frame.
   - Files: `CHANGELOG.md`, `src/backend/renderer/gles/mod.rs`.
6. `patches/0006-gles-memory-uploads.patch`
   - What: `GlesRenderer::memory_uploads` counts the textures `ImportMem` made or updated from memory and the bytes
     they sent (`MemoryUploads`).
   - Why: the compositor counts its own uploads per frame in its performance measurements.
   - Files: `CHANGELOG.md`, `src/backend/renderer/gles/mod.rs`.
7. `patches/0007-rescale-element-mut.patch`
   - What: `RescaleRenderElement::element_mut` gives the re-scaled element, to change it in place.
   - Why: the compositor updates a re-scaled element's opaque region with the frame that changes it, without building
     the element again.
   - Files: `CHANGELOG.md`, `src/backend/renderer/element/utils/elements.rs`.
8. `patches/0008-drm-first-frame-modeset.patch`
   - What: `DrmCompositor` commits the first frame after its creation or `reset_state` with modesets allowed, also
     when the crtc already has the surface's mode and connectors; the kernel still modesets only when the state needs
     it.
   - Why: the frame takes over what another program left on the crtc (a device opened without disabling the
     connectors, a resumed session), where a flip alone can be refused for a primary plane format the driver changes
     only in a modeset. The login screen hands the display to the desktop this way with no black frame between.
   - Files: `CHANGELOG.md`, `src/backend/drm/compositor/mod.rs`.

## Changing the tree

A change to a file of the release is a new patch at the end of the list (or an edit of the patch that owns those
lines), with its entry here and in the changelog's Unreleased section. To write one, diff a copy of the tree before
the change against the tree after it, with paths relative to this directory, and run `cargo xtask vendored`.
Updating to a new upstream release replaces the files and `UPSTREAM.sha256` with the new package's and rebases each
patch onto it, dropping those the release already contains; the same commit changes the digest of `UPSTREAM.sha256`
that `xtask/src/vendored.rs` pins.
