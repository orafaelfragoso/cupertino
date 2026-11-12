//! Pointer, touch and keyboard input: the raw `wl_pointer`, `wl_touch` and `wl_keyboard` streams turned into typed events. The
//! compositor describes the layout with an xkb keymap; the client translates and repeats keys itself.

use crate::compose::{ComposeEnv, ComposeState, ComposeTable, Composed};
use crate::{Error, SurfaceId};
use cupertino_geometry::Point;
use std::os::fd::OwnedFd;
use std::sync::OnceLock;
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
        /// Per axis `(x, y)`: the physical motion ran against the delta (natural scrolling), from
        /// `axis_relative_direction` (`wl_pointer` version 9); false when the compositor does not say.
        inverted: (bool, bool),
    },
}

/// Touch input on a surface. `id` names one contact from its down to its up; positions are surface-local logical
/// points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TouchEvent {
    Down {
        id: i32,
        position: Point,
    },
    Motion {
        id: i32,
        position: Point,
    },
    Up {
        id: i32,
    },
    /// The compositor took every contact on the surface over: none of them ends with an up.
    Cancel,
}

/// The surface each contact went down on: motion, up and cancel name only the contact.
#[derive(Debug, Default)]
pub(crate) struct TouchTracker {
    contacts: Vec<(i32, SurfaceId)>,
}

impl TouchTracker {
    pub(crate) fn down(&mut self, id: i32, surface: SurfaceId, position: Point) -> TouchEvent {
        self.contacts.retain(|(c, _)| *c != id);
        self.contacts.push((id, surface));
        TouchEvent::Down { id, position }
    }

    pub(crate) fn motion(&self, id: i32, position: Point) -> Option<(SurfaceId, TouchEvent)> {
        self.surface(id)
            .map(|s| (s, TouchEvent::Motion { id, position }))
    }

    pub(crate) fn up(&mut self, id: i32) -> Option<(SurfaceId, TouchEvent)> {
        let surface = self.surface(id)?;
        self.contacts.retain(|(c, _)| *c != id);
        Some((surface, TouchEvent::Up { id }))
    }

    /// One cancel for each surface with a contact on it.
    pub(crate) fn cancel(&mut self) -> Vec<(SurfaceId, TouchEvent)> {
        let mut surfaces: Vec<SurfaceId> = Vec::new();
        for (_, s) in self.contacts.drain(..) {
            if !surfaces.contains(&s) {
                surfaces.push(s);
            }
        }
        surfaces
            .into_iter()
            .map(|s| (s, TouchEvent::Cancel))
            .collect()
    }

    /// Drop the contacts on a surface that is going away.
    pub(crate) fn forget(&mut self, surface: SurfaceId) {
        self.contacts.retain(|(_, s)| *s != surface);
    }

    fn surface(&self, id: i32) -> Option<SurfaceId> {
        self.contacts
            .iter()
            .find(|(c, _)| *c == id)
            .map(|(_, s)| *s)
    }
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
    inverted: (bool, bool),
}

impl PointerTracker {
    /// The surface the pointer is on.
    pub(crate) fn focus(&self) -> Option<SurfaceId> {
        self.focus
    }

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

