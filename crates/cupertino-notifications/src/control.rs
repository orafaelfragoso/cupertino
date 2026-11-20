//! The component's bus interfaces, served on its own session bus connection, which owns their names (D20):
//! `org.freedesktop.Notifications` (version 1.2 of the specification, plus the `ActivationToken` signal of 1.3)
//! and `org.cupertino.NotificationCenter1`, through which the menu bar opens the list, the control panel switches do
//! not disturb and the lock screen reads what it may show.
//!
//! The handlers run on the component's UI thread, which ticks the connection, and do no work of their own there:
//! each call goes to the model as a [`Call`], with where its answer goes. The model names the caller from the
//! credentials the bus gave for it, off the UI thread, since that reads the caller's `/proc` entries.
//!
//! Replacing a notification (`Notify` with a `replaces_id`) and `CloseNotification` are checked against who sent
//! it ([`may_change`]): an unsandboxed process of the person may change any notification (it can act as the person
//! already, and `notify-send --replace-id` starts a new process for each call, so no narrower caller can be told
//! apart), a sandboxed application only the ones its own sandbox instance sent, and a caller the bus cannot name none.
//! A refused replacement is a new notification and a refused close does nothing, as for an id that does not exist, so
//! a sandboxed application learns nothing of another's ids.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use cupertino_peer::{own_parent, same_instance, Caller, CallerError, Identified, Relation};
use zbus::fdo::{ConnectionCredentials, RequestNameFlags, RequestNameReply};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};
use zbus::Connection;

use crate::center::{Event, LockItem};
use crate::focus::{self, Lifetime, LocalCalendar, SystemCalendar};
use crate::notification::{Expire, Hints, ImageData, NotifyInput, Urgency};

/// The `/proc` the callers are read from.
const PROC: &str = "/proc";

pub const BUS_NAME: &str = "org.freedesktop.Notifications";
pub const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
pub const SPEC_VERSION: &str = "1.2";

/// Name, path and interface of the control interface of the notification list.
pub const CENTRE_BUS_NAME: &str = "org.cupertino.NotificationCenter1";
pub const CENTRE_OBJECT_PATH: &str = "/org/cupertino/NotificationCenter1";

/// What `GetServerInformation` answers.
#[derive(Clone, Debug)]
pub struct ServerInfo {
    pub name: String,
    pub vendor: String,
    pub version: String,
}

