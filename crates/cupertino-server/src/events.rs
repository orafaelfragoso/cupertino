//! Input event processing for every backend: keyboard through the [`KeyRouter`], pointer routing
//! between the shell, the window model and clients, and touchpad gestures.

use cupertino_wm::{Command, Edges, Modifiers as WmModifiers, Point as MPoint, WindowId};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisSource, ButtonState, Device, DeviceCapability, Event,
    GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent, GestureSwipeUpdateEvent,
    InputBackend, InputEvent, KeyState, KeyboardKeyEvent, PointerAxisEvent, PointerButtonEvent,
    PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent as PinchUpdate, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent as SwipeUpdate, MotionEvent, RelativeMotionEvent,
};
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use cupertino_input::{
    Disposition, GestureEvent, GestureKind, GestureOutput, Outcome as GestureOutcome,
};
use cupertino_protocols::server::cupertino_dock_v1::Gesture as DockGesture;

use crate::input::{ActionPlan, KeyEvent, KeyOutcome, Mods};
use crate::kbd_filters::RepeatTarget;
use crate::kbd_ui::AccentRoute;
use crate::overlays::OverlayKey;
use crate::pointer_constraints::Constrained;
use crate::state::{Cupertino, PointerMode, WindowTarget};
use cupertino_keyboard::rewrite::Rewrite;
use smithay::input::keyboard::{keysyms, Keycode};

pub use cupertino_server_input::events::{
    axis_index, AxisCarry, AxisInput, GestureSource, MouseIdentity, PendingDrag, PointerDevice,
    BTN_LEFT, BTN_RIGHT,
};

/// Who takes one key event inside the keyboard filter.
#[derive(Clone, Debug)]
enum KeyRoute {
    /// The focused client.
    Forward,
    /// The focused client, released at once: a held letter with accent alternatives (see [`AccentRoute::Armed`]).
    AccentArmed(char),
    /// The input-source switch (Escape, Return, the arrows).
    Switch,
    /// The accent picker; `repeats` for its navigation keys.
    Accent { repeats: bool },
    /// A compositor shortcut or a swallowed release.
    Plan(ActionPlan),
    /// Escape during a client's drag and drop: the drag ends without a drop.
    CancelDrag,
}

/// The identity of a mouse for its resolution: its key in `mouse_dpi` and the hardware database's `MOUSE_DPI` of its
/// node, when it has one.
fn mouse_identity<D: Device>(device: &D) -> MouseIdentity {
    let key = device.usb_id().map(|(product, vendor)| {
        cupertino_defaults::compositor::mouse_dpi_key(vendor, product, &device.name())
    });
    #[cfg(feature = "udev")]
    let hwdb_dpi = device
        .syspath()
        .and_then(|path| cupertino_devices::Device::from_syspath(&path).ok())
        .and_then(|node| node.udev_property("MOUSE_DPI"));
    #[cfg(not(feature = "udev"))]
    let hwdb_dpi = None;
    MouseIdentity {
        key,
        name: device.name(),
        hwdb_dpi,
    }
}

impl Cupertino {
    /// The physical modifier keys of the keyboard (Super is `logo`): what the system shortcuts are matched on.
    pub fn current_physical_mods(&self) -> Mods {
        self.seat
            .get_keyboard()
            .map(|k| crate::kbd::KeyboardSettings::physical(&k.modifier_state()))
            .unwrap_or_default()
    }

    /// Logical modifiers of the keyboard (the primary modifier of the profile is `logo`).
    pub fn current_mods(&self) -> Mods {
        self.seat
            .get_keyboard()
            .map(|k| self.config.keyboard.translate(&k.modifier_state()))
            .unwrap_or_default()
    }

    /// A touchpad gesture began: the Super key held now is a modifier of it, not a tap. (Touch screens and tablets are
    /// not opened; when they are, a touch-down and a tip-down must call this too.)
    pub fn gesture_begun(&mut self) {
        self.key_filters.super_tap.disturb();
    }

