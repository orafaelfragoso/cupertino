//! Input event processing for every backend: keyboard through the [`KeyRouter`], pointer routing
//! between the shell, the window model and clients, and touchpad gestures.

use cupertino_wm::{Command, Edges, Modifiers as WmModifiers, Point as MPoint, WindowId};
use smithay::backend::input::{
    AbsolutePositionEvent, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device,
    DeviceCapability, Event, GestureBeginEvent, GestureEndEvent, GesturePinchUpdateEvent,
    GestureSwipeUpdateEvent, InputBackend, InputEvent, KeyState, KeyboardKeyEvent,
    PointerAxisEvent, PointerButtonEvent, PointerMotionEvent,
};
use smithay::input::keyboard::FilterResult;
use smithay::input::pointer::{
    AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
    GesturePinchEndEvent, GesturePinchUpdateEvent as PinchUpdate, GestureSwipeBeginEvent,
    GestureSwipeEndEvent, GestureSwipeUpdateEvent as SwipeUpdate, MotionEvent, RelativeMotionEvent,
};

/// Which kind of gesture was forwarded to the client.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum GestureSource {
    #[default]
    Swipe,
    Pinch,
}
use smithay::reexports::wayland_server::protocol::wl_pointer;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::reexports::wayland_server::Resource;
use smithay::utils::{Logical, Point, SERIAL_COUNTER};

use cupertino_input::{
    Disposition, GestureEvent, GestureKind, GestureOutput, MouseAccel, Outcome as GestureOutcome,
    PointerAccel, ScrollAxisAccel, ScrollPhase,
};
use cupertino_protocols::server::cupertino_dock_v1::Gesture as DockGesture;

use crate::config::PointerSettings;

use crate::input::{ActionPlan, KeyEvent, KeyOutcome, Mods};
use crate::kbd_filters::RepeatTarget;
use crate::kbd_ui::AccentRoute;
use crate::overlays::OverlayKey;
use crate::pointer_constraints::Constrained;
use crate::state::{Cupertino, PointerMode, WindowTarget};
use cupertino_keyboard::rewrite::Rewrite;
use smithay::input::keyboard::{keysyms, Keycode};

/// Who takes one key event inside the keyboard filter.
#[derive(Clone, Debug)]
enum KeyRoute {
    /// The focused client.
    Forward,
    /// The focused client, released at once: a held letter with accent alternatives (see [`AccentRoute::Armed`]).
    AccentArmed(char),
    /// The input-source HUD (Escape, Return, the arrows).
    Hud,
    /// The accent popup; `repeats` for its navigation keys.
    Accent { repeats: bool },
    /// A compositor shortcut or a swallowed release.
    Plan(ActionPlan),
    /// Escape during a client's drag and drop: the drag ends without a drop.
    CancelDrag,
}

/// How a device's motion is accelerated.
#[derive(Clone, Debug)]
enum Accel {
    Pad(PointerAccel),
    Mouse(MouseAccel),
}

/// What tells a mouse's resolution: the key of the user's stored value and the hardware database's `MOUSE_DPI`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MouseIdentity {
    /// `mouse_dpi_key` of the device.
    pub key: Option<String>,
    /// The device name.
    pub name: String,
    /// The `MOUSE_DPI` property of the device node.
    pub hwdb_dpi: Option<String>,
}

/// Acceleration state of one relative pointing device.
#[derive(Clone, Debug)]
pub struct PointerDevice {
    touchpad: bool,
    identity: MouseIdentity,
    accel: Accel,
    last_usec: Option<u64>,
    /// Scroll accelerators (horizontal, vertical) of a device with a scroll resolution: only the
    /// touchpad. A wheel has none: the design gives a generic wheel, high-resolution or
    /// not, no scroll resolution and so no scroll acceleration.
    scroll: Option<[ScrollAxisAccel; 2]>,
    /// Scroll count of the touchpad's flick series.
    scroll_count: cupertino_input::ScrollCounter,
    /// A finger scroll gesture is under way: its next event is a `Changed`, not a `Began`.
    scrolling: bool,
    /// The device has moved, scrolled or clicked: a keyboard's or a receiver's unused pointer interface never does.
    used: bool,
}

impl PointerDevice {
    /// Touchpads use the touchpad curve family and report rate, taking the touchpad model's motion
    /// (`MM_PER_COUNT` per count). Mice use the default acceleration table on the speed of the hand
    /// in inches per second, from counts, the resolution of the device and the event timestamps. Touchpad
    /// finger scrolling is accelerated with the trackpad scroll curves.
    pub fn new(touchpad: bool, cfg: &PointerSettings) -> Self {
        Self::with_identity(touchpad, cfg, MouseIdentity::default())
    }

    /// Like [`PointerDevice::new`] for a mouse that `identity` tells its resolution of.
    pub fn with_identity(touchpad: bool, cfg: &PointerSettings, identity: MouseIdentity) -> Self {
        let (accel, scroll) = if touchpad {
            let pad = cupertino_input::TouchpadSettings {
                tracking_speed: cfg.trackpad_speed,
                acceleration: cfg.trackpad_acceleration,
                ..cupertino_input::TouchpadSettings::default()
            };
            (
                Accel::Pad(pad.pointer_accel()),
                pad.scroll_accel().map(|axis| [axis.clone(), axis]),
            )
        } else {
            let stored = identity
                .key
                .as_ref()
                .and_then(|key| cfg.mouse_dpi.get(key))
                .copied();
            let dpi = cupertino_input::resolve_dpi(stored, identity.hwdb_dpi.as_deref());
            let mouse = if cfg.mouse_acceleration {
                let curve = cupertino_input::TableCurve::new(
                    &cupertino_input::AccelTable::default_table(),
                    cfg.mouse_speed,
                );
                MouseAccel::new(curve.map(cupertino_input::AccelCurve::Table), dpi)
            } else {
                MouseAccel::linear(cfg.mouse_speed, dpi)
            };
            (Accel::Mouse(mouse), None)
        };
        Self {
            touchpad,
            identity,
            accel,
            last_usec: None,
            scroll,
            scroll_count: cupertino_input::ScrollCounter::default(),
            scrolling: false,
            used: false,
        }
    }

    /// Note that the device moved, scrolled or clicked; true the first time.
    pub fn note_use(&mut self) -> bool {
        !std::mem::replace(&mut self.used, true)
    }

    /// A mouse (not a touchpad) that has moved, scrolled or clicked.
    pub fn is_used_mouse(&self) -> bool {
        !self.touchpad && self.used
    }

