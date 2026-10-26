//! `org.freedesktop.Notifications` server on `zbus`.
//!
//! The bus side is a thin shell around a shared [`NotificationCenter`]: `Notify`,
//! `CloseNotification`, `GetCapabilities` and `GetServerInformation` are served from the zbus
//! executor thread, which locks the center, mutates it and releases the lock before emitting
//! signals. The host (the compositor) uses the same center from its own thread:
//!
//! * [`Server::center`] locks the center for `tick`, `placements`, input handling, ...
//! * [`Server::flush_events`] sends the signals (`NotificationClosed`, `ActivationToken`,
//!   `ActionInvoked`) for events produced on the host side (expiry, clicks, swipes).
//! * the `wake` callback of [`ServerOptions`] runs on the zbus thread after every `Notify` /
//!   `CloseNotification`; use it to poke the host's event loop (for example a calloop ping).
//!
//! Spec version 1.2 is implemented, plus the `ActivationToken` signal of 1.3.

use crate::center::{CenterConfig, DndMode, Event, NotificationCenter};
use crate::focus::{self, Lifetime, LocalCalendar, SystemCalendar};
use crate::notification::{Expire, Hints, ImageData, NotifyInput, Urgency};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedValue, Value};

pub const BUS_NAME: &str = "org.freedesktop.Notifications";
pub const OBJECT_PATH: &str = "/org/freedesktop/Notifications";
pub const SPEC_VERSION: &str = "1.2";

/// Name, path and interface of the control interface of the notification centre list
/// ([`Server::serve_centre_control`]).
pub const CENTRE_BUS_NAME: &str = "org.cupertino.NotificationCenter1";
pub const CENTRE_OBJECT_PATH: &str = "/org/cupertino/NotificationCenter1";

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

#[derive(Default)]
pub struct ServerOptions {
    /// Bus address to connect to; `None` = the session bus.
    pub address: Option<String>,
    pub config: CenterConfig,
    pub info: ServerInfo,
    /// Seconds on the host's monotonic clock. Default: seconds since the server started.
    pub clock: Option<Box<dyn Fn() -> f64 + Send + Sync>>,
    /// Called on the zbus thread after the center changed.
    pub wake: Option<Box<dyn Fn() + Send + Sync>>,
    /// Take over the name from a running server that allows replacement.
    pub replace_existing: bool,
}

