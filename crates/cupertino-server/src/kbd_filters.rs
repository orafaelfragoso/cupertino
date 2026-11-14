//! The key path between the input devices and the keyboard state, in the order of the design keyboard filter
//! (§1, §10):
//!
//! 1. **slow keys**: a key counts once held for the delay; while they are on the caps-lock delay is
//!    skipped and the key repeat runs at 420 ms;
//! 2. the **caps-lock delay** or hold-to-switch ([`Cupertino::caps_gate`]);
//! 3. **sticky keys**: latched and locked modifiers, the five-Shift toggle;
//! 4. **mouse keys**: the keypad moves the pointer and presses its button, the five-Alt toggle;
//! 5. the **globe / fn key** tap;
//! 6. [`Cupertino::keyboard_event_now`]: the overlays, the menus, the shortcuts and the focused client; a key none of
//!    them claims may be rewritten for a third-party client first (see [`Cupertino::forward_rewritten`]).
//!
//! A key the device reports as already held when it is read (at open, so after a session switch, or after
//! `SYN_DROPPED`) skips steps 1 to 5 until it is released: no filter saw the keystroke that put it down, and sticky
//! keys would otherwise latch the modifiers of the chord that switched sessions.
//!
//! The compositor's own **key repeat** lives here as well. Clients repeat for themselves from
//! `wl_keyboard.repeat_info`; keys the compositor consumes (repeating shortcuts, the input-source HUD's arrows, the
//! accent popup's navigation) never reach a client, so the compositor repeats them with the same delay and interval
//! (one repeating key). A repeat replays the press through the overlays and the shortcut router
//! while the key and the same modifiers stay down. The accent picker's trigger is a repeat too: a held letter with
//! accent alternatives reaches the application as a press and an immediate release (so the application never
//! repeats it), and the compositor's first repeat opens the picker (the input method, not the application,
//! receives the repeats of such a key).
//!
//! Every timed filter is polled from the frame tick (`tick_overlays`); the backends iterate their loop every few
//! milliseconds.

use std::collections::{HashMap, HashSet};

use cupertino_keyboard::modifiers::{FnUsage, GlobeOutcome, GlobeTap, GLOBE_DEFAULT_REPEAT_MS};
use cupertino_keyboard::mouse_keys::{MouseKeys, PointerAction};
use cupertino_keyboard::repeat::KeyRepeater;
use cupertino_keyboard::rewrite::{decide, is_printable, ClientTarget, KeyPress, Rewrite};
use cupertino_keyboard::slow_keys::{SlowKeys, SlowStep};
use cupertino_keyboard::sticky_keys::StickyKeys;
use cupertino_keyboard::switcher::Step;
use cupertino_modifiers::RawMods;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::{xkb, KeyboardTarget, Keycode, ModifiersState, SerializedMods};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Point, SERIAL_COUNTER};

use crate::input::{Action, Chord, Mods};
use crate::kbd::KeyboardSettings;
use crate::overlays::OverlayKey;
use crate::state::Cupertino;

/// evdev `KEY_FN` (the globe / fn key on keyboards that report it).
const KEY_FN: u32 = 464;
/// Offset between evdev codes and the keycodes of the keymap.
const XKB_OFFSET: u32 = 8;

fn evdev(code: Keycode) -> u32 {
    code.raw().saturating_sub(XKB_OFFSET)
}

/// evdev `KEY_LEFTMETA` and `KEY_RIGHTMETA`, the Super keys.
const SUPER_KEYS: [u32; 2] = [125, 126];

/// How long the Super key may stay down alone and still be a tap, in ms. approximate.
const SUPER_TAP_MAX_MS: u64 = 750;

/// The Super key pressed and released alone: no other key and no pointer button between, and not held long.
#[derive(Debug, Default)]
pub struct SuperTap {
    /// When the Super key went down with nothing else down; `None` once anything else happened.
    down: Option<u64>,
}

