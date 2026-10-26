//! The framer: a state machine from raw [`InputEvent`]s to [`Frame`]s. It owns no fd, so recorded sequences drive it
//! directly in tests.

use crate::bitset::BitSet;
use crate::codes::*;
use crate::error::FrameError;
use crate::event::InputEvent;
use crate::frame::{AbsPoint, Contact, Frame, KeyEvent, KeyState, Motion, Switch, SwitchEvent, Tool, Wheel};
use crate::info::DeviceInfo;

/// What the framer needs to know about a device, derived once from its [`DeviceInfo`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Number of multitouch slots; 0 when the device has no multitouch.
    pub slot_count: u32,
    /// The device has `ABS_X` and `ABS_Y`.
    pub has_abs_xy: bool,
    /// The device has `ABS_MT_PRESSURE`.
    pub has_pressure: bool,
    /// The device has `ABS_MT_TOUCH_MAJOR`.
    pub has_touch_major: bool,
    /// The device has `ABS_MT_TOUCH_MINOR`.
    pub has_touch_minor: bool,
    /// The device has `ABS_MT_TOOL_TYPE`.
    pub has_tool_type: bool,
    /// The device has `REL_WHEEL_HI_RES`.
    pub hi_res_vertical: bool,
    /// The device has `REL_HWHEEL_HI_RES`.
    pub hi_res_horizontal: bool,
    /// The device has at least one of `BTN_TOOL_FINGER`..`BTN_TOOL_QUINTTAP`.
    pub has_finger_tools: bool,
}

impl Layout {
    /// Derives the layout; rejects multitouch layouts the framer cannot follow (protocol A, missing position axes).
    pub fn from_info(info: &DeviceInfo) -> Result<Self, FrameError> {
        let slot_count = match info.abs(ABS_MT_SLOT) {
            Some(slot) => u32::try_from(i64::from(slot.max) + 1)
                .ok()
                .filter(|&count| count > 0)
                .ok_or(FrameError::EmptySlotRange)?,
            None if info.abs(ABS_MT_POSITION_X).is_some() => return Err(FrameError::MultitouchWithoutSlots),
            None => 0,
        };
        if slot_count > 0
            && [ABS_MT_POSITION_X, ABS_MT_POSITION_Y, ABS_MT_TRACKING_ID].iter().any(|&code| info.abs(code).is_none())
        {
            return Err(FrameError::MultitouchWithoutPosition);
        }
        Ok(Self {
            slot_count,
            has_abs_xy: info.abs(ABS_X).is_some() && info.abs(ABS_Y).is_some(),
            has_pressure: info.abs(ABS_MT_PRESSURE).is_some(),
            has_touch_major: info.abs(ABS_MT_TOUCH_MAJOR).is_some(),
            has_touch_minor: info.abs(ABS_MT_TOUCH_MINOR).is_some(),
            has_tool_type: info.abs(ABS_MT_TOOL_TYPE).is_some(),
            hi_res_vertical: info.rel_axes.contains(REL_WHEEL_HI_RES),
            hi_res_horizontal: info.rel_axes.contains(REL_HWHEEL_HI_RES),
            has_finger_tools: FINGER_TOOLS.iter().any(|&(code, _)| info.keys.contains(code)),
        })
    }

    /// The single-value absolute axes a [`Snapshot`] must carry (`EVIOCGABS`).
    pub fn snapshot_abs_codes(&self) -> Vec<u16> {
        let mut codes = Vec::new();
        if self.has_abs_xy {
            codes.extend([ABS_X, ABS_Y]);
        }
        if self.slot_count > 0 {
            codes.push(ABS_MT_SLOT);
        }
        codes
    }

