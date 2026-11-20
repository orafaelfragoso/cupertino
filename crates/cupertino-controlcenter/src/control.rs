//! `org.cupertino.ControlPanel1`: the control panel's bus interface, served on the component's own session bus
//! connection, which owns the name (D20). The server calls `Toggle` for its shortcut, from the connection that owns
//! `org.cupertino.Session1`, and only that connection may: any other caller, another connection of the server's process
//! and the components of the shell host included, is refused with `org.cupertino.Error.UntrustedCaller`. The menu bar's
//! status item opens the panel over the host bus (`TogglePanel`).
//!
//! Its properties `Shown` and `Output` tell whether the panel is shown and on which output (the connector name, empty
//! while it is hidden or not known), with `PropertiesChanged` on every change, so the menu bar highlights the control
//! panel's item on that output's bar while the panel is open. A component that stops drops the name, which a reader
//! takes as not shown.
//!
//! The handlers run on the component's UI thread, which ticks the connection: the check asks the bus daemon who owns
//! `org.cupertino.Session1` and reads nothing else, and an accepted call goes to the model as a [`Call`].

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use cupertino_peer::component::SESSION_BUS_NAME;
use cupertino_peer::{CallerError, Denial};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::{BusName, InterfaceName};
use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;
use zbus::Connection;

/// Name, path and interface of the control interface (the callers use these strings).
pub const BUS_NAME: &str = "org.cupertino.ControlPanel1";
pub const OBJECT_PATH: &str = "/org/cupertino/ControlPanel1";
const INTERFACE: &str = "org.cupertino.ControlPanel1";

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
    /// Open the panel, or dismiss it when it is open.
    Toggle,
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
}

/// Whether the call with `header` comes from the server: its sender owns `org.cupertino.Session1` (D20). A call
/// without a sender, or one the bus daemon cannot say the owner for, is not the server's.
async fn from_server(connection: &Connection, header: &Header<'_>) -> bool {
    let Some(sender) = header.sender() else {
        return false;
    };
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return false;
    };
    let Ok(session) = BusName::try_from(SESSION_BUS_NAME) else {
        return false;
    };
    dbus.get_name_owner(session)
        .await
        .is_ok_and(|owner| owner.as_str() == sender.as_str())
}

#[zbus::interface(name = "org.cupertino.ControlPanel1")]
impl Control {
    /// Open the panel, or dismiss it when it is open. Only the server may; a refusal is logged with the method.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        if !from_server(connection, &header).await {
            eprintln!(
                "cupertino-controlcenter: {BUS_NAME}.Toggle: the caller is refused: it is not the server"
            );
            return Err(CallerError::from(Denial::Untrusted));
        }
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
