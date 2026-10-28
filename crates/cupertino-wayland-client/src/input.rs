//! Pointer and keyboard input: the raw `wl_pointer` and `wl_keyboard` streams turned into typed events. The
//! compositor describes the layout with an xkb keymap; the client translates and repeats keys itself.

use crate::{Error, SurfaceId};
use cupertino_geometry::Point;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};
use wayland_client::protocol::wl_pointer::AxisSource;
use xkbcommon::xkb;

/// Linux input event codes (`linux/input-event-codes.h`).
const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;

/// Offset between evdev key codes and xkb key codes.
const EVDEV_OFFSET: u32 = 8;

/// Evdev codes of common keys, as [`KeyboardEvent::Key`] carries them (`linux/input-event-codes.h`).
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
    Button {
        button: Button,
        pressed: bool,
        position: Point,
    },
    /// The wheel or a scroll gesture moved: `dx` and `dy` in logical points, positive right and down.
    Scroll {
        dx: f64,
        dy: f64,
        position: Point,
        /// What scrolled, when the compositor said (`wl_pointer` version 5 and later).
        source: Option<AxisSource>,
        /// Wheel detents in 120ths of a notch, `(x, y)`, for a wheel that has them; high-resolution wheels send
        /// fractions of 120.
        value120: Option<(i32, i32)>,
        /// The finger left the touchpad (or a continuous scroll ended): kinetic scrolling may start from here.
        stop: bool,
    },
}

/// The logical modifiers held while a key event happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Super key.
    pub logo: bool,
    pub caps_lock: bool,
}

/// Keyboard input on a surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardEvent {
    /// The surface got the keyboard focus.
    Enter,
    /// The surface lost the keyboard focus.
    Leave,
    /// A key changed state (evdev `code`). A held key that repeats arrives as further presses with `repeat` set.
    /// `keysym` and `character` are what the key gives under the layout and modifiers at that moment (control not
    /// applied to the character).
    Key {
        code: u32,
        pressed: bool,
        repeat: bool,
        keysym: u32,
        character: Option<char>,
        mods: Modifiers,
    },
}

/// Follows the pointer focus and position so button and scroll events carry the position.
#[derive(Debug, Default)]
pub(crate) struct PointerTracker {
    focus: Option<SurfaceId>,
    position: Point,
    scroll: Option<Scroll>,
}

/// A scroll collected over one pointer frame.
#[derive(Debug, Default)]
struct Scroll {
    delta: (f64, f64),
    source: Option<AxisSource>,
    value120: Option<(i32, i32)>,
    stop: bool,
}

impl PointerTracker {
    pub(crate) fn enter(
        &mut self,
        surface: SurfaceId,
        position: Point,
    ) -> (SurfaceId, PointerEvent) {
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
        self.focus.map(|s| {
            (
                s,
                PointerEvent::Button {
                    button: Button::from_evdev(code),
                    pressed,
                    position: self.position,
                },
            )
        })
    }

    /// Add one axis of a scroll to the pending one; `vertical` selects `dy`.
    pub(crate) fn axis(&mut self, vertical: bool, value: f64) {
        let (dx, dy) = &mut self.scroll.get_or_insert_default().delta;
        *if vertical { dy } else { dx } += value;
    }

    pub(crate) fn axis_source(&mut self, source: AxisSource) {
        self.scroll.get_or_insert_default().source = Some(source);
    }

    /// Wheel detents in 120ths of a notch on one axis (`axis_value120`, or `axis_discrete` times 120 before version 8).
    pub(crate) fn axis_value120(&mut self, vertical: bool, value120: i32) {
        let (x, y) = self
            .scroll
            .get_or_insert_default()
            .value120
            .get_or_insert((0, 0));
        let axis = if vertical { y } else { x };
        *axis = axis.saturating_add(value120);
    }

    pub(crate) fn axis_stop(&mut self) {
        self.scroll.get_or_insert_default().stop = true;
    }