impl SuperTap {
    /// A key event. `others_held`: another key was held when this one went down. True when it is the release that
    /// completes a tap.
    pub fn key(&mut self, evdev: u32, pressed: bool, now_ms: u64, others_held: bool) -> bool {
        if !SUPER_KEYS.contains(&evdev) {
            if pressed {
                self.down = None;
            }
            return false;
        }
        if pressed {
            self.down = (!others_held).then_some(now_ms);
            return false;
        }
        self.down
            .take()
            .is_some_and(|down| now_ms.saturating_sub(down) <= SUPER_TAP_MAX_MS)
    }

    /// A pointer button or a scroll: the Super key is a modifier of it, not a tap.
    pub fn disturb(&mut self) {
        self.down = None;
    }
}

/// What a compositor key repeat acts on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RepeatTarget {
    /// A key the compositor consumed: its press is replayed while it and the same modifiers stay down.
    Compositor(OverlayKey),
    /// A held letter with accent alternatives: the first repeat opens the accent picker.
    AccentTrigger(char),
}

/// State of the filters (built from `keyboard.conf`).
pub struct KeyFilters {
    /// `Some` while slow keys are on.
    pub slow: Option<SlowKeys>,
    pub sticky: StickyKeys,
    pub mouse: MouseKeys,
    pub globe: GlobeTap,
    pub super_tap: SuperTap,
    pub fn_usage: FnUsage,
    /// The compositor's repeating key (xkb keycode) and what it repeats.
    pub repeat: KeyRepeater,
    pub repeat_target: Option<RepeatTarget>,
    /// Evdev codes of keys found held when a device was (re)read; they reach the keyboard unfiltered until released.
    unfiltered: HashSet<u32>,
    /// Keys held with a rewrite (xkb keycode of the physical key): what the client got for each.
    rewritten: HashMap<u32, Sent>,
    /// The client was last told modifiers that differ from the keyboard's.
    override_active: bool,
}

/// What a client was sent in place of a rewritten key.
struct Sent {
    /// The key that went to the client: the physical key, or the replacement of a cursor chord.
    code: Keycode,
    /// The client it went to; a release is not followed by a modifier update when the focus has moved on.
    focus: WlSurface,
}

/// xkb indices of the real modifiers the rewrite changes, read from the live keymap.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ModIndices {
    shift: u32,
    control: u32,
    mod1: u32,
    mod4: u32,
    mod5: u32,
}

impl ModIndices {
    /// The real modifiers are part of every xkb keymap, so a missing one is a broken keymap.
    fn of(keymap: &xkb::Keymap) -> Self {
        let index = |name: &str| {
            let index = keymap.mod_get_index(name);
            assert_ne!(index, xkb::MOD_INVALID, "the keymap has no {name} modifier");
            index
        };
        Self {
            shift: index(xkb::MOD_NAME_SHIFT),
            control: index(xkb::MOD_NAME_CTRL),
            mod1: index(xkb::MOD_NAME_ALT),
            mod4: index(xkb::MOD_NAME_LOGO),
            mod5: index(xkb::MOD_NAME_ISO_LEVEL3_SHIFT),
        }
    }

    /// Mask of the modifier bits that go away with `mods`: Alt takes the third-symbol-level bit with it, which the
    /// layouts of the input sources set from the same keys.
    fn removed(&self, mods: RawMods) -> u32 {
        let bit = |index: u32| 1u32 << index;
        let mut mask = 0;
        for (raw, bits) in [
            (RawMods::SHIFT, bit(self.shift)),
            (RawMods::CTRL, bit(self.control)),
            (RawMods::ALT, bit(self.mod1) | bit(self.mod5)),
            (RawMods::SUPER, bit(self.mod4)),
        ] {
            if mods.contains(raw) {
                mask |= bits;
            }
        }
        mask
    }

    /// Mask of the modifier bits that `mods` puts down.
    fn added(&self, mods: RawMods) -> u32 {
        let bit = |index: u32| 1u32 << index;
        let mut mask = 0;
        for (raw, bits) in [
            (RawMods::SHIFT, bit(self.shift)),
            (RawMods::CTRL, bit(self.control)),
            (RawMods::ALT, bit(self.mod1)),
            (RawMods::SUPER, bit(self.mod4)),
        ] {
            if mods.contains(raw) {
                mask |= bits;
            }
        }
        mask
    }
}

