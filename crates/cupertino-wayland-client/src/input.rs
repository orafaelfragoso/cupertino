//! Pointer, touch and keyboard input: the raw `wl_pointer`, `wl_touch` and `wl_keyboard` streams turned into typed events. The
//! compositor describes the layout with an xkb keymap; the client translates and repeats keys itself.

use crate::compose::{ComposeEnv, ComposeState, ComposeTable, Composed};
use crate::{Error, SurfaceId};
use cupertino_geometry::Point;
use std::collections::HashMap;
use std::mem::ManuallyDrop;
use std::os::fd::OwnedFd;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError, Weak};
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
    keymap: Arc<SharedKeymap>,
    /// Made and dropped with the keymap's count held ([`SharedKeymap`]).
    state: ManuallyDrop<xkb::State>,
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

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A compiled keymap, shared by every connection of the process the compositor sent the same keymap text.
///
/// libxkbcommon never changes a compiled keymap but for its reference count, which it keeps without atomics. Here only
/// making a state on the keymap ([`SharedKeymap::state`]) and dropping one ([`Translator`]'s drop) change that count,
/// each with [`SharedKeymap::count`] held; every other call made on the keymap only reads it.
struct SharedKeymap {
    keymap: xkb::Keymap,
    /// Held while `keymap`'s reference count changes.
    count: Mutex<()>,
}

// SAFETY: the keymap is only read, but for its reference count, which changes only with `count` held (above). The last
// `Arc` drops the keymap once no state on it is left, when no other thread can reach it.
unsafe impl Send for SharedKeymap {}
// SAFETY: as for `Send`.
unsafe impl Sync for SharedKeymap {}

impl SharedKeymap {
    /// A keyboard state of its own on the keymap.
    fn state(&self) -> xkb::State {
        let _count = lock(&self.count);
        xkb::State::new(&self.keymap)
    }
}

/// The compiled keymaps by their text.
type Keymaps = Mutex<HashMap<Box<[u8]>, Weak<SharedKeymap>>>;

/// The keymaps of the process by their text (D5): an entry never changes once in, the lock is held only to look one up
/// or put one in, never while a keymap compiles, and a keymap goes when the last connection on it is gone.
fn keymaps() -> &'static Keymaps {
    static KEYMAPS: OnceLock<Keymaps> = OnceLock::new();
    KEYMAPS.get_or_init(Mutex::default)
}

/// The process's keymap compiled from `text`, compiled now when no connection has it; `None` when it does not
/// compile. Two connections compiling the same text at once both get the first one put in.
fn shared_keymap(text: Vec<u8>) -> Option<Arc<SharedKeymap>> {
    if let Some(keymap) = lock(keymaps()).get(text.as_slice()).and_then(Weak::upgrade) {
        return Some(keymap);
    }
    let compiled = Arc::new(SharedKeymap {
        keymap: compile(&text)?,
        count: Mutex::new(()),
    });
    let mut keymaps = lock(keymaps());
    if let Some(keymap) = keymaps.get(text.as_slice()).and_then(Weak::upgrade) {
        return Some(keymap);
    }
    keymaps.retain(|_, keymap| keymap.strong_count() > 0);
    keymaps.insert(text.into_boxed_slice(), Arc::downgrade(&compiled));
    Some(compiled)
}

/// The keymap `text` (in the text format, without a terminating NUL) describes, in a context of its own.
fn compile(text: &[u8]) -> Option<xkb::Keymap> {
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    // SAFETY: the context is valid and the buffer is `text`, read for its length during the call only
    let keymap = unsafe {
        xkb::ffi::xkb_keymap_new_from_buffer(
            context.get_raw_ptr(),
            text.as_ptr().cast(),
            text.len(),
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
    };
    // SAFETY: a keymap just compiled, whose one reference the `Keymap` takes over
    (!keymap.is_null()).then(|| unsafe { xkb::Keymap::from_raw_ptr(keymap) })
}

/// The compose table of the process's locale, read on first use.
fn compose_table() -> Option<&'static ComposeTable> {
    static TABLE: OnceLock<Option<ComposeTable>> = OnceLock::new();
    TABLE
        .get_or_init(|| ComposeTable::load(&ComposeEnv::from_process()))
        .as_ref()
}

impl Translator {
    /// Load the keymap the compositor sent over a file descriptor of `size` bytes (NUL included): the process's
    /// keymap of that text ([`shared_keymap`]), with a keyboard state of this connection's own. A size of 0, or one
    /// larger than the file, is refused before the file is read: the text is the first `size - 1` bytes.
    pub(crate) fn from_fd(fd: OwnedFd, size: usize) -> Result<Self, Error> {
        if size == 0 {
            return Err(Error::Protocol(
                "the keymap size leaves no room for its terminating NUL",
            ));
        }
        let file = std::fs::File::from(fd);
        let length = file.metadata().map_err(Error::Io)?.len();
        if u64::try_from(size).map_or(true, |size| size > length) {
            return Err(Error::Protocol("the keymap size is larger than its file"));
        }
        let mut text = vec![0; size - 1];
        file.read_exact_at(&mut text, 0).map_err(Error::Io)?;
        shared_keymap(text)
            .map(Self::new)
            .ok_or(Error::Protocol("the keymap does not compile"))
    }

    fn new(keymap: Arc<SharedKeymap>) -> Self {
        Self::with_compose(keymap, compose_table())
    }

    fn with_compose(
        keymap: Arc<SharedKeymap>,
        compose_table: Option<&'static ComposeTable>,
    ) -> Self {
        let state = ManuallyDrop::new(keymap.state());
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
            .keymap
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

impl Drop for Translator {
    /// The keyboard state goes with the keymap's count held ([`SharedKeymap`]), before the keymap may.
    fn drop(&mut self) {
        let _count = lock(&self.keymap.count);
        // SAFETY: the state is dropped here only, once, and never used after
        unsafe { ManuallyDrop::drop(&mut self.state) };
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