    /// The per-slot axes a [`Snapshot`] must carry (`EVIOCGMTSLOTS`); empty without multitouch.
    pub fn snapshot_mt_codes(&self) -> Vec<u16> {
        if self.slot_count == 0 {
            return Vec::new();
        }
        let mut codes = vec![ABS_MT_TRACKING_ID, ABS_MT_POSITION_X, ABS_MT_POSITION_Y];
        let optional = [
            (self.has_pressure, ABS_MT_PRESSURE),
            (self.has_touch_major, ABS_MT_TOUCH_MAJOR),
            (self.has_touch_minor, ABS_MT_TOUCH_MINOR),
            (self.has_tool_type, ABS_MT_TOOL_TYPE),
        ];
        codes.extend(optional.into_iter().filter(|&(present, _)| present).map(|(_, code)| code));
        codes
    }
}

/// `BTN_TOOL_*` finger-count buttons and the finger count each stands for (input-event-codes.h).
const FINGER_TOOLS: [(u16, u8); 5] = [
    (BTN_TOOL_FINGER, 1),
    (BTN_TOOL_DOUBLETAP, 2),
    (BTN_TOOL_TRIPLETAP, 3),
    (BTN_TOOL_QUADTAP, 4),
    (BTN_TOOL_QUINTTAP, 5),
];

fn finger_tool_count(code: u16) -> Option<u8> {
    FINGER_TOOLS.iter().find(|&&(tool, _)| tool == code).map(|&(_, count)| count)
}

/// Per-axis values of every slot, as `EVIOCGMTSLOTS` returns them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MtSlotValues {
    /// The multitouch axis (`ABS_MT_*`).
    pub code: u16,
    /// One value per slot.
    pub values: Vec<i32>,
}

/// The device state read after `SYN_DROPPED` (and at open): the input to [`Framer::apply_snapshot`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    /// Keys and buttons held (`EVIOCGKEY`).
    pub keys: BitSet,
    /// Switches on (`EVIOCGSW`).
    pub switches: BitSet,
    /// Current values of the axes in [`Layout::snapshot_abs_codes`] (`EVIOCGABS`), as `(code, value)`.
    pub abs: Vec<(u16, i32)>,
    /// Per-slot values of the axes in [`Layout::snapshot_mt_codes`] (`EVIOCGMTSLOTS`).
    pub mt: Vec<MtSlotValues>,
}

impl Snapshot {
    fn abs_value(&self, code: u16) -> Result<i32, FrameError> {
        self.abs
            .iter()
            .find(|&&(axis, _)| axis == code)
            .map(|&(_, value)| value)
            .ok_or(FrameError::MissingSnapshotAxis { code })
    }

    fn slot_values(&self, code: u16, slot_count: u32) -> Result<&[i32], FrameError> {
        let axis = self.mt.iter().find(|axis| axis.code == code).ok_or(FrameError::MissingSnapshotAxis { code })?;
        if axis.values.len() != slot_count as usize {
            return Err(FrameError::SnapshotSlotCount { code, actual: axis.values.len(), expected: slot_count });
        }
        Ok(&axis.values)
    }
}

/// What [`Framer::push`] produces when an event completes something.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FramerOutput {
    /// A complete frame.
    Frame(Frame),
    /// The kernel dropped events (`SYN_DROPPED`) and the stream has reached the next `SYN_REPORT`. The caller reads a
    /// [`Snapshot`] from the device and passes it to [`Framer::apply_snapshot`].
    ResyncNeeded {
        /// Timestamp of the closing `SYN_REPORT`, in microseconds.
        time_us: i64,
    },
}

#[derive(Clone, Copy)]
struct Slot {
    tracking_id: Option<u32>,
    x: i32,
    y: i32,
    pressure: i32,
    touch_major: i32,
    touch_minor: i32,
    tool: Tool,
    /// Contact data changed in the frame being built.
    touched: bool,
}

impl Slot {
    const EMPTY: Self = Self {
        tracking_id: None,
        x: 0,
        y: 0,
        pressure: 0,
        touch_major: 0,
        touch_minor: 0,
        tool: Tool::Finger,
        touched: false,
    };
}

