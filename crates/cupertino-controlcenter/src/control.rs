//! `org.cupertino.ControlPanel1`: the control panel's bus interface, served on the component's own session bus
//! connection, which owns the name (D20). The server (its shortcut) and the menu bar (its status item) call `Toggle`.
//! The menu bar is no relation of the shell host, so `Toggle` answers any unsandboxed process of the person and refuses
//! a Flatpak, a Snap and a caller the bus cannot name, as `org.cupertino.NotificationCenter1` does for the same callers.
//!
//! Its properties `Shown` and `Output` tell whether the panel is shown and on which output (the connector name, empty
//! while it is hidden or not known), with `PropertiesChanged` on every change, so the menu bar highlights the control
//! panel's item on that output's bar while the panel is open. A component that stops drops the name, which a reader
//! takes as not shown.
//!
//! The handlers run on the component's UI thread, which ticks the connection, and do no work of their own there: a
//! call goes to the model as a [`Call`], and the model names the caller from the credentials the bus gave for it off
//! the UI thread ([`judge`]), since that reads the caller's `/proc` entries.

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};

use cupertino_peer::{own_parent, Caller, CallerError, Relation};
use zbus::fdo::{ConnectionCredentials, RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;
use zbus::Connection;

/// Name, path and interface of the control interface (the callers use these strings).
pub const BUS_NAME: &str = "org.cupertino.ControlPanel1";
pub const OBJECT_PATH: &str = "/org/cupertino/ControlPanel1";
const INTERFACE: &str = "org.cupertino.ControlPanel1";

/// The `/proc` the callers are read from.
const PROC: &str = "/proc";

/// Who may toggle the panel: the server (the host's parent), the menu bar (no relation) and the host's own components;
/// never a sandboxed or anonymous caller.
const CALLERS: &[Relation] = &[Relation::Itself, Relation::Parent, Relation::Other];

/// How the panel is shown, as its properties say.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Presence {
    /// The panel is shown: from the toggle that opens it until its dismissal starts the fade.
    pub shown: bool,
    /// The connector name of the output the panel is on (`wl_output.name`), such as `DP-1`; empty while it is hidden
    /// or not known.
    pub output: String,
}

/// A call of the interface, handed to the model.
#[derive(Clone, Debug)]
pub enum Call {
    /// Name the caller of the credentials (`None`: the bus gave none) for the call that waits on `reply`.
    Judge {
        credentials: Option<Arc<ConnectionCredentials>>,
        reply: async_channel::Sender<Arc<Caller>>,
    },
    /// Open the panel, or dismiss it when it is open.
    Toggle,
}

/// The caller of `credentials`, read from its `/proc` entries: for the I/O pool, never the UI thread.
pub fn judge(credentials: Option<&ConnectionCredentials>) -> Caller {
    match credentials {
        Some(credentials) => Caller::from_credentials(credentials, Path::new(PROC)),
        None => Caller::Anonymous,
    }
}

struct Control {
    calls: async_channel::Sender<Call>,
    presence: Arc<Mutex<Presence>>,
}

fn lock(presence: &Mutex<Presence>) -> MutexGuard<'_, Presence> {
    presence.lock().unwrap_or_else(|e| e.into_inner())
}

impl Control {
    fn send(&self, call: Call) {
        // the model holds the receiver for as long as the interface is served
        let _ = self.calls.try_send(call);
    }

    /// The caller of a call with `header`, named by the model; anonymous when the bus or the model gave no answer.
    async fn caller(&self, connection: &Connection, header: &Header<'_>) -> Arc<Caller> {
        let credentials = credentials(connection, header).await;
        let (reply, answer) = async_channel::bounded(1);
        self.send(Call::Judge { credentials, reply });
        answer
            .recv()
            .await
            .unwrap_or_else(|_| Arc::new(Caller::Anonymous))
    }
}

/// The credentials the bus gives for the sender of a call with `header`; `None` when it gives none.
async fn credentials(
    connection: &Connection,
    header: &Header<'_>,
) -> Option<Arc<ConnectionCredentials>> {
    let sender = header.sender()?;
    let proxy = zbus::fdo::DBusProxy::new(connection).await.ok()?;
    proxy
        .get_connection_credentials(sender.clone().into())
        .await
        .ok()
        .map(Arc::new)
}

#[zbus::interface(name = "org.cupertino.ControlPanel1")]
impl Control {
    /// Open the panel, or dismiss it when it is open. Refused to a sandboxed or anonymous caller; a refusal is logged
    /// with the method.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        let caller = self.caller(connection, &header).await;
        caller
            .require_relation(std::process::id(), own_parent(), CALLERS)
            .map_err(|denial| {
                eprintln!(
                    "cupertino-controlcenter: {BUS_NAME}.Toggle: the caller is refused: {denial:?}"
                );
                CallerError::from(denial)
            })?;
        self.send(Call::Toggle);
        Ok(())
    }

    /// Whether the panel is shown.
    #[zbus(property)]
    fn shown(&self) -> bool {
        lock(&self.presence).shown
    }

    /// The connector name of the output the shown panel is on; empty while hidden or not known.
    #[zbus(property)]
    fn output(&self) -> String {
        lock(&self.presence).output.clone()
    }
}

/// The served interface: the connection that serves it, and what its properties say.
#[derive(Clone)]
pub struct Served {
    connection: Connection,
    presence: Arc<Mutex<Presence>>,
}

impl Served {
    /// Make the properties say `presence`, and announce those that changed with `PropertiesChanged`. Nothing is sent
    /// when nothing changed.
    pub async fn publish(&self, presence: Presence) -> zbus::Result<()> {
        let mut changed: HashMap<&str, Value<'_>> = HashMap::new();
        {
            let mut now = lock(&self.presence);
            if now.shown != presence.shown {
                changed.insert("Shown", Value::from(presence.shown));
            }
            if now.output != presence.output {
                changed.insert("Output", Value::from(presence.output.clone()));
            }
            *now = presence;
        }
        if changed.is_empty() {
            return Ok(());
        }
        let emitter = SignalEmitter::new(&self.connection, OBJECT_PATH)?;
        zbus::fdo::Properties::properties_changed(
            &emitter,
            InterfaceName::from_static_str_unchecked(INTERFACE),
            changed,
            Cow::Borrowed(&[]),
        )
        .await
    }

    /// Announce every presence that comes out of `presences`, in order, until the channel closes; one that cannot be
    /// announced is logged, and the properties say it all the same.
    pub async fn announce(self, presences: async_channel::Receiver<Presence>) {
        while let Ok(presence) = presences.recv().await {
            if let Err(e) = self.publish(presence).await {
                eprintln!(
                    "cupertino-controlcenter: cannot announce whether the panel is shown: {e}"
                );
            }
        }
    }
}

/// Exports the interface at [`OBJECT_PATH`] on `connection` and claims [`BUS_NAME`] on it; every accepted call goes
/// to `calls`. Its properties say the panel is hidden until [`Served::publish`] says otherwise. Fails with
/// `zbus::Error::NameTaken` when another panel owns the name.
pub async fn serve(
    connection: &Connection,
    calls: async_channel::Sender<Call>,
) -> zbus::Result<Served> {
    let presence = Arc::new(Mutex::new(Presence::default()));
    connection
        .object_server()
        .at(
            OBJECT_PATH,
            Control {
                calls,
                presence: Arc::clone(&presence),
            },
        )
        .await?;
    // after the object exists, so no call finds the name without it; never queue behind the owner
    match connection
        .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
        .await?
    {
        RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(Served {
            connection: connection.clone(),
            presence,
        }),
        RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
    }
}
