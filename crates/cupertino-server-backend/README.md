# cupertino-server-backend

Compositor backend primitives (`core` layer, D25). Depends on no other server crate.

## Modules

| Module | Contents |
|---|---|
| `session` | Smithay `Session` / `EventSource` adapters over `cupertino-seat`'s logind D-Bus controller (`LogindSession`, `LogindNotifier`, `SessionError`), with bounded compositor deadlines |
| `schedule` | Per-output frame scheduler (`FrameScheduler`, `PresentationId`, `Overdue`, `FrameCounts`) |
| `latch` | Late-latching frame start timing (`LateLatch`, `FrameTimes`, `GpuDone`, `periods_late`) |
| `sync_file` | Explicit-sync render fences (`RenderFence`) |
| `output_depth` | Primary-plane bit-depth selection (`Depth`, `format_bits`) |
| `drm_trace` | DRM flip/queue diagnostic trace (`DrmTrace`, `Planes`, `Branch`) |
| `frame_stats` | Frame timing statistics core (`FrameStats`, `GlassTime`, `MaterialCounts`, `UploadCounts`) |
| `acquire_waits` | Explicit-sync buffer acquire-wait tracking |
| `software_render` | Hardware-first renderer selection with Mesa software fallback (`choose`, `record`) |
| `gpu_reset` | GPU reset rate-limiting and logging (`Loss`, `Resets`, `status`, `record`) |
| `displays` | EDID parser (`edid`), display head data model (`Head`, `HeadInfo`, `HeadState`, `HeadMode`, `HeadCaps`), mode fallback, scale choice, and head validation |
| `backend` | Cross-GPU frame copy (`copy`), generic evdev input backend (`input::EvdevHost`), active-turn stall recorder (`stalls::Recorder<D>`), and shared DRM/EGL helpers (`drm`) |
