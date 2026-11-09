//! The D-Bus objects: `org.freedesktop.UPower` (Manager, Device, KbdBacklight) and
//! `org.cupertino.Power1`.
//!
//! Names, signatures and enum numbering follow UPower's documentation
//! (<https://upower.freedesktop.org/docs/>). Objects hold no state of their own: each call reads
//! the daemon's current snapshot, so a getter never disagrees with the last signal.

use std::sync::Arc;

use cupertino_diagnostic_events::{self as diag, Severity, Value as Field};
use zbus::fdo;
use zbus::message::Header;
use zbus::names::BusName;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};
use zbus::Connection;

use crate::auth::Decision;
use crate::backlight::KeyboardBacklight;
use crate::daemon::{announce_display, announce_keyboard, SetError, Shared};
use crate::supply::{Kind, Supply};
use crate::view::{on_battery, DeviceView};

pub const UPOWER_NAME: &str = "org.freedesktop.UPower";
pub const UPOWER_PATH: &str = "/org/freedesktop/UPower";
pub const DISPLAY_DEVICE_PATH: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
/// The keyboard backlight path older clients use; it resolves to the first keyboard backlight.
pub const KEYBOARD_BACKLIGHT_PATH: &str = "/org/freedesktop/UPower/KbdBacklight";
pub const POWER_NAME: &str = "org.cupertino.Power1";
pub const POWER_PATH: &str = "/org/cupertino/Power1";

pub const MANAGER_INTERFACE: &str = "org.freedesktop.UPower";
pub const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";

/// One property of an interface, for `PropertiesChanged` diffs.
pub type Property = (&'static str, Value<'static>);

fn sanitise(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}

/// UPower names a device object `devices/<kind>_<native name>`, with every character outside
/// `[A-Za-z0-9_]` replaced by `_`.
pub fn device_path(supply: &Supply) -> String {
    let kind = match supply.kind {
        Kind::Unknown => "unknown",
        Kind::Battery => "battery",
        Kind::Ups => "ups",
        Kind::LinePower => "line_power",
    };
    format!(
        "/org/freedesktop/UPower/devices/{kind}_{}",
        sanitise(&supply.name)
    )
}

/// The object path of a keyboard backlight, `KbdBacklight/<name>`.
pub fn keyboard_path(keyboard: &KeyboardBacklight) -> String {
    format!("{KEYBOARD_BACKLIGHT_PATH}/{}", sanitise(&keyboard.name))
}

fn object_path(path: String) -> OwnedObjectPath {
    OwnedObjectPath::try_from(path).expect("device paths are built from valid characters only")
}

/// The manager's properties.
pub fn manager_properties(snapshot: &crate::sysfs::Snapshot) -> Vec<Property> {
    vec![
        ("OnBattery", Value::from(on_battery(&snapshot.supplies))),
        ("LidIsClosed", Value::from(snapshot.lid.closed)),
        ("LidIsPresent", Value::from(snapshot.lid.present)),
    ]
}

/// The properties of a device.
pub fn device_properties(view: &DeviceView) -> Vec<Property> {
    vec![
        ("NativePath", Value::from(view.native_path.clone())),
        ("Vendor", Value::from(view.vendor.clone())),
        ("Model", Value::from(view.model.clone())),
        ("Serial", Value::from(view.serial.clone())),
        ("Type", Value::from(view.kind as u32)),
        ("PowerSupply", Value::from(view.power_supply)),
        ("Online", Value::from(view.online)),
        ("Energy", Value::from(view.energy)),
        ("EnergyEmpty", Value::from(view.energy_empty)),
        ("EnergyFull", Value::from(view.energy_full)),
        ("EnergyFullDesign", Value::from(view.energy_full_design)),
        ("EnergyRate", Value::from(view.energy_rate)),
        ("Voltage", Value::from(view.voltage)),
        ("TimeToEmpty", Value::from(view.time_to_empty)),
        ("TimeToFull", Value::from(view.time_to_full)),
        ("Percentage", Value::from(view.percentage)),
        ("IsPresent", Value::from(view.is_present)),
        ("State", Value::from(view.state as u32)),
        ("IsRechargeable", Value::from(view.is_rechargeable)),
        ("Capacity", Value::from(view.capacity)),
        ("IconName", Value::from(view.icon_name.clone())),
    ]
}

/// `org.freedesktop.UPower` at [`UPOWER_PATH`].
pub struct Manager(pub Arc<Shared>);

#[zbus::interface(name = "org.freedesktop.UPower")]
impl Manager {
    fn enumerate_devices(&self) -> Vec<OwnedObjectPath> {
        self.0
            .snapshot()
            .supplies
            .iter()
            .map(|supply| object_path(device_path(supply)))
            .collect()
    }

