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
//! `wl_keyboard.repeat_info`; keys the compositor consumes (repeating shortcuts, the input-source switcher's arrows, the
//! accent picker's navigation) never reach a client, so the compositor repeats them with the same delay and interval
//! (one repeating key). A repeat replays the press through the overlays and the shortcut router
//! while the key and the same modifiers stay down. The accent picker's trigger is a repeat too: a held letter with
//! accent alternatives reaches the application as a press and an immediate release (so the application never
//! repeats it), and the compositor's first repeat opens the picker (the input method, not the application,
//! receives the repeats of such a key).
//!
//! Every timed filter is polled from the frame tick (`tick_overlays`); the backends iterate their loop every few
//! milliseconds.

use cupertino_keyboard::modifiers::{FnUsage, GlobeOutcome, RawMods};
use cupertino_keyboard::mouse_keys::PointerAction;
use cupertino_keyboard::rewrite::{decide, is_printable, ClientTarget, KeyPress, Rewrite};
use cupertino_keyboard::slow_keys::SlowStep;
use cupertino_keyboard::switcher::Step;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::{KeyboardTarget, Keycode, ModifiersState};
use smithay::utils::{Point, SERIAL_COUNTER};

use crate::input::{Action, Chord, Mods};
use crate::state::Cupertino;

pub use cupertino_server_input::kbd_filters::{
    evdev, key_for_sym, rewrite_masks, KeyFilters, ModIndices, RepeatTarget, Sent, SuperTap,
    KEY_FN, XKB_OFFSET,
};

impl Cupertino {
    /// Whether the key window's client uses the desktop's toolkit: it bound `cupertino_app_effects_v1`, as every window of
    /// the toolkit does, whatever its application id. An X11 program's windows come from the satellite, which never does.
    pub(crate) fn key_client_kit(&self) -> bool {
        self.wm
            .key_window()
            .and_then(|w| self.entry(w))
            .and_then(|e| smithay::reexports::wayland_server::Resource::client(e.surface()))
            .and_then(|c| {
                c.get_data::<crate::state::ClientState>()
                    .map(|d| d.kit.load(std::sync::atomic::Ordering::Relaxed))
            })
            .unwrap_or(false)
    }

    /// Whether the keyboard is on the key window (one of its surfaces), not on a layer surface or the lock screen.
    pub(crate) fn key_window_has_keyboard(&self) -> bool {
        let Some(focus) = self.seat.get_keyboard().and_then(|k| k.current_focus()) else {
            return false;
        };
        self.wm
            .key_window()
            .and_then(|w| self.entry(w))
            .is_some_and(|e| e.has_surface(&focus))
    }

    /// The class of the key window's client for the chord rewrite ([`crate::kbd::KeyboardSettings::client_class`]).
    pub(crate) fn key_client_class(&self) -> cupertino_keyboard::rewrite::ClientClass {
        self.config
            .keyboard
            .client_class(&self.key_app_id(), self.key_client_kit())
    }

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
        if !self.key_window_has_keyboard() {
            return Rewrite::Pass;
        }
        let kb = &self.config.keyboard;
        let app = self.key_app_id();
        let target = ClientTarget {
            class: self.key_client_class(),
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
            // the globe key moves the switcher's selection to the next item (log string); without held modifiers
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