/// The modifiers to tell a client for a rewritten key: `real` without `remove` and with `add` held down. Locked
/// modifiers (caps and num lock) and the layout stay as they are.
fn rewrite_masks(
    real: SerializedMods,
    idx: &ModIndices,
    remove: RawMods,
    add: RawMods,
) -> SerializedMods {
    let gone = idx.removed(remove);
    SerializedMods {
        depressed: (real.depressed & !gone) | idx.added(add),
        latched: real.latched & !gone,
        ..real
    }
}

/// The key that types `sym` at the first level of the layout `state` gives it; else, the way the shortcut matcher reads
/// a key whose own symbol is not ASCII (`raw_latin_sym_or_raw_current_sym`), the key whose first ASCII symbol in another
/// layout is `sym`: the key a Latin user presses for it. `sym` is a first-level symbol, so a letter is its lower case.
fn key_for_sym(keymap: &xkb::Keymap, state: &xkb::State, sym: u32) -> Option<Keycode> {
    let first = |code: Keycode, layout: u32| {
        keymap
            .key_get_syms_by_level(code, layout, 0)
            .first()
            .copied()
    };
    let latin = |code: Keycode, own: u32| {
        (0..keymap.num_layouts())
            .filter(|l| *l != own)
            .filter_map(|l| first(code, l))
            .find(|s| {
                s.key_char()
                    .is_some_and(|c| c.is_ascii() && !c.is_ascii_control())
            })
    };
    let keys = || (keymap.min_keycode().raw()..=keymap.max_keycode().raw()).map(Keycode::new);
    let typed = |code: Keycode| first(code, state.key_get_layout(code));
    keys()
        .find(|&code| typed(code).is_some_and(|s| s.raw() == sym))
        .or_else(|| {
            keys().find(|&code| {
                typed(code).is_some_and(|s| s.key_char().is_some_and(|c| !c.is_ascii()))
                    && latin(code, state.key_get_layout(code)).is_some_and(|s| s.raw() == sym)
            })
        })
}

impl KeyFilters {
    pub fn new(settings: &KeyboardSettings) -> Self {
        let a = &settings.access;
        let repeat = settings.effective_repeat();
        // the translator's tap limit follows the configured initial repeat delay, 250 ms when repeat is off
        let globe_ms = if repeat.is_enabled() {
            f64::from(repeat.initial_delay_ms)
        } else {
            GLOBE_DEFAULT_REPEAT_MS
        };
        Self {
            slow: a.slow_keys.then(|| SlowKeys::new(a.slow_keys_delay_ms)),
            sticky: StickyKeys::new(a.sticky_keys, a.sticky_keys_shortcut),
            mouse: MouseKeys::new(
                a.mouse_keys,
                a.mouse_keys_shortcut,
                a.mouse_keys_delay_ms,
                a.mouse_keys_max_speed,
            ),
            globe: GlobeTap::new(globe_ms),
            super_tap: SuperTap::default(),
            fn_usage: settings.fn_action(),
            repeat: KeyRepeater::new(repeat),
            repeat_target: None,
            unfiltered: HashSet::new(),
            rewritten: HashMap::new(),
            override_active: false,
        }
    }

    /// The device reported `code` (evdev) as already held: its press and release skip the filters, because no filter
    /// saw the keystroke that put it down.
    pub fn pass_unfiltered(&mut self, code: u32) {
        self.unfiltered.insert(code);
    }
}