pub struct Shared {
    center: Mutex<NotificationCenter>,
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
    wake: Box<dyn Fn() + Send + Sync>,
    info: ServerInfo,
    /// Do not disturb as last announced on `org.cupertino.NotificationCenter1`, so its end is announced too.
    dnd_announced: AtomicBool,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, NotificationCenter> {
        self.center.lock().unwrap_or_else(|e| e.into_inner())
    }
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

/// Decode the `hints` dictionary of `Notify`. Unknown hints are ignored; `image-data` takes
/// precedence over the deprecated `image_data` and `icon_data` names.
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

struct Interface(Arc<Shared>);

async fn emit_events(emitter: &SignalEmitter<'_>, events: Vec<Event>) -> zbus::Result<()> {
    for e in events {
        match e {
            Event::Closed { id, reason } => {
                Interface::notification_closed(emitter, id, reason as u32).await?
            }
            Event::ActionInvoked {
                id,
                key,
                activation_token,
            } => {
                if let Some(t) = activation_token {
                    Interface::activation_token(emitter, id, t).await?;
                }
                Interface::action_invoked(emitter, id, key).await?;
            }
        }
    }
    Ok(())
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Interface {
    fn get_capabilities(&self) -> Vec<String> {
        self.0.lock().config.capabilities.clone()
    }

    #[allow(clippy::too_many_arguments)]
    async fn notify(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        app_name: String,
        replaces_id: u32,
        app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        expire_timeout: i32,
    ) -> u32 {
        let input = NotifyInput {
            app_name,
            replaces_id,
            icon: app_icon,
            summary,
            body,
            actions,
            expire: Expire::from_dbus(expire_timeout),
            hints: parse_hints(&hints),
        };
        let now = (self.0.clock)();
        let (id, events) = {
            let mut c = self.0.lock();
            let id = c.notify(input, now);
            (id, c.take_events())
        };
        let _ = emit_events(&emitter, events).await;
        (self.0.wake)();
        id
    }

    async fn close_notification(
        &self,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
        id: u32,
    ) {
        let now = (self.0.clock)();
        let events = {
            let mut c = self.0.lock();
            c.close(id, now);
            c.take_events()
        };
        let _ = emit_events(&emitter, events).await;
        (self.0.wake)();
    }

    fn get_server_information(&self) -> (String, String, String, String) {
        let i = &self.0.info;
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

/// `org.cupertino.NotificationCenter1`: lets the menu bar open and close the centre list and the control
/// panel switch do not disturb.
struct CentreControl {
    on_toggle: Box<dyn Fn() + Send + Sync>,
    shared: Arc<Shared>,
}

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl CentreControl {
    /// Open the notification centre list, or close it when it is open.
    fn toggle_centre(&self) {
        (self.on_toggle)();
    }

    /// Turn do not disturb on (until turned off) or off; banners of suppressed notifications are not shown.
    async fn set_do_not_disturb(
        &self,
        on: bool,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let now = (self.shared.clock)();
        self.shared
            .lock()
            .set_dnd(if on { DndMode::On } else { DndMode::Off }, now);
        self.announce(&emitter, on).await
    }

    /// Turn do not disturb on for one of the [`Self::focus_lifetimes`] rows, by its identifier; it ends by itself.
    async fn set_do_not_disturb_for(
        &self,
        lifetime: &str,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> zbus::fdo::Result<()> {
        let lifetime = Lifetime::from_id(lifetime).ok_or_else(|| {
            zbus::fdo::Error::InvalidArgs(format!("{lifetime:?} is not an offered duration"))
        })?;
        let unix = unix_now()?;
        let ends = focus::end(lifetime, unix, &SystemCalendar);
        let now = (self.shared.clock)();
        self.shared
            .lock()
            .set_dnd(DndMode::Until(now + (ends - unix) as f64), now);
        self.announce(&emitter, true).await
    }

    /// The duration rows offered now, in menu order: identifier (the argument of `SetDoNotDisturbFor`) and title.
    #[zbus(property)]
    fn focus_lifetimes(&self) -> zbus::fdo::Result<Vec<(String, String)>> {
        let hour = SystemCalendar.hour(unix_now()?);
        Ok(focus::offered(hour)
            .iter()
            .map(|l| (l.id().to_owned(), l.title().to_owned()))
            .collect())
    }

    /// Whether do not disturb is in effect now.
    #[zbus(property)]
    fn do_not_disturb(&self) -> bool {
        let now = (self.shared.clock)();
        self.shared.lock().dnd_active(now)
    }
}

impl CentreControl {
    /// Wake the host and announce the new do-not-disturb state.
    async fn announce(&self, emitter: &SignalEmitter<'_>, on: bool) -> zbus::fdo::Result<()> {
        (self.shared.wake)();
        self.shared.dnd_announced.store(on, Ordering::SeqCst);
        Ok(self.do_not_disturb_changed(emitter).await?)
    }
}

/// Seconds since the Unix epoch on the wall clock.
fn unix_now() -> zbus::fdo::Result<i64> {
    let since = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|e| zbus::fdo::Error::Failed(format!("the clock is before 1970: {e}")))?;
    Ok(since.as_secs() as i64)
}

pub struct Server {
    conn: zbus::blocking::Connection,
    shared: Arc<Shared>,
    /// `org.cupertino.NotificationCenter1` is exported ([`Server::serve_centre_control`]).
    centre_served: AtomicBool,
}

impl Server {
    /// Connect, export the object and request the well-known name. Fails with
    /// `zbus::Error::NameTaken` when another notification server owns it.
    pub fn start(opts: ServerOptions) -> zbus::Result<Server> {
        let started = Instant::now();
        let shared = Arc::new(Shared {
            center: Mutex::new(NotificationCenter::new(opts.config)),
            clock: opts
                .clock
                .unwrap_or_else(|| Box::new(move || started.elapsed().as_secs_f64())),
            wake: opts.wake.unwrap_or_else(|| Box::new(|| {})),
            info: opts.info,
            dnd_announced: AtomicBool::new(false),
        });
        let builder = match &opts.address {
            Some(a) => zbus::blocking::connection::Builder::address(a.as_str())?,
            None => zbus::blocking::connection::Builder::session()?,
        };
        use zbus::fdo::{RequestNameFlags, RequestNameReply};
        let mut flags = RequestNameFlags::AllowReplacement | RequestNameFlags::DoNotQueue;
        if opts.replace_existing {
            flags |= RequestNameFlags::ReplaceExisting;
        }
        let conn = builder
            .serve_at(OBJECT_PATH, Interface(shared.clone()))?
            .build()?;
        match conn.request_name_with_flags(BUS_NAME, flags)? {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
            RequestNameReply::Exists | RequestNameReply::InQueue => {
                return Err(zbus::Error::NameTaken)
            }
        }
        Ok(Server {
            conn,
            shared,
            centre_served: AtomicBool::new(false),
        })
    }

    /// Export `org.cupertino.NotificationCenter1` at [`CENTRE_OBJECT_PATH`] and request
    /// [`CENTRE_BUS_NAME`]. `on_toggle` runs on the zbus thread for every `ToggleCentre` call.
    /// Fails with `zbus::Error::NameTaken` when another process owns the name.
    pub fn serve_centre_control(
        &self,
        on_toggle: impl Fn() + Send + Sync + 'static,
    ) -> zbus::Result<()> {
        self.conn.object_server().at(
            CENTRE_OBJECT_PATH,
            CentreControl {
                on_toggle: Box::new(on_toggle),
                shared: self.shared.clone(),
            },
        )?;
        self.centre_served.store(true, Ordering::SeqCst);
        self.conn.request_name(CENTRE_BUS_NAME)
    }

    /// Lock the model. Keep the guard short: bus calls wait on it.
    pub fn center(&self) -> MutexGuard<'_, NotificationCenter> {
        self.shared.lock()
    }

    /// Current time on the clock given to [`ServerOptions`].
    pub fn now(&self) -> f64 {
        (self.shared.clock)()
    }

    pub fn connection(&self) -> &zbus::blocking::Connection {
        &self.conn
    }

    /// Emit the signals for events produced on the host side, and announce a do-not-disturb duration that ran out.
    /// Returns how many notification events were sent.
    pub fn flush_events(&self) -> zbus::Result<usize> {
        let (events, dnd) = {
            let mut center = self.shared.lock();
            (
                center.take_events(),
                center.dnd_active((self.shared.clock)()),
            )
        };
        if self.centre_served.load(Ordering::SeqCst)
            && self.shared.dnd_announced.swap(dnd, Ordering::SeqCst) != dnd
        {
            let iface = self
                .conn
                .object_server()
                .interface::<_, CentreControl>(CENTRE_OBJECT_PATH)?;
            zbus::block_on(iface.get().do_not_disturb_changed(iface.signal_emitter()))?;
        }
        let n = events.len();
        if n == 0 {
            return Ok(0);
        }
        let iface = self
            .conn
            .object_server()
            .interface::<_, Interface>(OBJECT_PATH)?;
        zbus::block_on(emit_events(iface.signal_emitter(), events))?;
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::{Array, StructureBuilder};

    fn ov(v: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(v).unwrap()
    }

    #[test]
    fn hints_decode_scalar_types() {
        let mut m = HashMap::new();
        m.insert("urgency".to_string(), ov(Value::U8(2)));
        m.insert("category".to_string(), ov(Value::from("email.arrived")));
        m.insert("transient".to_string(), ov(Value::Bool(true)));
        m.insert("resident".to_string(), ov(Value::I32(1)));
        m.insert(
            "desktop-entry".to_string(),
            ov(Value::from("org.example.Mail")),
        );
        m.insert("image-path".to_string(), ov(Value::from("/tmp/x.png")));
        m.insert("unknown-hint".to_string(), ov(Value::U8(9)));
        let h = parse_hints(&m);
        assert_eq!(h.urgency, Some(Urgency::Critical));
        assert_eq!(h.category.as_deref(), Some("email.arrived"));
        assert!(h.transient && h.resident && !h.suppress_sound);
        assert_eq!(h.desktop_entry.as_deref(), Some("org.example.Mail"));
        assert_eq!(h.image_path.as_deref(), Some("/tmp/x.png"));
        assert!(h.image.is_none());
        assert_eq!(parse_hints(&HashMap::new()), Hints::default());
    }

    #[test]
    fn hints_decode_image_data() {
        let pixels: Vec<u8> = vec![255, 0, 0, 255, 0, 255, 0, 255];
        let arr = Array::from(pixels.as_slice());
        let s = StructureBuilder::new()
            .add_field(2i32)
            .add_field(1i32)
            .add_field(8i32)
            .add_field(true)
            .add_field(8i32)
            .add_field(4i32)
            .append_field(Value::Array(arr))
            .build()
            .unwrap();
        let mut m = HashMap::new();
        m.insert("image-data".to_string(), ov(Value::Structure(s)));
        let img = parse_hints(&m).image.expect("image decoded");
        assert_eq!((img.width, img.height), (2, 1));
        assert_eq!(img.rgba, pixels);
    }

    #[test]
    fn malformed_image_data_is_ignored() {
        let s = StructureBuilder::new()
            .add_field(2i32)
            .add_field(1i32)
            .build()
            .unwrap();
        let mut m = HashMap::new();
        m.insert("image-data".to_string(), ov(Value::Structure(s)));
        assert!(parse_hints(&m).image.is_none());
    }
}
