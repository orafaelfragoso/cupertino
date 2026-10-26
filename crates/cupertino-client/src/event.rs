//! Events delivered to the application and the translation of raw seat input.

use crate::dock::DockUpdate;
use crate::keyboard::Modifiers;
use crate::menu_bar::MenuBarUpdate;
use crate::{Scale, SurfaceId};
use cupertino_geometry::Point;

/// Linux input event codes (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// Evdev key codes of keys the shell processes react to (`linux/input-event-codes.h`).
pub mod key {
    pub const ESCAPE: u32 = 1;
    pub const ENTER: u32 = 28;
    pub const KEYPAD_ENTER: u32 = 96;
    pub const SPACE: u32 = 57;
    pub const BACKSPACE: u32 = 14;
    pub const TAB: u32 = 15;
    pub const HOME: u32 = 102;
    pub const UP: u32 = 103;
    pub const PAGE_UP: u32 = 104;
    pub const LEFT: u32 = 105;
    pub const RIGHT: u32 = 106;
    pub const END: u32 = 107;
    pub const DOWN: u32 = 108;
    pub const PAGE_DOWN: u32 = 109;
    pub const DELETE: u32 = 111;
}

/// A pointer button.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
    /// Any other evdev button code.
    Other(u32),
}

impl Button {
    pub fn from_evdev(code: u32) -> Self {
        match code {
            BTN_LEFT => Button::Left,
            BTN_RIGHT => Button::Right,
            BTN_MIDDLE => Button::Middle,
            other => Button::Other(other),
        }
    }
}

/// Pointer input on a surface. Positions are surface-local logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PointerEvent {
    Enter(Point),
    Motion(Point),
    Leave,
    /// A button changed state with the pointer at `position`.
    Button { button: Button, pressed: bool, position: Point },
    /// The wheel or a scroll gesture moved: `dx` and `dy` in logical points (positive right and
    /// down), one per `wl_pointer.axis` event, so one of them is zero. `count` is the scroll count of
    /// the touchpad's flick series (`cupertino_scroll_v1`): 0 for a wheel, for a scroll too slow to
    /// start a series and when the server offers no count; 1 for the first flick, 2 for the second,
    /// and from the third on it grows with the speed.
    Scroll { dx: f64, dy: f64, count: u32, position: Point },
}

/// Handle of a request for an activation token, matched to [`Event::ActivationToken`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ActivationRequest(pub(crate) u32);

/// Something the application has to react to; returned by [`Client::wait`](crate::Client::wait).
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// The server sized the surface (logical points); also the first configure that maps it.
    Configured { surface: SurfaceId, size: (u32, u32) },
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
    Pointer { surface: SurfaceId, event: PointerEvent },
    /// The surface lost the keyboard focus.
    KeyboardLeave(SurfaceId),
    /// A key changed state on the keyboard-focused surface (evdev code, see [`key`]). A held key
    /// that repeats arrives as further presses. `character` is what the key types under the
    /// layout and `mods` the modifiers held at that moment.
    Key { surface: SurfaceId, key: u32, pressed: bool, mods: Modifiers, character: Option<char> },
    ActivationToken { request: ActivationRequest, token: String },
    /// The server described the active application and its menus to the menu bar of `surface`.
    MenuBar { surface: SurfaceId, update: MenuBarUpdate },
    /// The server described the windows of the session to the dock of `surface`.
    Dock { surface: SurfaceId, update: DockUpdate },
    /// The session's appearance changed: draw again with it. The appearance at connect is
    /// [`Client::is_dark`](crate::Client::is_dark) and queues no event.
    Appearance { dark: bool },
    /// The session's accessibility display options changed: draw again with them. The options at connect are
    /// [`Client::display_options`](crate::Client::display_options) and queue no event.
    DisplayOptions(crate::DisplayOptions),
    /// The server locked or unlocked the session. The state at connect is
    /// [`Client::session_locked`](crate::Client::session_locked) and queues no event.
    SessionLock { locked: bool },
    /// An output appeared or went away, or its geometry changed: read [`Client::outputs`](crate::Client::outputs) again.
    OutputsChanged,
}

/// Follows the pointer focus and position so button events carry the position.
#[derive(Debug, Default)]
pub(crate) struct PointerTracker {
    focus: Option<SurfaceId>,
    position: Point,
    /// Axis events of the frame being received, held back for the counts that follow them
    /// (`(vertical, value)`); used only when the server offers `cupertino_scroll_v1`.
    held: Vec<(bool, f64)>,
    /// The scroll counts of the frame being received (horizontal, vertical).
    counts: [u32; 2],
}