/// Changes accumulated since the last `SYN_REPORT`.
#[derive(Default)]
struct Pending {
    keys: Vec<KeyEvent>,
    switches: Vec<SwitchEvent>,
    motion: Motion,
    legacy_wheel: Wheel,
    hi_res_wheel: Wheel,
}

/// Turns the events of one device into frames. Create it from the device's [`Layout`] and feed it every event in
/// order with [`Framer::push`].
///
/// Events outside the modelled set (`EV_MSC`, `EV_LED`, `ABS_PRESSURE`, ...) are skipped.
///
/// Replayed state is idempotent: a key or switch event that matches the known state yields no transition, and
/// contact data repeated from a snapshot changes nothing. Contact data on an inactive slot is ignored.
///
/// Frame times never decrease: a frame stamped earlier than the previous one (events queued before a snapshot) is
/// clamped up to the previous time. Clamping keeps its transitions; dropping the frame would lose them.
///
/// The remaining errors are fatal because the stream is not one this crate models: protocol A, an unknown event type,
/// an undeclared axis or key, an invalid key, switch or tracking-id value, a slot index out of range, and a bad
/// snapshot. After an error the framer's pending frame is undefined, so drop the device.
pub struct Framer {
    layout: Layout,
    pressed: BitSet,
    switches_on: BitSet,
    finger_count: u8,
    abs_point: AbsPoint,
    slots: Vec<Slot>,
    current_slot: usize,
    pending: Pending,
    dropping: bool,
    /// Time of the last frame returned, in microseconds; the floor for the next one.
    last_time_us: i64,
}

impl Framer {
    /// A framer with every key up and no contacts. Apply a [`Snapshot`] to pick up the device's real state.
    pub fn new(layout: Layout) -> Self {
        Self {
            layout,
            pressed: BitSet::default(),
            switches_on: BitSet::default(),
            finger_count: 0,
            abs_point: AbsPoint { x: 0, y: 0 },
            slots: vec![Slot::EMPTY; layout.slot_count as usize],
            current_slot: 0,
            pending: Pending::default(),
            dropping: false,
            last_time_us: i64::MIN,
        }
    }

    /// The layout this framer was built for.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Feeds one event. A `SYN_REPORT` returns the finished frame; `SYN_DROPPED` discards the partial frame and every
    /// event up to the next `SYN_REPORT`, which then returns [`FramerOutput::ResyncNeeded`].
    pub fn push(&mut self, event: &InputEvent) -> Result<Option<FramerOutput>, FrameError> {
        if event.kind == EV_SYN && event.code == SYN_DROPPED {
            self.pending = Pending::default();
            self.dropping = true;
            return Ok(None);
        }
        if self.dropping {
            if event.kind == EV_SYN && event.code == SYN_REPORT {
                self.dropping = false;
                return Ok(Some(FramerOutput::ResyncNeeded { time_us: event.time_us() }));
            }
            return Ok(None);
        }
        match event.kind {
            EV_SYN => match event.code {
                SYN_REPORT => Ok(Some(FramerOutput::Frame(self.report(event.time_us(), false)?))),
                SYN_MT_REPORT => Err(FrameError::ProtocolA),
                _ => Ok(None),
            },
            EV_KEY => self.key(event.code, event.value).map(|()| None),
            EV_REL => self.rel(event.code, event.value).map(|()| None),
            EV_ABS => self.abs(event.code, event.value).map(|()| None),
            EV_SW => self.switch(event.code, event.value).map(|()| None),
            kind if kind > EV_MAX => Err(FrameError::UnknownEventType { kind }),
            _ => Ok(None),
        }
    }