impl Cupertino {
    /// The rewrite for a key press nothing in the compositor claimed (§10): the
    /// decision of `cupertino-keyboard` for the client that has the keyboard. `Pass` when that client is not the key
    /// window (a layer surface, the lock screen) or nothing has the keyboard.
    pub(crate) fn key_rewrite(
        &self,
        code: Keycode,
        sym: u32,
        ch: Option<char>,
        mods: &ModifiersState,
    ) -> Rewrite {
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else {
            return Rewrite::Pass;
        };
        if !self
            .wm
            .key_window()
            .and_then(|w| self.entry(w))
            .is_some_and(|e| e.has_surface(&focus))
        {
            return Rewrite::Pass;
        }
        let kb = &self.config.keyboard;
        let app = self.key_app_id();
        let target = ClientTarget {
            class: kb.client_class(&app),
            text_input_active: self.overlays.accent.bridge,
            opted_out: kb.rewrite_opted_out(&app),
        };
        // The key that selects the third symbol level counts as Alt: with `level3(alt_switch)` the Alt keys set that
        // modifier instead of Mod1, which the profile translation does not see.
        let mut raw = RawMods::default();
        for (on, bit) in [
            (mods.shift, RawMods::SHIFT),
            (mods.ctrl, RawMods::CTRL),
            (mods.alt || mods.iso_level3_shift, RawMods::ALT),
            (mods.logo, RawMods::SUPER),
        ] {
            if on {
                raw = raw | bit;
            }
        }
        let key = KeyPress {
            evdev: evdev(code),
            sym,
            printable: is_printable(ch),
            is_modifier: crate::overlays::is_modifier_sym(sym),
            raw,
        };
        decide(&kb.modifier_profile(), &target, &key)
    }

    /// Send a rewritten key to the focused client instead of the physical one (the keyboard state already took the
    /// event in). A press tells the client the rewritten modifiers, then the key; the release sends the same key up and
    /// puts the real modifiers back. Smithay sends a key before its modifiers, hence the manual modifiers event and
    /// `mods_changed = false`. Key repeat stays with the client.
    pub(crate) fn forward_rewritten(
        &mut self,
        code: Keycode,
        state: KeyState,
        rewrite: Rewrite,
        serial: smithay::utils::Serial,
        time: u32,
    ) {
        let keyboard = self.seat.get_keyboard().unwrap();
        if state == KeyState::Released {
            let Some(sent) = self.key_filters.rewritten.remove(&code.raw()) else {
                return;
            };
            keyboard.input_forward(self, sent.code, KeyState::Released, serial, time, false);
            // another rewritten key may still be held; the focus may have moved to a client that never saw the override
            if self.key_filters.rewritten.is_empty() {
                if keyboard.current_focus().as_ref() == Some(&sent.focus) {
                    self.restore_client_mods();
                } else {
                    self.key_filters.override_active = false;
                }
            }
            return;
        }
        let Some(focus) = keyboard.current_focus() else {
            return;
        };
        let (sent_code, remove, add) = match rewrite {
            Rewrite::Pass => return,
            Rewrite::Mods { remove, add } => (code, remove, add),
            Rewrite::Key { evdev, remove, add } => (Keycode::new(evdev + XKB_OFFSET), remove, add),
        };
        // SAFETY: the keymap reference only lives for this lookup of modifier indices, inside the lock
        let idx = keyboard.with_xkb_state(self, |ctx| {
            ModIndices::of(unsafe { ctx.xkb().lock().unwrap().keymap() })
        });
        let mut told = keyboard.modifier_state();
        told.serialized = rewrite_masks(told.serialized, &idx, remove, add);
        KeyboardTarget::modifiers(&focus, &self.seat.clone(), self, told, serial);
        keyboard.input_forward(self, sent_code, KeyState::Pressed, serial, time, false);
        self.key_filters.rewritten.insert(
            code.raw(),
            Sent {
                code: sent_code,
                focus,
            },
        );
        self.key_filters.override_active = true;
    }