impl PointerTracker {
    pub(crate) fn enter(&mut self, surface: SurfaceId, position: Point) -> (SurfaceId, PointerEvent) {
        self.focus = Some(surface);
        self.position = position;
        (surface, PointerEvent::Enter(position))
    }

    /// Drop the focus when it is on a surface that is going away.
    pub(crate) fn forget(&mut self, surface: SurfaceId) {
        if self.focus == Some(surface) {
            self.focus = None;
        }
    }

    pub(crate) fn leave(&mut self) -> Option<(SurfaceId, PointerEvent)> {
        self.focus.take().map(|s| (s, PointerEvent::Leave))
    }

    pub(crate) fn motion(&mut self, position: Point) -> Option<(SurfaceId, PointerEvent)> {
        self.position = position;
        self.focus.map(|s| (s, PointerEvent::Motion(position)))
    }

    pub(crate) fn button(&self, code: u32, pressed: bool) -> Option<(SurfaceId, PointerEvent)> {
        self.focus.map(|s| (s, PointerEvent::Button { button: Button::from_evdev(code), pressed, position: self.position }))
    }

    /// One axis of a scroll; `vertical` selects `dy`. With `counted` the event waits for the frame
    /// ([`PointerTracker::frame`]), which brings the scroll counts.
    pub(crate) fn scroll(&mut self, vertical: bool, value: f64, counted: bool) -> Option<(SurfaceId, PointerEvent)> {
        if counted {
            self.held.push((vertical, value));
            return None;
        }
        self.scroll_event(vertical, value, 0)
    }

    /// The count of the scroll of one axis in the frame being received.
    pub(crate) fn scroll_count(&mut self, vertical: bool, count: u32) {
        self.counts[usize::from(vertical)] = count;
    }

    /// The end of a frame: the held scrolls with their counts.
    pub(crate) fn frame(&mut self) -> Vec<(SurfaceId, PointerEvent)> {
        let counts = std::mem::take(&mut self.counts);
        std::mem::take(&mut self.held).into_iter().filter_map(|(vertical, value)| self.scroll_event(vertical, value, counts[usize::from(vertical)])).collect()
    }

    fn scroll_event(&self, vertical: bool, value: f64, count: u32) -> Option<(SurfaceId, PointerEvent)> {
        let (dx, dy) = if vertical { (0.0, value) } else { (value, 0.0) };
        self.focus.map(|s| (s, PointerEvent::Scroll { dx, dy, count, position: self.position }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evdev_codes_map_to_buttons() {
        assert_eq!(Button::from_evdev(0x110), Button::Left);
        assert_eq!(Button::from_evdev(0x111), Button::Right);
        assert_eq!(Button::from_evdev(0x112), Button::Middle);
        assert_eq!(Button::from_evdev(0x113), Button::Other(0x113));
    }

    #[test]
    fn button_events_carry_the_last_position_and_need_focus() {
        let mut t = PointerTracker::default();
        assert_eq!(t.button(0x110, true), None);
        let s = SurfaceId(3);
        t.enter(s, Point::new(1.0, 2.0));
        t.motion(Point::new(10.5, 20.5));
        assert_eq!(t.button(0x110, true), Some((s, PointerEvent::Button { button: Button::Left, pressed: true, position: Point::new(10.5, 20.5) })));
        assert_eq!(t.scroll(true, -10.0, false), Some((s, PointerEvent::Scroll { dx: 0.0, dy: -10.0, count: 0, position: Point::new(10.5, 20.5) })));
        assert_eq!(t.leave(), Some((s, PointerEvent::Leave)));
        assert_eq!(t.motion(Point::new(0.0, 0.0)), None);
        assert_eq!(t.scroll(false, 3.0, false), None);
    }

    #[test]
    fn counted_scrolls_wait_for_the_frame_and_take_the_count_of_their_axis() {
        let mut t = PointerTracker::default();
        let s = SurfaceId(3);
        let position = Point::new(1.0, 2.0);
        t.enter(s, position);
        assert_eq!(t.scroll(true, 15.0, true), None);
        assert_eq!(t.scroll(false, -2.0, true), None);
        t.scroll_count(true, 4);
        assert_eq!(
            t.frame(),
            vec![(s, PointerEvent::Scroll { dx: 0.0, dy: 15.0, count: 4, position }), (s, PointerEvent::Scroll { dx: -2.0, dy: 0.0, count: 0, position })]
        );
        // the counts belong to one frame
        assert_eq!(t.scroll(true, 1.0, true), None);
        assert_eq!(t.frame(), vec![(s, PointerEvent::Scroll { dx: 0.0, dy: 1.0, count: 0, position })]);
    }
}
