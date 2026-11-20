//! What the agent asks of the system: logind (the session's ID, whether the machine can sleep, restart and shut down,
//! the block inhibitor on the power key, sleeping without the session manager), the session manager's
//! `org.cupertino.LoginManager1` (whether it is there, sleeping through it), the power daemon's display device and the
//! notification service. Every call is awaited on the component's loop; none blocks it.

use std::collections::HashMap;

use futures_util::stream::{self, StreamExt};
use zbus::fdo::{DBusProxy, PropertiesProxy};
use zbus::names::InterfaceName;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{OwnedFd, OwnedObjectPath, OwnedValue, Value};
use zbus::Connection;

use crate::capabilities::{Answer, Inputs};
use crate::low_battery::{Level, Notice, APP_NAME};

const LOGIND: &str = "org.freedesktop.login1";
const LOGIND_PATH: &str = "/org/freedesktop/login1";
const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
const LOGIND_SESSION: &str = "org.freedesktop.login1.Session";

const MANAGER: &str = "org.cupertino.LoginManager1";
const MANAGER_PATH: &str = "/org/cupertino/LoginManager1";
/// The prefix of the names of the errors the session manager answers with: each is a refusal, not a fault.
const MANAGER_ERROR_PREFIX: &str = "org.cupertino.LoginManager1.Error.";

const UPOWER: &str = "org.freedesktop.UPower";
const DISPLAY_DEVICE: &str = "/org/freedesktop/UPower/devices/DisplayDevice";
const DEVICE_INTERFACE: &str = "org.freedesktop.UPower.Device";

const NOTIFICATIONS: &str = "org.freedesktop.Notifications";
const NOTIFICATIONS_PATH: &str = "/org/freedesktop/Notifications";

/// What is asked of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Power {
    Restart,
    ShutDown,
    Sleep,
}

impl Power {
    /// The word `RequestPower` of the session manager takes.
    fn word(self) -> &'static str {
        match self {
            Power::Restart => "restart",
            Power::ShutDown => "shut-down",
            Power::Sleep => "sleep",
        }
    }
}

async fn call<B, T>(
    connection: &Connection,
    destination: &str,
    path: &str,
    interface: &str,
    method: &str,
    body: &B,
) -> zbus::Result<T>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    T: for<'d> zbus::zvariant::DynamicDeserialize<'d>,
{
    connection
        .call_method(Some(destination), path, Some(interface), method, body)
        .await?
        .body()
        .deserialize()
}

/// The login session this process belongs to: `XDG_SESSION_ID` when the login set it, else logind's session of this
/// process. `None` when there is none (a container, a test bus without logind): login items and restore then wait for
/// a session they can be told apart in, and never run.
pub async fn session_id(system: &Connection) -> Option<String> {
    if let Some(id) = std::env::var("XDG_SESSION_ID")
        .ok()
        .filter(|id| !id.is_empty())
    {
        return Some(id);
    }
    let path: OwnedObjectPath = match call(
        system,
        LOGIND,
        LOGIND_PATH,
        LOGIND_MANAGER,
        "GetSessionByPID",
        &(std::process::id(),),
    )
    .await
    {
        Ok(path) => path,
        Err(e) => {
            eprintln!("cupertino-shell-agent: no login session: {e}");
            return None;
        }
    };
    let id: zbus::Result<OwnedValue> = call(
        system,
        LOGIND,
        path.as_str(),
        "org.freedesktop.DBus.Properties",
        "Get",
        &(LOGIND_SESSION, "Id"),
    )
    .await;
    match id.and_then(|v| String::try_from(v).map_err(zbus::Error::from)) {
        Ok(id) => Some(id),
        Err(e) => {
            eprintln!("cupertino-shell-agent: the login session has no ID: {e}");
            None
        }
    }
}

/// Whether the machine can `action` and whether the person may (`CanSuspend`, `CanReboot`, `CanPowerOff`); an
/// answer that does not come is "not available".
async fn can(system: &Connection, action: Power) -> Answer {
    let method = match action {
        Power::Sleep => "CanSuspend",
        Power::Restart => "CanReboot",
        Power::ShutDown => "CanPowerOff",
    };
    match call::<_, String>(system, LOGIND, LOGIND_PATH, LOGIND_MANAGER, method, &()).await {
        Ok(text) => Answer::from_logind(&text),
        Err(e) => {
            eprintln!("cupertino-shell-agent: logind's {method}: {e}");
            Answer::NotAvailable
        }
    }
}

