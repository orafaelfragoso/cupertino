//! A remote device as the daemon tracks it: what discovery reported, whether it is paired and
//! whether it is connected.

use std::time::Duration;

use cupertino_bluetooth::{Address, AddressType, ClassOfDevice, DeviceId, Eir, MajorClass, Uuid};

/// What a device is, for the icon the shell draws. The classification reads the GAP appearance
/// first (low energy devices send it) and the class of device second. Both tables are as designed:
/// Bluetooth Assigned Numbers, `appearance_values.yaml` and `class_of_device.yaml` (the SIG's
/// public repository).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Unknown,
    Computer,
    Tablet,
    Phone,
    Watch,
    Display,
    Headphones,
    Headset,
    Speaker,
    /// Audio equipment that is none of the above.
    Audio,
    Keyboard,
    Mouse,
    Trackpad,
    Gamepad,
    RemoteControl,
    Camera,
    Printer,
}

impl DeviceKind {
    /// The identifier the shell maps to a glyph.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Computer => "computer",
            Self::Tablet => "tablet",
            Self::Phone => "phone",
            Self::Watch => "watch",
            Self::Display => "display",
            Self::Headphones => "headphones",
            Self::Headset => "headset",
            Self::Speaker => "speaker",
            Self::Audio => "audio",
            Self::Keyboard => "keyboard",
            Self::Mouse => "mouse",
            Self::Trackpad => "trackpad",
            Self::Gamepad => "gamepad",
            Self::RemoteControl => "remote-control",
            Self::Camera => "camera",
            Self::Printer => "printer",
        }
    }

    /// Classifies by appearance, then by class of device.
    pub fn classify(class: Option<ClassOfDevice>, appearance: Option<u16>) -> Self {
        match appearance.map(from_appearance) {
            Some(kind) if kind != Self::Unknown => kind,
            _ => class.map_or(Self::Unknown, from_class),
        }
    }
}

/// `appearance_values.yaml`: the value is `category << 6 | subcategory`.
fn from_appearance(appearance: u16) -> DeviceKind {
    let (category, sub) = (appearance >> 6, appearance & 0x3F);
    match (category, sub) {
        (0x001, _) => DeviceKind::Phone,
        (0x002, 0x07) => DeviceKind::Tablet,
        (0x002, _) => DeviceKind::Computer,
        (0x003, _) => DeviceKind::Watch,
        (0x005, _) => DeviceKind::Display,
        (0x006, _) => DeviceKind::RemoteControl,
        (0x00A, _) => DeviceKind::Audio,
        (0x00F, 0x01) => DeviceKind::Keyboard,
        (0x00F, 0x02) => DeviceKind::Mouse,
        (0x00F, 0x03 | 0x04) => DeviceKind::Gamepad,
        (0x00F, 0x05) => DeviceKind::Tablet,
        (0x00F, 0x09) => DeviceKind::Trackpad,
        (0x00F, 0x0A) => DeviceKind::RemoteControl,
        (0x021, _) => DeviceKind::Speaker,
        (0x022, _) => DeviceKind::Audio,
        (0x025, 0x02) => DeviceKind::Headset,
        (0x025, 0x01 | 0x03 | 0x04 | 0x05 | 0x06) => DeviceKind::Headphones,
        _ => DeviceKind::Unknown,
    }
}

/// `class_of_device.yaml`: the major class picks the table, the six minor bits are read per table.
fn from_class(class: ClassOfDevice) -> DeviceKind {
    let minor = class.minor();
    match class.major() {
        MajorClass::Computer if minor == 7 => DeviceKind::Tablet,
        MajorClass::Computer => DeviceKind::Computer,
        MajorClass::Phone => DeviceKind::Phone,
        MajorClass::AudioVideo => match minor {
            1 | 2 => DeviceKind::Headset,
            6 => DeviceKind::Headphones,
            5 | 7 | 8 | 10 => DeviceKind::Speaker,
            12 | 13 => DeviceKind::Camera,
            14 | 15 => DeviceKind::Display,
            _ => DeviceKind::Audio,
        },
        // The top two minor bits say keyboard (1), pointing device (2) or both (3); the low four
        // bits say what kind of peripheral.
        MajorClass::Peripheral => match (minor >> 4, minor & 0x0F) {
            (1 | 3, _) => DeviceKind::Keyboard,
            (_, 5) => DeviceKind::Tablet,
            (_, 3) => DeviceKind::RemoteControl,
            (_, 1 | 2) => DeviceKind::Gamepad,
            (2, _) => DeviceKind::Mouse,
            _ => DeviceKind::Unknown,
        },
        // Imaging: minor bit 5 printer, bit 3 camera, bit 2 display (class bits 7, 5 and 4).
        MajorClass::Imaging if minor & 0x20 != 0 => DeviceKind::Printer,
        MajorClass::Imaging if minor & 0x08 != 0 => DeviceKind::Camera,
        MajorClass::Imaging if minor & 0x04 != 0 => DeviceKind::Display,
        MajorClass::Wearable if minor == 1 => DeviceKind::Watch,
        _ => DeviceKind::Unknown,
    }
}

