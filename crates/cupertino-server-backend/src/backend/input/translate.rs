use super::*;

struct AbsRange {
    x: (i32, i32),
    y: (i32, i32),
}

impl AbsRange {
    fn position(&self, point: AbsPoint) -> (f64, f64) {
        let fraction =
            |value: i32, (min, max): (i32, i32)| f64::from(value - min) / f64::from(max - min + 1);
        (fraction(point.x, self.x), fraction(point.y, self.y))
    }
}

/// One touchpad axis: where it starts and how many device units make a millimetre.
struct PadAxis {
    min: i32,
    units_per_mm: f64,
}

impl PadAxis {
    /// The axis `code` and its length in millimetres. An axis without a resolution gets
    /// `fallback_mm` as its length, and the log says so.
    fn new(info: &DeviceInfo, code: u16, fallback_mm: f64) -> Result<(Self, f64)> {
        let axis = info
            .abs(code)
            .with_context(|| format!("{} has no absolute axis {code:#x}", info.name))?;
        let range = f64::from(axis.max - axis.min);
        match axis.resolution {
            Resolution::UnitsPerMm(per_mm) => {
                let units_per_mm = f64::from(per_mm.get());
                Ok((
                    Self {
                        min: axis.min,
                        units_per_mm,
                    },
                    range / units_per_mm,
                ))
            }
            Resolution::Unknown => {
                tracing::warn!(device = %info.name, axis = code, size_mm = fallback_mm, "touchpad axis reports no resolution, assuming the libinput fallback size");
                Ok((
                    Self {
                        min: axis.min,
                        units_per_mm: range / fallback_mm,
                    },
                    fallback_mm,
                ))
            }
        }
    }

    fn mm(&self, value: i32) -> f64 {
        f64::from(value - self.min) / self.units_per_mm
    }
}

/// The touchpad model of one device with the conversion from device units to its input.
struct Pad {
    model: Touchpad,
    x: PadAxis,
    y: PadAxis,
    /// Range of `ABS_MT_PRESSURE`, when the device has a usable one.
    pressure: Option<(i32, i32)>,
    /// The device reports contacts in multitouch slots; otherwise it is a single-touch pad whose
    /// one position (`ABS_X`/`ABS_Y`) stands for every finger down.
    multitouch: bool,
    /// Contacts the device can report at once.
    slots: usize,
}

impl Pad {
    /// `Err` when the device has no position axes at all.
    fn new(info: &DeviceInfo, clickpad: bool, settings: TouchpadSettings) -> Result<Self> {
        let multitouch =
            info.abs(ABS_MT_POSITION_X).is_some() && info.abs(ABS_MT_POSITION_Y).is_some();
        let (x_code, y_code) = if multitouch {
            (ABS_MT_POSITION_X, ABS_MT_POSITION_Y)
        } else {
            (ABS_X, ABS_Y)
        };
        let (x, width_mm) = PadAxis::new(info, x_code, FALLBACK_PAD_SIZE_MM.0)?;
        let (y, height_mm) = PadAxis::new(info, y_code, FALLBACK_PAD_SIZE_MM.1)?;
        let pressure = info
            .abs(ABS_MT_PRESSURE)
            .filter(|axis| axis.max > axis.min)
            .map(|axis| (axis.min, axis.max));
        let slots = match info.abs(ABS_MT_SLOT) {
            Some(axis) if multitouch => {
                usize::try_from(axis.max + 1).expect("the framer rejects an empty slot range")
            }
            _ => 1,
        };
        Ok(Self {
            model: Touchpad::new(
                TouchpadGeometry {
                    width_mm,
                    height_mm,
                    clickpad,
                },
                settings,
            ),
            x,
            y,
            pressure,
            multitouch,
            slots,
        })
    }

    /// The model's view of a contact; pens and unknown tools are not fingers.
    fn contact(&self, contact: &Contact) -> Option<TouchContact> {
        let palm = match contact.tool {
            Tool::Finger => false,
            Tool::Palm => true,
            Tool::Pen | Tool::Other(_) => return None,
        };
        let pressure = contact
            .pressure
            .zip(self.pressure)
            .map(|(value, (min, max))| {
                (f64::from(value - min) / f64::from(max - min)).clamp(0.0, 1.0)
            });
        Some(TouchContact {
            id: contact.tracking_id,
            position_mm: (self.x.mm(contact.x), self.y.mm(contact.y)),
            pressure,
            palm,
        })
    }