/// Whether the session manager owns its name now.
async fn manager_present(system: &Connection) -> bool {
    match DBusProxy::new(system).await {
        Ok(bus) => bus
            .name_has_owner(MANAGER.try_into().expect("a well-formed bus name"))
            .await
            .unwrap_or(false),
        Err(_) => false,
    }
}

/// What the session menu rests on, read now.
pub async fn inputs(system: &Connection) -> Inputs {
    let (can_suspend, can_reboot, can_power_off, session_manager) = futures_util::join!(
        can(system, Power::Sleep),
        can(system, Power::Restart),
        can(system, Power::ShutDown),
        manager_present(system),
    );
    Inputs {
        can_suspend,
        can_reboot,
        can_power_off,
        session_manager,
    }
}

/// Call `notify` whenever the session manager's name changes owner (it started, restarted or left), until the stream
/// ends; returns why. The change counts from the bus alone.
pub async fn follow_manager(system: &Connection, notify: impl Fn()) -> String {
    let bus = match DBusProxy::new(system).await {
        Ok(bus) => bus,
        Err(e) => return format!("reaching the bus: {e}"),
    };
    let mut owners = match bus
        .receive_name_owner_changed_with_args(&[(0, MANAGER)])
        .await
    {
        Ok(owners) => owners,
        Err(e) => return format!("following the session manager's name: {e}"),
    };
    while owners.next().await.is_some() {
        notify();
    }
    "the bus's signal stream ended".into()
}

/// Keep logind from powering off on the power key until the returned descriptor is dropped; the key then reaches the
/// compositor, which asks the agent ([`crate::control`]).
pub async fn block_power_key(system: &Connection) -> zbus::Result<OwnedFd> {
    call(
        system,
        LOGIND,
        LOGIND_PATH,
        LOGIND_MANAGER,
        "Inhibit",
        &(
            "handle-power-key",
            "Cupertino",
            "Ask before shutting down",
            "block",
        ),
    )
    .await
}

/// Why a power request did not happen.
#[derive(Clone, Debug)]
pub enum PowerError {
    /// It was refused (another user's session is open, an inhibitor, the person may not): an outcome, not a fault.
    Refused(String),
    /// The call failed.
    Failed(String),
}

/// Sleep, restart or shut down through the session manager or, when there is none (a session another display manager
/// started), through logind, without an authentication dialog.
pub async fn request_power(system: &Connection, action: Power) -> Result<(), PowerError> {
    let asked: zbus::Result<()> = call(
        system,
        MANAGER,
        MANAGER_PATH,
        MANAGER,
        "RequestPower",
        &(action.word(),),
    )
    .await;
    let absent = match &asked {
        Ok(()) => return Ok(()),
        Err(zbus::Error::MethodError(name, _, _))
            if name.as_str().starts_with(MANAGER_ERROR_PREFIX) =>
        {
            return Err(PowerError::Refused(asked.unwrap_err().to_string()))
        }
        Err(zbus::Error::MethodError(name, _, _)) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
        ),
        Err(_) => false,
    };
    if !absent {
        return Err(PowerError::Failed(format!(
            "asking the session manager: {}",
            asked.unwrap_err()
        )));
    }
    eprintln!("cupertino-shell-agent: no session manager: asking logind to {action:?}");
    let method = match action {
        Power::Sleep => "Suspend",
        Power::Restart => "Reboot",
        Power::ShutDown => "PowerOff",
    };
    match call::<_, ()>(
        system,
        LOGIND,
        LOGIND_PATH,
        LOGIND_MANAGER,
        method,
        &(false,),
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(e @ zbus::Error::MethodError(..)) => Err(PowerError::Refused(e.to_string())),
        Err(e) => Err(PowerError::Failed(format!("asking logind: {e}"))),
    }
}

/// One report of the power daemon's display device.
#[derive(Clone, Debug, PartialEq)]
pub enum Battery {
    Reading {
        level: Level,
        percentage: f64,
    },
    /// The power daemon is not on the bus (or left it): there is nothing to warn about until it comes back.
    NoDaemon(String),
}

