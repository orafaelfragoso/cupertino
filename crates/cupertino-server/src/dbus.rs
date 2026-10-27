//! Session bus worker: relays the shell toggles (notification centre, launcher, control panel), the media keys
//! (overlay process), the lock request and the power key (login process) to the processes that own them.
//!
//! Everything runs on one background thread that drives `zbus` futures; the compositor talks to it
//! through [`DbusHandle::send`]. Without a session bus the worker ends after logging and the
//! requests fail with a warning.

use crate::caller::identify;
use crate::input::OsdKey;

/// What the worker asks a shell process to toggle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Toggle {
    /// The notification centre list of `cupertino-notifications`.
    NotificationCentre,
    /// The panel of `cupertino-launcher`.
    Launcher,
    /// The panel of `cupertino-controlcenter`.
    ControlPanel,
}

/// One call the worker makes on a shell process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Request {
    Toggle(Toggle),
    /// A media key for `cupertino-osd`; `fine` is the Shift+Alt step.
    Osd {
        key: OsdKey,
        fine: bool,
    },
    /// Lock the session through `cupertino-login`: the display slept for the password delay (lock request type 5 of the design).
    Lock,
    /// Lock the session on the person's request (the lock shortcut; lock request type 8, whose lock screen hides after 5 s).
    LockNow,
    /// Ask `cupertino-login` whether to log out (its confirmation panel).
    LogOut,
    /// Open `cupertino-login`'s power panel.
    PowerDialog,
    /// The menu bar's entries, forwarded by the server: sleep without a panel, the restart and shut down panels, the
    /// switch to the login window, and reading what the machine allows again.
    Sleep,
    Restart,
    ShutDown,
    SwitchUser,
    RefreshCapabilities,
    /// The power key went down (`true`) or up; `cupertino-login` times the hold.
    PowerKey {
        pressed: bool,
    },
}

fn login_call(
    method: &'static str,
) -> (&'static str, &'static str, &'static str, &'static str, Body) {
    (
        "org.cupertino.Login1",
        "/org/cupertino/Login1",
        "org.cupertino.Login1",
        method,
        Body::Empty,
    )
}

/// Arguments of a control method.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Body {
    Empty,
    /// Direction (+1 or -1) and the fine flag of the overlay's step methods.
    Step(i32, bool),
    /// Whether the power key went down (`PowerKey`).
    Pressed(bool),
}

/// Bus name, object path and interface of the overlay process (`cupertino_osd::control`).
const OSD: (&str, &str, &str) = (
    "org.cupertino.Osd1",
    "/org/cupertino/Osd1",
    "org.cupertino.Osd1",
);

impl Request {
    /// Bus name, object path, interface, method and arguments of the control interface of the process
    /// (`cupertino_notifications::dbus`, `cupertino_launcher::control`, `cupertino_controlcenter::control`, `cupertino_osd::control`; the lock request goes to the login process).
    fn call(self) -> (&'static str, &'static str, &'static str, &'static str, Body) {
        let (name, path, interface) = OSD;
        match self {
            Request::Toggle(Toggle::NotificationCentre) => (
                "org.cupertino.NotificationCenter1",
                "/org/cupertino/NotificationCenter1",
                "org.cupertino.NotificationCenter1",
                "ToggleCentre",
                Body::Empty,
            ),
            Request::Toggle(Toggle::Launcher) => (
                "org.cupertino.Launcher1",
                "/org/cupertino/Launcher1",
                "org.cupertino.Launcher1",
                "Toggle",
                Body::Empty,
            ),
            Request::Toggle(Toggle::ControlPanel) => (
                "org.cupertino.ControlPanel1",
                "/org/cupertino/ControlPanel1",
                "org.cupertino.ControlPanel1",
                "Toggle",
                Body::Empty,
            ),
            Request::Lock => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "Lock",
                Body::Empty,
            ),
            Request::LockNow => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "LockNow",
                Body::Empty,
            ),
            Request::LogOut => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "RequestLogOut",
                Body::Empty,
            ),
            Request::PowerDialog => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "RequestPowerDialog",
                Body::Empty,
            ),
            Request::Sleep => login_call("Sleep"),
            Request::Restart => login_call("RequestRestart"),
            Request::ShutDown => login_call("RequestShutDown"),
            Request::SwitchUser => login_call("SwitchToLoginWindow"),
            Request::RefreshCapabilities => login_call("RefreshCapabilities"),
            Request::PowerKey { pressed } => (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "PowerKey",
                Body::Pressed(pressed),
            ),
            Request::Osd { key, fine } => {
                let (method, body) = match key {
                    OsdKey::VolumeUp => ("VolumeStep", Body::Step(1, fine)),
                    OsdKey::VolumeDown => ("VolumeStep", Body::Step(-1, fine)),
                    OsdKey::Mute => ("ToggleMute", Body::Empty),
                    OsdKey::BrightnessUp => ("BrightnessStep", Body::Step(1, fine)),
                    OsdKey::BrightnessDown => ("BrightnessStep", Body::Step(-1, fine)),
                    OsdKey::KeyboardBrightnessUp => ("KeyboardBrightnessStep", Body::Step(1, fine)),
                    OsdKey::KeyboardBrightnessDown => {
                        ("KeyboardBrightnessStep", Body::Step(-1, fine))
                    }
                };
                (name, path, interface, method, body)
            }
        }
    }
}