impl Default for ServerInfo {
    fn default() -> Self {
        Self {
            name: "cupertino-notifications".into(),
            vendor: "Cupertino".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }
}

/// Who may open the list, switch do not disturb or read the lock screen's list: any unsandboxed process of the person
/// (the menu bar, the control panel, the lock screen and the settings app are not this process's parent); a Flatpak,
/// a Snap or a caller the bus cannot name may not.
const SHELL_CALLERS: &[Relation] = &[Relation::Itself, Relation::Parent, Relation::Other];

/// Whether `caller` may replace or close a notification that `origin` sent (`None`: an unsandboxed process, or one the
/// bus could not name). An unsandboxed caller may change any; a sandboxed one only what its own sandbox instance sent
/// ([`same_instance`], under `proc_root`); an anonymous or unclassified one none. It reads `proc_root`.
pub fn may_change(caller: &Caller, origin: Option<&Identified>, proc_root: &Path) -> bool {
    match caller {
        Caller::Process(caller) if caller.sandbox.is_none() => true,
        Caller::Process(caller) => {
            origin.is_some_and(|origin| same_instance(caller, origin, proc_root))
        }
        Caller::Anonymous | Caller::Unclassified(_) => false,
    }
}

/// The sandboxed process a notification came from, when it came from one: only it may change the notification
/// besides the person's unsandboxed processes.
pub fn sandboxed(caller: &Caller) -> Option<&Identified> {
    match caller {
        Caller::Process(process) if process.sandbox.is_some() => Some(process),
        _ => None,
    }
}

/// The caller of `Notify` and the notification it asks for, with its hints decoded: a replacement the caller may not
/// make is a new notification, and a notification without an icon of its own shows its desktop entry's. `origin` is
/// the sandboxed sender of the notification it would replace. For the I/O pool: it reads `/proc` and a desktop entry.
pub fn prepare_notify(
    credentials: Option<&ConnectionCredentials>,
    arrival: Arrival,
    origin: Option<&Caller>,
) -> (Caller, NotifyInput) {
    let caller = judge(credentials);
    let replaces_id = if arrival.replaces_id != 0
        && !may_change(&caller, origin.and_then(sandboxed), Path::new(PROC))
    {
        0
    } else {
        arrival.replaces_id
    };
    let hints = parse_hints(&arrival.hints);
    let icon = match (arrival.app_icon.as_str(), &hints.desktop_entry) {
        ("", Some(entry)) => entry_icon(entry).unwrap_or_default(),
        _ => arrival.app_icon.clone(),
    };
    let input = arrival.input(replaces_id, icon, hints);
    (caller, input)
}

/// Whether the caller of `credentials` may close a notification `origin` sent. For the I/O pool: it reads `/proc`.
pub fn may_close(credentials: Option<&ConnectionCredentials>, origin: Option<&Caller>) -> bool {
    may_change(
        &judge(credentials),
        origin.and_then(sandboxed),
        Path::new(PROC),
    )
}

/// The icon of the desktop entry `id` (with or without `.desktop`), when one is installed. For the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn entry_icon(id: &str) -> Option<String> {
    use cupertino_apps::entry::{parse_handler, xdg_application_dirs, Locale, Visibility};
    let id = id.strip_suffix(".desktop").unwrap_or(id);
    if id.is_empty() || id.contains('/') {
        return None;
    }
    let text = xdg_application_dirs()
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok())?;
    parse_handler(id, &text, &Locale::from_env(), &Visibility::from_env())
        .ok()?
        .entry
        .icon
}

fn int_of(v: &Value<'_>) -> Option<i64> {
    Some(match v {
        Value::U8(x) => *x as i64,
        Value::I16(x) => *x as i64,
        Value::U16(x) => *x as i64,
        Value::I32(x) => *x as i64,
        Value::U32(x) => *x as i64,
        Value::I64(x) => *x,
        Value::U64(x) => i64::try_from(*x).ok()?,
        Value::Value(inner) => return int_of(inner),
        _ => return None,
    })
}

fn str_of(v: &Value<'_>) -> Option<String> {
    match v {
        Value::Str(s) => Some(s.to_string()),
        Value::Value(inner) => str_of(inner),
        _ => None,
    }
}

fn bool_of(v: &Value<'_>) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        Value::Value(inner) => bool_of(inner),
        other => int_of(other).map(|i| i != 0),
    }
}

fn image_of(v: &Value<'_>) -> Option<ImageData> {
    let Value::Structure(s) = v else { return None };
    let f = s.fields();
    if f.len() != 7 {
        return None;
    }
    let Value::Array(bytes) = &f[6] else {
        return None;
    };
    let data: Vec<u8> = bytes
        .iter()
        .filter_map(|b| if let Value::U8(b) = b { Some(*b) } else { None })
        .collect();
    ImageData::from_dbus(
        int_of(&f[0])? as i32,
        int_of(&f[1])? as i32,
        int_of(&f[2])? as i32,
        bool_of(&f[3])?,
        int_of(&f[4])? as i32,
        int_of(&f[5])? as i32,
        &data,
    )
}