    /// The fingers down. A single-touch pad reports one position and says how many fingers
    /// (`BTN_TOOL_DOUBLETAP` and up) make it; a multitouch pad with fewer slots than fingers does
    /// the same for the fingers it cannot track. The missing fingers are "fake fingers", as
    /// libinput calls them: each sits where the top-most real finger is.
    fn contacts(&self, frame: &Frame) -> Vec<TouchContact> {
        let fingers = usize::from(frame.finger_count.unwrap_or(0));
        let mut contacts: Vec<TouchContact> = if self.multitouch {
            frame
                .contacts
                .iter()
                .filter_map(|c| self.contact(c))
                .collect()
        } else {
            let single = |point: AbsPoint| TouchContact {
                id: 0,
                position_mm: (self.x.mm(point.x), self.y.mm(point.y)),
                pressure: None,
                palm: false,
            };
            frame
                .abs_pointer
                .filter(|_| fingers > 0)
                .map(single)
                .into_iter()
                .collect()
        };
        if fingers > self.slots {
            let top = contacts
                .iter()
                .filter(|c| !c.palm)
                .min_by(|a, b| a.position_mm.1.total_cmp(&b.position_mm.1))
                .copied();
            if let Some(top) = top {
                for index in contacts.len()..fingers {
                    contacts.push(TouchContact {
                        id: FAKE_CONTACT_ID_BASE + index as u32,
                        ..top
                    });
                }
            }
        }
        contacts
    }

    fn frame(&self, frame: &Frame) -> TouchpadFrame {
        TouchpadFrame {
            time_s: frame.time_us as f64 / 1e6,
            contacts: self.contacts(frame),
            // On a clickpad the left button is the physical click.
            button_down: frame.held_buttons.contains(&BTN_LEFT),
        }
    }
}

/// Timestamps come from `CLOCK_MONOTONIC`, which is never negative.
fn micros(time_us: i64) -> u64 {
    u64::try_from(time_us).expect("evdev timestamps are not negative")
}

fn micros_from_seconds(seconds: f64) -> u64 {
    (seconds * 1e6) as u64
}

/// Turns the frames of one device into input events.
struct Translator {
    device: InputDevice,
    natural_scrolling: bool,
    /// Keys and buttons reported down and not yet up, to release them when the device goes away.
    held: BTreeSet<u16>,
    abs: Option<AbsRange>,
    last_abs: Option<AbsPoint>,
    pad: Option<Pad>,
    /// Axis and last progress of the running three- or four-finger swipe.
    swipe: Option<(SwipeAxis, f64)>,
    /// A finger scroll has begun and not ended.
    scrolling: bool,
    /// A pinch has begun and not ended.
    pinching: bool,
    /// Rotation reported so far in the running pinch, degrees.
    rotation_deg: f64,
}

impl Translator {
    /// Applies the device's natural scrolling setting (a touchpad's or a wheel's) from the next frame on.
    fn set_natural_scrolling(&mut self, natural_scrolling: bool) {
        self.natural_scrolling = natural_scrolling;
        if let Some(pad) = &mut self.pad {
            pad.model.settings.natural_scrolling = natural_scrolling;
        }
    }

    /// A touchpad that cannot be modelled (no position axes) still gets its buttons: it is driven
    /// as a plain pointer, and the log says why.
    fn new(device: InputDevice, info: &DeviceInfo, natural_scrolling: bool) -> Self {
        let class = device.0.class;
        let pad = class.touchpad.then(|| Pad::new(info, class.clickpad, TouchpadSettings { tap_to_click: true, natural_scrolling, ..TouchpadSettings::default() })).and_then(|pad| {
            pad.map_err(|e| tracing::warn!(device = %info.name, error = %format!("{e:#}"), "touchpad cannot be modelled, using its buttons only")).ok()
        });
        let abs = match (info.abs(ABS_X), info.abs(ABS_Y)) {
            (Some(x), Some(y)) if class.mouse => Some(AbsRange {
                x: (x.min, x.max),
                y: (y.min, y.max),
            }),
            _ => None,
        };
        Self {
            device,
            natural_scrolling,
            held: BTreeSet::new(),
            abs,
            last_abs: None,
            pad,
            swipe: None,
            scrolling: false,
            pinching: false,
            rotation_deg: 0.0,
        }
    }

    fn stamp(&self, time_us: u64) -> Stamp {
        Stamp {
            device: self.device.clone(),
            time_us,
        }
    }