/// Who may own `org.cupertino.Login1`: the lock, log out and power key requests go to nobody else.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoginTrust {
    /// No login process is supervised: no owner of the name is trusted.
    Nobody,
    /// The supervised login process, by its process id.
    Agent(u32),
    /// A development run: the owner of the name is whoever started it.
    Any,
}

/// Handle on the worker thread.
pub struct DbusHandle {
    tx: async_channel::Sender<Request>,
    login: std::sync::Arc<std::sync::Mutex<LoginTrust>>,
}

/// Runs on the worker thread when the login process, the one that shows the log out panel, is not on the bus.
pub type LoginMissing = std::sync::Arc<dyn Fn() + Send + Sync>;

impl DbusHandle {
    /// Start the worker.
    pub fn start() -> Option<DbusHandle> {
        Self::start_with(|| {})
    }

    /// Start the worker; `login_missing` runs on it when a log out request finds nobody to ask (no `org.cupertino.Login1`
    /// on the bus), so the caller can do what a session without a login process does.
    pub fn start_with(login_missing: impl Fn() + Send + Sync + 'static) -> Option<DbusHandle> {
        let (tx, rx) = async_channel::unbounded::<Request>();
        let login_missing: LoginMissing = std::sync::Arc::new(login_missing);
        let login = std::sync::Arc::new(std::sync::Mutex::new(LoginTrust::Nobody));
        let worker_login = login.clone();
        let spawned = std::thread::Builder::new()
            .name("cupertino-dbus".into())
            .spawn(move || zbus::block_on(worker(rx, login_missing, worker_login)));
        match spawned {
            Ok(_) => Some(DbusHandle { tx, login }),
            Err(e) => {
                tracing::warn!(error = %e, "cannot start the D-Bus worker");
                None
            }
        }
    }

    /// Say whose ownership of `org.cupertino.Login1` the worker may trust.
    pub fn set_login(&self, trust: LoginTrust) {
        *self
            .login
            .lock()
            .expect("no thread panics while holding the login trust") = trust;
    }

    /// Queue a call on a shell process; false when the worker is gone (no session bus).
    pub fn send(&self, what: Request) -> bool {
        let queued = self.tx.try_send(what).is_ok();
        if !queued {
            tracing::warn!(?what, "no session bus: cannot reach the shell process");
        }
        queued
    }
}

type SharedLoginTrust = std::sync::Arc<std::sync::Mutex<LoginTrust>>;

async fn worker(
    rx: async_channel::Receiver<Request>,
    login_missing: LoginMissing,
    login: SharedLoginTrust,
) {
    let conn = match zbus::Connection::session().await {
        Ok(c) => c,
        Err(e) => {
            tracing::info!(error = %e, "no session bus: the shell toggles and media keys are unavailable");
            return;
        }
    };
    while let Ok(what) = rx.recv().await {
        let conn = conn.clone();
        // a key's press and release must arrive in the order they happened, so those calls are awaited in turn
        if matches!(what, Request::PowerKey { .. }) {
            call(&conn, what, &login_missing, &login).await;
        } else {
            let login_missing = login_missing.clone();
            let login = login.clone();
            conn.executor()
                .clone()
                .spawn(
                    async move { call(&conn, what, &login_missing, &login).await },
                    "cupertino-shell-call",
                )
                .detach();
        }
    }
}