    /// End a pointer frame: both axes of the pending scroll, and a stop without movement, become one event.
    pub(crate) fn frame(&mut self) -> Option<(SurfaceId, PointerEvent)> {
        let Scroll {
            delta: (dx, dy),
            source,
            value120,
            stop,
        } = self.scroll.take()?;
        self.focus.map(|s| {
            (
                s,
                PointerEvent::Scroll {
                    dx,
                    dy,
                    position: self.position,
                    source,
                    value120,
                    stop,
                },
            )
        })
    }
}

/// The compositor's input clock as last seen: its latest timestamp and when that arrived.
#[derive(Debug, Default)]
pub(crate) struct InputClock(Option<(u32, Instant)>);

impl InputClock {
    pub(crate) fn note(&mut self, time: u32, at: Instant) {
        self.0 = Some((time, at));
    }

    /// The input time at `now`: the latest timestamp advanced by the milliseconds since it arrived (wrapping); 0
    /// before any.
    pub(crate) fn at(&self, now: Instant) -> u32 {
        self.0.map_or(0, |(time, at)| {
            time.wrapping_add(now.saturating_duration_since(at).as_millis() as u32)
        })
    }
}

/// What a key gives under the current layout and modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Translation {
    pub keysym: u32,
    pub character: Option<char>,
    pub mods: Modifiers,
}

/// The compositor's keymap and the modifier state it reported.
pub(crate) struct Translator {
    keymap: xkb::Keymap,
    state: xkb::State,
}

impl Translator {
    /// Load the keymap the compositor sent over a file descriptor of `size` bytes (NUL included).
    pub(crate) fn from_fd(fd: OwnedFd, size: usize) -> Result<Self, Error> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // SAFETY: the descriptor is the compositor's keymap file, mapped privately and read only
        let keymap = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                fd,
                size,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        }
        .map_err(Error::Io)?;
        keymap
            .map(Self::new)
            .ok_or(Error::Protocol("the keymap does not compile"))
    }

    fn new(keymap: xkb::Keymap) -> Self {
        let state = xkb::State::new(&keymap);
        Self { keymap, state }
    }

    /// Apply a `wl_keyboard.modifiers` event.
    pub(crate) fn update_modifiers(
        &mut self,
        depressed: u32,
        latched: u32,
        locked: u32,
        group: u32,
    ) {
        self.state
            .update_mask(depressed, latched, locked, 0, 0, group);
    }

    /// Whether holding the evdev key `code` repeats it.
    pub(crate) fn repeats(&self, code: u32) -> bool {
        self.keymap
            .key_repeats(xkb::Keycode::new(code + EVDEV_OFFSET))
    }

    pub(crate) fn modifiers(&self) -> Modifiers {
        let active = |name: &str| {
            self.state
                .mod_name_is_active(name, xkb::STATE_MODS_EFFECTIVE)
        };
        Modifiers {
            ctrl: active(xkb::MOD_NAME_CTRL),
            alt: active(xkb::MOD_NAME_ALT),
            shift: active(xkb::MOD_NAME_SHIFT),
            logo: active(xkb::MOD_NAME_LOGO),
            caps_lock: active(xkb::MOD_NAME_CAPS),
        }
    }

    /// The symbol, character and modifiers of the evdev key `code` now.
    pub(crate) fn translate(&self, code: u32) -> Translation {
        let symbol = self
            .state
            .key_get_one_sym(xkb::Keycode::new(code + EVDEV_OFFSET));
        let character =
            char::from_u32(xkb::keysym_to_utf32(symbol)).filter(|c| *c != '\0' && !c.is_control());
        Translation {
            keysym: symbol.raw(),
            character,
            mods: self.modifiers(),
        }
    }
}

/// Repeat rates above this many keys per second are clamped to it.
const MAX_REPEAT_RATE: u64 = 1000;