    fn enumerate_kbd_backlights(&self) -> Vec<OwnedObjectPath> {
        self.0
            .snapshot()
            .keyboards
            .iter()
            .map(|keyboard| object_path(keyboard_path(keyboard)))
            .collect()
    }

    fn get_display_device(&self) -> OwnedObjectPath {
        object_path(DISPLAY_DEVICE_PATH.to_owned())
    }

    #[zbus(signal)]
    pub async fn device_added(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        device: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn device_removed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        device: ObjectPath<'_>,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn daemon_version(&self) -> String {
        env!("CARGO_PKG_VERSION").to_owned()
    }

    #[zbus(property)]
    fn on_battery(&self) -> bool {
        on_battery(&self.0.snapshot().supplies)
    }

    #[zbus(property)]
    fn lid_is_closed(&self) -> bool {
        self.0.snapshot().lid.closed
    }

    #[zbus(property)]
    fn lid_is_present(&self) -> bool {
        self.0.snapshot().lid.present
    }
}

/// Which device a [`DeviceObject`] stands for.
pub enum Target {
    Supply(String),
    Display,
}

/// `org.freedesktop.UPower.Device` for one supply or for the display device.
pub struct DeviceObject {
    pub shared: Arc<Shared>,
    pub target: Target,
}

impl DeviceObject {
    fn view(&self) -> fdo::Result<DeviceView> {
        let snapshot = self.shared.snapshot();
        match &self.target {
            Target::Display => Ok(DeviceView::display(&snapshot.supplies)),
            Target::Supply(name) => snapshot
                .supplies
                .iter()
                .find(|supply| &supply.name == name)
                .map(DeviceView::of_supply)
                .ok_or_else(|| {
                    fdo::Error::UnknownObject(format!("the power supply {name} is gone"))
                }),
        }
    }
}

#[zbus::interface(name = "org.freedesktop.UPower.Device")]
impl DeviceObject {
    #[zbus(property)]
    fn native_path(&self) -> fdo::Result<String> {
        Ok(self.view()?.native_path)
    }
    #[zbus(property)]
    fn vendor(&self) -> fdo::Result<String> {
        Ok(self.view()?.vendor)
    }
    #[zbus(property)]
    fn model(&self) -> fdo::Result<String> {
        Ok(self.view()?.model)
    }
    #[zbus(property)]
    fn serial(&self) -> fdo::Result<String> {
        Ok(self.view()?.serial)
    }
    #[zbus(property, name = "Type")]
    fn kind(&self) -> fdo::Result<u32> {
        Ok(self.view()?.kind as u32)
    }
    #[zbus(property)]
    fn power_supply(&self) -> fdo::Result<bool> {
        Ok(self.view()?.power_supply)
    }
    #[zbus(property)]
    fn online(&self) -> fdo::Result<bool> {
        Ok(self.view()?.online)
    }
    #[zbus(property)]
    fn energy(&self) -> fdo::Result<f64> {
        Ok(self.view()?.energy)
    }
    #[zbus(property)]
    fn energy_empty(&self) -> fdo::Result<f64> {
        Ok(self.view()?.energy_empty)
    }
    #[zbus(property)]
    fn energy_full(&self) -> fdo::Result<f64> {
        Ok(self.view()?.energy_full)
    }
    #[zbus(property)]
    fn energy_full_design(&self) -> fdo::Result<f64> {
        Ok(self.view()?.energy_full_design)
    }
    #[zbus(property)]
    fn energy_rate(&self) -> fdo::Result<f64> {
        Ok(self.view()?.energy_rate)
    }
    #[zbus(property)]
    fn voltage(&self) -> fdo::Result<f64> {
        Ok(self.view()?.voltage)
    }
    #[zbus(property)]
    fn time_to_empty(&self) -> fdo::Result<i64> {
        Ok(self.view()?.time_to_empty)
    }
    #[zbus(property)]
    fn time_to_full(&self) -> fdo::Result<i64> {
        Ok(self.view()?.time_to_full)
    }
    #[zbus(property)]
    fn percentage(&self) -> fdo::Result<f64> {
        Ok(self.view()?.percentage)
    }
    #[zbus(property)]
    fn is_present(&self) -> fdo::Result<bool> {
        Ok(self.view()?.is_present)
    }
    #[zbus(property)]
    fn state(&self) -> fdo::Result<u32> {
        Ok(self.view()?.state as u32)
    }
    #[zbus(property)]
    fn is_rechargeable(&self) -> fdo::Result<bool> {
        Ok(self.view()?.is_rechargeable)
    }
    #[zbus(property)]
    fn capacity(&self) -> fdo::Result<f64> {
        Ok(self.view()?.capacity)
    }
    #[zbus(property)]
    fn icon_name(&self) -> fdo::Result<String> {
        Ok(self.view()?.icon_name)
    }
    /// The machine's low-battery level (`warning`) on the display device and the power-supply batteries; 1 (none) on the
    /// other devices, whose levels this daemon does not compute.
    #[zbus(property)]
    fn warning_level(&self) -> fdo::Result<u32> {
        let shared = match &self.target {
            Target::Display => true,
            Target::Supply(name) => self
                .shared
                .snapshot()
                .supplies
                .iter()
                .find(|supply| &supply.name == name)
                .map(crate::daemon::shares_warning)
                .ok_or_else(|| {
                    fdo::Error::UnknownObject(format!("the power supply {name} is gone"))
                })?,
        };
        Ok(if shared {
            self.shared.warning_level().upower()
        } else {
            crate::warning::Level::None.upower()
        })
    }
}

