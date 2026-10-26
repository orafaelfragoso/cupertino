//! The display and keyboard backlights through the power daemon on the system bus: `org.cupertino.Power1` for the
//! display and `org.freedesktop.UPower.KbdBacklight` for the keyboard. The bus connection is made on the first use and
//! again after a failure, so a daemon that starts later is picked up.

use zbus::blocking::{Connection, Proxy};

use crate::backend::{linear_raw, Backlight, BackendError};
use crate::sender::DISPLAY_FLOOR_RAW;

const POWER_NAME: &str = "org.cupertino.Power1";
const POWER_PATH: &str = "/org/cupertino/Power1";
const POWER_INTERFACE: &str = "org.cupertino.Power1";
const UPOWER_NAME: &str = "org.freedesktop.UPower";
const KEYBOARD_PATH: &str = "/org/freedesktop/UPower/KbdBacklight";
const KEYBOARD_INTERFACE: &str = "org.freedesktop.UPower.KbdBacklight";

/// The system bus connection, made when first needed.
struct SystemBus {
    connection: Option<Connection>,
}

impl SystemBus {
    fn proxy(&mut self, name: &'static str, path: &'static str, interface: &'static str) -> zbus::Result<Proxy<'static>> {
        if self.connection.is_none() {
            self.connection = Some(Connection::system()?);
        }
        Proxy::new(self.connection.as_ref().expect("connected above"), name, path, interface)
    }

    /// Forget the connection after a failure so the next call connects again.
    fn failed<T>(&mut self, result: zbus::Result<T>) -> Result<T, BackendError> {
        if result.is_err() {
            self.connection = None;
        }
        Ok(result?)
    }
}

/// The raw value of a backlight and its maximum. The inner error is a daemon refusing the numbers, which is not a bus
/// failure: the connection is kept.
type RawRead = zbus::Result<Result<(u32, u32), BackendError>>;

/// The display brightness, in the daemon's raw units, mapped linearly onto 0 to 1 (no fixed curve is known,
/// ). A level never reaches below raw [`DISPLAY_FLOOR_RAW`].
pub struct DisplayBacklight {
    bus: SystemBus,
}

impl DisplayBacklight {
    /// A display backlight; nothing is contacted until the first press.
    pub fn new() -> Self {
        Self { bus: SystemBus { connection: None } }
    }

    fn read_raw(proxy: &Proxy<'_>) -> RawRead {
        let current: u32 = proxy.call("GetBrightness", &())?;
        let max: u32 = proxy.call("GetMaxBrightness", &())?;
        if max == 0 {
            return Ok(Err("the display backlight reports a maximum of 0".into()));
        }
        Ok(Ok((current, max)))
    }

    fn level_raw(proxy: &Proxy<'_>) -> zbus::Result<Result<f64, BackendError>> {
        Ok(Self::read_raw(proxy)?.map(|(current, max)| f64::from(current) / f64::from(max)))
    }

    fn set_raw(proxy: &Proxy<'_>, level: f64) -> zbus::Result<Result<f64, BackendError>> {
        let (current, max) = match Self::read_raw(proxy)? {
            Ok(raw) => raw,
            Err(e) => return Ok(Err(e)),
        };
        let target = linear_raw(current, max, level, DISPLAY_FLOOR_RAW.min(max));
        if target != current {
            proxy.call::<_, _, ()>("SetBrightness", &(target,))?;
        }
        Ok(Ok(f64::from(target) / f64::from(max)))
    }
}

impl Default for DisplayBacklight {
    fn default() -> Self {
        Self::new()
    }
}

impl Backlight for DisplayBacklight {
    fn level(&mut self) -> Result<f64, BackendError> {
        let result = self.bus.proxy(POWER_NAME, POWER_PATH, POWER_INTERFACE).and_then(|proxy| Self::level_raw(&proxy));
        self.bus.failed(result)?
    }

    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        let result = self.bus.proxy(POWER_NAME, POWER_PATH, POWER_INTERFACE).and_then(|proxy| Self::set_raw(&proxy, level));
        self.bus.failed(result)?
    }
}

/// The keyboard backlight, in the daemon's raw units, mapped linearly onto 0 to 1; it reaches 0.
pub struct KeyboardBacklight {
    bus: SystemBus,
}

impl KeyboardBacklight {
    /// A keyboard backlight; nothing is contacted until the first press.
    pub fn new() -> Self {
        Self { bus: SystemBus { connection: None } }
    }

    fn read_raw(proxy: &Proxy<'_>) -> RawRead {
        let current: i32 = proxy.call("GetBrightness", &())?;
        let max: i32 = proxy.call("GetMaxBrightness", &())?;
        let (Ok(current), Ok(max)) = (u32::try_from(current), u32::try_from(max)) else {
            return Ok(Err(format!("the keyboard backlight reports {current} of {max}").into()));
        };
        if max == 0 {
            return Ok(Err("the keyboard backlight reports a maximum of 0".into()));
        }
        Ok(Ok((current, max)))
    }

    fn level_raw(proxy: &Proxy<'_>) -> zbus::Result<Result<f64, BackendError>> {
        Ok(Self::read_raw(proxy)?.map(|(current, max)| f64::from(current) / f64::from(max)))
    }

    fn set_raw(proxy: &Proxy<'_>, level: f64) -> zbus::Result<Result<f64, BackendError>> {
        let (current, max) = match Self::read_raw(proxy)? {
            Ok(raw) => raw,
            Err(e) => return Ok(Err(e)),
        };
        let target = linear_raw(current, max, level, 0);
        if target != current {
            proxy.call::<_, _, ()>("SetBrightness", &(target as i32,))?;
        }
        Ok(Ok(f64::from(target) / f64::from(max)))
    }
}

impl Default for KeyboardBacklight {
    fn default() -> Self {
        Self::new()
    }
}

impl Backlight for KeyboardBacklight {
    fn level(&mut self) -> Result<f64, BackendError> {
        let result = self.bus.proxy(UPOWER_NAME, KEYBOARD_PATH, KEYBOARD_INTERFACE).and_then(|proxy| Self::level_raw(&proxy));
        self.bus.failed(result)?
    }

    fn set_level(&mut self, level: f64) -> Result<f64, BackendError> {
        let result = self.bus.proxy(UPOWER_NAME, KEYBOARD_PATH, KEYBOARD_INTERFACE).and_then(|proxy| Self::set_raw(&proxy, level));
        self.bus.failed(result)?
    }
}
