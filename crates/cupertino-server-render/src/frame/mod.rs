//! Shared frame constants, cursor choice, and background snapshot PNG writer.

use std::time::Duration;

#[path = "snapshot-writes.rs"]
pub mod snapshot_writes;

/// Frame callbacks are throttled to this interval for surfaces that are not on screen.
pub const HIDDEN_FRAME_THROTTLE: Duration = Duration::from_secs(1);

/// Background of offscreen output renderings (`--screenshot`, screen capture) where nothing is drawn.
pub const SCREENSHOT_CLEAR: smithay::backend::renderer::Color32F =
    smithay::backend::renderer::Color32F::new(0.05, 0.05, 0.07, 1.0);

/// What the pointer shows, before it is drawn.
#[derive(Clone, Debug)]
pub enum CursorChoice {
    Hidden,
    /// A client's cursor surface.
    Surface(smithay::reexports::wayland_server::protocol::wl_surface::WlSurface),
    /// A named cursor of the theme.
    Icon(smithay::input::pointer::CursorIcon),
}