/// Records `power.request.failed`: `method` of `interface` was answered with the D-Bus error `error`; `backlight` names
/// the keyboard backlight a call was for.
fn request_failed(
    interface: &'static str,
    method: &'static str,
    error: &str,
    backlight: Option<&str>,
) {
    let mut event = diag::event(
        Severity::Warn,
        "power.request.failed",
        "a request to the power service failed",
    )
    .field("interface", Field::Public(interface))
    .field("method", Field::Public(method))
    .field("error", Field::Public(error));
    if let Some(backlight) = backlight {
        event = event.field("backlight", Field::Public(backlight));
    }
    event.emit();
}

/// Why a caller was not served, before the call reached the hardware.
enum Refusal {
    Denied(String),
    Failed(String),
}

/// Maps the caller's bus name to its process through `GetConnectionCredentials` and asks the
/// authorizer. The pid comes from the bus daemon, not from the message, so the caller cannot
/// choose it.
async fn authorize(
    shared: &Shared,
    header: &Header<'_>,
    connection: &Connection,
) -> Result<(), Refusal> {
    let failed =
        |what: &str, error: &dyn std::fmt::Display| Refusal::Failed(format!("{what}: {error}"));
    let sender = header
        .sender()
        .ok_or_else(|| Refusal::Failed("the call has no sender".to_owned()))?;
    let bus = fdo::DBusProxy::new(connection)
        .await
        .map_err(|e| failed("connecting to the bus daemon", &e))?;
    let credentials = bus
        .get_connection_credentials(BusName::from(sender.clone()))
        .await
        .map_err(|e| failed("reading the caller's credentials", &e))?;
    let pid = credentials
        .process_id()
        .ok_or_else(|| Refusal::Failed("the bus did not report the caller's process".to_owned()))?;
    match shared.authorizer().authorize(pid) {
        Ok(Decision::Allowed) => Ok(()),
        Ok(Decision::Denied(reason)) => Err(Refusal::Denied(reason)),
        Err(error) => Err(failed(
            "asking whether the caller may set the brightness",
            &error,
        )),
    }
}

/// The errors of `org.freedesktop.UPower.KbdBacklight`; the interface documents one name.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.UPower")]
pub enum UPowerError {
    #[zbus(error)]
    ZBus(zbus::Error),
    GeneralError(String),
}

/// `org.freedesktop.UPower.KbdBacklight`, at [`KEYBOARD_BACKLIGHT_PATH`] (`name` is `None`, the
/// first keyboard backlight) and at `KbdBacklight/<name>`.
pub struct KeyboardBacklightObject {
    pub shared: Arc<Shared>,
    pub name: Option<String>,
}

impl KeyboardBacklightObject {
    fn keyboard(&self) -> Result<KeyboardBacklight, UPowerError> {
        let snapshot = self.shared.snapshot();
        let found = match &self.name {
            Some(name) => snapshot
                .keyboards
                .iter()
                .find(|keyboard| &keyboard.name == name),
            None => snapshot.keyboards.first(),
        };
        found
            .cloned()
            .ok_or_else(|| UPowerError::GeneralError("there is no keyboard backlight".to_owned()))
    }
}

fn to_i32(value: u32) -> Result<i32, UPowerError> {
    i32::try_from(value).map_err(|_| {
        UPowerError::GeneralError(format!("brightness {value} does not fit the interface"))
    })
}

#[zbus::interface(name = "org.freedesktop.UPower.KbdBacklight")]
impl KeyboardBacklightObject {
    fn get_max_brightness(&self) -> Result<i32, UPowerError> {
        to_i32(self.keyboard()?.max_brightness)
    }