    /// The `Mice` entry of this device, for a used mouse that has a key.
    pub fn mouse_entry(&self, cfg: &PointerSettings) -> Option<crate::quit::MouseEntry> {
        if self.touchpad || !self.used {
            return None;
        }
        let key = self.identity.key.clone()?;
        let stored = cfg.mouse_dpi.get(&key).copied();
        let dpi = cupertino_input::resolve_dpi(stored, self.identity.hwdb_dpi.as_deref());
        Some(crate::quit::MouseEntry {
            key,
            name: self.identity.name.clone(),
            dpi: dpi.round() as u32,
            stored: stored.is_some(),
        })
    }

    /// Take new tracking settings: the device starts over as [`PointerDevice::new`] made it.
    pub fn reconfigure(&mut self, cfg: &PointerSettings) {
        *self = Self {
            used: self.used,
            ..Self::with_identity(self.touchpad, cfg, self.identity.clone())
        };
    }

    /// Accelerated delta for unaccelerated `delta` at `usec`.
    pub fn accelerate(&mut self, delta: Point<f64, Logical>, usec: u64) -> Point<f64, Logical> {
        let dt_ms = self
            .last_usec
            .map_or(0.0, |last| usec.saturating_sub(last) as f64 / 1000.0);
        self.last_usec = Some(usec);
        match &mut self.accel {
            Accel::Pad(pad) => pad.accelerate(delta.x, delta.y, dt_ms).into(),
            Accel::Mouse(mouse) => mouse.accelerate(delta.x, delta.y, usec).into(),
        }
    }

    /// Points to scroll for `counts` raw scroll counts on `axis` at `usec`: the accelerator's
    /// lines, coalesced as the design's event translator does, at 10 points each;
    /// zero while the event is held back, `None` for a device without a scroll
    /// resolution. The coalescing already quantises to whole points.
    pub fn scroll_points(&mut self, axis: Axis, counts: f64, usec: u64) -> Option<f64> {
        let scroll = self.scroll.as_mut()?;
        let accel = match axis {
            Axis::Horizontal => &mut scroll[0],
            Axis::Vertical => &mut scroll[1],
        };
        Some(accel.scroll_lines(counts, usec as f64 / 1000.0) * cupertino_input::POINTS_PER_LINE)
    }

    /// The scroll count after a finger scroll event at `usec` that sends `points` (horizontal, vertical),
    /// or after the end of the gesture when `stop`: the coalescing's held-back events (no points) and
    /// the end of a gesture have nothing to count and give `None`, as does a device without a scroll
    /// resolution (a wheel has no gesture phases, so no count).
    pub fn scroll_count(&mut self, points: (f64, f64), stop: bool, usec: u64) -> Option<u32> {
        self.scroll.as_ref()?;
        let ms = usec as f64 / 1000.0;
        if stop {
            if std::mem::take(&mut self.scrolling) {
                self.scroll_count.update(ScrollPhase::Ended, 0.0, 0.0, ms);
            }
            return None;
        }
        if points == (0.0, 0.0) {
            return None;
        }
        let phase = if std::mem::replace(&mut self.scrolling, true) {
            ScrollPhase::Changed
        } else {
            ScrollPhase::Began
        };
        Some(self.scroll_count.update(phase, points.0, points.1, ms))
    }
}

/// A pointer axis event as the server reads it. `amount` is the backend's value (touchpad counts for finger
/// scrolling) and `v120` the wheel's high-resolution steps; the arrays hold the horizontal axis first.
#[derive(Clone, Debug)]
pub struct AxisInput {
    /// Identifier of the pointing device, the key of [`Cupertino::pointer_devices`].
    pub device: String,
    pub source: AxisSource,
    /// Event time in microseconds.
    pub usec: u64,
    pub amount: [Option<f64>; 2],
    pub v120: [Option<f64>; 2],
    pub relative_direction: [AxisRelativeDirection; 2],
}

impl AxisInput {
    /// The axes this event stops, horizontal first: a finger scroll stops an axis with an amount of exactly zero.
    pub fn stopped_axes(&self) -> [bool; 2] {
        self.amount
            .map(|a| self.source == AxisSource::Finger && a == Some(0.0))
    }

    /// Whether this event ends a touchpad gesture: a finger scroll where no axis moves and one is exactly zero.
    pub fn ends_gesture(&self) -> bool {
        self.source == AxisSource::Finger
            && self.amount.contains(&Some(0.0))
            && self.amount.iter().all(|a| a.is_none_or(|a| a == 0.0))
    }
}

/// `wl_pointer.axis` carries a fixed-point value in 1/256 steps. A value is sent rounded to that step and what the
/// rounding lost is added to the next value of the same axis, source and direction, so a run of small scrolls (a
/// high-resolution wheel's fractions of a notch) adds up to the distance the device moved. A stop, another source or
/// a reversal starts the axis over.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisCarry {
    carried: [f64; 2],
    /// Whether the last value of each axis was positive.
    positive: [bool; 2],
    source: Option<AxisSource>,
}

/// Steps of the protocol's fixed-point numbers per unit.
const FIXED_STEPS: f64 = 256.0;

impl AxisCarry {
    /// The value to send on axis `i` for `value` of `source`: `value` plus what earlier values lost, rounded to the
    /// fixed-point step.
    pub fn send(&mut self, i: usize, source: AxisSource, value: f64) -> f64 {
        if self.source != Some(source) {
            *self = AxisCarry {
                source: Some(source),
                ..AxisCarry::default()
            };
        }
        let positive = value > 0.0;
        let carried = &mut self.carried[i];
        if self.positive[i] != positive {
            self.positive[i] = positive;
            *carried = 0.0;
        }
        let total = value + *carried;
        let sent = (total * FIXED_STEPS).round() / FIXED_STEPS;
        *carried = total - sent;
        sent
    }

    /// The axis `i` stopped: nothing carries into the next gesture.
    pub fn stop(&mut self, i: usize) {
        self.carried[i] = 0.0;
    }
}

fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::Horizontal => 0,
        Axis::Vertical => 1,
    }
}

/// A press the server took for a window move or resize, which starts only once the pointer leaves
/// the drag-start box around the press (5 pt), so a click with a little jitter stays a
/// click. Held in [`Cupertino::pending_drag`] until the drag starts or the button is released.
#[derive(Clone, Debug)]
pub struct PendingDrag {
    button: u32,
    press: MPoint,
    /// The `BeginMove` or `BeginResize`, anchored at the press point.
    command: Command,
}

