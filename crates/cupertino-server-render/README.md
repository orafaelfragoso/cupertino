# cupertino-server-render

Compositor rendering primitives (`core` layer, D25). Depends only on `cupertino-server-backend` among the server crates.

## Modules

| Module | Contents |
|---|---|
| `config` | Rendering configuration value types (`Insets`, `WallpaperSettings`) |
| `cursor` | XCursor/procedural cursor loader (`CursorLoader`), recolouring (`recolor`), fallback arrow, and shake-to-locate state (`CursorShake`) |
| `decor` | Server-side decoration layout/caching (`Decor`, `DecorInput`, `DecorationBuffers`) |
| `decor_access` | Window control button accessibility model and AT-SPI sink (`WindowButtons`, `ButtonAccessibility`, `AtspiSink`) |
| `drag_return` | Cancelled-drag icon flight and fade animation model (`DragStart`, `DragEnd`, `DragReturn`, `placement`) |
| `frame` | Frame constants (`HIDDEN_FRAME_THROTTLE`, `SCREENSHOT_CLEAR`), `CursorChoice`, and off-loop snapshot writer (`snapshot_writes`) |
| `frame_fill` | Client-drawn frame opacity detection (`opaque_frame`, `Frame`) |
| `fx` | Window closing ghosts, snapshot store, and show-desktop slide (`Effects`, `ClosingWindow`) |
| `popup_linger` | Dismissed-popup fade-out retention and expiry deadlines (`Expiries`, `keep_dismissed`, `shown`) |
| `toolkit_programs` | Ahead-of-time compilation of toolkit GPU shaders into the per-user program cache |
| `render` | `CupertinoElement`, GLES renderer (`gles::Gles`, `WindowGraphics`), lowering (`lower`), scene composition (`compose`), core scene planning & resource cache (`core::RenderResources`, `finalize_materials`, `WindowPlan`), material/glass/mesh/chrome/ui/clip/reduce/supersample/wallpaper/screenshot/uploads passes, generational `snapshots`, and `shaders/` |
