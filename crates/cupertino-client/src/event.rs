//! Events delivered to the application.

use crate::keyboard::Modifiers;
use crate::{Scale, SurfaceId};
use cupertino_geometry::Point;
use cupertino_shell_client::DockUpdate;
use cupertino_shell_client::MenuBarUpdate;
pub use cupertino_wayland_client::{key, Button};

/// Pointer input on a surface. Positions are surface-local logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Enter(Point),
    Motion(Point),
    Leave,
    /// A button changed state with the pointer at `position`.
    Button {
        button: Button,
        pressed: bool,
        position: Point,
    },
    /// The wheel or a scroll gesture moved: `dx` and `dy` in logical points (positive right and
    /// down), one per axis a `wl_pointer.frame` moved, so one of them is zero. `count` is the scroll count of
    /// the touchpad's flick series (`cupertino_scroll_v1`): 0 for a wheel, for a scroll too slow to
    /// start a series and when the server offers no count; 1 for the first flick, 2 for the second,
    /// and from the third on it grows with the speed.
    Scroll {
        dx: f64,
        dy: f64,
        count: u32,
        position: Point,
    },
}

/// Handle of a request for an activation token, matched to [`Event::ActivationToken`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivationRequest(pub(crate) cupertino_wayland_client::TokenRequest);

/// Something the application has to react to; returned by [`Client::wait`](crate::Client::wait).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The server sized the surface (logical points); also the first configure that maps it.
    Configured {
        surface: SurfaceId,
        size: (u32, u32),
    },
    /// The buffer scale changed: draw again.
    ScaleChanged { surface: SurfaceId, scale: Scale },
    /// The surface was asked to redraw ([`Client::request_redraw`](crate::Client::request_redraw)) and can take a frame now.
    Draw(SurfaceId),
    /// The server closed the surface (for example its output went away). For a toplevel window it
    /// is the request to close it (`xdg_toplevel.close`, the close button or the close shortcut): the
    /// application decides and destroys the window.
    Closed(SurfaceId),
    /// The server dismissed the popup, usually because the user clicked outside its grab. The
    /// application destroys it ([`Client::destroy_surface`](crate::Client::destroy_surface)).
    PopupDone(SurfaceId),
    Pointer {
        surface: SurfaceId,
        event: PointerEvent,
    },
    /// The surface lost the keyboard focus.
    KeyboardLeave(SurfaceId),
    /// A key changed state on the keyboard-focused surface (evdev code, see [`key`]). A held key
    /// that repeats arrives as further presses. `character` is what the key types under the
    /// layout and `mods` the modifiers held at that moment.
    Key {
        surface: SurfaceId,
        key: u32,
        pressed: bool,
        mods: Modifiers,
        character: Option<char>,
    },
    ActivationToken {
        request: ActivationRequest,
        token: String,
    },
    /// The server described the active application and its menus to the menu bar of `surface`.
    MenuBar {
        surface: SurfaceId,
        update: MenuBarUpdate,
    },
    /// The server described the windows of the session to the dock of `surface`.
    Dock {
        surface: SurfaceId,
        update: DockUpdate,
    },
    /// The session's appearance changed: draw again with it. The appearance at connect is
    /// [`Client::is_dark`](crate::Client::is_dark) and queues no event.
    Appearance { dark: bool },
    /// The session's accessibility display options changed: draw again with them. The options at connect are
    /// [`Client::display_options`](crate::Client::display_options) and queue no event.
    DisplayOptions(crate::DisplayOptions),
    /// The server locked or unlocked the session. The state at connect is
    /// [`Client::session_locked`](crate::Client::session_locked) and queues no event.
    SessionLock { locked: bool },
    /// The server asks this process, the lock screen, to lock the session (`explicit`: the person asked). Only the
    /// process with the session lock role is told.
    LockRequested { explicit: bool },
    /// The server asks this process, the login process, to ask whether to log out. Only the process with the session
    /// lock role is told.
    LogOutRequested,
    /// An output appeared or went away, or its geometry changed: read [`Client::outputs`](crate::Client::outputs) again.
    OutputsChanged,
}

/// The scroll counts of the pointer frame being received: they arrive before the frame's scroll.
#[derive(Debug, Default)]
pub(crate) struct ScrollCounts([u32; 2]);

impl ScrollCounts {
    pub(crate) fn set(&mut self, vertical: bool, count: u32) {
        self.0[usize::from(vertical)] = count;
    }

    /// The frame's scroll as one event per axis it moved, vertical first, each with the count of its axis; the counts
    /// then start over for the next frame.
    pub(crate) fn split(&mut self, dx: f64, dy: f64, position: Point) -> Vec<PointerEvent> {
        let [horizontal, vertical] = std::mem::take(&mut self.0);
        let scroll = |dx, dy, count| PointerEvent::Scroll {
            dx,
            dy,
            count,
            position,
        };
        [
            (dy != 0.0).then(|| scroll(0.0, dy, vertical)),
            (dx != 0.0).then(|| scroll(dx, 0.0, horizontal)),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_frame_scrolls_once_per_axis_with_the_count_of_its_axis_and_the_counts_start_over() {
        let mut counts = ScrollCounts::default();
        let position = Point::new(1.0, 2.0);
        counts.set(true, 4);
        assert_eq!(
            counts.split(-2.0, 15.0, position),
            [
                PointerEvent::Scroll {
                    dx: 0.0,
                    dy: 15.0,
                    count: 4,
                    position
                },
                PointerEvent::Scroll {
                    dx: -2.0,
                    dy: 0.0,
                    count: 0,
                    position
                }
            ]
        );
        assert_eq!(
            counts.split(0.0, 1.0, position),
            [PointerEvent::Scroll {
                dx: 0.0,
                dy: 1.0,
                count: 0,
                position
            }]
        );
    }
}
