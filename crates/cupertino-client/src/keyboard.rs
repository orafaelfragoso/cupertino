//! Keymap, modifiers and key repeat: turns the raw `wl_keyboard` stream into typed characters.
//! The server describes the layout with an xkb keymap; clients translate and repeat keys
//! themselves.

use crate::ClientError;
use std::os::fd::OwnedFd;
use std::time::{Duration, Instant};
use xkbcommon::xkb;

/// Offset between evdev key codes and xkb key codes.
const EVDEV_OFFSET: u32 = 8;

/// The logical modifiers held while a key event happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Super key.
    pub logo: bool,
}

/// What a key produced under the current layout and modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Translation {
    pub mods: Modifiers,
    /// The character of the key's symbol (shift and layout level applied, control not); `None`
    /// for keys without one (arrows, function keys, modifiers, Enter, Escape).
    pub character: Option<char>,
}

/// The server's keymap and the modifier state it reported.
pub(crate) struct Translator {
    keymap: xkb::Keymap,
    state: xkb::State,
}

impl Translator {
    /// Load the keymap the server sent over a file descriptor of `size` bytes (NUL included).
    pub(crate) fn from_fd(fd: OwnedFd, size: usize) -> Result<Self, ClientError> {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        // SAFETY: the descriptor is the server's keymap file, mapped privately and read only
        let keymap = unsafe {
            xkb::Keymap::new_from_fd(
                &context,
                fd,
                size,
                xkb::KEYMAP_FORMAT_TEXT_V1,
                xkb::KEYMAP_COMPILE_NO_FLAGS,
            )
        }
        .map_err(ClientError::Keymap)?;
        keymap
            .map(Self::new)
            .ok_or(ClientError::Protocol("the keymap does not compile"))
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

    /// Whether Caps Lock is on (the xkb `Lock` modifier is effective).
    pub(crate) fn caps_lock(&self) -> bool {
        self.state
            .mod_name_is_active(xkb::MOD_NAME_CAPS, xkb::STATE_MODS_EFFECTIVE)
    }

    /// Whether holding the evdev key `code` repeats it.
    pub(crate) fn repeats(&self, code: u32) -> bool {
        self.keymap
            .key_repeats(xkb::Keycode::new(code + EVDEV_OFFSET))
    }

    /// The modifiers in effect now.
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
        }
    }

    /// The characters and modifiers of the evdev key `code` now.
    pub(crate) fn translate(&self, code: u32) -> Translation {
        let mods = self.modifiers();
        let symbol = self
            .state
            .key_get_one_sym(xkb::Keycode::new(code + EVDEV_OFFSET));
        let character =
            char::from_u32(xkb::keysym_to_utf32(symbol)).filter(|c| *c != '\0' && !c.is_control());
        Translation { mods, character }
    }
}

/// Client-side key repeat from `wl_keyboard.repeat_info`: one key repeats at a time, the last one
/// pressed.
#[derive(Debug, Default)]
pub(crate) struct Repeat {
    /// Delay before the first repeat and the interval after it; `None` while the server has not
    /// reported them or turned repeat off.
    timing: Option<(Duration, Duration)>,
    held: Option<(u32, Instant)>,
}

impl Repeat {
    /// Apply `repeat_info`: `rate` keys per second (0 turns repeat off), `delay` in milliseconds.
    pub(crate) fn set_info(&mut self, rate: i32, delay: i32) {
        self.timing = u64::try_from(rate).ok().filter(|r| *r > 0).map(|r| {
            (
                Duration::from_millis(delay.max(0) as u64),
                Duration::from_micros(1_000_000 / r),
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

    /// The key to repeat once per repeat that came due by `now`.
    pub(crate) fn due(&mut self, now: Instant) -> Vec<u32> {
        let (Some((_, interval)), Some((code, at))) = (self.timing, self.held.as_mut()) else {
            return Vec::new();
        };
        let mut due = Vec::new();
        while *at <= now {
            due.push(*code);
            *at += interval;
        }
        due
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LEVEL_SHIFT: u32 = 1;
    const KEY_A: u32 = 30;
    const KEY_ENTER: u32 = 28;
    const KEY_LEFT_CTRL_MASK: u32 = 1 << 2;
    const LOCK_MASK: u32 = 1 << 1;

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
    fn letters_follow_shift_and_control_keys_have_no_character() {
        let mut t = us_layout();
        assert_eq!(
            t.translate(KEY_A),
            Translation {
                mods: Modifiers::default(),
                character: Some('a')
            }
        );
        t.update_modifiers(LEVEL_SHIFT, 0, 0, 0);
        assert_eq!(
            t.translate(KEY_A),
            Translation {
                mods: Modifiers {
                    shift: true,
                    ..Modifiers::default()
                },
                character: Some('A')
            }
        );
        assert_eq!(t.translate(KEY_ENTER).character, None);
    }

    #[test]
    fn control_is_reported_but_leaves_the_character() {
        let mut t = us_layout();
        t.update_modifiers(KEY_LEFT_CTRL_MASK, 0, 0, 0);
        assert_eq!(
            t.translate(KEY_A),
            Translation {
                mods: Modifiers {
                    ctrl: true,
                    ..Modifiers::default()
                },
                character: Some('a')
            }
        );
    }

    #[test]
    fn caps_lock_follows_the_locked_modifiers_and_capitalises_letters() {
        let mut t = us_layout();
        assert!(!t.caps_lock());
        t.update_modifiers(0, 0, LOCK_MASK, 0);
        assert!(t.caps_lock());
        assert_eq!(t.translate(KEY_A).character, Some('A'));
        t.update_modifiers(0, 0, 0, 0);
        assert!(!t.caps_lock());
    }

    #[test]
    fn letters_repeat_and_modifiers_do_not() {
        let t = us_layout();
        assert!(t.repeats(KEY_A));
        assert!(!t.repeats(42));
    }

    #[test]
    fn repeat_starts_after_the_delay_and_fires_once_per_interval() {
        let mut r = Repeat::default();
        r.set_info(25, 600);
        let t0 = Instant::now();
        r.press(KEY_A, t0);
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(600)));
        assert_eq!(r.due(t0 + Duration::from_millis(599)), Vec::<u32>::new());
        // 600 ms, then every 40 ms: 600, 640 and 680 are due at 690
        assert_eq!(r.due(t0 + Duration::from_millis(690)), vec![KEY_A; 3]);
        assert_eq!(r.deadline(), Some(t0 + Duration::from_millis(720)));
    }

    #[test]
    fn only_the_repeating_key_stops_the_repeat_and_rate_zero_turns_it_off() {
        let mut r = Repeat::default();
        r.set_info(25, 600);
        let t0 = Instant::now();
        r.press(KEY_A, t0);
        r.release(KEY_ENTER);
        assert!(r.deadline().is_some());
        r.release(KEY_A);
        assert_eq!(r.deadline(), None);
        r.press(KEY_A, t0);
        r.set_info(0, 600);
        assert_eq!(r.deadline(), None);
        r.press(KEY_A, t0);
        assert_eq!(r.deadline(), None);
    }
}