    pub fn process_input_event<B: InputBackend>(&mut self, event: InputEvent<B>) {
        // The person's input never pauses desktop control: both use the desktop at once, each source's presses its own.
        let was_physical = self.control.physical_input;
        self.control.physical_input = true;
        if !matches!(
            event,
            InputEvent::DeviceAdded { .. } | InputEvent::DeviceRemoved { .. }
        ) {
            self.input_activity();
        }
        match event {
            InputEvent::DeviceAdded { device }
                if device.has_capability(DeviceCapability::Pointer) =>
            {
                let touchpad = device.has_capability(DeviceCapability::Gesture);
                let identity = if touchpad {
                    MouseIdentity::default()
                } else {
                    mouse_identity(&device)
                };
                self.pointer_devices.insert(
                    device.id(),
                    PointerDevice::with_identity(touchpad, &self.config.pointer, identity),
                );
            }
            InputEvent::DeviceRemoved { device } => {
                if self.pointer_devices.remove(&device.id()).is_some() {
                    self.publish_session_settings();
                    self.sync_scroll_bars();
                }
            }
            InputEvent::Keyboard { event } => self.on_keyboard::<B>(event),
            InputEvent::PointerMotion { event } => self.on_pointer_motion::<B>(event),
            InputEvent::PointerMotionAbsolute { event } => {
                let Some((id, pixels)) = self.absolute_device_space() else {
                    self.control.physical_input = was_physical;
                    return;
                };
                let at = event.position_transformed(pixels);
                let Some(pos) = self.output_pixels_to_global(id, (at.x, at.y)) else {
                    self.control.physical_input = was_physical;
                    return;
                };
                self.pointer_moved_to(pos, event.time_msec(), None);
            }
            InputEvent::PointerButton { event } => self.on_pointer_button::<B>(event),
            InputEvent::PointerAxis { event } => self.on_pointer_axis::<B>(event),
            InputEvent::GestureSwipeBegin { event } => {
                self.gesture_begun();
                self.on_swipe_begin::<B>(event)
            }
            InputEvent::GestureSwipeUpdate { event } => self.on_swipe_update::<B>(event),
            InputEvent::GestureSwipeEnd { event } => self.on_swipe_end::<B>(event),
            InputEvent::GesturePinchBegin { event } => {
                self.gesture_begun();
                self.on_pinch_begin::<B>(event)
            }
            InputEvent::GesturePinchUpdate { event } => self.on_pinch_update::<B>(event),
            InputEvent::GesturePinchEnd { event } => self.on_pinch_end::<B>(event),
            InputEvent::GestureHoldBegin { event } => {
                self.gesture_begun();
                let pointer = self.pointer.clone();
                let (time, fingers) = (event.time_msec(), event.fingers());
                self.gestures.handle(GestureEvent::HoldBegin {
                    time: f64::from(time) / 1000.0,
                    fingers,
                });
                pointer.gesture_hold_begin(
                    self,
                    &GestureHoldBeginEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                        fingers,
                    },
                );
            }
            InputEvent::GestureHoldEnd { event } => {
                let pointer = self.pointer.clone();
                let (time, cancelled) = (event.time_msec(), event.cancelled());
                self.gestures.handle(GestureEvent::HoldEnd {
                    time: f64::from(time) / 1000.0,
                    cancelled,
                });
                pointer.gesture_hold_end(
                    self,
                    &GestureHoldEndEvent {
                        serial: SERIAL_COUNTER.next_serial(),
                        time,
                        cancelled,
                    },
                );
            }
            _ => {}
        }
        self.control.physical_input = was_physical;
    }

    fn on_keyboard<B: InputBackend>(&mut self, evt: B::KeyboardKeyEvent) {
        self.key_from_device(evt.key_code(), evt.state(), evt.time_msec());
    }

    /// One key event through the overlays (input-source switch, accent picker), the menus and
    /// the key router; whatever none of them takes goes to the focused client. A second press of a
    /// key that is down, or a release of a key that is not, changes nothing and is dropped (sticky
    /// keys and mouse keys can produce both).
    pub fn keyboard_event_now(
        &mut self,
        code: smithay::input::keyboard::Keycode,
        state: KeyState,
        time: u32,
    ) {
        self.input_came();
        let serial = SERIAL_COUNTER.next_serial();
        let keyboard = self.seat.get_keyboard().unwrap();
        let pressed = state == KeyState::Pressed;
        if !self
            .control_physical_transition(cupertino_control_model::Input::Key(code.raw()), pressed)
        {
            return;
        }
        self.control_user_input(serial.into(), keyboard.current_focus());
        if keyboard.pressed_keys().contains(&code) == pressed {
            return;
        }
        let ctx = self.action_context();
        let mut mod_plan = crate::input::ActionPlan::default();
        let mut route = KeyRoute::Forward;
        let mut okey = OverlayKey {
            code: code.raw(),
            sym: 0,
            text: None,
            mods: Mods::NONE,
            alt_gr: false,
        };
        // a key that went to the client rewritten is released the same way, whatever the router says about the release
        let rewritten_release = !pressed && self.is_rewritten(code);
        let mut rewrite = Rewrite::Pass;
        keyboard.input::<(), _>(self, code, state, serial, time, |st, modifiers, handle| {
            let mods = crate::kbd::KeyboardSettings::physical(modifiers);
            mod_plan = st.key_router.modifiers_changed(mods, &ctx);
            let sym = handle
                .raw_latin_sym_or_raw_current_sym()
                .unwrap_or_else(|| handle.modified_sym())
                .raw();
            okey = OverlayKey {
                code: code.raw(),
                sym,
                text: handle.modified_sym().key_char(),
                mods,
                alt_gr: st.config.keyboard.alt_gr(modifiers),
            };
            // Super+Alt+S goes to the screen reader (the worker's at the login screen) before anything else sees it, while the
            // session is locked too: it is the one shortcut that acts at the lock screen, so the reader can be turned on to
            // unlock; the router turns every other one off then
            route = if !st.control.dispatching
                && st.screen_reader_key(code.raw(), sym, &mods, pressed)
            {
                KeyRoute::Plan(crate::input::ActionPlan::default())
            } else {
                st.route_key(&okey, pressed, false, &ctx)
            };
            match route {
                _ if rewritten_release => FilterResult::Intercept(()),
                KeyRoute::Forward => {
                    if pressed && !ctx.input_held {
                        rewrite = st.key_rewrite(code, sym, okey.text, modifiers);
                    }
                    if rewrite == Rewrite::Pass {
                        // the client was told rewritten modifiers for an earlier key; a new key goes with the real ones
                        if pressed {
                            st.restore_client_mods();
                        }
                        FilterResult::Forward
                    } else {
                        FilterResult::Intercept(())
                    }
                }
                KeyRoute::AccentArmed(_) => FilterResult::Forward,
                _ => FilterResult::Intercept(()),
            }
        });
        if rewritten_release || rewrite != Rewrite::Pass {
            self.forward_rewritten(code, state, rewrite, serial, time);
        }
        self.execute_plan(mod_plan);
        if pressed && !crate::overlays::is_modifier_sym(okey.sym) {
            self.last_user_event = Some(self.now());
            let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
            self.note_input(focus.as_ref());
        }
        if !ctx.locked {
            self.input_sources_modifiers(okey.mods);
        }
        // the accent trigger arms its own repeat
        let armed = matches!(route, KeyRoute::AccentArmed(_));
        let repeats = self.finish_key_route(route, &okey, time);
        if pressed && repeats {
            self.arm_key_repeat(okey.code, RepeatTarget::Compositor(okey));
        } else if !armed && (!pressed || !crate::overlays::is_modifier_sym(okey.sym)) {
            // a new key replaces the repeating one, its release stops it; modifiers end a repeat at its next tick
            self.key_repeat_event(okey.code, pressed);
        }
        self.flush_text_ops();
        self.request_redraw();
    }

    /// The decision for one key event (shared by real keys and the compositor's repeats): the input-source HUD,
    /// the accent picker, then the shortcut router.
    fn route_key(
        &mut self,
        key: &OverlayKey,
        pressed: bool,
        repeat: bool,
        ctx: &crate::input::ActionContext,
    ) -> KeyRoute {
        let sym = key.sym;
        let modifier_key = crate::overlays::is_modifier_sym(sym);
        if pressed && sym == keysyms::KEY_Escape && key.mods.ctrl && key.mods.alt {
            if self.control.physical_input {
                self.revoke_controls(cupertino_control_model::Revocation::Stopped);
            }
            if self.control.physical_input || self.control.dispatching {
                self.key_router.swallow(sym);
                return KeyRoute::Plan(ActionPlan::default());
            }
        }
        if pressed
            && self.control.dispatching
            && self.key_router.control_reserved(&KeyEvent {
                sym,
                pressed,
                mods: key.mods,
                repeat,
                shifted: None,
                alt_gr: key.alt_gr,
            })
        {
            self.key_router.swallow(sym);
            return KeyRoute::Plan(ActionPlan::default());
        }
        // 0. Escape ends a client's drag; the client never sees the key
        if pressed && !repeat && sym == keysyms::KEY_Escape && self.client_drag.is_some() {
            self.key_router.swallow(sym);
            return KeyRoute::CancelDrag;
        }
        // 1. the input-source switch takes Escape, Return and the arrows
        if pressed
            && !modifier_key
            && !ctx.locked
            && !ctx.input_held
            && self.overlays.inputs.switch_open()
            && matches!(
                sym,
                keysyms::KEY_Escape
                    | keysyms::KEY_Return
                    | keysyms::KEY_KP_Enter
                    | keysyms::KEY_Up
                    | keysyms::KEY_Down
            )
        {
            self.key_router.swallow(sym);
            return KeyRoute::Switch;
        }
        // 2. accent picker (hold a letter key)
        if !modifier_key && !ctx.locked && !ctx.input_held {
            match self.accent_filter(key, pressed) {
                AccentRoute::Pass => {}
                AccentRoute::Armed(c) => return KeyRoute::AccentArmed(c),
                AccentRoute::Consumed { repeats } => {
                    if pressed {
                        self.key_router.swallow(sym);
                    }
                    return KeyRoute::Accent { repeats };
                }
            }
        }
        match self.key_router.key(
            KeyEvent {
                sym,
                pressed,
                mods: key.mods,
                repeat,
                shifted: key
                    .text
                    .filter(|_| key.mods.shift)
                    .map(|c| smithay::input::keyboard::xkb::utf32_to_keysym(c as u32).raw()),
                alt_gr: key.alt_gr,
            },
            ctx,
        ) {
            KeyOutcome::Forward => KeyRoute::Forward,
            KeyOutcome::Consumed(plan) => KeyRoute::Plan(plan),
        }
    }

    /// Act on a [`KeyRoute`] after the keyboard state saw the event. True when the consumer repeats the key while it
    /// is held (the caller starts the compositor's repeat).
    fn finish_key_route(&mut self, route: KeyRoute, key: &OverlayKey, time: u32) -> bool {
        match route {
            KeyRoute::Forward => false,
            KeyRoute::AccentArmed(c) => {
                // the application gets the press and at once the release, so it never repeats the key; the
                // compositor's first repeat opens the picker and the physical release is swallowed
                if let Some(kb) = self.seat.get_keyboard() {
                    kb.input_forward(
                        self,
                        Keycode::new(key.code),
                        KeyState::Released,
                        SERIAL_COUNTER.next_serial(),
                        time,
                        false,
                    );
                }
                self.overlays.accent.swallow_release.insert(key.code);
                self.arm_key_repeat(key.code, RepeatTarget::AccentTrigger(c));
                false
            }
            KeyRoute::Switch => {
                self.input_sources_key(key.sym);
                matches!(key.sym, keysyms::KEY_Up | keysyms::KEY_Down)
            }
            KeyRoute::Accent { repeats } => repeats,
            KeyRoute::CancelDrag => {
                self.cancel_client_drag(time);
                false
            }
            KeyRoute::Plan(plan) => {
                let repeats = plan.repeats();
                self.execute_plan(plan);
                repeats
            }
        }
    }

    /// Replay the press of a key the compositor repeats. False when the key does not repeat any more: nothing in the
    /// compositor takes it (the HUD or popup closed, the shortcut is gone) or its consumer does not repeat it. The
    /// client never saw the press, so it gets nothing either way.
    pub fn replay_key(&mut self, key: &OverlayKey) -> bool {
        let ctx = self.action_context();
        let route = self.route_key(key, true, true, &ctx);
        if matches!(route, KeyRoute::Forward | KeyRoute::AccentArmed(_)) {
            return false;
        }
        let time = (self.now() * 1000.0) as u32;
        let repeats = self.finish_key_route(route, key, time);
        self.flush_text_ops();
        self.request_redraw();
        repeats
    }

    /// Scripted key press (`--key`, `--type`): a chord such as `Super+space`, `Return` or `a` goes through the
    /// same decisions as a real key; one nothing consumes reaches the focused client as real key events.
    pub fn scripted_key(&mut self, chord: &str) -> Result<(), String> {
        crate::frame_stats::note_input(self);
        let ch = crate::input::parse_chord(chord)?;
        let command = ch.mods.ctrl || ch.mods.alt || ch.mods.logo;
        let mut text = char::from_u32(smithay::input::keyboard::xkb::keysym_to_utf32(
            smithay::input::keyboard::xkb::Keysym::new(ch.sym),
        ))
        .filter(|c| *c != '\0');
        if ch.mods.shift {
            text = text.map(|c| c.to_ascii_uppercase());
        }
        self.scripted_overlay_key(OverlayKey {
            code: 0,
            sym: ch.sym,
            text: text.filter(|_| !command),
            mods: ch.mods,
            alt_gr: false,
        });
        Ok(())
    }

    /// Scripted modifier key held down or let go; it reaches the focused client as a real key event.
    pub fn scripted_modifier(&mut self, name: &str, pressed: bool) -> Result<(), String> {
        // evdev codes of the modifier keys
        let evdev = match name.to_ascii_lowercase().as_str() {
            "shift" => 42,
            "ctrl" => 29,
            "alt" => 56,
            "super" => 125,
            other => return Err(format!("not a modifier: {other}")),
        };
        self.scripted_evdev_key(evdev, pressed);
        Ok(())
    }

    /// Scripted key of evdev code `evdev` held down or let go, as a device would deliver it: through the filters, the
    /// shortcuts, the Super tap, the input-source switch and the accent picker's trigger, and on to the focused client
    /// when nothing takes it. A letter held down repeats as a real one does.
    pub fn scripted_evdev_key(&mut self, evdev: u32, pressed: bool) {
        let time = (self.now() * 1000.0) as u32;
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        self.key_after_delays(
            smithay::input::keyboard::Keycode::new(evdev + 8),
            state,
            time,
        );
    }

    /// Scripted text: every character is typed as its own key.
    pub fn scripted_type(&mut self, text: &str) {
        for c in text.chars() {
            let sym = smithay::input::keyboard::xkb::utf32_to_keysym(c as u32).raw();
            self.scripted_overlay_key(OverlayKey {
                code: 0,
                sym: crate::input::normalize_sym(sym),
                text: Some(c),
                mods: Mods::NONE,
                alt_gr: false,
            });
        }
    }

    fn scripted_overlay_key(&mut self, key: OverlayKey) {
        let ctx = self.action_context();
        let sym = key.sym;
        if !ctx.locked && self.overlays.inputs.switch_open() && self.input_sources_key(sym) {
            return;
        }
        // a scripted key is a press and a release at once, so it never arms the accent trigger
        if !ctx.locked && matches!(self.accent_filter(&key, true), AccentRoute::Consumed { .. }) {
            self.flush_text_ops();
            return;
        }
        match self.key_router.key(
            KeyEvent {
                sym,
                pressed: true,
                mods: key.mods,
                repeat: false,
                shifted: None,
                alt_gr: false,
            },
            &ctx,
        ) {
            KeyOutcome::Consumed(plan) => {
                self.execute_plan(plan);
                // a scripted key is a press and a release: the release ends a held binding (the power key's hold)
                let ctx = self.action_context();
                if let KeyOutcome::Consumed(plan) = self.key_router.key(
                    KeyEvent {
                        sym,
                        pressed: false,
                        mods: key.mods,
                        repeat: false,
                        shifted: None,
                        alt_gr: false,
                    },
                    &ctx,
                ) {
                    self.execute_plan(plan);
                }
            }
            KeyOutcome::Forward => self.forward_scripted_key(&key),
        }
        self.request_redraw();
    }

    /// Send a scripted key to the keyboard-focused client as a press and release of the key that types
    /// it in the seat keymap, with the chord's modifiers (and Shift for a shifted symbol) held around it.
    /// Scripted keys have no keycode of their own.
    fn forward_scripted_key(&mut self, key: &OverlayKey) {
        let target = key.text.map_or(key.sym, |c| {
            smithay::input::keyboard::xkb::utf32_to_keysym(c as u32).raw()
        });
        let Some((code, shifted)) = self.keycode_typing(target) else {
            tracing::warn!(sym = target, "scripted key has no key in the seat keymap");
            return;
        };
        // evdev codes of the modifier keys
        let modifiers: Vec<u32> = [
            (key.mods.shift || shifted, 42),
            (key.mods.ctrl, 29),
            (key.mods.alt, 56),
            (key.mods.logo, 125),
        ]
        .into_iter()
        .filter(|(held, _)| *held)
        .map(|(_, evdev)| evdev + 8)
        .collect();
        let keyboard = self.seat.get_keyboard().unwrap();
        let time = (self.now() * 1000.0) as u32;
        let steps = modifiers
            .iter()
            .map(|m| (*m, KeyState::Pressed))
            .chain([
                (code.raw(), KeyState::Pressed),
                (code.raw(), KeyState::Released),
            ])
            .chain(modifiers.iter().rev().map(|m| (*m, KeyState::Released)));
        for (raw, state) in steps {
            keyboard.input::<(), _>(
                self,
                smithay::input::keyboard::Keycode::new(raw),
                state,
                SERIAL_COUNTER.next_serial(),
                time,
                |_, _, _| FilterResult::Forward,
            );
        }
    }

    /// The key of the seat keymap that types `sym` (first layout), and whether it takes Shift.
    fn keycode_typing(&mut self, sym: u32) -> Option<(smithay::input::keyboard::Keycode, bool)> {
        let keyboard = self.seat.get_keyboard().unwrap();
        keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            // SAFETY: the design does not outlive the lock guard
            let keymap = unsafe { xkb.keymap() };
            let mut found = [None, None];
            keymap.key_for_each(|km, code| {
                for (level, slot) in found.iter_mut().enumerate() {
                    if slot.is_none()
                        && km
                            .key_get_syms_by_level(code, 0, level as u32)
                            .iter()
                            .any(|s| s.raw() == sym)
                    {
                        *slot = Some(code);
                    }
                }
            });
            // an unshifted key wins over a shifted one
            found[0].map(|c| (c, false)).or(found[1].map(|c| (c, true)))
        })
    }
}

mod pointer;
