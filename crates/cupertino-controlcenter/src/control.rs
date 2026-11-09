//! `org.cupertino.ControlPanel1`: the control interface of the `cupertino-controlcenter` process. The server (shortcut)
//! and the menu bar (status item click) call `Toggle`. The menu bar is no relation of this process (the server started
//! both), so `Toggle` answers any unsandboxed process of the person and refuses a Flatpak, a Snap and a caller the bus
//! cannot name (`cupertino_peer::check_call`), as `org.cupertino.NotificationCenter1` does for the same callers.
//!
//! Its properties `Shown` and `Output` tell whether the panel is shown and on which output (the connector name, empty
//! until the panel's surfaces are told), with `PropertiesChanged` on every change, so the menu bar highlights the
//! control panel's item on that output's bar while the panel is open. A panel whose process ends drops the name, which
//! a reader takes as not shown.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use cupertino_peer::{check_call, CallerError, Relation};
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::message::Header;
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::Value;
use zbus::{connection, Connection};

/// Who may toggle the panel: the server (the parent), the menu bar (no relation) and this process; never a sandboxed or
/// anonymous caller.
const CALLERS: &[Relation] = &[Relation::Itself, Relation::Parent, Relation::Other];

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
    /// or before its surfaces are told their output.
    pub output: String,
}

struct Control {
    on_toggle: Box<dyn Fn() + Send + Sync>,
    presence: Arc<Mutex<Presence>>,
}

fn lock(presence: &Mutex<Presence>) -> MutexGuard<'_, Presence> {
    presence.lock().unwrap_or_else(|e| e.into_inner())
}

#[zbus::interface(name = "org.cupertino.ControlPanel1")]
impl Control {
    /// Open the panel, or dismiss it when it is open. Refused to a sandboxed or anonymous caller.
    async fn toggle(
        &self,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] connection: &Connection,
    ) -> Result<(), CallerError> {
        check_call(
            connection,
            &header,
            "org.cupertino.ControlPanel1",
            "Toggle",
            CALLERS,
        )
        .await?;
        (self.on_toggle)();
        Ok(())
    }

    /// Whether the panel is shown.
    #[zbus(property)]
    fn shown(&self) -> bool {
        lock(&self.presence).shown
    }

    /// The connector name of the output the shown panel is on; empty while hidden or not yet known.
    #[zbus(property)]
    fn output(&self) -> String {
        lock(&self.presence).output.clone()
    }
}

/// The served interface: the connection that serves it, and what its properties say.
pub struct Served {
    connection: Connection,
    presence: Arc<Mutex<Presence>>,
}

impl Served {
    /// The connection that serves the interface; it is served while this lives.
    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    /// Make the properties say `presence`, and announce those that changed with `PropertiesChanged`. Nothing is sent
    /// when nothing changed.
    pub fn publish(&self, presence: Presence) -> zbus::Result<()> {
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
        zbus::block_on(async {
            let emitter = SignalEmitter::new(&self.connection, OBJECT_PATH)?;
            zbus::fdo::Properties::properties_changed(
                &emitter,
                InterfaceName::from_static_str_unchecked(INTERFACE),
                changed,
                Cow::Borrowed(&[]),
            )
            .await
        })
    }
}

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to. `on_toggle` runs on
/// the bus thread for every `Toggle` call. The interface is served while the returned [`Served`] lives; its properties
/// say the panel is hidden until [`Served::publish`] says otherwise. Fails with `zbus::Error::NameTaken` when another
/// panel owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    on_toggle: impl Fn() + Send + Sync + 'static,
) -> zbus::Result<Served> {
    let presence = Arc::new(Mutex::new(Presence::default()));
    let shared = Arc::clone(&presence);
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Control {
                    on_toggle: Box::new(on_toggle),
                    presence: shared,
                },
            )?
            .build()
            .await?;
        // after the object exists, so no call finds the name without it; never queue behind the owner
        match connection
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => Ok(Served {
                connection,
                presence,
            }),
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}