    /// Replaces the framer's state with `snapshot` and returns a frame marked `resynced`: its `keys` and `switches`
    /// are the transitions needed to get from the old state to the new one (stuck keys come out as releases), and its
    /// contacts, buttons and pointer are the device's state now. Any partial frame is discarded.
    pub fn apply_snapshot(&mut self, snapshot: &Snapshot, time_us: i64) -> Result<Frame, FrameError> {
        let layout = self.layout;
        // Validate and decode everything before touching state, so a bad snapshot leaves the framer unchanged.
        let abs_point = if layout.has_abs_xy {
            Some(AbsPoint { x: snapshot.abs_value(ABS_X)?, y: snapshot.abs_value(ABS_Y)? })
        } else {
            None
        };
        let (slots, current_slot) = if layout.slot_count > 0 { self.decode_slots(snapshot)? } else { (Vec::new(), 0) };

        let mut pending = Pending::default();
        for code in 0..=KEY_MAX {
            let was = self.pressed.contains(code);
            let now = snapshot.keys.contains(code);
            if finger_tool_count(code).is_some() || was == now {
                continue;
            }
            self.pressed.insert(code, now);
            let state = if now { KeyState::Pressed } else { KeyState::Released };
            pending.keys.push(KeyEvent { code, state });
        }
        self.finger_count = FINGER_TOOLS
            .iter()
            .filter(|&&(code, _)| snapshot.keys.contains(code))
            .map(|&(_, count)| count)
            .max()
            .unwrap_or(0);
        for (code, switch) in [(SW_LID, Switch::Lid), (SW_TABLET_MODE, Switch::TabletMode)] {
            let on = snapshot.switches.contains(code);
            if self.switches_on.contains(code) != on {
                self.switches_on.insert(code, on);
                pending.switches.push(SwitchEvent { switch, on });
            }
        }
        if let Some(point) = abs_point {
            self.abs_point = point;
        }
        self.slots = slots;
        self.current_slot = current_slot;
        self.pending = pending;
        self.dropping = false;
        self.report(time_us, true)
    }

    fn decode_slots(&self, snapshot: &Snapshot) -> Result<(Vec<Slot>, usize), FrameError> {
        let layout = &self.layout;
        let count = layout.slot_count;
        let current = snapshot.abs_value(ABS_MT_SLOT)?;
        let current_slot = usize::try_from(current)
            .ok()
            .filter(|&slot| slot < count as usize)
            .ok_or(FrameError::SlotOutOfRange { slot: current, slot_count: count })?;
        let ids = snapshot.slot_values(ABS_MT_TRACKING_ID, count)?;
        let xs = snapshot.slot_values(ABS_MT_POSITION_X, count)?;
        let ys = snapshot.slot_values(ABS_MT_POSITION_Y, count)?;
        let optional = |present: bool, code: u16| -> Result<Option<&[i32]>, FrameError> {
            if present {
                snapshot.slot_values(code, count).map(Some)
            } else {
                Ok(None)
            }
        };
        let pressures = optional(layout.has_pressure, ABS_MT_PRESSURE)?;
        let majors = optional(layout.has_touch_major, ABS_MT_TOUCH_MAJOR)?;
        let minors = optional(layout.has_touch_minor, ABS_MT_TOUCH_MINOR)?;
        let tools = optional(layout.has_tool_type, ABS_MT_TOOL_TYPE)?;

        let mut slots = vec![Slot::EMPTY; count as usize];
        for (index, slot) in slots.iter_mut().enumerate() {
            let id = ids[index];
            if id == -1 {
                continue;
            }
            slot.tracking_id = Some(u32::try_from(id).map_err(|_| FrameError::InvalidTrackingId { value: id })?);
            slot.x = xs[index];
            slot.y = ys[index];
            slot.pressure = pressures.map_or(0, |values| values[index]);
            slot.touch_major = majors.map_or(0, |values| values[index]);
            slot.touch_minor = minors.map_or(0, |values| values[index]);
            slot.tool = tools.map_or(Tool::Finger, |values| Tool::from_raw(values[index]));
        }
        Ok((slots, current_slot))
    }