/// Decode the `hints` dictionary of `Notify`. Unknown hints are ignored; `image-data` takes precedence over the
/// deprecated `image_data` and `icon_data` names.
pub fn parse_hints(map: &HashMap<String, OwnedValue>) -> Hints {
    let get = |k: &str| map.get(k).map(|v| &**v);
    Hints {
        urgency: get("urgency")
            .and_then(int_of)
            .map(|u| Urgency::from_byte(u.clamp(0, 255) as u8)),
        category: get("category").and_then(str_of),
        transient: get("transient").and_then(bool_of).unwrap_or(false),
        resident: get("resident").and_then(bool_of).unwrap_or(false),
        suppress_sound: get("suppress-sound").and_then(bool_of).unwrap_or(false),
        desktop_entry: get("desktop-entry").and_then(str_of),
        image_path: get("image-path")
            .or_else(|| get("image_path"))
            .and_then(str_of),
        image: ["image-data", "image_data", "icon_data"]
            .iter()
            .find_map(|k| get(k).and_then(image_of)),
    }
}

/// The arguments of a `Notify` call as they came, before the caller is named and the hints decoded.
#[derive(Clone, Debug)]
pub struct Arrival {
    pub app_name: String,
    pub replaces_id: u32,
    pub app_icon: String,
    pub summary: String,
    pub body: String,
    pub actions: Vec<String>,
    pub hints: Arc<HashMap<String, OwnedValue>>,
    pub expire_timeout: i32,
}

impl Arrival {
    /// The notification it asks for, with `hints` decoded and `icon` in place of the caller's own icon.
    fn input(self, replaces_id: u32, icon: String, hints: Hints) -> NotifyInput {
        NotifyInput {
            app_name: self.app_name,
            replaces_id,
            icon,
            summary: self.summary,
            body: self.body,
            actions: self.actions,
            expire: Expire::from_dbus(self.expire_timeout),
            hints,
        }
        .capped()
    }
}

/// How do not disturb is to change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DndChange {
    Off,
    /// On until it is turned off.
    On,
    /// On for one of the offered durations; it ends by itself.
    For(Lifetime),
}

/// A call of the interfaces, handed to the model with where its answer goes.
#[derive(Clone, Debug)]
pub enum Call {
    /// Name the caller of the credentials (`None`: the bus gave none) for the call that waits on `reply`.
    Judge {
        credentials: Option<Arc<ConnectionCredentials>>,
        reply: async_channel::Sender<Arc<Caller>>,
    },
    /// Show a notification; its id goes to `reply`.
    Notify {
        credentials: Option<Arc<ConnectionCredentials>>,
        arrival: Arrival,
        reply: async_channel::Sender<u32>,
    },
    /// Close notification `id`, when the caller may.
    Close {
        credentials: Option<Arc<ConnectionCredentials>>,
        id: u32,
    },
    /// Open the list, or close it when it is open.
    Toggle,
    Dnd(DndChange),
    /// The lock screen's list, as `LockScreenNotifications` answers it.
    LockScreen {
        reply: async_channel::Sender<Vec<(u32, String, String, String)>>,
    },
    /// The applications of the notifications, as `Applications` answers them.
    Applications {
        reply: async_channel::Sender<Vec<(String, String)>>,
    },
}

/// What the model's state says on the bus: the properties of `org.cupertino.NotificationCenter1`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Shown {
    /// Do not disturb is in effect.
    pub dnd: bool,
    /// The list is open.
    pub centre: bool,
    /// The connector name of the output the open list is on; empty while it is closed or not known.
    pub output: String,
}

/// What the model sends on the bus, in the order it happened.
#[derive(Clone, Debug)]
pub enum Signal {
    Event(Event),
    DoNotDisturb,
    CentreShown,
    CentreOutput,
    LockScreen,
}

/// The part of the interfaces both share: where calls go, and what the properties say.
#[derive(Clone)]
struct Link {
    calls: async_channel::Sender<Call>,
    shown: Arc<Mutex<Shown>>,
}

impl Link {
    fn send(&self, call: Call) {
        // the model holds the receiver for as long as the interfaces are served
        let _ = self.calls.try_send(call);
    }

    fn shown(&self) -> MutexGuard<'_, Shown> {
        self.shown.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// An answer the model sends to `reply`.
    async fn ask<T>(
        &self,
        call: impl FnOnce(async_channel::Sender<T>) -> Call,
    ) -> zbus::fdo::Result<T> {
        let (reply, answer) = async_channel::bounded(1);
        self.send(call(reply));
        answer
            .recv()
            .await
            .map_err(|_| zbus::fdo::Error::Failed("the notification list went away".into()))
    }