async fn read_battery(device: &PropertiesProxy<'_>) -> Battery {
    let interface = InterfaceName::from_static_str_unchecked(DEVICE_INTERFACE);
    let level = device
        .get(interface.clone(), "WarningLevel")
        .await
        .map_err(zbus::Error::from)
        .and_then(|v| u32::try_from(v).map_err(zbus::Error::from));
    let percentage = device
        .get(interface, "Percentage")
        .await
        .map_err(zbus::Error::from)
        .and_then(|v| f64::try_from(v).map_err(zbus::Error::from));
    match (level, percentage) {
        (Ok(level), Ok(percentage)) => Battery::Reading {
            level: Level::from_upower(level),
            percentage,
        },
        (Err(e), _) | (_, Err(e)) => Battery::NoDaemon(format!("reading the display device: {e}")),
    }
}

enum BatteryNews {
    Changed(bool),
    Owner(bool),
}

/// Read the display device now and whenever its warning level or charge changes or the power daemon comes back, and
/// hand each reading to `notify`, until a stream ends; returns why. A change counts from the daemon's owner alone,
/// and its leaving from the bus alone (the proxies follow the owners).
pub async fn follow_battery(system: &Connection, notify: impl Fn(Battery)) -> String {
    let device = match PropertiesProxy::builder(system)
        .destination(UPOWER)
        .and_then(|b| b.path(DISPLAY_DEVICE))
    {
        Ok(builder) => builder.cache_properties(CacheProperties::No).build().await,
        Err(e) => Err(e),
    };
    let device = match device {
        Ok(device) => device,
        Err(e) => return format!("reaching the power daemon: {e}"),
    };
    let changes = match device.receive_properties_changed().await {
        Ok(changes) => changes.map(|signal| {
            BatteryNews::Changed(signal.args().is_ok_and(|args| {
                args.interface_name.as_str() == DEVICE_INTERFACE
                    && (args.changed_properties.contains_key("WarningLevel")
                        || args.changed_properties.contains_key("Percentage"))
            }))
        }),
        Err(e) => return format!("subscribing to the power daemon's changes: {e}"),
    };
    let owners = match DBusProxy::new(system).await {
        Ok(bus) => match bus
            .receive_name_owner_changed_with_args(&[(0, UPOWER)])
            .await
        {
            Ok(owners) => owners.map(|signal| {
                BatteryNews::Owner(signal.args().is_ok_and(|args| args.new_owner.is_some()))
            }),
            Err(e) => return format!("following the power daemon's name: {e}"),
        },
        Err(e) => return format!("reaching the bus: {e}"),
    };
    notify(read_battery(&device).await);
    let mut news = stream::select(changes, owners);
    while let Some(news) = news.next().await {
        match news {
            // a change of either is reported with both, read again so they belong together
            BatteryNews::Changed(true) | BatteryNews::Owner(true) => {
                notify(read_battery(&device).await)
            }
            BatteryNews::Changed(false) => {}
            BatteryNews::Owner(false) => {
                notify(Battery::NoDaemon(format!("{UPOWER} left the bus")))
            }
        }
    }
    "the power daemon's signal streams ended".into()
}

/// `Notify` on the session bus; returns the notification's id.
pub async fn post_notice(
    session: &Connection,
    replaces: u32,
    notice: &Notice,
) -> zbus::Result<u32> {
    let hints = HashMap::from([("urgency", Value::U8(notice.urgency))]);
    let actions: Vec<&str> = Vec::new();
    // -1: the notification service decides how long the banner stays
    call(
        session,
        NOTIFICATIONS,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS,
        "Notify",
        &(
            APP_NAME,
            replaces,
            notice.icon,
            notice.summary.as_str(),
            notice.body.as_str(),
            actions,
            hints,
            -1i32,
        ),
    )
    .await
}

/// `CloseNotification` on the session bus.
pub async fn close_notice(session: &Connection, id: u32) -> zbus::Result<()> {
    call(
        session,
        NOTIFICATIONS,
        NOTIFICATIONS_PATH,
        NOTIFICATIONS,
        "CloseNotification",
        &(id,),
    )
    .await
}