/// Client-side key repeat from `wl_keyboard.repeat_info`: one key repeats at a time, the last one pressed.
#[derive(Debug, Default)]
pub(crate) struct Repeat {
    /// Delay before the first repeat and the interval after it; `None` while the compositor has not reported them or
    /// turned repeat off.
    timing: Option<(Duration, Duration)>,
    held: Option<(u32, Instant)>,
}

impl Repeat {
    /// Apply `repeat_info`: `rate` keys per second (0 turns repeat off), `delay` in milliseconds.
    pub(crate) fn set_info(&mut self, rate: i32, delay: i32) {
        self.timing = u64::try_from(rate).ok().filter(|r| *r > 0).map(|r| {
            (
                Duration::from_millis(delay.max(0) as u64),
                Duration::from_micros(1_000_000 / r.min(MAX_REPEAT_RATE)),
            )
        });
        if self.timing.is_none() {
            self.held = None;
        }
    }

    /// A repeating key went down.
    pub(crate) fn press(&mut self, code: u32, now: Instant) {
        self.held = self.timing.map(|(delay, _)| (code, now + delay));
    }

    /// A key went up; releasing another key than the repeating one does not stop it.
    pub(crate) fn release(&mut self, code: u32) {
        if self.held.is_some_and(|(held, _)| held == code) {
            self.held = None;
        }
    }

    pub(crate) fn clear(&mut self) {
        self.held = None;
    }