    fn frame(&mut self, frame: &Frame, out: &mut Vec<Event_>) {
        let time_us = micros(frame.time_us);
        for key in &frame.keys {
            self.key(*key, frame.resynced, time_us, out);
        }
        if is_pointer(&self.device.0.class) && self.pad.is_none() {
            self.pointer(frame, time_us, out);
        }
        for switch in &frame.switches {
            self.switch(*switch, time_us, out);
        }
        let events = self
            .pad
            .as_mut()
            .map(|pad| pad.model.process_frame(&pad.frame(frame)));
        for event in events.into_iter().flatten() {
            self.pad_event(event, time_us, out);
        }
    }

    /// Runs the touchpad timers (a held-back tap, a drag release) due at `now_s`.
    fn tick(&mut self, now_s: f64, out: &mut Vec<Event_>) {
        let events = self.pad.as_mut().map(|pad| pad.model.tick(now_s));
        for event in events.into_iter().flatten() {
            self.pad_event(event, micros_from_seconds(now_s), out);
        }
    }

    /// When the touchpad model next needs [`Translator::tick`], in seconds of the frame clock.
    fn next_deadline(&self) -> Option<f64> {
        self.pad.as_ref().and_then(|pad| pad.model.next_deadline())
    }

    /// Cancels the running gesture and releases everything still held, before the device goes away.
    fn release_all(&mut self, time_us: u64, out: &mut Vec<Event_>) {
        let stamp = self.stamp(time_us);
        if self.swipe.take().is_some() {
            out.push(InputEvent::GestureSwipeEnd {
                event: GestureEnd {
                    stamp: stamp.clone(),
                    cancelled: true,
                },
            });
        }
        if std::mem::take(&mut self.pinching) {
            out.push(InputEvent::GesturePinchEnd {
                event: GestureEnd {
                    stamp: stamp.clone(),
                    cancelled: true,
                },
            });
        }
        if std::mem::take(&mut self.scrolling) {
            out.push(self.scroll_stop(stamp));
        }
        for code in std::mem::take(&mut self.held) {
            if is_key_code(code) {
                self.push_key(code, false, false, time_us, out);
            } else {
                self.push_button(code, false, time_us, out);
            }
        }
    }

    fn key(&mut self, key: EvdevKey, resynced: bool, time_us: u64, out: &mut Vec<Event_>) {
        let pressed = match key.state {
            EvdevKeyState::Pressed => true,
            EvdevKeyState::Released => false,
            // clients repeat keys themselves
            EvdevKeyState::Repeat => return,
        };
        let class = self.device.0.class;
        if is_key_code(key.code) {
            if class.key {
                self.set_held(key.code, pressed);
                self.push_key(key.code, pressed, resynced, time_us, out);
            }
        } else if (BTN_LEFT..=BTN_TASK).contains(&key.code) {
            // The touchpad model turns the physical click into button events.
            if is_pointer(&class) && !(self.pad.is_some() && key.code == BTN_LEFT) {
                self.set_held(key.code, pressed);
                self.push_button(key.code, pressed, time_us, out);
            }
        }
        // The remaining codes are touch, tool and stylus buttons: nothing consumes them.
    }

    fn set_held(&mut self, code: u16, pressed: bool) {
        if pressed {
            self.held.insert(code);
        } else {
            self.held.remove(&code);
        }
    }

    fn push_key(
        &self,
        code: u16,
        pressed: bool,
        resynced: bool,
        time_us: u64,
        out: &mut Vec<Event_>,
    ) {
        let count = self.held.iter().filter(|c| is_key_code(**c)).count() as u32;
        let state = if pressed {
            KeyState::Pressed
        } else {
            KeyState::Released
        };
        out.push(InputEvent::Keyboard {
            event: KeyEvent {
                stamp: self.stamp(time_us),
                code: u32::from(code),
                state,
                count,
                resynced,
            },
        });
    }

    fn push_button(&self, code: u16, pressed: bool, time_us: u64, out: &mut Vec<Event_>) {
        let state = if pressed {
            ButtonState::Pressed
        } else {
            ButtonState::Released
        };
        out.push(InputEvent::PointerButton {
            event: ButtonEvent {
                stamp: self.stamp(time_us),
                code: u32::from(code),
                state,
            },
        });
    }