    fn get_brightness(&self) -> Result<i32, UPowerError> {
        to_i32(self.keyboard()?.brightness)
    }

    async fn set_brightness(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        value: i32,
    ) -> Result<(), UPowerError> {
        let result = self.set(&header, connection, value).await;
        if let Err(error) = &result {
            request_failed(
                "org.freedesktop.UPower.KbdBacklight",
                "SetBrightness",
                zbus::DBusError::name(error).as_str(),
                self.name.as_deref(),
            );
        }
        result
    }

    #[zbus(signal)]
    pub async fn brightness_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        value: i32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    pub async fn brightness_changed_with_source(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        value: i32,
        source: &str,
    ) -> zbus::Result<()>;

    #[zbus(property)]
    fn native_path(&self) -> fdo::Result<String> {
        let keyboard = self
            .keyboard()
            .map_err(|error| fdo::Error::Failed(error.to_string()))?;
        Ok(
            crate::sysfs::led_dir(self.shared.sysfs_root(), &keyboard.name)
                .display()
                .to_string(),
        )
    }
}

impl KeyboardBacklightObject {
    /// `SetBrightness`: authorises the caller, then writes the level.
    async fn set(
        &self,
        header: &Header<'_>,
        connection: &Connection,
        value: i32,
    ) -> Result<(), UPowerError> {
        let keyboard = self.keyboard()?;
        authorize(&self.shared, header, connection)
            .await
            .map_err(|refusal| match refusal {
                Refusal::Denied(reason) => {
                    UPowerError::GeneralError(format!("not authorised: {reason}"))
                }
                Refusal::Failed(reason) => UPowerError::GeneralError(reason),
            })?;
        let value = u32::try_from(value)
            .map_err(|_| UPowerError::GeneralError(format!("brightness {value} is negative")))?;
        let level = self
            .shared
            .set_keyboard(&keyboard.name, value)
            .map_err(|error| UPowerError::GeneralError(error.to_string()))?;
        announce_keyboard(connection, &self.shared, &keyboard.name, level, "external")
            .await
            .map_err(|error| UPowerError::GeneralError(error.to_string()))
    }
}

/// The errors of `org.cupertino.Power1`.
#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Power1")]
pub enum PowerError {
    #[zbus(error)]
    ZBus(zbus::Error),
    /// The machine has no display backlight.
    NoBacklight(String),
    /// The brightness is above the maximum.
    OutOfRange(String),
    /// The caller is not in a session that is active on a seat.
    NotAuthorized(String),
    /// Reading or writing sysfs failed, or the caller could not be checked.
    Failed(String),
}

/// `org.cupertino.Power1`: the display backlight. Brightness is in the units of the backlight
/// device (`0..=GetMaxBrightness`), not a percentage.
pub struct PowerObject(pub Arc<Shared>);

#[zbus::interface(name = "org.cupertino.Power1")]
impl PowerObject {
    fn get_brightness(&self) -> Result<u32, PowerError> {
        self.0
            .snapshot()
            .backlight()
            .map(|backlight| backlight.brightness)
            .ok_or_else(no_backlight)
    }

    fn get_max_brightness(&self) -> Result<u32, PowerError> {
        self.0
            .snapshot()
            .backlight()
            .map(|backlight| backlight.max_brightness)
            .ok_or_else(no_backlight)
    }

    async fn set_brightness(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
        value: u32,
    ) -> Result<(), PowerError> {
        let result = async {
            authorize(&self.0, &header, connection)
                .await
                .map_err(|refusal| match refusal {
                    Refusal::Denied(reason) => PowerError::NotAuthorized(reason),
                    Refusal::Failed(reason) => PowerError::Failed(reason),
                })?;
            let level = self.0.set_display(value).map_err(|error| match error {
                SetError::NoDevice => no_backlight(),
                SetError::OutOfRange { .. } => PowerError::OutOfRange(error.to_string()),
                SetError::Io(_) => PowerError::Failed(error.to_string()),
            })?;
            announce_display(connection, level)
                .await
                .map_err(|error| PowerError::Failed(error.to_string()))
        }
        .await;
        if let Err(error) = &result {
            request_failed(
                POWER_NAME,
                "SetBrightness",
                zbus::DBusError::name(error).as_str(),
                None,
            );
        }
        result
    }

    /// Emitted when the display brightness changes, whoever changed it.
    #[zbus(signal)]
    pub async fn brightness_changed(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        brightness: u32,
    ) -> zbus::Result<()>;
}

fn no_backlight() -> PowerError {
    PowerError::NoBacklight("this machine has no display backlight".to_owned())
}