    /// Type the chord `mods` + `sym` into the focused client for the compositor (the menu bar's Edit items): the client
    /// is told only `mods`, then the key that types `sym` in the active layout ([`key_for_sym`]) goes down and at once
    /// up through [`Self::forward_rewritten`], which puts the real modifiers back. The chord passes neither the shortcuts
    /// nor the rewrite, and no repeat is armed. Nothing is sent while a rewritten key is held (it keeps the modifiers
    /// it was given) or while the key itself is held.
    pub(crate) fn forward_chord(&mut self, sym: u32, mods: RawMods) {
        if !self.key_filters.rewritten.is_empty() {
            return;
        }
        let keyboard = self.seat.get_keyboard().unwrap();
        // SAFETY: the keymap and state references only live for this lookup, inside the lock
        let code = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            key_for_sym(unsafe { xkb.keymap() }, unsafe { xkb.state() }, sym)
        });
        let Some(code) = code.filter(|c| !keyboard.pressed_keys().contains(c)) else {
            return;
        };
        let rewrite = Rewrite::Key {
            evdev: evdev(code),
            remove: RawMods::SHIFT | RawMods::CTRL | RawMods::ALT | RawMods::SUPER,
            add: mods,
        };
        let time = self.now_ms() as u32;
        for state in [KeyState::Pressed, KeyState::Released] {
            self.forward_rewritten(code, state, rewrite, SERIAL_COUNTER.next_serial(), time);
        }
    }

    /// Keep a release chord (`release-shortcuts`) the active keymap can type: when no chord bound to it has a key in the
    /// layout, the default is bound as well, so the person always has a way to get the shortcuts back. Called when the
    /// keymap or the layout changes.
    pub fn keep_release_chord_typeable(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let syms: Vec<u32> = self
            .key_router
            .bindings
            .entries()
            .iter()
            .filter(|(_, action)| *action == Action::ReleaseShortcuts)
            .map(|(chord, _)| chord.sym)
            .collect();
        // SAFETY: the keymap and state references only live for this lookup, inside the lock
        let typeable = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            let (keymap, state) = (unsafe { xkb.keymap() }, unsafe { xkb.state() });
            syms.iter()
                .any(|sym| key_for_sym(keymap, state, *sym).is_some())
        });
        if typeable {
            return;
        }
        let default = cupertino_input::ShortcutMap::release_chord();
        let chord = Chord {
            mods: Mods {
                logo: true,
                ..Mods::NONE
            },
            sym: default.keysym,
        };
        tracing::warn!("no chord that releases the shortcuts can be typed on the active keymap: Super+Escape is bound");
        self.key_router
            .bindings
            .bind(chord, Action::ReleaseShortcuts);
    }

    /// Tell the focused client the keyboard's real modifiers again, when a rewrite replaced them.
    pub(crate) fn restore_client_mods(&mut self) {
        if !self.key_filters.override_active {
            return;
        }
        self.key_filters.override_active = false;
        let keyboard = self.seat.get_keyboard().unwrap();
        if let Some(focus) = keyboard.current_focus() {
            KeyboardTarget::modifiers(
                &focus,
                &self.seat.clone(),
                self,
                keyboard.modifier_state(),
                SERIAL_COUNTER.next_serial(),
            );
        }
    }

    /// True when the key is the physical key of a rewrite in progress: its release goes through [`Self::forward_rewritten`].
    pub(crate) fn is_rewritten(&self, code: Keycode) -> bool {
        self.key_filters.rewritten.contains_key(&code.raw())
    }

    /// A key event from an input device: the start of the filter chain.
    pub fn key_from_device(&mut self, code: Keycode, state: KeyState, time: u32) {
        crate::frame_stats::note_input(self);
        if self.key_filters.unfiltered.contains(&evdev(code)) {
            if state == KeyState::Released {
                self.key_filters.unfiltered.remove(&evdev(code));
            }
            self.keyboard_event_now(code, state, time);
            return;
        }
        let now = self.now_ms();
        if let Some(slow) = self.key_filters.slow.as_mut() {
            let step = match state {
                KeyState::Pressed => slow.press(evdev(code), now),
                KeyState::Released => slow.release(evdev(code)),
            };
            if step == SlowStep::Deliver {
                // with slow keys on the caps-lock delay is not applied (filter order)
                self.key_after_delays(code, state, time);
            }
            return;
        }
        if self.caps_gate(code, state, time) {
            return;
        }
        self.key_after_delays(code, state, time);
    }

    /// A key event that passed slow keys and the caps-lock delay: sticky keys, mouse keys, then the keyboard.
    pub fn key_after_delays(&mut self, code: Keycode, state: KeyState, time: u32) {
        let now = self.now_ms();
        let pressed = state == KeyState::Pressed;
        let sticky = self.key_filters.sticky.key(evdev(code), pressed, now);
        if let Some(on) = sticky.toggled {
            tracing::info!(on, "sticky keys toggled by the Shift key");
        }
        if sticky.deliver {
            self.key_after_sticky(code, state, time);
        }
        for key in sticky.release {
            self.key_after_sticky(Keycode::new(key + XKB_OFFSET), KeyState::Released, time);
        }
    }

    fn key_after_sticky(&mut self, code: Keycode, state: KeyState, time: u32) {
        let pressed = state == KeyState::Pressed;
        // a release of a key the keyboard has seen pressed always reaches it (mouse keys may have been turned on
        // while the key was down)
        let down = self
            .seat
            .get_keyboard()
            .is_some_and(|k| k.pressed_keys().contains(&code));
        if pressed || !down {
            let out = self
                .key_filters
                .mouse
                .key(evdev(code), pressed, self.now_ms());
            if let Some(on) = out.toggled {
                tracing::info!(on, "mouse keys toggled by the Alt key");
            }
            for action in out.actions {
                self.mouse_keys_action(action, time);
            }
            if out.consumed {
                // a key of the pointer keypad: the Super key held now is a modifier of it, not a tap
                self.key_filters.super_tap.disturb();
                return;
            }
        }
        self.globe_key(code, pressed);
        let others_held = self
            .seat
            .get_keyboard()
            .is_some_and(|k| !k.pressed_keys().is_empty());
        let tapped =
            self.key_filters
                .super_tap
                .key(evdev(code), pressed, self.now_ms(), others_held);
        self.keyboard_event_now(code, state, time);
        if tapped && !self.shortcut_inhibit.holding() {
            let ctx = self.action_context();
            if let Some(plan) = self.key_router.tap_plan(&ctx) {
                self.execute_plan(plan);
            }
        }
    }

    /// The globe / fn key: a quick tap runs the configured action.
    fn globe_key(&mut self, code: Keycode, pressed: bool) {
        let now = self.now_ms() as f64;
        if evdev(code) != KEY_FN {
            self.key_filters.globe.other_key();
            return;
        }
        if pressed {
            self.key_filters.globe.down(now);
            return;
        }
        if self.key_filters.globe.up(now) != GlobeOutcome::Tap {
            return;
        }
        match self.key_filters.fn_usage {
            FnUsage::DoNothing => {}
            // the globe key moves the HUD selection to the next item (log string); without held modifiers
            // the switch is immediate
            FnUsage::ChangeInputSource => self.input_source_step(true, Step::Next),
            FnUsage::ShowEmojiPicker => {
                tracing::info!("globe key: the emoji picker is not available in this compositor")
            }
            FnUsage::StartDictation => {
                tracing::info!("globe key: dictation is not available in this compositor")
            }
        }
    }

    fn mouse_keys_action(&mut self, action: PointerAction, time: u32) {
        match action {
            PointerAction::Move { dx, dy } => {
                let pos =
                    self.pointer.current_location() + Point::from((f64::from(dx), f64::from(dy)));
                self.pointer_moved_to(pos, time, None);
            }
            PointerAction::Press => {
                self.pointer_button(crate::events::BTN_LEFT, ButtonState::Pressed, time)
            }
            PointerAction::Release => {
                self.pointer_button(crate::events::BTN_LEFT, ButtonState::Released, time)
            }
        }
    }

    /// Start the compositor repeat of `code` (xkb keycode); it replaces any repeating key.
    pub fn arm_key_repeat(&mut self, code: u32, target: RepeatTarget) {
        let now = self.now_ms();
        self.key_filters
            .repeat
            .set_config(self.config.keyboard.effective_repeat());
        // the repeat generator skips modifiers and the non-repeating keys
        self.key_filters
            .repeat
            .press(code.saturating_sub(XKB_OFFSET), now, false);
        self.key_filters.repeat_target = self.key_filters.repeat.key().map(|_| target);
    }

    /// A key went down that the compositor does not repeat, or the repeating key went up.
    pub fn key_repeat_event(&mut self, code: u32, pressed: bool) {
        let ev = code.saturating_sub(XKB_OFFSET);
        if pressed || self.key_filters.repeat.key() == Some(ev) {
            self.stop_key_repeat();
        }
    }

    pub fn stop_key_repeat(&mut self) {
        self.key_filters.repeat.stop();
        self.key_filters.repeat_target = None;
    }

    /// Timed work of the filters: slow-keys acceptance, mouse-keys motion, the compositor's key repeat.
    pub fn tick_key_filters(&mut self) {
        let now = self.now_ms();
        let time = now as u32;
        if let Some(code) = self.key_filters.slow.as_mut().and_then(|s| s.due(now)) {
            self.key_after_delays(Keycode::new(code + XKB_OFFSET), KeyState::Pressed, time);
        }
        for action in self.key_filters.mouse.poll(now) {
            self.mouse_keys_action(action, time);
        }
        let ticks = self.key_filters.repeat.poll(now);
        for _ in ticks {
            let Some(target) = self.key_filters.repeat_target else {
                break;
            };
            match target {
                RepeatTarget::AccentTrigger(ch) => {
                    self.stop_key_repeat();
                    self.accent_trigger(ch);
                }
                RepeatTarget::Compositor(key) => {
                    let mods = self.current_physical_mods();
                    // Shift pressed or released while Tab repeats in the switcher turns the repeat
                    // round; any other modifier change ends the repeat
                    let flipped = self.wm.switcher().is_some()
                        && mods.shift != key.mods.shift
                        && crate::input::Mods {
                            shift: key.mods.shift,
                            ..mods
                        } == key.mods;
                    let key = if flipped {
                        crate::overlays::OverlayKey { mods, ..key }
                    } else {
                        key
                    };
                    if mods != key.mods || !self.replay_key(&key) {
                        self.stop_key_repeat();
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SUPER: u32 = 125;
    const TAB: u32 = 15;

    #[test]
    fn the_super_key_alone_is_a_tap() {
        let mut tap = SuperTap::default();
        assert!(!tap.key(SUPER, true, 1000, false));
        assert!(tap.key(SUPER, false, 1100, false));
        // the right Super key too
        assert!(!tap.key(126, true, 2000, false));
        assert!(tap.key(126, false, 2100, false));
    }

    #[test]
    fn a_key_a_click_or_a_long_hold_cancels_the_tap() {
        let mut tap = SuperTap::default();
        tap.key(SUPER, true, 0, false);
        tap.key(TAB, true, 10, false);
        tap.key(TAB, false, 20, false);
        assert!(!tap.key(SUPER, false, 30, false), "Super+Tab");

        tap.key(SUPER, true, 100, false);
        tap.disturb();
        assert!(
            !tap.key(SUPER, false, 110, false),
            "Super and a pointer button"
        );

        tap.key(SUPER, true, 200, false);
        assert!(
            !tap.key(SUPER, false, 200 + SUPER_TAP_MAX_MS + 1, false),
            "held too long"
        );
        tap.key(SUPER, true, 2000, false);
        assert!(
            tap.key(SUPER, false, 2000 + SUPER_TAP_MAX_MS, false),
            "held for the limit"
        );

        // pressed while another key was down: a chord already
        assert!(!tap.key(SUPER, true, 3000, true));
        assert!(!tap.key(SUPER, false, 3010, false));
        // a release of Super that was not preceded by its press
        assert!(!tap.key(SUPER, false, 4000, false));
    }

    /// The keymap of a generated layout with both Alt keys as the third-level shift, as the decoded input sources use.
    fn keymap() -> xkb::Keymap {
        let layout = cupertino_keyboard::Layout::from_json(include_str!(
            "../../cupertino-keyboard/tests/test_layout.json"
        ))
        .unwrap();
        let out = cupertino_keyboard::generate(&layout, &cupertino_keyboard::GenOptions::default())
            .unwrap();
        xkb::Keymap::new_from_string(
            &xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            out.keymap,
            xkb::KEYMAP_FORMAT_TEXT_V1,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the generated keymap compiles")
    }

    fn system_keymap(layout: &str) -> xkb::Keymap {
        let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
        xkb::Keymap::new_from_names(
            &context,
            "",
            "",
            layout,
            "",
            None,
            xkb::KEYMAP_COMPILE_NO_FLAGS,
        )
        .expect("the system layout compiles")
    }

    #[test]
    fn a_chord_takes_the_key_that_types_its_letter_in_the_active_layout() {
        use smithay::input::keyboard::keysyms::{KEY_a, KEY_c, KEY_z};
        // evdev KEY_Z, KEY_W and KEY_C, as xkb keycodes
        let (z, w, c) = (44 + XKB_OFFSET, 17 + XKB_OFFSET, 46 + XKB_OFFSET);
        let us = system_keymap("us");
        let key = |keymap: &xkb::Keymap, state: &xkb::State, sym| {
            key_for_sym(keymap, state, sym).map(|k| k.raw())
        };
        assert_eq!(key(&us, &xkb::State::new(&us), KEY_z), Some(z));
        // AZERTY types z with the key of the QWERTY w, and a with that of q
        let fr = system_keymap("fr");
        assert_eq!(key(&fr, &xkb::State::new(&fr), KEY_z), Some(w));
        assert_eq!(
            key(&fr, &xkb::State::new(&fr), KEY_a),
            Some(16 + XKB_OFFSET)
        );
        // no key of the Russian group types c: the shortcuts' Latin fallback names the key of the first group
        let both = system_keymap("us,ru");
        let mut russian = xkb::State::new(&both);
        russian.update_mask(0, 0, 0, 0, 0, 1);
        assert_eq!(key(&both, &russian, KEY_c), Some(c));
        assert_eq!(key(&both, &xkb::State::new(&both), KEY_c), Some(c));
    }

    #[test]
    fn rewritten_masks_swap_the_modifiers_and_keep_the_locks() {
        let idx = ModIndices::of(&keymap());
        // the real modifiers sit at their fixed positions: Shift 0, Control 2, Mod1 3, Mod4 6, Mod5 7
        assert_eq!(
            idx,
            ModIndices {
                shift: 0,
                control: 2,
                mod1: 3,
                mod4: 6,
                mod5: 7
            }
        );
        let (shift, ctrl, mod4, mod5, lock, num) = (1, 1 << 2, 1 << 6, 1 << 7, 1 << 1, 1 << 4);
        let real = SerializedMods {
            depressed: shift | mod4,
            latched: 0,
            locked: lock | num,
            layout_effective: 1,
        };
        let told = rewrite_masks(real, &idx, RawMods::SUPER, RawMods::CTRL);
        assert_eq!(
            told,
            SerializedMods {
                depressed: shift | ctrl,
                latched: 0,
                locked: lock | num,
                layout_effective: 1
            }
        );
        // Super becomes Ctrl and Shift: the terminal copy chord
        assert_eq!(
            rewrite_masks(
                SerializedMods {
                    depressed: mod4,
                    ..real
                },
                &idx,
                RawMods::SUPER,
                RawMods::CTRL | RawMods::SHIFT
            )
            .depressed,
            ctrl | shift
        );
        // Alt under `level3(alt_switch)` is Mod5: it goes with Alt
        assert_eq!(
            rewrite_masks(
                SerializedMods {
                    depressed: mod5 | shift,
                    ..real
                },
                &idx,
                RawMods::ALT,
                RawMods::CTRL
            )
            .depressed,
            shift | ctrl
        );
    }
}