    /// Relative motion, wheels and an absolute position of a mouse.
    fn pointer(&mut self, frame: &Frame, time_us: u64, out: &mut Vec<Event_>) {
        if frame.motion.dx != 0 || frame.motion.dy != 0 {
            let delta = (f64::from(frame.motion.dx), f64::from(frame.motion.dy));
            out.push(InputEvent::PointerMotion {
                event: MotionEvent {
                    stamp: self.stamp(time_us),
                    delta,
                },
            });
        }
        // The kernel counts a wheel turned away from the user as positive; the axis points the
        // other way. Natural scrolling (the mouse's setting) flips both wheels.
        let sign = if self.natural_scrolling { -1.0 } else { 1.0 };
        let v120 = Axes::moved(
            f64::from(frame.wheel.horizontal) * sign,
            -f64::from(frame.wheel.vertical) * sign,
        );
        if v120 != Axes::default() {
            let axis = AxisEvent {
                stamp: self.stamp(time_us),
                source: AxisSource::Wheel,
                amount: Axes::default(),
                v120,
                direction: self.direction(),
            };
            out.push(InputEvent::PointerAxis { event: axis });
        }
        if let (Some(range), Some(point)) = (&self.abs, frame.abs_pointer) {
            // The first frame of a device only reports where it is.
            if !frame.resynced && self.last_abs != Some(point) {
                out.push(InputEvent::PointerMotionAbsolute {
                    event: AbsoluteMotionEvent {
                        stamp: self.stamp(time_us),
                        position: range.position(point),
                    },
                });
            }
            self.last_abs = Some(point);
        }
    }

    fn direction(&self) -> AxisRelativeDirection {
        if self.natural_scrolling {
            AxisRelativeDirection::Inverted
        } else {
            AxisRelativeDirection::Identical
        }
    }

    fn switch(&self, switch: EvdevSwitchEvent, time_us: u64, out: &mut Vec<Event_>) {
        let kind = match switch.switch {
            EvdevSwitch::Lid => Switch::Lid,
            EvdevSwitch::TabletMode => Switch::TabletMode,
        };
        let state = if switch.on {
            SwitchState::On
        } else {
            SwitchState::Off
        };
        out.push(InputEvent::SwitchToggle {
            event: SwitchEvent {
                stamp: self.stamp(time_us),
                switch: kind,
                state,
            },
        });
    }

    /// Zero on both axes ends a finger scroll; clients run their own momentum.
    fn scroll_stop(&self, stamp: Stamp) -> Event_ {
        let amount = Axes {
            horizontal: Some(0.0),
            vertical: Some(0.0),
        };
        InputEvent::PointerAxis {
            event: AxisEvent {
                stamp,
                source: AxisSource::Finger,
                amount,
                v120: Axes::default(),
                direction: self.direction(),
            },
        }
    }

    fn pad_event(&mut self, event: TouchpadEvent, time_us: u64, out: &mut Vec<Event_>) {
        let stamp = self.stamp(time_us);
        match event {
            TouchpadEvent::PointerMotion { dx, dy } => out.push(InputEvent::PointerMotion {
                event: MotionEvent {
                    stamp,
                    delta: (dx, dy),
                },
            }),
            TouchpadEvent::Button { button, pressed } => {
                let code = match button {
                    TouchpadButton::Left => BTN_LEFT,
                    TouchpadButton::Right => BTN_RIGHT,
                };
                self.set_held(code, pressed);
                self.push_button(code, pressed, time_us, out);
            }
            // A scroll starts with its first update.
            TouchpadEvent::ScrollBegin => {}
            TouchpadEvent::ScrollUpdate { dx, dy } => {
                self.scrolling = true;
                // The model reports how the content moves; an axis value is the opposite.
                let amount = Axes::moved(-dx, -dy);
                out.push(InputEvent::PointerAxis {
                    event: AxisEvent {
                        stamp,
                        source: AxisSource::Finger,
                        amount,
                        v120: Axes::default(),
                        direction: self.direction(),
                    },
                });
            }
            TouchpadEvent::ScrollEnd { .. } => {
                self.scrolling = false;
                out.push(self.scroll_stop(stamp));
            }
            TouchpadEvent::SwipeBegin { fingers, axis } => {
                self.swipe = Some((axis, 0.0));
                out.push(InputEvent::GestureSwipeBegin {
                    event: GestureBegin { stamp, fingers },
                });
            }
            TouchpadEvent::SwipeUpdate { progress, .. } => {
                let (axis, last) = self
                    .swipe
                    .as_mut()
                    .expect("the touchpad model begins a swipe before updating it");
                let travel = (progress - *last) * SWIPE_COMPLETE_MM * GESTURE_UNITS_PER_MM;
                *last = progress;
                let delta = match axis {
                    SwipeAxis::Horizontal => (travel, 0.0),
                    SwipeAxis::Vertical => (0.0, travel),
                };
                out.push(InputEvent::GestureSwipeUpdate {
                    event: SwipeUpdate { stamp, delta },
                });
            }
            TouchpadEvent::SwipeEnd { committed, .. } => {
                self.swipe = None;
                out.push(InputEvent::GestureSwipeEnd {
                    event: GestureEnd {
                        stamp,
                        cancelled: !committed,
                    },
                });
            }
            TouchpadEvent::PinchBegin { fingers } => {
                self.pinching = true;
                self.rotation_deg = 0.0;
                out.push(InputEvent::GesturePinchBegin {
                    event: GestureBegin { stamp, fingers },
                });
            }
            TouchpadEvent::PinchUpdate {
                scale,
                rotation_deg,
                ..
            } => {
                let rotation = rotation_deg - self.rotation_deg;
                self.rotation_deg = rotation_deg;
                out.push(InputEvent::GesturePinchUpdate {
                    event: PinchUpdate {
                        stamp,
                        delta: (0.0, 0.0),
                        scale,
                        rotation,
                    },
                });
            }
            TouchpadEvent::PinchEnd { committed, .. } => {
                self.pinching = false;
                out.push(InputEvent::GesturePinchEnd {
                    event: GestureEnd {
                        stamp,
                        cancelled: !committed,
                    },
                });
            }
            // Nothing consumes the force click yet.
            TouchpadEvent::ForceClick { .. } => {}
        }
    }
}

