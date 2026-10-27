//! `org.cupertino.Login1`: the control interface of the `cupertino-login` process, which is the session agent. The server
//! calls `Lock` for the lock shortcut and `PowerKey` for the press and the release of the power key; any client may call
//! the `Request*` methods, which show the confirmation panel of [`crate::panel`], and read `Locked`, `UserName` and
//! `Capabilities` (what the session menu of the menu bar offers).

use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::{connection, Connection};

use std::collections::HashMap;

use crate::capabilities::Capabilities;
use crate::panel::Kind;

/// Name, path and interface of the control interface (the callers use these strings).
pub const BUS_NAME: &str = "org.cupertino.Login1";
pub const OBJECT_PATH: &str = "/org/cupertino/Login1";

/// What a caller of the interface asks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    /// `Lock` (`explicit` false: the display slept, logind asked or the computer is going to sleep) or `LockNow` (`explicit` true:
    /// the person asked, lock request type 8 of the design, whose lock screen hides after 5 s).
    Lock { explicit: bool },
    /// `RequestLogOut`, `RequestRestart` or `RequestShutDown`.
    Panel(Kind),
    /// `PowerKey`: the key went down (`true`) or up (`false`).
    PowerKey { pressed: bool },
    /// `SwitchToLoginWindow`.
    SwitchToLoginWindow,
    /// `Sleep`: lock and put the computer to sleep, without a panel.
    Sleep,
}

struct Control {
    on_request: Box<dyn Fn(Request) + Send + Sync>,
    locked: bool,
    user_name: String,
    capabilities: Capabilities,
}

impl Control {
    /// A panel cannot be shown on a locked session: its surface is not drawn there, and an action no one sees must not run.
    fn panel(&self, kind: Kind) -> zbus::fdo::Result<()> {
        if self.locked {
            return Err(zbus::fdo::Error::Failed("the session is locked".into()));
        }
        (self.on_request)(Request::Panel(kind));
        Ok(())
    }
}

#[zbus::interface(name = "org.cupertino.Login1")]
impl Control {
    /// Lock the session, or do nothing when it is locked already.
    fn lock(&self) {
        (self.on_request)(Request::Lock { explicit: false });
    }

    /// Lock the session because the person asked for it (the lock shortcut), or do nothing when it is locked already.
    fn lock_now(&self) {
        (self.on_request)(Request::Lock { explicit: true });
    }

    /// Ask whether to log out, with a countdown that logs out when it ends. Fails while the session is locked.
    fn request_log_out(&self) -> zbus::fdo::Result<()> {
        self.panel(Kind::LogOut)
    }

    /// Ask whether to restart the computer, with a countdown. Fails while the session is locked.
    fn request_restart(&self) -> zbus::fdo::Result<()> {
        self.panel(Kind::Restart)
    }

    /// Ask whether to shut the computer down, with a countdown. Fails while the session is locked.
    fn request_shut_down(&self) -> zbus::fdo::Result<()> {
        self.panel(Kind::ShutDown)
    }

    /// Put the computer to sleep without asking: the session locks first (the sleep preparation does it). Fails while the
    /// session is locked, like the panels.
    fn sleep(&self) -> zbus::fdo::Result<()> {
        if self.locked {
            return Err(zbus::fdo::Error::Failed("the session is locked".into()));
        }
        (self.on_request)(Request::Sleep);
        Ok(())
    }

    /// Show the power panel (Restart, Sleep, Cancel, Shut Down), the one a held power button shows. Fails while the session
    /// is locked.
    fn request_power_dialog(&self) -> zbus::fdo::Result<()> {
        self.panel(Kind::PowerButton)
    }

    /// Lock the session and hand the console to the login screen, where another user can log in and this one can come back
    /// (fast user switching). A session that is locked already switches too.
    fn switch_to_login_window(&self) {
        (self.on_request)(Request::SwitchToLoginWindow);
    }

    /// The power key went down or up; this process times the hold ([`crate::power_button`]). A locked session ignores the
    /// call, which never fails.
    fn power_key(&self, pressed: bool) {
        (self.on_request)(Request::PowerKey { pressed });
    }

    /// Whether the session is locked (the server confirmed the lock and it has not been unlocked).
    #[zbus(property)]
    fn locked(&self) -> bool {
        self.locked
    }

    /// The name of the person, for "Log Out <name>…": the account's full name, else its login.
    #[zbus(property)]
    fn user_name(&self) -> String {
        self.user_name.clone()
    }

    /// Which of `sleep`, `restart`, `shut-down`, `log-out`, `switch-user` and `lock` work on this machine; a menu hides or
    /// disables the others. See [`Capabilities`].
    #[zbus(property)]
    fn capabilities(&self) -> HashMap<String, bool> {
        self.capabilities.to_map()
    }
}

/// The served interface; it stays on the bus while this lives.
pub struct Service {
    connection: Connection,
}

impl Service {
    /// The session bus connection the interface is served on.
    pub fn connection(&self) -> zbus::blocking::Connection {
        zbus::blocking::Connection::from(self.connection.clone())
    }

    /// Set `Capabilities`, and emit `PropertiesChanged` when the value changes.
    pub fn set_capabilities(&self, capabilities: Capabilities) -> zbus::Result<()> {
        zbus::block_on(async {
            let interface = self
                .connection
                .object_server()
                .interface::<_, Control>(OBJECT_PATH)
                .await?;
            let mut control = interface.get_mut().await;
            if control.capabilities != capabilities {
                control.capabilities = capabilities;
                control
                    .capabilities_changed(interface.signal_emitter())
                    .await?;
            }
            Ok(())
        })
    }

    /// Set `Locked`, and emit `PropertiesChanged` when the value changes.
    pub fn set_locked(&self, locked: bool) -> zbus::Result<()> {
        zbus::block_on(async {
            let interface = self
                .connection
                .object_server()
                .interface::<_, Control>(OBJECT_PATH)
                .await?;
            let mut control = interface.get_mut().await;
            if control.locked != locked {
                control.locked = locked;
                control.locked_changed(interface.signal_emitter()).await?;
            }
            Ok(())
        })
    }
}

/// Export the interface at [`OBJECT_PATH`] and claim [`BUS_NAME`] on the bus `builder` connects to. `on_request` runs on
/// the bus thread for every call. The interface is served while the returned [`Service`] lives, with `user_name` and
/// `capabilities` as the properties start out. Fails with `zbus::Error::NameTaken` when another lock screen owns the name.
pub fn serve(
    builder: connection::Builder<'static>,
    user_name: String,
    capabilities: Capabilities,
    on_request: impl Fn(Request) + Send + Sync + 'static,
) -> zbus::Result<Service> {
    zbus::block_on(async {
        let connection = builder
            .serve_at(
                OBJECT_PATH,
                Control {
                    on_request: Box::new(on_request),
                    locked: false,
                    user_name,
                    capabilities,
                },
            )?
            .build()
            .await?;
        // after the object exists, so no call finds the name without it; never queue behind the owner
        match connection
            .request_name_with_flags(BUS_NAME, RequestNameFlags::DoNotQueue.into())
            .await?
        {
            RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {
                Ok(Service { connection })
            }
            RequestNameReply::Exists | RequestNameReply::InQueue => Err(zbus::Error::NameTaken),
        }
    })
}