    /// The caller of a call with `header`, named by the model.
    async fn caller(
        &self,
        connection: &Connection,
        header: &zbus::message::Header<'_>,
    ) -> Arc<Caller> {
        let credentials = credentials(connection, header).await;
        match self.ask(|reply| Call::Judge { credentials, reply }).await {
            Ok(caller) => caller,
            Err(_) => Arc::new(Caller::Anonymous),
        }
    }

    /// Refuse the caller of `method` unless it is unsandboxed and stands in one of [`SHELL_CALLERS`] to this process.
    /// A refusal is logged with the method, never with the arguments.
    async fn guard(
        &self,
        connection: &Connection,
        header: &zbus::message::Header<'_>,
        method: &'static str,
    ) -> Result<(), CallerError> {
        let caller = self.caller(connection, header).await;
        caller
            .require_relation(std::process::id(), own_parent(), SHELL_CALLERS)
            .map_err(|denial| {
                eprintln!(
                    "cupertino-notifications: {CENTRE_BUS_NAME}.{method}: the caller is refused: {denial:?}"
                );
                denial.into()
            })
    }
}

/// The credentials the bus gives for the sender of a call with `header`; `None` when it gives none.
async fn credentials(
    connection: &Connection,
    header: &zbus::message::Header<'_>,
) -> Option<Arc<ConnectionCredentials>> {
    let sender = header.sender()?;
    let proxy = zbus::fdo::DBusProxy::new(connection).await.ok()?;
    proxy
        .get_connection_credentials(sender.clone().into())
        .await
        .ok()
        .map(Arc::new)
}

/// The caller of `credentials`, read from its `/proc` entries: for the I/O pool, never the UI thread.
pub fn judge(credentials: Option<&ConnectionCredentials>) -> Caller {
    match credentials {
        Some(credentials) => Caller::from_credentials(credentials, Path::new(PROC)),
        None => Caller::Anonymous,
    }
}