/// Keyboard keys: everything outside the button block.
fn is_key_code(code: u16) -> bool {
    !is_button(code)
}

// ---- devices on the main loop ----------------------------------------------------------------------

/// An open device: the translator, its loop source and the descriptor the session handed out.
struct Opened {
    translator: Translator,
    /// The node it was opened from, to tell a reused device number from the same device.
    devnode: PathBuf,
    token: RegistrationToken,
    /// Returned to the session when the device closes. The loop source reads a duplicate.
    session_fd: OwnedFd,
}

/// The pending touchpad timer.
struct ArmedTimer {
    token: RegistrationToken,
    deadline: f64,
}

pub struct Inputs<S: 'static> {
    handle: LoopHandle<'static, S>,
    session: LogindSession,
    devices: HashMap<DevT, Opened>,
    timer: Option<ArmedTimer>,
}

pub type SharedInputs<S> = Rc<RefCell<Inputs<S>>>;

/// An empty device table; [`open_all`] fills it.
pub fn new_inputs<S: EvdevHost>(
    handle: LoopHandle<'static, S>,
    session: LogindSession,
) -> SharedInputs<S> {
    Rc::new(RefCell::new(Inputs {
        handle,
        session,
        devices: HashMap::new(),
        timer: None,
    }))
}

fn now<S: EvdevHost>(state: &S) -> Duration {
    state.evdev_clock_now()
}

fn deliver<S: EvdevHost>(state: &mut S, events: Vec<Event_>) {
    for event in events {
        // A key that was already down is state, not a keystroke: it bypasses the accessibility filters (sticky keys
        // would latch the modifiers of the chord that switched to this session).
        if let InputEvent::Keyboard { event: key } = &event {
            if key.resynced && key.state == KeyState::Pressed {
                state.evdev_pass_unfiltered_key(key.code);
            }
        }
        state.evdev_process_input_event(event);
    }
}

fn close_fd(session: &mut LogindSession, fd: OwnedFd) {
    if let Err(e) = session.close(fd) {
        tracing::warn!(error = %e, "closing an input device failed");
    }
}

/// Reads what the device has queued and translates it.
fn read(
    evdev: &mut EvdevDevice,
    translator: &mut Translator,
    out: &mut Vec<Event_>,
) -> Result<(), cupertino_devices::evdev::Error> {
    let mut frames = Vec::new();
    evdev.read_frames(&mut frames)?;
    for frame in &frames {
        translator.frame(frame, out);
    }
    Ok(())
}

/// Opens every input node and announces it.
pub fn open_all<S: EvdevHost>(shared: &SharedInputs<S>, state: &mut S) -> Result<()> {
    let nodes = enumerate(Path::new(SYSFS), Subsystem::Input)
        .map_err(|e| anyhow!("listing input devices: {e}"))?;
    for node in &nodes {
        open_reporting(shared, node, state);
    }
    Ok(())
}