    /// The physical direction of one axis of the pending scroll: `inverted` when it runs against the delta.
    pub(crate) fn axis_relative_direction(&mut self, vertical: bool, inverted: bool) {
        let (x, y) = &mut self.scroll.get_or_insert_default().inverted;
        *if vertical { y } else { x } = inverted;
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
            inverted,
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
                    inverted,
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

    /// The latest timestamp as it arrived.
    pub(crate) fn latest(&self) -> Option<u32> {
        self.0.map(|(time, _)| time)
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

/// How the text of a pending dead key sequence shows in a text field after a key press.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Preview {
    /// The marks to show composing, or `None` to take the shown ones away.
    pub preedit: Option<String>,
    /// Text that goes in before the key's own character: the marks of a sequence the key did not go on with.
    pub commit: Option<String>,
    /// The key only took the sequence away (Escape, Backspace) and goes no further.
    pub swallow: bool,
}

/// The compositor's keymap and the modifier state it reported, and the compose sequence being typed.
pub(crate) struct Translator {
    keymap: xkb::Keymap,
    state: xkb::State,
    compose_table: Option<&'static ComposeTable>,
    compose: ComposeState,
    preview: Option<Preview>,
}

/// The spacing mark a dead key shows while it waits for the next key.
fn spacing_mark(keysym: u32) -> Option<char> {
    use xkb::keysyms as k;
    Some(match keysym {
        k::KEY_dead_grave => '`',
        k::KEY_dead_acute => '\u{b4}',
        k::KEY_dead_circumflex => '\u{2c6}',
        k::KEY_dead_tilde => '\u{2dc}',
        k::KEY_dead_macron => '\u{af}',
        k::KEY_dead_breve => '\u{2d8}',
        k::KEY_dead_abovedot => '\u{2d9}',
        k::KEY_dead_diaeresis => '\u{a8}',
        k::KEY_dead_abovering => '\u{2da}',
        k::KEY_dead_doubleacute => '\u{2dd}',
        k::KEY_dead_caron => '\u{2c7}',
        k::KEY_dead_cedilla => '\u{b8}',
        k::KEY_dead_ogonek => '\u{2db}',
        _ => return None,
    })
}

/// The marks of a pending sequence: dead keys as their spacing marks, the letters after one as typed. A sequence
/// started with the compose key shows nothing, as it types nothing when it is cancelled.
fn marks(pending: &[u32]) -> String {
    if pending.first().and_then(|k| spacing_mark(*k)).is_none() {
        return String::new();
    }
    pending
        .iter()
        .filter_map(|k| {
            spacing_mark(*k).or_else(|| {
                char::from_u32(xkb::keysym_to_utf32(xkb::Keysym::new(*k)))
                    .filter(|c| *c != '\0' && !c.is_control())
            })
        })
        .collect()
}

/// The compose table of the process's locale, read on first use.
fn compose_table() -> Option<&'static ComposeTable> {
    static TABLE: OnceLock<Option<ComposeTable>> = OnceLock::new();
    TABLE
        .get_or_init(|| ComposeTable::load(&ComposeEnv::from_process()))
        .as_ref()
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
        Self::with_compose(keymap, compose_table())
    }