/// Where to send a call for the owner of `name`. The login process is looked for by its connection: the owner of
/// `org.cupertino.Login1` must be the supervised process, and the call goes to its unique name, so a client that takes
/// the name later gets nothing. `None`: the owner is not the login process.
async fn destination(
    conn: &zbus::Connection,
    name: &str,
    login: &SharedLoginTrust,
) -> Option<String> {
    if name != LOGIN_NAME {
        return Some(name.to_string());
    }
    let trust = *login
        .lock()
        .expect("no thread panics while holding the login trust");
    let Ok(proxy) = zbus::fdo::DBusProxy::new(conn).await else {
        return Some(name.to_string());
    };
    let Ok(bus_name) = zbus::names::BusName::try_from(name) else {
        return Some(name.to_string());
    };
    let Ok(owner) = proxy.get_name_owner(bus_name).await else {
        // nobody owns it: the call fails the way it did, and the log out request finds nobody to ask
        return Some(name.to_string());
    };
    let caller = identify(conn, &owner).await;
    let trusted = match (trust, caller.pid()) {
        (LoginTrust::Any, _) => true,
        (LoginTrust::Agent(expected), Some(pid)) => pid == expected,
        (LoginTrust::Agent(_) | LoginTrust::Nobody, _) => false,
    };
    if !trusted {
        tracing::error!(
            name,
            owner = %owner,
            pid = caller.pid(),
            "the owner of the login process's name is not the login process: the request is not sent"
        );
        return None;
    }
    Some(owner.to_string())
}

const LOGIN_NAME: &str = "org.cupertino.Login1";

/// Make the call of `what` and log a failure.
async fn call(
    conn: &zbus::Connection,
    what: Request,
    login_missing: &LoginMissing,
    login: &SharedLoginTrust,
) {
    let (name, path, interface, method, body) = what.call();
    let Some(destination) = destination(conn, name, login).await else {
        if what == Request::LogOut {
            tracing::warn!("no login process on the bus: the log out shortcut ends the server");
            login_missing();
        }
        return;
    };
    let destination = Some(destination.as_str());
    let sent = match body {
        Body::Empty => {
            conn.call_method(destination, path, Some(interface), method, &())
                .await
        }
        Body::Step(direction, fine) => {
            conn.call_method(
                destination,
                path,
                Some(interface),
                method,
                &(direction, fine),
            )
            .await
        }
        Body::Pressed(pressed) => {
            conn.call_method(destination, path, Some(interface), method, &pressed)
                .await
        }
    };
    if let Err(e) = sent {
        if what == Request::LogOut && is_nobody_there(&e) {
            tracing::warn!("no login process on the bus: the log out shortcut ends the server");
            login_missing();
            return;
        }
        tracing::warn!(?what, name, error = %e, "cannot reach the shell process");
    }
}

/// Whether a call failed because nobody owns the name it went to.
fn is_nobody_there(e: &zbus::Error) -> bool {
    matches!(
        e,
        zbus::Error::MethodError(name, ..)
            if matches!(
                name.as_str(),
                "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner"
            )
    )
}

impl crate::state::Cupertino {
    /// Ask the notification process to open or close its centre list.
    pub fn toggle_notification_center(&mut self) {
        self.shell_call(Request::Toggle(Toggle::NotificationCentre));
    }

    /// Ask the launcher process to show or hide its panel.
    pub fn toggle_launcher(&mut self) {
        self.shell_call(Request::Toggle(Toggle::Launcher));
    }

    /// Ask the control panel process to show or hide its panel.
    pub fn toggle_control_panel(&mut self) {
        self.shell_call(Request::Toggle(Toggle::ControlPanel));
    }

    /// Ask the login process to lock the session. The server's private connection to the lock screen carries the request;
    /// the session bus is the way only while no lock screen is connected, so a process that took `org.cupertino.Login1`
    /// before the login process did cannot make the lock go missing.
    pub fn lock_session(&mut self) {
        if !self.sessions.lock_requested(false) {
            self.shell_call(Request::Lock);
        }
    }