/// Closes every device, releasing what they hold, for a session that is paused.
pub fn close_all<S: EvdevHost>(shared: &SharedInputs<S>, state: &mut S) {
    let devnums: Vec<DevT> = shared.borrow().devices.keys().copied().collect();
    for devnum in devnums {
        close_device(shared, devnum, true, state);
    }
}

/// Kernel uevent for the `input` subsystem: a node appeared or went away.
pub fn hotplug<S: EvdevHost>(shared: &SharedInputs<S>, event: &DeviceEvent, state: &mut S) {
    let Some(devnum) = event.devnum else { return };
    match event.action {
        Action::Added if event.devnode.as_deref().is_some_and(is_event_node) => {
            let syspath = Path::new(SYSFS).join(event.devpath.trim_start_matches('/'));
            let handle = shared.borrow().handle.clone();
            let shared = shared.clone();
            let path = syspath.clone();
            retry(
                &handle,
                HOTPLUG_RETRY_INTERVAL,
                HOTPLUG_RETRIES,
                state,
                move |state| {
                    // Unplugged again while waiting: nothing to open, and not an error.
                    if !path.exists() {
                        return Ok(());
                    }
                    let node = SysDevice::from_syspath(&path)
                        .map_err(|e| anyhow!("reading the new input device: {e}"))?;
                    open_device(&shared, &node, state)
                },
                move |e| tracing::error!(path = %syspath.display(), error = %format!("{e:#}"), "skipping input device"),
            );
        }
        Action::Removed => close_device(shared, devnum, true, state),
        _ => {}
    }
}

/// How long and how often a hot-plugged node is retried (approximate: udevd normally finishes within
/// milliseconds). The kernel announces a node before udevd has processed it, and logind refuses
/// `TakeDevice` for it (`ENODEV`) until then.
const HOTPLUG_RETRY_INTERVAL: Duration = Duration::from_millis(50);
const HOTPLUG_RETRIES: u32 = 40;

/// Runs `attempt` now and, while it fails, again every `interval` on the loop, up to `retries` more
/// times. `give_up` receives the last failure.
fn retry<S: 'static>(
    handle: &LoopHandle<'static, S>,
    interval: Duration,
    retries: u32,
    state: &mut S,
    mut attempt: impl FnMut(&mut S) -> Result<()> + 'static,
    give_up: impl FnOnce(anyhow::Error) + 'static,
) {
    let Err(failure) = attempt(state) else { return };
    if retries == 0 {
        give_up(failure);
        return;
    }
    let next_handle = handle.clone();
    // The loop wants a `FnMut`, but the timer fires once and hands both callbacks on.
    let mut pending = Some((attempt, give_up));
    let inserted = handle.insert_source(Timer::from_duration(interval), move |_, _, state| {
        if let Some((attempt, give_up)) = pending.take() {
            retry(&next_handle, interval, retries - 1, state, attempt, give_up);
        }
        TimeoutAction::Drop
    });
    if let Err(e) = inserted {
        tracing::error!(error = %e, failure = %format!("{failure:#}"), "scheduling the retry of a failed attempt failed");
    }
}

/// An input node as the kernel names it: its device number and its file. Both must match for a
/// node to be the device that is open, because the kernel reuses device numbers.
#[derive(Clone, Debug, PartialEq, Eq)]
struct NodeKey {
    devnum: DevT,
    devnode: PathBuf,
}

/// What to do to bring the open set in line with the nodes that exist.
#[derive(Debug, Default, PartialEq, Eq)]
struct NodeDiff {
    /// Open devices whose node is gone, or whose device number now belongs to another node.
    close: Vec<NodeKey>,
    /// Nodes that are not open.
    open: Vec<NodeKey>,
}

/// Compares the open devices with the nodes present now. Close what [`NodeDiff::close`] lists
/// before opening what [`NodeDiff::open`] lists: a reused device number is in both.
fn diff_nodes(open: &[NodeKey], present: &[NodeKey]) -> NodeDiff {
    NodeDiff {
        close: open
            .iter()
            .filter(|key| !present.contains(key))
            .cloned()
            .collect(),
        open: present
            .iter()
            .filter(|key| !open.contains(key))
            .cloned()
            .collect(),
    }
}

