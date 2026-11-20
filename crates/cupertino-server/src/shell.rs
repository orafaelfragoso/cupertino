//! Shell geometry: the usable area of an output. The overview is a component of the shell host (`overview_v1.rs` feeds
//! it and `shell_ctl.rs` asks it to open); the application switcher's panel is the dock's.

use cupertino_wm::Rect;

/// Area available to windows: the output minus the menu bar at the top, intersected with the
/// layer-shell exclusive zones (`layer_usable`, in the same space; the dock's is one of them).
pub fn usable_area(output: Rect, menu_bar: f64, layer_usable: Option<Rect>) -> Rect {
    let base = Rect::new(
        output.x,
        output.y + menu_bar,
        output.w,
        (output.h - menu_bar).max(1.0),
    );
    match layer_usable {
        Some(l) => base.intersection(&l).unwrap_or(base),
        None => base,
    }
}