/// A snapshot of a device for the bus.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    pub id: DeviceId,
    pub name: Option<String>,
    pub icon: DeviceKind,
    pub class: Option<ClassOfDevice>,
    pub appearance: Option<u16>,
    pub rssi: Option<i8>,
    pub paired: bool,
    pub bonded: bool,
    pub connected: bool,
    /// A pairing with this device is in progress.
    pub pairing: bool,
    pub uuids: Vec<Uuid>,
}

impl DeviceInfo {
    /// The identifier methods accept: the address, a slash, the address type.
    pub fn key(&self) -> String {
        device_key(&self.id)
    }
}

/// `AA:BB:CC:DD:EE:FF/bredr`.
pub fn device_key(id: &DeviceId) -> String {
    format!("{}/{}", id.address, id.kind)
}

/// A device named in a request: an address, with the type when the caller knows it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DeviceRef {
    pub address: Address,
    pub kind: Option<AddressType>,
}

impl std::str::FromStr for DeviceRef {
    type Err = cupertino_bluetooth::ParseError;

    /// `AA:BB:CC:DD:EE:FF` or `AA:BB:CC:DD:EE:FF/bredr` (`le-public`, `le-random`).
    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once('/') {
            None => Ok(Self { address: text.parse()?, kind: None }),
            Some((address, kind)) => Ok(Self { address: address.parse()?, kind: Some(kind.parse()?) }),
        }
    }
}

/// A remote device.
#[derive(Clone, Debug)]
pub struct Device {
    pub id: DeviceId,
    name: Option<String>,
    name_complete: bool,
    class: Option<ClassOfDevice>,
    appearance: Option<u16>,
    uuids: Vec<Uuid>,
    rssi: Option<i8>,
    pub paired: bool,
    pub bonded: bool,
    pub connected: bool,
    /// When the device was last reported, on the daemon's clock.
    pub last_seen: Duration,
}

impl Device {
    pub fn new(id: DeviceId, now: Duration) -> Self {
        Self { id, name: None, name_complete: false, class: None, appearance: None, uuids: Vec::new(), rssi: None, paired: false, bonded: false, connected: false, last_seen: now }
    }

    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// Sets the name remembered with a bond. A name from the air that arrives later wins.
    pub fn set_stored_name(&mut self, name: &str) {
        self.name = Some(name.to_owned());
        self.name_complete = true;
    }

    /// Folds a report into the device and returns whether anything a client can see changed.
    ///
    /// - A complete name replaces any name; a shortened name only fills a missing one.
    /// - Class and appearance are replaced when the report has them; UUIDs are added.
    /// - An RSSI replaces the old one; a report without one (the controller said "not available")
    ///   keeps the last reading.
    pub fn merge(&mut self, now: Duration, rssi: Option<i8>, eir: &Eir) -> bool {
        self.last_seen = now;
        let before = self.info(false);
        match &eir.name {
            Some(name) if name.complete => {
                self.name = Some(name.text.clone());
                self.name_complete = true;
            }
            Some(name) if self.name.is_none() => self.name = Some(name.text.clone()),
            _ => {}
        }
        self.class = eir.class.or(self.class);
        self.appearance = eir.appearance.or(self.appearance);
        for uuid in &eir.uuids {
            if !self.uuids.contains(uuid) {
                self.uuids.push(*uuid);
            }
        }
        self.rssi = rssi.or(self.rssi);
        self.info(false) != before
    }

    pub fn info(&self, pairing: bool) -> DeviceInfo {
        DeviceInfo {
            id: self.id,
            name: self.name.clone(),
            icon: DeviceKind::classify(self.class, self.appearance),
            class: self.class,
            appearance: self.appearance,
            rssi: self.rssi,
            paired: self.paired,
            bonded: self.bonded,
            connected: self.connected,
            pairing,
            uuids: self.uuids.clone(),
        }
    }
}