/// Brings the open devices in line with the nodes that exist, after uevents were lost: opens the
/// new ones and closes the gone ones.
pub fn resync<S: EvdevHost>(shared: &SharedInputs<S>, state: &mut S) {
    let nodes = match enumerate(Path::new(SYSFS), Subsystem::Input) {
        Ok(nodes) => nodes,
        Err(e) => {
            tracing::error!(error = %e, "listing input devices to resynchronise failed");
            return;
        }
    };
    let present: Vec<(NodeKey, &SysDevice)> = nodes
        .iter()
        .filter_map(|node| {
            Some((
                NodeKey {
                    devnum: node.devnum()?,
                    devnode: node.devnode()?.to_owned(),
                },
                node,
            ))
        })
        .collect();
    let open: Vec<NodeKey> = shared
        .borrow()
        .devices
        .iter()
        .map(|(devnum, opened)| NodeKey {
            devnum: *devnum,
            devnode: opened.devnode.clone(),
        })
        .collect();
    let diff = diff_nodes(
        &open,
        &present
            .iter()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>(),
    );
    for key in &diff.close {
        close_device(shared, key.devnum, true, state);
    }
    for (key, node) in &present {
        if diff.open.contains(key) {
            open_reporting(shared, node, state);
        }
    }
}

fn is_event_node(devnode: &Path) -> bool {
    devnode
        .file_name()
        .and_then(|name| name.to_str())
        .and_then(|name| name.strip_prefix("event"))
        .is_some_and(|number| number.parse::<u32>().is_ok())
}

fn open_reporting<S: EvdevHost>(shared: &SharedInputs<S>, node: &SysDevice, state: &mut S) {
    if let Err(e) = open_device(shared, node, state) {
        tracing::error!(device = node.sysname(), error = %format!("{e:#}"), "skipping input device");
    }
}

/// Opens one node through the session and registers it with the loop. A node the compositor routes
/// nothing from, one that is already open, or any node while the session is paused, is skipped.
fn open_device<S: EvdevHost>(
    shared: &SharedInputs<S>,
    node: &SysDevice,
    state: &mut S,
) -> Result<()> {
    let devnum = node.devnum().context("the node has no device number")?;
    let devnode = node
        .devnode()
        .context("the node has no device file")?
        .to_owned();
    let Some(class) = classify_device(node, Path::new(SYSFS))
        .map_err(|e| anyhow!("classifying the device: {e}"))?
    else {
        return Ok(());
    };
    if !is_routed(&class) || shared.borrow().devices.contains_key(&devnum) {
        return Ok(());
    }
    let mut session = shared.borrow().session.clone();
    // A node that appears while the session is paused would be handed out revoked; `open_all`
    // opens it on the activation.
    if !session.is_active() {
        return Ok(());
    }
    let fd = session
        .open(&devnode, OFlags::RDWR | OFlags::CLOEXEC | OFlags::NONBLOCK)
        .map_err(|e| anyhow!("opening {}: {e}", devnode.display()))?;
    match register(
        node,
        class,
        &fd,
        state.evdev_natural_scrolling_for(class.touchpad),
    ) {
        Ok((evdev, translator, mut events)) => {
            let device = translator.device.clone();
            let handle = shared.borrow().handle.clone();
            let token = match insert_device(shared, devnum, evdev, &handle) {
                Ok(token) => token,
                Err(e) => {
                    close_fd(&mut session, fd);
                    return Err(e);
                }
            };
            shared.borrow_mut().devices.insert(
                devnum,
                Opened {
                    translator,
                    devnode,
                    token,
                    session_fd: fd,
                },
            );
            events.insert(0, InputEvent::DeviceAdded { device });
            deliver(state, events);
            arm_timer(shared, state);
            Ok(())
        }
        Err(e) => {
            close_fd(&mut session, fd);
            Err(e)
        }
    }
}

/// Everything about a new device that can fail before it joins the loop: the evdev handle, its
/// translator, and the events of its open-time state (held keys, the lid).
fn register(
    node: &SysDevice,
    class: InputClass,
    fd: &OwnedFd,
    natural_scrolling: bool,
) -> Result<(EvdevDevice, Translator, Vec<Event_>)> {
    let mut evdev = EvdevDevice::open(fd.try_clone().context("duplicating the descriptor")?)
        .map_err(|e| anyhow!("reading the device capabilities: {e}"))?;
    let device = InputDevice::new(node, class, evdev.info());
    let mut translator = Translator::new(device, evdev.info(), natural_scrolling);
    let mut events = Vec::new();
    read(&mut evdev, &mut translator, &mut events)
        .map_err(|e| anyhow!("reading the device state: {e}"))?;
    Ok((evdev, translator, events))
}