    /// When the next repeat is due.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.held.map(|(_, at)| at)
    }

    /// The key to repeat when a repeat came due by `now`; repeats missed while the caller was late are dropped.
    pub(crate) fn due(&mut self, now: Instant) -> Option<u32> {
        let (Some((_, interval)), Some((code, at))) = (self.timing, self.held.as_mut()) else {
            return None;
        };
        if *at > now {
            return None;
        }
        *at = now + interval;
        Some(*code)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVEL_SHIFT: u32 = 1;
    const LOCK_MASK: u32 = 1 << 1;
    const CONTROL_MASK: u32 = 1 << 2;
    const KEY_A: u32 = 30;
    const KEY_ENTER: u32 = 28;
    const KEY_LEFT_SHIFT: u32 = 42;

    fn us_layout() -> Translator {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us",
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the us layout compiles");
        Translator::new(keymap)
    }

    #[test]
    fn evdev_codes_map_to_buttons() {
        assert_eq!(Button::from_evdev(0x110), Button::Left);
        assert_eq!(Button::from_evdev(0x112), Button::Middle);
        assert_eq!(Button::from_evdev(0x113), Button::Other(0x113));
    }

    #[test]
    fn buttons_and_scrolls_carry_the_last_position_and_need_focus_and_a_frame_sums_its_axes() {
        let mut t = PointerTracker::default();
        assert_eq!(t.button(BTN_LEFT, true), None);
        let s = SurfaceId(3);
        t.enter(s, Point::new(1.0, 2.0));
        t.motion(Point::new(10.5, 20.5));
        let position = Point::new(10.5, 20.5);
        assert_eq!(
            t.button(BTN_LEFT, true),
            Some((
                s,
                PointerEvent::Button {
                    button: Button::Left,
                    pressed: true,
                    position
                }
            ))
        );
        assert_eq!(t.frame(), None);
        t.axis(true, -10.0);
        t.axis(false, 4.0);
        t.axis(true, -2.5);
        assert_eq!(
            t.frame(),
            Some((
                s,
                PointerEvent::Scroll {
                    dx: 4.0,
                    dy: -12.5,
                    position,
                    source: None,
                    value120: None,
                    stop: false,
                }
            ))
        );
        assert_eq!(t.frame(), None);
        t.forget(s);
        assert_eq!(t.leave(), None);
        assert_eq!(t.motion(Point::ZERO), None);
    }

    #[test]
    fn a_wheel_frame_carries_its_source_and_detents_and_a_lift_alone_is_a_stop() {
        let mut t = PointerTracker::default();
        let s = SurfaceId(1);
        t.enter(s, Point::ZERO);
        t.axis_source(AxisSource::Wheel);
        t.axis(true, 15.0);
        t.axis_value120(true, 120);
        t.axis_value120(false, -60);
        t.axis_value120(true, 60);
        let Some((
            _,
            PointerEvent::Scroll {
                source,
                value120,
                stop,
                ..
            },
        )) = t.frame()
        else {
            panic!("a scroll");
        };
        assert_eq!(
            (source, value120, stop),
            (Some(AxisSource::Wheel), Some((-60, 180)), false)
        );
        t.axis_source(AxisSource::Finger);
        t.axis_stop();
        assert_eq!(
            t.frame(),
            Some((
                s,
                PointerEvent::Scroll {
                    dx: 0.0,
                    dy: 0.0,
                    position: Point::ZERO,
                    source: Some(AxisSource::Finger),
                    value120: None,
                    stop: true,
                }
            ))
        );
    }

    #[test]
    fn the_input_clock_advances_from_the_latest_timestamp_and_wraps() {
        let mut clock = InputClock::default();
        let start = Instant::now();
        assert_eq!(clock.at(start), 0);
        clock.note(u32::MAX - 1, start);
        assert_eq!(clock.at(start), u32::MAX - 1);
        assert_eq!(clock.at(start + Duration::from_millis(5)), 3);
        assert_eq!(clock.at(start - Duration::from_millis(5)), u32::MAX - 1);
    }

    #[test]
    fn letters_follow_shift_and_caps_lock_and_enter_types_nothing() {
        let mut t = us_layout();
        assert_eq!(t.translate(KEY_A).character, Some('a'));
        t.update_modifiers(LEVEL_SHIFT, 0, 0, 0);
        let shifted = t.translate(KEY_A);
        assert_eq!((shifted.character, shifted.mods.shift), (Some('A'), true));
        t.update_modifiers(0, 0, LOCK_MASK, 0);
        let locked = t.translate(KEY_A);
        assert_eq!((locked.character, locked.mods.caps_lock), (Some('A'), true));
        assert_eq!(t.translate(KEY_ENTER).character, None);
    }

    #[test]
    fn control_is_reported_but_leaves_the_character() {
        let mut t = us_layout();
        t.update_modifiers(CONTROL_MASK, 0, 0, 0);
        let k = t.translate(KEY_A);
        assert_eq!((k.character, k.mods.ctrl), (Some('a'), true));
        assert_eq!(k.keysym, xkb::keysyms::KEY_a);
    }

    #[test]
    fn letters_repeat_and_modifiers_do_not() {
        let t = us_layout();
        assert!(t.repeats(KEY_A));
        assert!(!t.repeats(KEY_LEFT_SHIFT));
    }

    #[test]
    fn repeat_starts_after_the_delay_and_fires_once_per_interval() {
        let mut r = Repeat::default();
        r.set_info(25, 600);
        let t0 = Instant::now();
        r.press(KEY_A, t0);
        assert_eq!(r.due(t0 + Duration::from_millis(599)), None);
        assert_eq!(r.due(t0 + Duration::from_millis(600)), Some(KEY_A));
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(640)));
        // 640 and 680 were missed: one repeat, and the next one interval after it
        assert_eq!(r.due(t0 + Duration::from_millis(690)), Some(KEY_A));
        assert_eq!(r.due(t0 + Duration::from_millis(690)), None);
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(730)));
        r.release(KEY_ENTER);
        assert!(r.deadline().is_some());
        r.release(KEY_A);
        assert_eq!(r.deadline(), None);
        r.set_info(0, 600);
        r.press(KEY_A, t0);
        assert_eq!(r.deadline(), None);
    }

    #[test]
    fn a_rate_beyond_the_limit_repeats_once_per_millisecond() {
        let mut r = Repeat::default();
        r.set_info(i32::MAX, 0);
        let t0 = Instant::now();
        r.press(KEY_A, t0);
        assert_eq!(r.due(t0 + Duration::from_secs(1)), Some(KEY_A));
        assert_eq!(r.due(t0 + Duration::from_secs(1)), None);
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(1001)));
    }
}