pub const BTN_LEFT: u32 = 0x110;
pub const BTN_RIGHT: u32 = 0x111;

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

    /// One key event through the overlays (input-source HUD, accent picker), the menus and
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
        // 1. the input-source HUD takes Escape, Return and the arrows
        if pressed
            && !modifier_key
            && !ctx.locked
            && !ctx.input_held
            && self.overlays.inputs.hud_open()
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
            return KeyRoute::Hud;
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
            KeyRoute::Hud => {
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
        let time = (self.now() * 1000.0) as u32;
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        // as a device would deliver it: through the filters, the shortcuts and the Super tap
        self.key_after_delays(
            smithay::input::keyboard::Keycode::new(evdev + 8),
            state,
            time,
        );
        Ok(())
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
            });
        }
    }

    fn scripted_overlay_key(&mut self, key: OverlayKey) {
        let ctx = self.action_context();
        let sym = key.sym;
        if !ctx.locked && self.overlays.inputs.hud_open() && self.input_sources_key(sym) {
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

    /// Where an absolute pointing device reports: the output under its pointer, and that output's mode turned as it
    /// is shown, in panel pixels. The device's range spans those pixels, so a position maps through the output's exact
    /// scale ([`Cupertino::output_pixels_to_global`]) and lands where drawing puts that pixel, up to the far edge.
    fn absolute_device_space(
        &self,
    ) -> Option<(cupertino_wm::OutputId, smithay::utils::Size<i32, Logical>)> {
        let id = self.pointer_output()?;
        let geometry = self.output_geometry(id)?;
        let turned = if geometry.rotated {
            geometry.mode.swapped()
        } else {
            geometry.mode
        };
        // the size type of `position_transformed`; its values here are panel pixels
        let pixels = smithay::utils::Size::<i32, Logical>::from((
            i32::try_from(turned.width).ok()?,
            i32::try_from(turned.height).ok()?,
        ));
        Some((id, pixels))
    }

    /// A mouse is listed in `Mice` from its first motion, scroll or click on.
    fn pointer_device_used(&mut self, id: &str) {
        if self
            .pointer_devices
            .get_mut(id)
            .is_some_and(|device| !device.touchpad && device.note_use())
        {
            self.publish_session_settings();
            self.sync_scroll_bars();
        }
    }

    fn on_pointer_motion<B: InputBackend>(&mut self, evt: B::PointerMotionEvent) {
        self.pointer_device_used(&evt.device().id());
        // Devices announced by the backend get the design curves; others keep the backend's delta.
        let delta = match self.pointer_devices.get_mut(&evt.device().id()) {
            Some(dev) => dev.accelerate(evt.delta_unaccel(), evt.time()),
            None => evt.delta(),
        };
        let pos = self.pointer.current_location() + delta;
        let rel = RelativeMotionEvent {
            delta,
            delta_unaccel: evt.delta_unaccel(),
            utime: evt.time(),
        };
        self.pointer_moved_to(pos, evt.time_msec(), Some(rel));
    }

    /// Feed a window move's motion to the top zone: true once the pointer, at its output's top edge, pushed up past it
    /// far enough to open the overview.
    fn window_drag_reached_top(
        &mut self,
        pos: Point<f64, Logical>,
        rel: Option<&RelativeMotionEvent>,
    ) -> bool {
        if !self.wm.is_moving() || self.shell.overview_open() {
            return false;
        }
        let (Some(rel), Some(top)) = (rel, self.output_at(pos).map(|o| o.rect.y)) else {
            return false;
        };
        self.top_zone.motion(pos.y - top, rel.delta.x, rel.delta.y)
    }

    /// Clamp `pos` to the union of the outputs.
    fn clamp_to_outputs(&self, pos: Point<f64, Logical>) -> Point<f64, Logical> {
        if self.outputs.is_empty() {
            return pos;
        }
        let (x0, y0) = (
            self.outputs
                .iter()
                .map(|o| o.rect.x)
                .fold(f64::MAX, f64::min),
            self.outputs
                .iter()
                .map(|o| o.rect.y)
                .fold(f64::MAX, f64::min),
        );
        let (x1, y1) = (
            self.outputs
                .iter()
                .map(|o| o.rect.right())
                .fold(f64::MIN, f64::max),
            self.outputs
                .iter()
                .map(|o| o.rect.bottom())
                .fold(f64::MIN, f64::max),
        );
        Point::from((pos.x.clamp(x0, x1 - 1.0), pos.y.clamp(y0, y1 - 1.0)))
    }

    pub fn pointer_moved_to(
        &mut self,
        pos: Point<f64, Logical>,
        time: u32,
        rel: Option<RelativeMotionEvent>,
    ) {
        crate::frame_stats::note_pointer_motion(self);
        self.input_came();
        let pos = match self.constrain_motion(pos) {
            Some(Constrained::Locked) => {
                // the pointer stays; the client gets the motion as relative motion only
                if let Some(rel) = rel {
                    let pointer = self.pointer.clone();
                    let under = self.surface_under(pointer.current_location());
                    pointer.relative_motion(self, under, &rel);
                    pointer.frame(self);
                }
                return;
            }
            Some(Constrained::Confined(to)) => to,
            None => pos,
        };
        let pos = self.clamp_to_outputs(pos);
        self.shake_motion(pos);
        let now = self.now();
        let mp = MPoint::new(pos.x, pos.y);
        let serial = SERIAL_COUNTER.next_serial();
        let pointer = self.pointer.clone();

        if self.session_lock.is_active() {
            let under = self.surface_under(pos);
            self.send_pointer_motion(under, pos, time, rel);
            return;
        }
        if self.pointer_mode == PointerMode::Normal {
            self.track_active_menu_bar(pos, now);
        }
        self.update_input_pointer(pos, now);
        if let Some(drag) = self.pending_drag.take_if(|d| {
            cupertino_input::drag_started(
                cupertino_input::Point::new(d.press.x, d.press.y),
                cupertino_input::Point::new(mp.x, mp.y),
            )
        }) {
            // anchored at the press, so the window catches up with the pointer at once
            self.begin_model_interaction(drag.command);
        }

        if self.pointer_mode == PointerMode::Model {
            let mods = self.current_mods();
            let m = WmModifiers {
                primary: mods.logo,
                option: mods.alt,
                shift: mods.shift,
                ctrl: mods.ctrl,
            };
            self.command(Command::PointerMotion {
                pointer: mp,
                modifiers: m,
            });
            if self.window_drag_reached_top(pos, rel.as_ref()) {
                // the window stays where it was taken and the overview opens on its output
                self.command(Command::PointerRelease);
                if let Some(output) = self.output_at(pos).map(|o| o.id) {
                    self.open_overview(output, cupertino_overview::OverviewMode::Overview);
                }
            }
            if !self.wm.is_interacting() {
                self.pointer_mode = PointerMode::Normal;
                self.top_zone.reset();
            }
            pointer.motion(
                self,
                None,
                &MotionEvent {
                    location: pos,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
            self.request_redraw();
            return;
        }

        // in the overview the windows are the dock's to hit-test; `surface_under` skips them
        let target = if self.shell.overview_open() {
            None
        } else {
            self.window_target_on_top(pos)
        };
        self.update_decor_hover(target.as_ref());
        let under = self.surface_under(pos);
        self.send_pointer_motion(under, pos, time, rel);
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        self.refresh_pointer_constraints(focus.as_ref());
    }

    /// Move the pointer to `pos`, focused on `under`.
    pub(crate) fn send_pointer_motion(
        &mut self,
        under: Option<(WlSurface, Point<f64, Logical>)>,
        pos: Point<f64, Logical>,
        time: u32,
        rel: Option<RelativeMotionEvent>,
    ) {
        let pointer = self.pointer.clone();
        pointer.motion(
            self,
            under.clone(),
            &MotionEvent {
                location: pos,
                serial: SERIAL_COUNTER.next_serial(),
                time,
            },
        );
        if let Some(rel) = rel {
            pointer.relative_motion(self, under, &rel);
        }
        pointer.frame(self);
        self.request_redraw();
    }

    fn on_pointer_button<B: InputBackend>(&mut self, evt: B::PointerButtonEvent) {
        self.pointer_device_used(&evt.device().id());
        self.pointer_button(evt.button_code(), evt.state(), evt.time_msec());
    }

    /// A pointer button event (also used by scripted input).
    ///
    /// A press the server consumes (overview, decorations, a model move or resize) never reaches a client, so its release
    /// is consumed too; every other press and release goes to `pointer.button`, whose implicit grab keeps a client's
    /// release with the client that got the press.
    pub fn pointer_button(&mut self, button: u32, state: ButtonState, time: u32) {
        crate::frame_stats::note_input(self);
        self.input_came();
        if !self.control_physical_transition(
            cupertino_control_model::Input::Button(button),
            state == ButtonState::Pressed,
        ) {
            return;
        }
        self.as_input(|s| s.handle_pointer_button(button, state, time));
        // a release that ended a drag without a drop sends its icon back
        self.drag_ended();
    }

    fn handle_pointer_button(&mut self, button: u32, state: ButtonState, time: u32) {
        self.key_filters.super_tap.disturb();
        let serial = SERIAL_COUNTER.next_serial();
        let pos = self.pointer.current_location();
        let pointer = self.pointer.clone();
        self.control_user_input(serial.into(), pointer.current_focus());
        let now = self.now();
        if state == ButtonState::Pressed {
            self.last_user_event = Some(now);
            let focus = pointer.current_focus();
            self.note_input(focus.as_ref());
        }
        let release_consumed =
            state == ButtonState::Released && self.consumed_buttons.contains(&button);
        if release_consumed {
            self.consumed_buttons.retain(|b| *b != button);
        }
        if state == ButtonState::Released
            && self
                .pending_drag
                .as_ref()
                .is_some_and(|d| d.button == button)
        {
            // released inside the drag-start box: the press was a click
            self.pending_drag = None;
        }

        if self.session_lock.is_active() {
            if release_consumed {
                return;
            }
            if state == ButtonState::Pressed {
                self.focus_lock_surface_at(pos);
            }
            pointer.button(
                self,
                &ButtonEvent {
                    button,
                    state,
                    serial,
                    time,
                },
            );
            pointer.frame(self);
            return;
        }
        if release_consumed {
            if self.pointer_mode == PointerMode::Model {
                self.end_model_interaction(pos, time);
            } else if self.decor.pressed.is_some() {
                self.decor_button_release(pos);
            } else if self.wm.switcher().is_some() {
                // a press off the switcher's panel: its release cancels the switch
                self.command(Command::SwitcherCancel);
            }
            return;
        }
        if state == ButtonState::Released && self.pointer_mode == PointerMode::Model {
            // a move or resize the client asked for after it got the press: the client still gets the release
            self.end_model_interaction(pos, time);
        }
        if state == ButtonState::Pressed && self.release_engaged_layer(pos, serial, time) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed && self.dismiss_popup_grab_for_press(pos, serial, time) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed && self.accent_pointer_press(pos) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed && self.press_consumed(pos, button, now) {
            self.consumed_buttons.push(button);
            return;
        }
        if state == ButtonState::Pressed {
            self.arm_by_press();
        }

        pointer.button(
            self,
            &ButtonEvent {
                button,
                state,
                serial,
                time,
            },
        );
        pointer.frame(self);
    }

    /// Hand the pointer back to the clients after a model move or resize.
    fn end_model_interaction(&mut self, pos: Point<f64, Logical>, time: u32) {
        self.command(Command::PointerRelease);
        self.pointer_mode = PointerMode::Normal;
        self.pointer_moved_to(pos, time, None);
    }

    /// Run the server's own handling of a press at `pos`. True when the press was consumed and must not reach a client.
    fn press_consumed(&mut self, pos: Point<f64, Logical>, button: u32, now: f64) -> bool {
        // the dock's panel takes the pointer while a switch is open; anything else is the server's
        // and cancels the switch on release
        if self.wm.switcher().is_some() {
            return !self
                .surface_under(pos)
                .is_some_and(|(surface, _)| self.is_switcher_panel(&surface));
        }
        if self.shell.overview_open() {
            // a shell surface (the dock's overview layer) takes the press; anything else is the
            // server's own pick, so the overview works without a dock
            return self.surface_under(pos).is_none() && self.overview_click(pos);
        }
        if button == BTN_LEFT && self.window_manager_press(pos) {
            return true;
        }
        if self.decoration_press(pos, button, now) {
            return true;
        }
        // the content of a window a modal sheet or dialog blocks takes no clicks: the press goes
        // to the model, which brings the blocking window forward (the title bar still moves it)
        if let Some(id) = self
            .window_surface_on_top(pos)
            .filter(|id| self.wm.is_blocked(*id) && !self.current_mods().logo)
        {
            self.command(Command::FocusWindow { window: id });
            return true;
        }
        self.focus_at(pos);
        self.pending_drag
            .as_ref()
            .is_some_and(|d| d.button == button)
    }

    /// Arm `command` (a move or resize anchored at `press`) to start when the pointer leaves the
    /// drag-start box.
    fn defer_drag(&mut self, button: u32, press: MPoint, command: Command) {
        self.pending_drag = Some(PendingDrag {
            button,
            press,
            command,
        });
    }

    /// Hover state of the decorations under the pointer: button group glyphs, resize cursors.
    fn update_decor_hover(&mut self, target: Option<&WindowTarget>) {
        use cupertino_decorations::Hit;
        let (group, cursor) = match target {
            Some(WindowTarget::Decoration {
                id,
                style,
                hit,
                content,
                local,
            }) => {
                // a client-decorated window has no button group of ours, but in full screen
                let server_side = self
                    .entry(*id)
                    .is_some_and(|e| e.server_side || e.fullscreen);
                let group = server_side
                    && (matches!(hit, Hit::Button(_))
                        || self.decor.group_hovered(*style, *content, *local));
                let cursor = match hit {
                    Hit::Resize(edge) => self.resize_cursor(*id, *edge),
                    _ => None,
                };
                (group.then_some(*id), cursor)
            }
            _ => (None, None),
        };
        if self.decor.hover_group != group || self.decor.cursor != cursor {
            self.decor.hover_group = group;
            self.decor.cursor = cursor;
            self.request_redraw();
        }
    }

    /// A press on the decorations of a window: buttons arm, the title bar moves (double click:
    /// the configured action), the resize zones resize; moves and resizes start past the drag
    /// threshold ([`PendingDrag`]). True when the press was consumed.
    fn decoration_press(&mut self, pos: Point<f64, Logical>, button: u32, now: f64) -> bool {
        use cupertino_decorations::{DoubleClickAction, Hit};
        let Some(WindowTarget::Decoration { id, hit, .. }) = self.window_target_on_top(pos) else {
            return false;
        };
        if button != BTN_LEFT {
            return true;
        }
        let mp = MPoint::new(pos.x, pos.y);
        self.command(Command::FocusWindow { window: id });
        self.return_keyboard_to_key_window(id);
        match hit {
            Hit::Button(kind) if self.decor_button_enabled(id, kind) => {
                self.decor.pressed = Some((id, kind));
                self.request_redraw();
            }
            // a disabled button on the title bar revealed over a full-screen window is as the rest of that bar: nothing
            Hit::Button(_) if self.entry(id).is_some_and(|e| e.fullscreen) => {}
            // a disabled button is no control: the press is the title bar's, which moves the window past the drag
            // threshold, and fires nothing
            Hit::Button(_) | Hit::Titlebar => {
                if self.decor.register_click(id, now) {
                    match self.decor.double_click_action(Hit::Titlebar) {
                        DoubleClickAction::Zoom => self.command(Command::ToggleZoom { window: id }),
                        DoubleClickAction::Minimize => {
                            let target = self.minimize_target(id);
                            self.command(Command::Minimize { window: id, target });
                        }
                        DoubleClickAction::None => {}
                    }
                } else {
                    self.defer_drag(
                        button,
                        mp,
                        Command::BeginMove {
                            window: id,
                            pointer: mp,
                        },
                    );
                }
            }
            Hit::Resize(edge) => {
                let (left, right, top, bottom) = edge.sides();
                let edges = Edges {
                    left,
                    right,
                    top,
                    bottom,
                };
                if self.decor.register_click(id, now) {
                    // double click: the edges snap to the screen, Alt adds the opposite ones
                    self.command(Command::SnapEdgesToScreen {
                        window: id,
                        edges,
                        opposite: self.current_mods().alt,
                    });
                } else {
                    self.defer_drag(
                        button,
                        mp,
                        Command::BeginResize {
                            window: id,
                            edges,
                            pointer: mp,
                        },
                    );
                }
            }
            Hit::Content | Hit::None => {}
        }
        true
    }

    /// Whether window control button `kind` of window `id` takes clicks ([`crate::decor::button_enabled`]).
    pub(crate) fn decor_button_enabled(
        &self,
        id: WindowId,
        kind: cupertino_decorations::ButtonKind,
    ) -> bool {
        // a full-screen window does not minimize: its revealed title bar draws minimize disabled
        let fullscreen = self.entry(id).is_some_and(|e| e.fullscreen);
        self.wm.window(id).is_some_and(|w| {
            crate::decor::button_enabled(w.can_minimize() && !fullscreen, w.can_zoom())
                [kind.index()]
        })
    }

    /// Button released after a press on a window control button: run the action when the pointer
    /// is still over the same button.
    fn decor_button_release(&mut self, pos: Point<f64, Logical>) {
        use cupertino_decorations::Hit;
        let Some((id, kind)) = self.decor.pressed.take() else {
            return;
        };
        self.request_redraw();
        let still = matches!(self.window_target_on_top(pos), Some(WindowTarget::Decoration { id: w, hit: Hit::Button(k), .. }) if w == id && k == kind);
        if still {
            self.press_window_button(id, kind);
        }
    }

    /// Do what window control button `kind` of window `id` is for, as a click on it does: an assistive technology's
    /// press comes here too ([`crate::decor_access`]). Nothing when the button is disabled.
    pub fn press_window_button(&mut self, id: WindowId, kind: cupertino_decorations::ButtonKind) {
        use cupertino_decorations::ButtonKind;
        if !self.decor_button_enabled(id, kind) {
            return;
        }
        match kind {
            ButtonKind::Close => self.command(Command::RequestClose { window: id }),
            ButtonKind::Minimize => {
                let target = self.minimize_target(id);
                self.command(Command::Minimize { window: id, target });
            }
            ButtonKind::Zoom => self.command(Command::ToggleZoom { window: id }),
        }
    }

    /// Gives the keyboard back to `window` when it is still the key window but a layer surface that takes the keyboard
    /// on demand (the desktop) holds it: the window manager sees no change of key window then and moves nothing.
    fn return_keyboard_to_key_window(&mut self, window: cupertino_wm::WindowId) {
        if self.exclusive_layer.is_some() || self.wm.key_window() != Some(window) {
            return;
        }
        let Some(surface) = self.entry(window).map(|e| e.surface().clone()) else {
            return;
        };
        let focus = self.seat.get_keyboard().and_then(|k| k.current_focus());
        if focus.as_ref() != Some(&surface) {
            self.set_keyboard_focus(Some(surface));
        }
    }

    /// The window whose own surface takes a press at `pos`: none when a surface drawn above the windows (a layer surface
    /// such as the dock, or a popup of one, such as its menu) lies there, so a press on it never focuses the window
    /// under it nor takes the keyboard from it.
    fn window_surface_on_top(&self, pos: Point<f64, Logical>) -> Option<WindowId> {
        match self.window_target_on_top(pos)? {
            WindowTarget::Surface { id, .. } => Some(id),
            WindowTarget::Decoration { .. } => None,
        }
    }

    /// Click-to-focus at `pos`. A modified click reaches the client unchanged: the design has no
    /// modifier drag that moves or resizes windows, so a primary-modifier click stays the client's.
    fn focus_at(&mut self, pos: Point<f64, Logical>) {
        if let Some(id) = self.window_surface_on_top(pos) {
            self.command(Command::FocusWindow { window: id });
            self.return_keyboard_to_key_window(id);
            return;
        }
        if let Some((surface, _)) = self.surface_under(pos) {
            // layer-shell surface that accepts keyboard input
            let wants = self.outputs.iter().any(|o| {
                let map = smithay::desktop::layer_map_for_output(&o.output);
                map.layer_for_surface(&surface, smithay::desktop::WindowSurfaceType::ALL)
                    .is_some_and(|l| l.can_receive_keyboard_focus())
            });
            if wants {
                self.set_keyboard_focus(Some(surface));
            }
        }
    }

    fn on_pointer_axis<B: InputBackend>(&mut self, evt: B::PointerAxisEvent) {
        self.pointer_device_used(&evt.device().id());
        let both =
            |read: &dyn Fn(Axis) -> Option<f64>| [read(Axis::Horizontal), read(Axis::Vertical)];
        let input = AxisInput {
            device: evt.device().id(),
            source: evt.source(),
            usec: evt.time(),
            amount: both(&|axis| evt.amount(axis)),
            v120: both(&|axis| evt.amount_v120(axis)),
            relative_direction: [
                evt.relative_direction(Axis::Horizontal),
                evt.relative_direction(Axis::Vertical),
            ],
        };
        self.pointer_axis(&input);
    }

    /// A pointer axis event (also used by scripted input): the scroll goes to the focused client as one
    /// `wl_pointer.frame`, with the scroll count of a touchpad scroll before the frame.
    pub fn pointer_axis(&mut self, input: &AxisInput) {
        crate::frame_stats::note_input(self);
        self.input_came();
        self.key_filters.super_tap.disturb();
        // A wheel notch (120 v120 units) scrolls three lines. Finger scrolling of a
        // device with a scroll resolution (the touchpad's counts) is accelerated; other sources,
        // and finger scrolling of other devices, keep the backend's value.
        let usec = input.usec;
        let mut device = self.pointer_devices.get_mut(&input.device);
        let mut amount = |axis: Axis| match (
            input.source,
            input.v120[axis_index(axis)],
            input.amount[axis_index(axis)],
        ) {
            (AxisSource::Wheel, Some(v120), _) => cupertino_input::wheel_points(v120),
            (AxisSource::Finger, _, Some(counts)) => device
                .as_deref_mut()
                .and_then(|d| d.scroll_points(axis, counts, usec))
                .unwrap_or(counts),
            (_, v120, amount) => {
                amount.unwrap_or_else(|| cupertino_input::wheel_points(v120.unwrap_or(0.0)))
            }
        };
        let (h, v) = (amount(Axis::Horizontal), amount(Axis::Vertical));
        let count = (input.source == AxisSource::Finger)
            .then(|| device.and_then(|d| d.scroll_count((h, v), input.ends_gesture(), usec)))
            .flatten();
        let stops = input.stopped_axes();
        let mut frame = AxisFrame::new((usec / 1000) as u32).source(input.source);
        let mut carries_axis = false;
        for (axis, value) in [(Axis::Horizontal, h), (Axis::Vertical, v)] {
            let i = axis_index(axis);
            if value != 0.0 {
                carries_axis = true;
                let value = self.axis_carry.send(i, input.source, value);
                frame = frame
                    .relative_direction(axis, input.relative_direction[i])
                    .value(axis, value);
                if let Some(d) = input.v120[i] {
                    frame = frame.v120(axis, d as i32);
                }
            }
            if stops[i] {
                carries_axis = true;
                self.axis_carry.stop(i);
                frame = frame.stop(axis);
            }
        }
        // An event the scroll coalescing holds back has nothing to tell the client yet.
        if !carries_axis {
            return;
        }
        let pointer = self.pointer.clone();
        pointer.axis(self, frame);
        // The count goes to the `cupertino_scroll_v1` objects of the focused client, for each axis that moved.
        if let (Some(count), Some(client)) =
            (count, pointer.current_focus().and_then(|s| s.client()))
        {
            for (value, wl_axis) in [
                (h, wl_pointer::Axis::HorizontalScroll),
                (v, wl_pointer::Axis::VerticalScroll),
            ] {
                if value != 0.0 {
                    self.scroll_counts.send(&client.id(), wl_axis, count);
                }
            }
        }
        pointer.frame(self);
    }

    // ---- gestures ----------------------------------------------------------------------------

    fn pointer_output(&self) -> Option<cupertino_wm::OutputId> {
        self.output_at(self.pointer.current_location())
            .map(|o| o.id)
    }

    /// Feed the recognizer and act on its outputs. Events stay with the focused client until the
    /// recognizer claims the gesture; then the client's gesture is cancelled.
    pub fn gesture(&mut self, ev: GestureEvent) {
        if self.session_lock.is_active() {
            return;
        }
        for out in self.gestures.handle(ev) {
            self.gesture_output_event(out);
        }
    }

    fn gesture_output_event(&mut self, out: GestureOutput) {
        match out {
            GestureOutput::Begin { kind, .. } => {
                self.cancel_forwarded_gesture();
                if kind == GestureKind::WorkspaceSwitch {
                    self.gesture_output = self.pointer_output();
                    if let Some(output) = self.gesture_output {
                        self.command(Command::WorkspaceGestureBegin { output });
                    }
                }
            }
            GestureOutput::Update {
                kind: GestureKind::WorkspaceSwitch,
                progress,
                ..
            } => {
                if let Some(output) = self.gesture_output {
                    // progress is positive towards the next workspace; the model's fraction is the
                    // displacement of the content (positive = to the right)
                    self.command(Command::WorkspaceGestureUpdate {
                        output,
                        fraction: -progress,
                    });
                }
            }
            GestureOutput::End { kind, outcome, .. } => {
                let commit = outcome == GestureOutcome::Commit;
                match kind {
                    GestureKind::WorkspaceSwitch => {
                        if let Some(output) = self.gesture_output.take() {
                            self.command(Command::WorkspaceGestureEnd { output });
                        }
                    }
                    // the dock of the output maps the gesture to its action; without a dock the
                    // server still leaves an open overview
                    GestureKind::Overview
                    | GestureKind::AppOverview
                    | GestureKind::AppGrid
                    | GestureKind::ShowDesktop
                        if commit =>
                    {
                        let gesture = match kind {
                            GestureKind::Overview => DockGesture::SwipeUp,
                            GestureKind::AppOverview => DockGesture::SwipeDown,
                            GestureKind::AppGrid => DockGesture::Pinch,
                            _ => DockGesture::Spread,
                        };
                        let sent = self
                            .pointer_output()
                            .is_some_and(|output| self.docks.gesture(output, gesture));
                        if !sent && self.shell.overview_open() {
                            self.close_overview();
                        }
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    /// Terminate a gesture the client already saw once the compositor takes it over.
    fn cancel_forwarded_gesture(&mut self) {
        if !std::mem::take(&mut self.gesture_forwarded) {
            return;
        }
        let pointer = self.pointer.clone();
        let time = (self.now() * 1000.0) as u32;
        let serial = SERIAL_COUNTER.next_serial();
        match self.gesture_source {
            GestureSource::Swipe => pointer.gesture_swipe_end(
                self,
                &GestureSwipeEndEvent {
                    serial,
                    time,
                    cancelled: true,
                },
            ),
            GestureSource::Pinch => pointer.gesture_pinch_end(
                self,
                &GesturePinchEndEvent {
                    serial,
                    time,
                    cancelled: true,
                },
            ),
        }
    }

    fn claimed(&self) -> bool {
        self.gestures.disposition() == Disposition::Claimed
    }

    fn on_swipe_begin<B: InputBackend>(&mut self, evt: B::GestureSwipeBeginEvent) {
        let (time, fingers) = (evt.time_msec(), evt.fingers());
        self.gesture(GestureEvent::SwipeBegin {
            time: f64::from(time) / 1000.0,
            fingers,
        });
        if !self.claimed() {
            self.gesture_forwarded = true;
            self.gesture_source = GestureSource::Swipe;
            let pointer = self.pointer.clone();
            pointer.gesture_swipe_begin(
                self,
                &GestureSwipeBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    fingers,
                },
            );
        }
    }

    fn on_swipe_update<B: InputBackend>(&mut self, evt: B::GestureSwipeUpdateEvent) {
        let (time, d) = (evt.time_msec(), evt.delta());
        self.gesture(GestureEvent::SwipeUpdate {
            time: f64::from(time) / 1000.0,
            dx: d.x,
            dy: d.y,
        });
        if self.gesture_forwarded && !self.claimed() {
            let pointer = self.pointer.clone();
            pointer.gesture_swipe_update(self, &SwipeUpdate { time, delta: d });
        }
    }

    fn on_swipe_end<B: InputBackend>(&mut self, evt: B::GestureSwipeEndEvent) {
        let (time, cancelled) = (evt.time_msec(), evt.cancelled());
        let forwarded = self.gesture_forwarded && !self.claimed();
        self.gesture(GestureEvent::SwipeEnd {
            time: f64::from(time) / 1000.0,
            cancelled,
        });
        if forwarded {
            self.gesture_forwarded = false;
            let pointer = self.pointer.clone();
            pointer.gesture_swipe_end(
                self,
                &GestureSwipeEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    cancelled,
                },
            );
        }
        self.gesture_forwarded = false;
    }

    fn on_pinch_begin<B: InputBackend>(&mut self, evt: B::GesturePinchBeginEvent) {
        let (time, fingers) = (evt.time_msec(), evt.fingers());
        self.gesture(GestureEvent::PinchBegin {
            time: f64::from(time) / 1000.0,
            fingers,
        });
        if !self.claimed() {
            self.gesture_forwarded = true;
            self.gesture_source = GestureSource::Pinch;
            let pointer = self.pointer.clone();
            pointer.gesture_pinch_begin(
                self,
                &GesturePinchBeginEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    fingers,
                },
            );
        }
    }

    fn on_pinch_update<B: InputBackend>(&mut self, evt: B::GesturePinchUpdateEvent) {
        let (time, d, scale, rotation) =
            (evt.time_msec(), evt.delta(), evt.scale(), evt.rotation());
        self.gesture(GestureEvent::PinchUpdate {
            time: f64::from(time) / 1000.0,
            dx: d.x,
            dy: d.y,
            scale,
            rotation,
        });
        if self.gesture_forwarded && !self.claimed() {
            let pointer = self.pointer.clone();
            pointer.gesture_pinch_update(
                self,
                &PinchUpdate {
                    time,
                    delta: d,
                    scale,
                    rotation,
                },
            );
        }
    }

    fn on_pinch_end<B: InputBackend>(&mut self, evt: B::GesturePinchEndEvent) {
        let (time, cancelled) = (evt.time_msec(), evt.cancelled());
        let forwarded = self.gesture_forwarded && !self.claimed();
        self.gesture(GestureEvent::PinchEnd {
            time: f64::from(time) / 1000.0,
            cancelled,
        });
        if forwarded {
            let pointer = self.pointer.clone();
            pointer.gesture_pinch_end(
                self,
                &GesturePinchEndEvent {
                    serial: SERIAL_COUNTER.next_serial(),
                    time,
                    cancelled,
                },
            );
        }
        self.gesture_forwarded = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pointer_device_is_listed_as_a_mouse_from_its_first_use_on() {
        let cfg = PointerSettings::default();
        let mut device = PointerDevice::with_identity(
            false,
            &cfg,
            MouseIdentity {
                key: Some("1a2c:0001:Keyboard".into()),
                name: "Keyboard".into(),
                hwdb_dpi: None,
            },
        );
        assert_eq!(device.mouse_entry(&cfg), None);
        assert!(device.note_use());
        assert!(!device.note_use());
        device.reconfigure(&cfg);
        assert_eq!(
            device.mouse_entry(&cfg),
            Some(crate::quit::MouseEntry {
                key: "1a2c:0001:Keyboard".into(),
                name: "Keyboard".into(),
                dpi: 1000,
                stored: false,
            })
        );
    }

    #[test]
    fn pointer_devices_follow_the_settings() {
        let cfg = PointerSettings::default();
        // 60 counts in one nominal period on a typical 1000 dpi mouse, default table at the default speed: gain
        // 1.30798… px per count (design value)
        let at_1000 = MouseIdentity {
            key: Some("046d:c077:Mouse".into()),
            name: "Mouse".into(),
            hwdb_dpi: Some("*1000".into()),
        };
        let mut mouse = PointerDevice::with_identity(false, &cfg, at_1000.clone());
        assert!(
            (mouse.accelerate((60.0, 0.0).into(), 1_000).x - 60.0 * 1.3079891292521268).abs()
                < 1e-9
        );
        // a stored resolution wins over the database's: 4000 dpi moves a quarter of the counts' inches
        let stored = PointerSettings {
            mouse_dpi: [("046d:c077:Mouse".to_string(), 4000)].into(),
            ..cfg.clone()
        };
        let mut same_hand = PointerDevice::with_identity(false, &stored, at_1000);
        let at_4000 = same_hand.accelerate((240.0, 0.0).into(), 1_000).x;
        assert!(
            (at_4000 - 60.0 * 1.3079891292521268).abs() < 1e-9,
            "{at_4000}"
        );
        // acceleration off: a constant gain per count of a 1000 dpi mouse
        let flat = PointerSettings {
            mouse_acceleration: false,
            mouse_speed: 2.0,
            ..cfg.clone()
        };
        let mut flat_1000 = PointerDevice::with_identity(
            false,
            &flat,
            MouseIdentity {
                key: None,
                name: String::new(),
                hwdb_dpi: Some("1000".into()),
            },
        );
        assert_eq!(
            flat_1000.accelerate((5.0, -3.0).into(), 0),
            (10.0, -6.0).into()
        );
        // Touchpads time their events: four report periods late is a quarter of the speed.
        let mut pad = PointerDevice::new(true, &cfg);
        let first = pad.accelerate((40.0, 0.0).into(), 0).x;
        let late = pad.accelerate((40.0, 0.0).into(), 33_333).x;
        assert!(late < first, "{late} < {first}");
    }

    /// Each profile is applied once, on device counts: the adaptive one grows with speed, the flat one is one gain at
    /// every speed, and neither depends on the cursor's size, colours or shake.
    #[test]
    fn a_touchpad_has_an_adaptive_and_a_flat_profile_independent_of_the_cursor() {
        let ratio = |cfg: &PointerSettings, counts: f64| {
            let mut pad = PointerDevice::new(true, cfg);
            pad.accelerate((counts, 0.0).into(), 0).x / counts
        };
        let adaptive = PointerSettings::default();
        assert!(ratio(&adaptive, 4.0) < ratio(&adaptive, 80.0));
        let flat = PointerSettings {
            trackpad_acceleration: false,
            trackpad_speed: 1.5,
            ..adaptive.clone()
        };
        let (slow, fast) = (ratio(&flat, 4.0), ratio(&flat, 80.0));
        assert!((slow - fast).abs() < 1e-12, "{slow} {fast}");
        assert!(
            (slow - 1.5).abs() < 1e-12,
            "the tracking speed is the gain: {slow}"
        );
        let big_cursor = PointerSettings {
            cursor_scale: 4.0,
            cursor_colors: crate::cursor::CursorColors {
                fill: [255, 0, 0, 255],
                outline: [0, 0, 255, 255],
            },
            shake_to_locate: false,
            ..adaptive.clone()
        };
        for counts in [4.0, 80.0] {
            assert_eq!(ratio(&big_cursor, counts), ratio(&adaptive, counts));
        }
        let mouse = |cfg: &PointerSettings| {
            PointerDevice::new(false, cfg)
                .accelerate((60.0, 0.0).into(), 1_000)
                .x
        };
        assert_eq!(mouse(&big_cursor), mouse(&adaptive));
    }

    #[test]
    fn touchpad_finger_scrolling_is_accelerated_and_wheels_are_not() {
        let cfg = PointerSettings::default();
        // 30 counts every 8 ms on the trackpad scroll curves at the fresh 0.3125: lines from
        // `ScrollAxis` of the design tool; every event is above the consume unit (8 counts), so
        // each is emitted at once, truncated to whole points (0.1 line), what it truncated going out with the next
        let lines: [f64; 5] = [
            0.6697329504898516,
            1.7655862070282453,
            2.270026594958617,
            2.548338533127098,
            2.7236750541732406,
        ];
        let mut pad = PointerDevice::new(true, &cfg);
        let mut sum = 0.0;
        for (i, want) in lines.iter().enumerate() {
            let got = pad
                .scroll_points(Axis::Vertical, 30.0, i as u64 * 8_000)
                .unwrap();
            let before = (sum * 10.0_f64).trunc();
            sum += want;
            assert!(
                (got - ((sum * 10.0).trunc() - before)).abs() < 1e-9,
                "event {i}: {got}"
            );
        }
        // the axes keep separate histories
        assert!(
            (pad.scroll_points(Axis::Horizontal, 30.0, 40_000).unwrap()
                - (lines[0] * 10.0).trunc())
            .abs()
                < 1e-9
        );
        // a mouse wheel, high-resolution or not, has no scroll resolution
        assert_eq!(
            PointerDevice::new(false, &cfg).scroll_points(Axis::Vertical, 30.0, 0),
            None
        );
    }

    #[test]
    fn axis_values_carry_what_the_fixed_point_rounding_lost() {
        // a high-resolution wheel turned one notch in 120 steps of 1/120: 0.4 points each
        let mut carry = AxisCarry::default();
        let step = cupertino_input::wheel_points(1.0);
        let sent: Vec<f64> = (0..120)
            .map(|_| carry.send(1, AxisSource::Wheel, step))
            .collect();
        assert!(sent.iter().all(|v| (v * 256.0).fract() == 0.0), "{sent:?}");
        let total: f64 = sent.iter().sum();
        assert!(
            (total - cupertino_input::wheel_points(120.0)).abs() <= 0.5 / 256.0,
            "{total}"
        );
        // without the carry every step would lose the same fraction
        let rounded = (step * 256.0).round() / 256.0 * 120.0;
        assert!((rounded - total).abs() > 0.1, "{rounded} {total}");
        // a reversal, another source and a stop start the axis over
        carry.send(1, AxisSource::Wheel, 0.001);
        assert_eq!(carry.send(1, AxisSource::Wheel, -0.0005), 0.0);
        assert_eq!(carry.carried[1], -0.0005);
        assert_eq!(carry.send(1, AxisSource::Wheel, -0.0005), 0.0);
        assert_eq!(carry.carried[1], -0.001, "the same direction carries on");
        assert_eq!(carry.send(1, AxisSource::Finger, 0.001), 0.0);
        assert_eq!(carry.carried[1], 0.001);
        carry.stop(1);
        assert_eq!(carry.carried, [0.0, 0.0]);
    }

    #[test]
    fn touchpad_scroll_events_below_the_consume_unit_are_coalesced() {
        // 2 counts per event: nothing is emitted until the raw sum reaches 24 (3 · 8),
        // then one value carries the accumulated lines.
        let mut pad = PointerDevice::new(true, &PointerSettings::default());
        let out: Vec<f64> = (0..12)
            .map(|i| pad.scroll_points(Axis::Vertical, 2.0, i * 8_000).unwrap())
            .collect();
        assert_eq!(out[..11], [0.0; 11]);
        assert!(out[11] >= 1.0 && out[11].fract() == 0.0, "{}", out[11]);
    }
}