fn insert_device<S: EvdevHost>(
    shared: &SharedInputs<S>,
    devnum: DevT,
    evdev: EvdevDevice,
    handle: &LoopHandle<'static, S>,
) -> Result<RegistrationToken> {
    let shared = shared.clone();
    handle
        .insert_source(
            Generic::new(evdev, Interest::READ, Mode::Level),
            move |_, evdev, state| {
                // SAFETY: the descriptor is only read; the device is never replaced or dropped here.
                let evdev = unsafe { evdev.get_mut() };
                Ok(read_ready(&shared, devnum, evdev, state))
            },
        )
        .map_err(|e| anyhow!("registering the device with the event loop: {e}"))
}

/// The device became readable.
fn read_ready<S: EvdevHost>(
    shared: &SharedInputs<S>,
    devnum: DevT,
    evdev: &mut EvdevDevice,
    state: &mut S,
) -> PostAction {
    let mut events = Vec::new();
    let result = {
        let mut inputs = shared.borrow_mut();
        let opened = inputs
            .devices
            .get_mut(&devnum)
            .expect("a registered device has an entry");
        let touchpad = opened.translator.device.0.class.touchpad;
        opened
            .translator
            .set_natural_scrolling(state.evdev_natural_scrolling_for(touchpad));
        read(evdev, &mut opened.translator, &mut events)
    };
    match result {
        Ok(()) => {
            deliver(state, events);
            arm_timer(shared, state);
            PostAction::Continue
        }
        Err(e) => {
            match &e {
                cupertino_devices::evdev::Error::Io {
                    errno: Errno::NODEV,
                    ..
                } => tracing::info!(devnum = %devnum, "input device unplugged"),
                _ => {
                    tracing::error!(devnum = %devnum, error = %e, "reading an input device failed, closing it")
                }
            }
            // The loop drops this source when the callback returns.
            close_device(shared, devnum, false, state);
            PostAction::Remove
        }
    }
}

/// Removes a device, releases what it held and announces its removal. `remove_source` is false
/// when called from the device's own loop callback, which removes the source by its return value.
fn close_device<S: EvdevHost>(
    shared: &SharedInputs<S>,
    devnum: DevT,
    remove_source: bool,
    state: &mut S,
) {
    let (opened, mut session, handle) = {
        let mut inputs = shared.borrow_mut();
        let Some(opened) = inputs.devices.remove(&devnum) else {
            return;
        };
        (opened, inputs.session.clone(), inputs.handle.clone())
    };
    if remove_source {
        handle.remove(opened.token);
    }
    close_fd(&mut session, opened.session_fd);
    let mut translator = opened.translator;
    let mut events = Vec::new();
    translator.release_all(micros_from_seconds(now(state).as_secs_f64()), &mut events);
    events.push(InputEvent::DeviceRemoved {
        device: translator.device,
    });
    deliver(state, events);
}

/// Makes sure a timer fires when the earliest touchpad deadline passes.
fn arm_timer<S: EvdevHost>(shared: &SharedInputs<S>, state: &S) {
    let mut inputs = shared.borrow_mut();
    let Some(deadline) = inputs
        .devices
        .values()
        .filter_map(|opened| opened.translator.next_deadline())
        .reduce(f64::min)
    else {
        return;
    };
    // A pending timer that fires first re-arms for this deadline.
    if inputs
        .timer
        .as_ref()
        .is_some_and(|timer| timer.deadline <= deadline)
    {
        return;
    }
    if let Some(timer) = inputs.timer.take() {
        inputs.handle.remove(timer.token);
    }
    let delay = Duration::from_secs_f64((deadline - now(state).as_secs_f64()).max(0.0));
    let callback_shared = shared.clone();
    let inserted = inputs
        .handle
        .insert_source(Timer::from_duration(delay), move |_, _, state| {
            let now_s = now(state).as_secs_f64();
            let mut events = Vec::new();
            {
                let mut inputs = callback_shared.borrow_mut();
                inputs.timer = None;
                for opened in inputs.devices.values_mut() {
                    opened.translator.tick(now_s, &mut events);
                }
            }
            deliver(state, events);
            arm_timer(&callback_shared, state);
            TimeoutAction::Drop
        });
    match inserted {
        Ok(token) => inputs.timer = Some(ArmedTimer { token, deadline }),
        Err(e) => tracing::error!(error = %e, "arming the touchpad timer failed"),
    }
}