struct Notifications {
    link: Link,
    info: ServerInfo,
    capabilities: Vec<String>,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    fn get_capabilities(&self) -> Vec<String> {
        self.capabilities.clone()
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> zbus::fdo::Result<u32> {
        let credentials = credentials(connection, &header).await;
        let arrival = Arrival {
            app_name,
            replaces_id,
            app_icon,
            summary,
            body,
            actions,
            hints: Arc::new(hints),
            expire_timeout,
        };
        self.link
            .ask(|reply| Call::Notify {
                credentials,
                arrival,
                reply,
            })
            .await
    }

    async fn close_notification(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
        id: u32,
    ) {
        let credentials = credentials(connection, &header).await;
        self.link.send(Call::Close { credentials, id });
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        let i = &self.info;
        (
            i.name.clone(),
            i.vendor.clone(),
            i.version.clone(),
            SPEC_VERSION.to_string(),
        )
    }

    #[zbus(signal)]
    async fn notification_closed(
        emitter: &SignalEmitter<'_>,
        id: u32,
        reason: u32,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn action_invoked(
        emitter: &SignalEmitter<'_>,
        id: u32,
        action_key: String,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn activation_token(
        emitter: &SignalEmitter<'_>,
        id: u32,
        activation_token: String,
    ) -> zbus::Result<()>;
}

/// `org.cupertino.NotificationCenter1`: lets the menu bar open and close the list, the control panel switch do not
/// disturb and the lock screen read what it shows.
struct CentreControl {
    link: Link,
}

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl CentreControl {
    /// Open the notification list, or close it when it is open.
    async fn toggle_centre(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.link.guard(connection, &header, "ToggleCentre").await?;
        self.link.send(Call::Toggle);
        Ok(())
    }

    /// Turn do not disturb on (until turned off) or off; banners of suppressed notifications are not shown.
    async fn set_do_not_disturb(
        &self,
        on: bool,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.link
            .guard(connection, &header, "SetDoNotDisturb")
            .await?;
        self.link
            .send(Call::Dnd(if on { DndChange::On } else { DndChange::Off }));
        Ok(())
    }

    /// Turn do not disturb on for one of the [`Self::focus_lifetimes`] rows, by its identifier; it ends by itself.
    async fn set_do_not_disturb_for(
        &self,
        lifetime: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        self.link
            .guard(connection, &header, "SetDoNotDisturbFor")
            .await?;
        let lifetime = Lifetime::from_id(lifetime).ok_or_else(|| {
            zbus::fdo::Error::InvalidArgs(format!("{lifetime:?} is not an offered duration"))
        })?;
        self.link.send(Call::Dnd(DndChange::For(lifetime)));
        Ok(())
    }

    /// The duration rows offered now, in menu order: identifier (the argument of `SetDoNotDisturbFor`) and title.
    #[zbus(property)]
    fn focus_lifetimes(&self) -> zbus::fdo::Result<Vec<(String, String)>> {
        Ok(lifetimes(unix_now()?))
    }

    /// Whether do not disturb is in effect now.
    #[zbus(property)]
    fn do_not_disturb(&self) -> bool {
        self.link.shown().dnd
    }

    /// Whether the list is open, so the menu bar highlights the clock that opens it.
    #[zbus(property)]
    fn centre_shown(&self) -> bool {
        self.link.shown().centre
    }

    /// The connector name of the output the open list is on; empty while it is closed or not known.
    #[zbus(property)]
    fn centre_output(&self) -> String {
        self.link.shown().output.clone()
    }

    /// What the lock screen shows while the session is locked, newest first: id, application, title and body of each
    /// notification that arrived since the lock from an application allowed on the lock screen. The title is
    /// "Notification" and the body empty unless the application's previews are always shown, so nothing else of a
    /// notification leaves this process. Empty while the session is unlocked. Same caller check as
    /// `SetDoNotDisturb`.
    async fn lock_screen_notifications(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<Vec<(u32, String, String, String)>, CallerError> {
        self.link
            .guard(connection, &header, "LockScreenNotifications")
            .await?;
        Ok(self.link.ask(|reply| Call::LockScreen { reply }).await?)
    }

    /// The applications of the notifications on screen and in the list: the id their settings are kept under
    /// (desktop entry, else name) and their name. Same caller check as `SetDoNotDisturb`.
    async fn applications(
        &self,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<Vec<(String, String)>, CallerError> {
        self.link.guard(connection, &header, "Applications").await?;
        Ok(self.link.ask(|reply| Call::Applications { reply }).await?)
    }

    /// The list of [`Self::lock_screen_notifications`] changed. It carries nothing: a listener asks for the list.
    #[zbus(signal)]
    async fn lock_screen_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// The lock screen's list as `LockScreenNotifications` returns it.
pub fn lock_rows(items: &[LockItem]) -> Vec<(u32, String, String, String)> {
    items
        .iter()
        .map(|i| (i.id, i.app.clone(), i.title.clone(), i.body.clone()))
        .collect()
}

/// The do-not-disturb durations offered at `unix`, in menu order: identifier and title.
pub fn lifetimes(unix: i64) -> Vec<(String, String)> {
    let hour = SystemCalendar.hour(unix);
    focus::offered(hour)
        .iter()
        .map(|l| (l.id().to_owned(), l.title().to_owned()))
        .collect()
}

/// Seconds since the Unix epoch on the wall clock.
pub fn unix_now() -> zbus::fdo::Result<i64> {
    let since = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|e| zbus::fdo::Error::Failed(format!("the clock is before 1970: {e}")))?;
    Ok(since.as_secs() as i64)
}

/// Why a name is not served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ServeError {
    /// Another program owns the name.
    Taken(&'static str),
    /// The bus failed, as this says.
    Bus(String),
}

impl std::fmt::Display for ServeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ServeError::Taken(name) => write!(f, "another program owns {name}"),
            ServeError::Bus(e) => write!(f, "{e}"),
        }
    }
}

impl From<zbus::Error> for ServeError {
    fn from(e: zbus::Error) -> Self {
        ServeError::Bus(e.to_string())
    }
}

/// Claim `name` on `connection`, without queueing behind another owner; a later program may take it over.
async fn claim(connection: &Connection, name: &'static str) -> Result<(), ServeError> {
    let flags = RequestNameFlags::DoNotQueue | RequestNameFlags::AllowReplacement;
    match connection.request_name_with_flags(name, flags).await? {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(()),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(ServeError::Taken(name)),
    }
}

/// How serving went: `org.freedesktop.Notifications`, then `org.cupertino.NotificationCenter1` once the first is
/// served.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Served {
    pub notifications: Result<(), ServeError>,
    pub centre: Option<Result<(), ServeError>>,
}

/// Exports both interfaces on `connection` and claims their names, each once its object exists: every call goes to
/// `calls`, and the properties say what `shown` holds. `org.freedesktop.Notifications` first: without it there is
/// nothing to serve, and another notification daemon keeps it.
pub async fn serve(
    connection: &Connection,
    calls: async_channel::Sender<Call>,
    shown: Arc<Mutex<Shown>>,
    capabilities: Vec<String>,
) -> Served {
    let link = Link { calls, shown };
    let notifications = Notifications {
        link: link.clone(),
        info: ServerInfo::default(),
        capabilities,
    };
    let notifications = async {
        connection
            .object_server()
            .at(OBJECT_PATH, notifications)
            .await?;
        claim(connection, BUS_NAME).await
    }
    .await;
    if notifications.is_err() {
        return Served {
            notifications,
            centre: None,
        };
    }
    let centre = async {
        connection
            .object_server()
            .at(CENTRE_OBJECT_PATH, CentreControl { link })
            .await?;
        claim(connection, CENTRE_BUS_NAME).await
    }
    .await;
    Served {
        notifications,
        centre: Some(centre),
    }
}

/// Sends `signals` on `connection` in the order they come, for as long as the component runs; a signal that cannot
/// be sent is reported and the next one goes on.
pub async fn emit(connection: Connection, signals: async_channel::Receiver<Signal>) {
    while let Ok(signal) = signals.recv().await {
        if let Err(e) = send(&connection, signal).await {
            eprintln!("cupertino-notifications: cannot send a signal: {e}");
        }
    }
}

async fn send(connection: &Connection, signal: Signal) -> zbus::Result<()> {
    let notifications = || SignalEmitter::new(connection, OBJECT_PATH);
    let centre = || async {
        connection
            .object_server()
            .interface::<_, CentreControl>(CENTRE_OBJECT_PATH)
            .await
    };
    match signal {
        Signal::Event(Event::Closed { id, reason }) => {
            Notifications::notification_closed(&notifications()?, id, reason as u32).await
        }
        Signal::Event(Event::ActionInvoked {
            id,
            key,
            activation_token,
        }) => {
            let emitter = notifications()?;
            if let Some(token) = activation_token {
                Notifications::activation_token(&emitter, id, token).await?;
            }
            Notifications::action_invoked(&emitter, id, key).await
        }
        Signal::DoNotDisturb => {
            let iface = centre().await?;
            let control = iface.get().await;
            control.do_not_disturb_changed(iface.signal_emitter()).await
        }
        Signal::CentreShown => {
            let iface = centre().await?;
            let control = iface.get().await;
            control.centre_shown_changed(iface.signal_emitter()).await
        }
        Signal::CentreOutput => {
            let iface = centre().await?;
            let control = iface.get().await;
            control.centre_output_changed(iface.signal_emitter()).await
        }
        Signal::LockScreen => {
            let iface = centre().await?;
            CentreControl::lock_screen_changed(iface.signal_emitter()).await
        }
    }
}