    fn key(&mut self, code: u16, value: i32) -> Result<(), FrameError> {
        if code > KEY_MAX {
            return Err(FrameError::UnexpectedEvent { kind: EV_KEY, code });
        }
        let state = match value {
            0 => KeyState::Released,
            1 => KeyState::Pressed,
            2 => KeyState::Repeat,
            value => return Err(FrameError::InvalidKeyValue { code, value }),
        };
        if let Some(count) = finger_tool_count(code) {
            if !self.layout.has_finger_tools {
                return Err(FrameError::UnexpectedEvent { kind: EV_KEY, code });
            }
            match state {
                KeyState::Pressed => self.finger_count = count,
                // Drivers may press the new count before releasing the old one.
                KeyState::Released if self.finger_count == count => self.finger_count = 0,
                KeyState::Released | KeyState::Repeat => {}
            }
            return Ok(());
        }
        // A press of a held key or a release of a released one is a replay of state a snapshot already reported.
        match state {
            KeyState::Pressed if self.pressed.contains(code) => return Ok(()),
            KeyState::Released if !self.pressed.contains(code) => return Ok(()),
            KeyState::Pressed => self.pressed.insert(code, true),
            KeyState::Released => self.pressed.insert(code, false),
            KeyState::Repeat => {}
        }
        self.pending.keys.push(KeyEvent { code, state });
        Ok(())
    }

    fn rel(&mut self, code: u16, value: i32) -> Result<(), FrameError> {
        let pending = &mut self.pending;
        match code {
            REL_X => pending.motion.dx = pending.motion.dx.saturating_add(value),
            REL_Y => pending.motion.dy = pending.motion.dy.saturating_add(value),
            REL_WHEEL => pending.legacy_wheel.vertical = pending.legacy_wheel.vertical.saturating_add(value),
            REL_HWHEEL => pending.legacy_wheel.horizontal = pending.legacy_wheel.horizontal.saturating_add(value),
            REL_WHEEL_HI_RES if self.layout.hi_res_vertical => {
                pending.hi_res_wheel.vertical = pending.hi_res_wheel.vertical.saturating_add(value)
            }
            REL_HWHEEL_HI_RES if self.layout.hi_res_horizontal => {
                pending.hi_res_wheel.horizontal = pending.hi_res_wheel.horizontal.saturating_add(value)
            }
            REL_WHEEL_HI_RES | REL_HWHEEL_HI_RES => return Err(FrameError::UnexpectedEvent { kind: EV_REL, code }),
            _ => {}
        }
        Ok(())
    }

    fn abs(&mut self, code: u16, value: i32) -> Result<(), FrameError> {
        let unexpected = FrameError::UnexpectedEvent { kind: EV_ABS, code };
        match code {
            ABS_X | ABS_Y => {
                if !self.layout.has_abs_xy {
                    return Err(unexpected);
                }
                if code == ABS_X {
                    self.abs_point.x = value;
                } else {
                    self.abs_point.y = value;
                }
            }
            ABS_MT_SLOT => {
                let slot_count = self.layout.slot_count;
                if slot_count == 0 {
                    return Err(unexpected);
                }
                self.current_slot = usize::try_from(value)
                    .ok()
                    .filter(|&slot| slot < slot_count as usize)
                    .ok_or(FrameError::SlotOutOfRange { slot: value, slot_count })?;
            }
            ABS_MT_TRACKING_ID | ABS_MT_POSITION_X | ABS_MT_POSITION_Y | ABS_MT_PRESSURE | ABS_MT_TOUCH_MAJOR
            | ABS_MT_TOUCH_MINOR | ABS_MT_TOOL_TYPE => self.mt_axis(code, value)?,
            _ => {}
        }
        Ok(())
    }

