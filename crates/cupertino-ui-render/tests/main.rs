//! The integration tests of `cupertino-ui-render`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod support;

#[path = "cpu-renderer.rs"]
mod cpu_renderer;
#[path = "render-thread.rs"]
mod render_thread;
#[path = "render-thread-shutdown.rs"]
mod render_thread_shutdown;
#[path = "renderer-agreement.rs"]
mod renderer_agreement;