    /// Ask the login process to lock the session because the person asked for it.
    pub fn lock_session_now(&mut self) {
        if !self.sessions.lock_requested(true) {
            self.shell_call(Request::LockNow);
        }
    }

    /// Ask the login process whether to log out. A session without one (no session bus, or no login process on it: a
    /// headless or capture run) ends the server instead, as the shortcut did before there was a panel.
    pub fn log_out_session(&mut self) {
        if !self.dbus.as_ref().is_some_and(|d| d.send(Request::LogOut)) {
            tracing::warn!("no login process to ask: the log out shortcut ends the server");
            self.quit_compositor();
        }
    }

    /// A menu bar entry of the session menu: the server asks the login process on the menu bar's behalf, because the login
    /// process answers only the server.
    pub fn menu_bar_session_action(&mut self, action: Request) {
        self.shell_call(action);
    }

    /// Open the login process's power panel.
    pub fn power_dialog(&mut self) {
        self.shell_call(Request::PowerDialog);
    }

    /// Tell the login process the power key went down or up.
    pub fn power_key(&mut self, pressed: bool) {
        self.shell_call(Request::PowerKey { pressed });
    }

    /// Forward a media key to the overlay process, which changes the level and shows it.
    pub fn osd_key(&mut self, key: OsdKey, fine: bool) {
        self.shell_call(Request::Osd { key, fine });
    }

    fn shell_call(&mut self, what: Request) {
        match &self.dbus {
            Some(dbus) => {
                dbus.send(what);
            }
            None => tracing::warn!(?what, "no session bus: cannot reach the shell process"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_request_calls_the_login_process() {
        assert_eq!(
            Request::Lock.call(),
            (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "Lock",
                Body::Empty
            )
        );
    }

    #[test]
    fn the_log_out_and_power_dialog_requests_call_the_panels_of_the_login_process() {
        for (request, method) in [
            (Request::LogOut, "RequestLogOut"),
            (Request::PowerDialog, "RequestPowerDialog"),
        ] {
            assert_eq!(
                request.call(),
                (
                    "org.cupertino.Login1",
                    "/org/cupertino/Login1",
                    "org.cupertino.Login1",
                    method,
                    Body::Empty
                )
            );
        }
    }

    #[test]
    fn the_explicit_lock_request_calls_lock_now() {
        assert_eq!(
            Request::LockNow.call(),
            (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "LockNow",
                Body::Empty
            )
        );
    }

    #[test]
    fn the_power_key_request_calls_the_power_key_method_of_the_login_process_with_the_key_state() {
        assert_eq!(
            Request::PowerKey { pressed: true }.call(),
            (
                "org.cupertino.Login1",
                "/org/cupertino/Login1",
                "org.cupertino.Login1",
                "PowerKey",
                Body::Pressed(true)
            )
        );
        assert_eq!(
            Request::PowerKey { pressed: false }.call().4,
            Body::Pressed(false)
        );
    }

    #[test]
    fn media_keys_map_to_the_overlay_methods() {
        let call = |key, fine| {
            let (name, _, _, method, body) = Request::Osd { key, fine }.call();
            (name, method, body)
        };
        assert_eq!(
            call(OsdKey::VolumeUp, false),
            ("org.cupertino.Osd1", "VolumeStep", Body::Step(1, false))
        );
        assert_eq!(
            call(OsdKey::VolumeDown, true),
            ("org.cupertino.Osd1", "VolumeStep", Body::Step(-1, true))
        );
        assert_eq!(
            call(OsdKey::Mute, false),
            ("org.cupertino.Osd1", "ToggleMute", Body::Empty)
        );
        assert_eq!(
            call(OsdKey::BrightnessDown, false),
            (
                "org.cupertino.Osd1",
                "BrightnessStep",
                Body::Step(-1, false)
            )
        );
        assert_eq!(
            call(OsdKey::KeyboardBrightnessUp, true),
            (
                "org.cupertino.Osd1",
                "KeyboardBrightnessStep",
                Body::Step(1, true)
            )
        );
    }
}