    fn with_compose(keymap: xkb::Keymap, compose_table: Option<&'static ComposeTable>) -> Self {
        let state = xkb::State::new(&keymap);
        Self {
            keymap,
            state,
            compose_table,
            compose: ComposeState::default(),
            preview: None,
        }
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

    /// [`Self::translate`] for a press, which also goes on with a compose sequence: a key inside one types nothing,
    /// the key that ends one types its character. A dead key sequence the key does not go on with types its marks
    /// and then the key, except Escape and Backspace, which only take it away. Only a key that types outside a
    /// sequence starts `repeat`; one that starts, goes on with or ends a sequence stops it. What the press does to
    /// the shown marks waits in [`Self::take_preview`].
    pub(crate) fn press(&mut self, code: u32, repeat: &mut Repeat, now: Instant) -> Translation {
        let mut translation = self.translate(code);
        let before = marks(self.compose.pending());
        let composed = match self.compose_table {
            Some(table) => self.compose.feed(table, translation.keysym),
            None => Composed::Passed,
        };
        self.preview = None;
        match composed {
            Composed::Passed => {
                if self.repeats(code) {
                    repeat.press(code, now);
                }
                return translation;
            }
            Composed::Composing => {
                translation.character = None;
                let after = marks(self.compose.pending());
                if after != before {
                    self.preview = Some(Preview {
                        preedit: (!after.is_empty()).then_some(after),
                        ..Preview::default()
                    });
                }
            }
            Composed::Done(c) => {
                translation.character = Some(c);
                if !before.is_empty() {
                    self.preview = Some(Preview::default());
                }
            }
            Composed::Cancelled if before.is_empty() => translation.character = None,
            Composed::Cancelled => {
                let swallow = matches!(
                    translation.keysym,
                    xkb::keysyms::KEY_Escape | xkb::keysyms::KEY_BackSpace
                );
                let chord = translation.mods.ctrl || translation.mods.logo;
                if swallow {
                    translation.character = None;
                }
                self.preview = Some(Preview {
                    preedit: None,
                    commit: (!swallow && !chord).then_some(before),
                    swallow,
                });
            }
        }
        repeat.clear();
        translation
    }

    /// What the last press did to the marks of a pending sequence, `None` when it left them as they were.
    pub(crate) fn take_preview(&mut self) -> Option<Preview> {
        self.preview.take()
    }

    /// Forget a compose sequence, when the keyboard focus leaves. Returns whether marks were showing.
    pub(crate) fn reset_compose(&mut self) -> bool {
        let shown = !marks(self.compose.pending()).is_empty();
        self.compose.reset();
        self.preview = None;
        shown
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
/// The most repeats one late wake-up delivers: a caller busy for a few intervals keeps the rate, one that stalled
/// longer starts the repeat over instead of typing the whole stall at once (our choice).
const MAX_CATCH_UP: u32 = 8;

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

    /// The key to repeat and how many repeats came due by `now`. The repeats keep to the rate however late the
    /// caller is: those that came due while it was busy come at once, at most [`MAX_CATCH_UP`], after which the
    /// repeat starts over one interval from `now`.
    pub(crate) fn due(&mut self, now: Instant) -> Option<(u32, u32)> {
        let (Some((_, interval)), Some((code, at))) = (self.timing, self.held.as_mut()) else {
            return None;
        };
        let mut count = 0;
        while *at <= now && count < MAX_CATCH_UP {
            *at += interval;
            count += 1;
        }
        if *at <= now {
            *at = now + interval;
        }
        (count > 0).then_some((*code, count))
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
    const KEY_APOSTROPHE: u32 = 40;
    const KEY_GRAVE: u32 = 41;
    const KEY_C: u32 = 46;
    const KEY_X: u32 = 45;
    const KEY_O: u32 = 24;
    const KEY_COMPOSE: u32 = 127;
    const KEY_E: u32 = 18;
    const KEY_ESC: u32 = 1;

    fn keymap(layout: &str, variant: &str) -> xkb::Keymap {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            variant,
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the layout compiles")
    }

    fn us_layout() -> Translator {
        Translator::with_compose(keymap("us", ""), None)
    }

    fn compose_with(options: &str, table: &str) -> Translator {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Compose");
        std::fs::write(&file, table).unwrap();
        let env = ComposeEnv {
            compose_file: Some(file),
            ..ComposeEnv::default()
        };
        let table: &'static ComposeTable = Box::leak(Box::new(ComposeTable::load(&env).unwrap()));
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        let keymap = xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            "us",
            "intl",
            Some(options.to_string()),
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the layout compiles");
        Translator::with_compose(keymap, Some(table))
    }

    fn repeating() -> Repeat {
        let mut r = Repeat::default();
        r.set_info(25, 100);
        r
    }

    #[test]
    fn the_letter_that_ends_a_dead_key_sequence_does_not_repeat() {
        let mut t = compose_with("", "<dead_tilde> <a> : \"\u{e3}\"\n");
        let mut r = repeating();
        let t0 = Instant::now();
        t.update_modifiers(1, 0, 0, 0);
        assert_eq!(t.press(KEY_GRAVE, &mut r, t0).character, None);
        t.update_modifiers(0, 0, 0, 0);
        assert_eq!(t.press(KEY_A, &mut r, t0).character, Some('\u{e3}'));
        assert_eq!(
            r.due(t0 + Duration::from_secs(1)),
            None,
            "held a after ~a types no aaaa"
        );
        t.press(KEY_A, &mut r, t0);
        assert_eq!(
            r.due(t0 + Duration::from_secs(1)).map(|(code, _)| code),
            Some(KEY_A),
            "a typed on its own repeats"
        );
    }

    #[test]
    fn a_key_held_inside_a_compose_sequence_types_nothing_until_the_sequence_ends() {
        let mut t = compose_with("compose:menu", "<Multi_key> <o> <o> : \"\u{b0}\"\n");
        let mut r = repeating();
        let t0 = Instant::now();
        t.press(KEY_X, &mut r, t0);
        assert_eq!(t.press(KEY_COMPOSE, &mut r, t0).character, None);
        assert_eq!(
            r.due(t0 + Duration::from_secs(1)),
            None,
            "the held x stops with the sequence"
        );
        assert_eq!(t.press(KEY_O, &mut r, t0).character, None);
        assert_eq!(
            r.due(t0 + Duration::from_secs(1)),
            None,
            "held o mid-sequence types no ooo"
        );
        assert_eq!(t.press(KEY_O, &mut r, t0).character, Some('\u{b0}'));
        assert_eq!(r.due(t0 + Duration::from_secs(1)), None);
    }

    #[test]
    fn the_dead_keys_of_us_intl_compose_with_the_next_letter() {
        let mut t = compose_with(
            "",
            "<dead_acute> <c> : \"\u{e7}\"\n<dead_tilde> <a> : \"\u{e3}\"\n",
        );
        let mut r = Repeat::default();
        let mut press = |t: &mut Translator, key| t.press(key, &mut r, Instant::now()).character;
        let mut typed = vec![press(&mut t, KEY_APOSTROPHE), press(&mut t, KEY_C)];
        t.update_modifiers(1, 0, 0, 0);
        typed.push(press(&mut t, KEY_GRAVE));
        t.update_modifiers(0, 0, 0, 0);
        typed.push(press(&mut t, KEY_A));
        typed.extend([KEY_APOSTROPHE, KEY_X, KEY_X].map(|key| press(&mut t, key)));
        assert_eq!(
            typed,
            [None, Some('ç'), None, Some('ã'), None, Some('x'), Some('x')],
            "a dead key before a letter it has no sequence with gives the letter, and the next key types"
        );
    }

    #[test]
    fn a_pending_dead_key_shows_its_mark_until_the_next_key_ends_or_cancels_it() {
        let mut t = compose_with("", "<dead_acute> <e> : \"\u{e9}\"\n");
        let mut r = Repeat::default();
        let mut press = |t: &mut Translator, key| {
            let c = t.press(key, &mut r, Instant::now()).character;
            (c, t.take_preview())
        };
        let shown = |text: &str| Preview {
            preedit: Some(text.into()),
            ..Preview::default()
        };
        assert_eq!(press(&mut t, KEY_APOSTROPHE), (None, Some(shown("\u{b4}"))));
        assert_eq!(
            press(&mut t, KEY_E),
            (Some('\u{e9}'), Some(Preview::default())),
            "the letter takes the mark away and types the accented letter"
        );
        assert_eq!(
            press(&mut t, KEY_E),
            (Some('e'), None),
            "outside a sequence nothing changes"
        );

        press(&mut t, KEY_APOSTROPHE);
        assert_eq!(
            press(&mut t, KEY_X),
            (
                Some('x'),
                Some(Preview {
                    preedit: None,
                    commit: Some("\u{b4}".into()),
                    swallow: false
                })
            ),
            "a letter without a sequence types the mark, then itself"
        );

        press(&mut t, KEY_APOSTROPHE);
        assert_eq!(
            press(&mut t, KEY_ESC),
            (
                None,
                Some(Preview {
                    preedit: None,
                    commit: None,
                    swallow: true
                })
            ),
            "Escape only takes the mark away"
        );
        press(&mut t, KEY_APOSTROPHE);
        assert!(
            t.reset_compose(),
            "leaving the surface reports the shown mark"
        );
        assert!(!t.reset_compose());
    }

    #[test]
    fn a_compose_key_sequence_shows_nothing() {
        let mut t = compose_with("compose:menu", "<Multi_key> <o> <o> : \"\u{b0}\"\n");
        let mut r = Repeat::default();
        for key in [KEY_COMPOSE, KEY_O, KEY_X] {
            t.press(key, &mut r, Instant::now());
            assert_eq!(t.take_preview(), None);
        }
    }

    #[test]
    fn evdev_codes_map_to_buttons() {
        assert_eq!(Button::from_evdev(0x110), Button::Left);
        assert_eq!(Button::from_evdev(0x112), Button::Middle);
        assert_eq!(Button::from_evdev(0x113), Button::Other(0x113));
    }

    #[test]
    fn a_contact_reports_on_the_surface_it_went_down_on_until_its_up_and_a_cancel_ends_every_surface_once(
    ) {
        let mut t = TouchTracker::default();
        let (a, b) = (SurfaceId(1), SurfaceId(2));
        let p = Point::new(3.0, 4.0);
        assert_eq!(t.motion(0, p), None);
        assert_eq!(t.down(0, a, p), TouchEvent::Down { id: 0, position: p });
        t.down(1, b, p);
        t.down(2, b, p);
        assert_eq!(
            t.motion(0, p),
            Some((a, TouchEvent::Motion { id: 0, position: p }))
        );
        assert_eq!(t.up(0), Some((a, TouchEvent::Up { id: 0 })));
        assert_eq!(t.up(0), None);
        assert_eq!(t.cancel(), vec![(b, TouchEvent::Cancel)]);
        assert_eq!(t.motion(1, p), None);
        t.down(3, a, p);
        t.forget(a);
        assert_eq!(t.up(3), None);
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
                    inverted: (false, false),
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
                    inverted: (false, false),
                }
            ))
        );
    }

    #[test]
    fn a_frame_carries_the_physical_direction_of_each_axis() {
        let mut t = PointerTracker::default();
        t.enter(SurfaceId(1), Point::ZERO);
        t.axis_source(AxisSource::Finger);
        t.axis_relative_direction(true, true);
        t.axis(true, -3.25);
        let Some((_, PointerEvent::Scroll { dy, inverted, .. })) = t.frame() else {
            panic!("a scroll");
        };
        assert_eq!((dy, inverted), (-3.25, (false, true)));
        t.axis(false, 1.0);
        let Some((_, PointerEvent::Scroll { inverted, .. })) = t.frame() else {
            panic!("a scroll");
        };
        assert_eq!(inverted, (false, false), "each frame starts identical");
    }

    #[test]
    fn the_input_clock_advances_from_the_latest_timestamp_and_wraps() {
        let mut clock = InputClock::default();
        let start = Instant::now();
        assert_eq!((clock.at(start), clock.latest()), (0, None));
        clock.note(u32::MAX - 1, start);
        assert_eq!(clock.latest(), Some(u32::MAX - 1));
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
        assert_eq!(r.due(t0 + Duration::from_millis(600)), Some((KEY_A, 1)));
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(640)));
        // 640 and 680 came due while the caller was busy: both come now, and the next keeps to the interval
        assert_eq!(r.due(t0 + Duration::from_millis(690)), Some((KEY_A, 2)));
        assert_eq!(r.due(t0 + Duration::from_millis(690)), None);
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(720)));
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
        assert_eq!(r.due(t0), Some((KEY_A, 1)));
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(1)));
    }

    #[test]
    fn a_caller_busy_longer_than_the_interval_still_gets_the_reported_rate() {
        let mut r = Repeat::default();
        r.set_info(25, 600);
        let t0 = Instant::now();
        r.press(KEY_A, t0);
        // a text field that takes 100 ms to handle each key looks again every 100 ms for 2.6 s
        let repeats: u32 = (1..=26)
            .filter_map(|i| r.due(t0 + Duration::from_millis(100 * i)))
            .map(|(_, count)| count)
            .sum();
        // 25 per second after the 600 ms delay: at 600, 640, ..., 2600 ms
        assert_eq!(repeats, 51);
    }

    #[test]
    fn a_stalled_caller_gets_a_bounded_burst_and_the_repeat_starts_over() {
        let mut r = Repeat::default();
        r.set_info(25, 600);
        let t0 = Instant::now();
        r.press(KEY_A, t0);
        let woke = t0 + Duration::from_secs(5);
        assert_eq!(r.due(woke), Some((KEY_A, MAX_CATCH_UP)));
        assert_eq!(r.deadline(), Some(woke + Duration::from_millis(40)));
    }
}