    fn mt_axis(&mut self, code: u16, value: i32) -> Result<(), FrameError> {
        let layout = self.layout;
        let declared = match code {
            ABS_MT_PRESSURE => layout.has_pressure,
            ABS_MT_TOUCH_MAJOR => layout.has_touch_major,
            ABS_MT_TOUCH_MINOR => layout.has_touch_minor,
            ABS_MT_TOOL_TYPE => layout.has_tool_type,
            _ => layout.slot_count > 0,
        };
        if !declared {
            return Err(FrameError::UnexpectedEvent { kind: EV_ABS, code });
        }
        let slot = &mut self.slots[self.current_slot];
        match code {
            ABS_MT_TRACKING_ID => match value {
                -1 => {
                    slot.tracking_id = None;
                    slot.touched = false;
                }
                0.. => slot.tracking_id = Some(value as u32),
                value => return Err(FrameError::InvalidTrackingId { value }),
            },
            ABS_MT_POSITION_X => slot.x = value,
            ABS_MT_POSITION_Y => slot.y = value,
            ABS_MT_PRESSURE => slot.pressure = value,
            ABS_MT_TOUCH_MAJOR => slot.touch_major = value,
            ABS_MT_TOUCH_MINOR => slot.touch_minor = value,
            _ => slot.tool = Tool::from_raw(value),
        }
        // A tracking id only starts or ends a contact; everything else is contact data.
        if code != ABS_MT_TRACKING_ID {
            slot.touched = true;
        }
        Ok(())
    }

    fn switch(&mut self, code: u16, value: i32) -> Result<(), FrameError> {
        let on = match value {
            0 => false,
            1 => true,
            value => return Err(FrameError::InvalidSwitchValue { code, value }),
        };
        let switch = match code {
            SW_LID => Switch::Lid,
            SW_TABLET_MODE => Switch::TabletMode,
            _ => return Ok(()),
        };
        // A replay of the state a snapshot already reported is not a transition.
        if self.switches_on.contains(code) == on {
            return Ok(());
        }
        self.switches_on.insert(code, on);
        self.pending.switches.push(SwitchEvent { switch, on });
        Ok(())
    }

    /// Closes the frame being built.
    fn report(&mut self, time_us: i64, resynced: bool) -> Result<Frame, FrameError> {
        let layout = self.layout;
        let time_us = time_us.max(self.last_time_us);
        self.last_time_us = time_us;
        let pending = std::mem::take(&mut self.pending);
        let mut contacts = Vec::new();
        for (index, slot) in self.slots.iter_mut().enumerate() {
            let slot_index = index as u32;
            let Some(tracking_id) = slot.tracking_id else {
                // Quirk: some HID/RMI drivers send contact data for a slot whose tracking id is -1 in the same frame,
                // and a replay after a resync can do the same for a contact the snapshot already ended. libevdev
                // ignores such data, so the contact stays absent.
                slot.touched = false;
                continue;
            };
            slot.touched = false;
            contacts.push(Contact {
                slot: slot_index,
                tracking_id,
                x: slot.x,
                y: slot.y,
                pressure: layout.has_pressure.then_some(slot.pressure),
                touch_major: layout.has_touch_major.then_some(slot.touch_major),
                touch_minor: layout.has_touch_minor.then_some(slot.touch_minor),
                tool: slot.tool,
            });
        }
        let wheel = Wheel {
            vertical: if layout.hi_res_vertical {
                pending.hi_res_wheel.vertical
            } else {
                pending.legacy_wheel.vertical.saturating_mul(120)
            },
            horizontal: if layout.hi_res_horizontal {
                pending.hi_res_wheel.horizontal
            } else {
                pending.legacy_wheel.horizontal.saturating_mul(120)
            },
        };
        Ok(Frame {
            time_us,
            resynced,
            keys: pending.keys,
            held_buttons: self.pressed.iter().filter(|code| is_button(*code)).collect(),
            motion: pending.motion,
            wheel,
            abs_pointer: layout.has_abs_xy.then_some(self.abs_point),
            contacts,
            finger_count: layout.has_finger_tools.then_some(self.finger_count),
            switches: pending.switches,
        })
    }
}
