//! `org.cupertino.Session1` on the session bus: quitting the applications before a log out, which the session agent
//! (`cupertino-login`) calls between the confirmed log out and logind's `TerminateSession`, and the session settings the
//! server alone writes (the appearance and `accessibility.conf`), which the settings application sets here.
//!
//! The design quits the foreground applications one at a time: it brings the application forward so that a save
//! sheet is visible, sends it the quit event, and gives it 45 s before the interrupt panel shows; the next application
//! follows 0.1 s after one quits. The timers are the agent's; this side does the Wayland part:
//!
//! | Member | What it does |
//! |---|---|
//! | `QuitNextApplication -> (s app_id, u windows)` | activates the most recently used application that has toplevels and sends `close` to each of them (`xdg_toplevel.close`); `("", 0) when no application has a window |
//! | `ForceQuitApplication(s app_id) -> u` | sends SIGKILL to the client processes of that application's toplevels; returns how many processes |
//! | `ApplicationQuit(s app_id) signal | the last toplevel of the application being quit went away |
//! | `SetAppearance(s appearance)` | `light` or `dark`: stored as `appearance` in `compositor.json`, then applied at once; `auto` is refused with `NotSupported` (no time-of-day or location backend), anything else with `InvalidArgs`; a failed store, or no settings directory to store in, changes nothing (`Failed`) |
//! | `SetAccessibility(s key, s value)` | one key of `accessibility.conf` with a value as the file writes it (`reduce_motion` `on`, `color_filter` `tint 0.5`, ...): applied and stored like the key actions; an unknown key or bad value is `InvalidArgs` |
//! | `Appearance` property `s` | `light` or `dark`, the session's appearance (with `PropertiesChanged`) |
//! | `Accessibility` property `a{ss}` | every key of `accessibility.conf` with its value (with `PropertiesChanged`) |
//! | `Mice` property `a(ssub)` | read-only: the connected mice (no touchpads), each as `(key, name, dpi, stored)`: the `mouse_dpi` key of `compositor.json` (`vendor:product:name`, hex ids), the device name, the resolution in use (the stored one, else the hardware database's, else 1000) and whether it is the stored one; changes with `PropertiesChanged` on a mouse being plugged or removed and on a change of `mouse_dpi` |
//!
//! Which application goes first is approximate: the design walks its process list from the end, whose order was not
//! read; most recently used first is our choice.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use smithay::reexports::calloop::channel::{self, Event};
use smithay::reexports::wayland_server::Resource;
use zbus::object_server::SignalEmitter;

use crate::state::Cupertino;
use cupertino_wm::{AppId, Command};

pub const BUS_NAME: &str = "org.cupertino.Session1";
pub const OBJECT_PATH: &str = "/org/cupertino/Session1";

/// The application being quit.
#[derive(Debug)]
struct Quitting {
    app: AppId,
    identity: String,
}

/// State of the quit sequence and the link to the bus thread.
#[derive(Debug, Default)]
pub struct Quit {
    quitting: Option<Quitting>,
    /// What the bus thread is to emit; `None` until it runs.
    signals: Option<async_channel::Sender<BusOut>>,
}

/// What the compositor tells the bus thread.
#[derive(Debug)]
pub enum BusOut {
    ApplicationQuit(String),
    /// The appearance, the accessibility settings or the mice changed: the values now.
    Settings(SessionSettings),
}

/// One connected mouse as the `Mice` property lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MouseEntry {
    /// `cupertino_defaults::compositor::mouse_dpi_key` of the device.
    pub key: String,
    pub name: String,
    /// The resolution in use, in dots per inch.
    pub dpi: u32,
    /// The user stored the resolution (`mouse_dpi`), as opposed to the hardware database or the default.
    pub stored: bool,
}

/// The values of the properties.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SessionSettings {
    pub appearance: String,
    pub accessibility: HashMap<String, String>,
    pub mice: Vec<MouseEntry>,
}

/// Why a settings call was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    InvalidArgs(String),
    NotSupported(String),
    Failed(String),
}

/// A call that arrived on the bus, with where its answer goes.
pub enum Call {
    QuitNext(async_channel::Sender<(String, u32)>),
    ForceQuit(String, async_channel::Sender<u32>),
    SetAppearance(String, async_channel::Sender<Result<(), Refusal>>),
    SetAccessibility(String, String, async_channel::Sender<Result<(), Refusal>>),
}

impl Cupertino {
    /// The applications that have toplevels, most recently used first, then the others by id.
    fn apps_with_windows(&self) -> Vec<AppId> {
        let has_windows = |app: AppId| {
            self.windows
                .iter()
                .any(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        };
        let mut apps: Vec<AppId> = self
            .wm
            .app_mru()
            .iter()
            .copied()
            .filter(|a| has_windows(*a))
            .collect();
        let mut rest: Vec<AppId> = self
            .wm
            .apps()
            .map(|a| a.id)
            .filter(|a| !apps.contains(a) && has_windows(*a))
            .collect();
        rest.sort();
        apps.extend(rest);
        apps
    }

    /// The application being quit, until its last toplevel is gone.
    pub fn quitting(&self) -> Option<&str> {
        self.quit.quitting.as_ref().map(|q| q.identity.as_str())
    }

    /// `QuitNextApplication`: bring the next application forward and ask each of its toplevels to close.
    pub fn quit_next_application(&mut self) -> (String, u32) {
        let Some(app) = self.apps_with_windows().first().copied() else {
            self.quit.quitting = None;
            return (String::new(), 0);
        };
        let identity = self
            .app_identity_of(app)
            .expect("an application with windows is known to the window model");
        self.command(Command::ActivateApp { app });
        let mut windows = 0;
        for entry in self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        {
            entry.close();
            windows += 1;
        }
        tracing::info!(
            app = identity,
            windows,
            "log out: asking the application to quit"
        );
        self.quit.quitting = Some(Quitting {
            app,
            identity: identity.clone(),
        });
        (identity, windows)
    }

    /// `ForceQuitApplication`: SIGKILL the processes behind the application's toplevels.
    pub fn force_quit_application(&mut self, identity: &str) -> u32 {
        let Some(app) = self.wm.app_for_identity(identity) else {
            return 0;
        };
        let mut pids: Vec<u32> = Vec::new();
        for entry in self
            .windows
            .iter()
            .filter(|e| self.wm.window(e.id).is_some_and(|w| w.app == app))
        {
            let pid = entry
                .surface()
                .client()
                .and_then(|c| c.get_credentials(&self.display_handle).ok())
                .map(|c| c.pid as u32);
            match pid {
                Some(pid) if !pids.contains(&pid) => pids.push(pid),
                Some(_) => {}
                None => tracing::warn!(
                    app = identity,
                    "a window of the application has no known process"
                ),
            }
        }
        for pid in &pids {
            // Safety: kill(2) with a pid read from the client's socket credentials.
            if unsafe { libc::kill(*pid as libc::pid_t, libc::SIGKILL) } != 0 {
                tracing::warn!(app = identity, pid, error = %std::io::Error::last_os_error(), "force quit failed");
            }
        }
        tracing::info!(
            app = identity,
            processes = pids.len(),
            "log out: force quit"
        );
        pids.len() as u32
    }

    /// Announce the end of the application being quit once its last toplevel is gone; called from [`Cupertino::tick`].
    pub fn tick_quit(&mut self) {
        let Some(q) = &self.quit.quitting else { return };
        if self
            .windows
            .iter()
            .any(|e| self.wm.window(e.id).is_some_and(|w| w.app == q.app))
        {
            return;
        }
        let identity = self.quit.quitting.take().expect("checked above").identity;
        tracing::info!(app = identity, "log out: the application quit");
        if let Some(signals) = &self.quit.signals {
            if signals.try_send(BusOut::ApplicationQuit(identity)).is_err() {
                tracing::warn!(
                    "the session bus thread has stopped: cannot announce that the application quit"
                );
            }
        }
    }

    /// The values of the session interface's properties.
    pub fn session_settings(&self) -> SessionSettings {
        let appearance = if self.config.is_dark() {
            "dark"
        } else {
            "light"
        }
        .to_string();
        let accessibility = self
            .accessibility
            .settings
            .entries()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        SessionSettings {
            appearance,
            accessibility,
            mice: self.mice(),
        }
    }

    /// The connected mice by key; a key two devices share (the nodes of one mouse, identical mice) is listed once.
    fn mice(&self) -> Vec<MouseEntry> {
        let mut mice: Vec<MouseEntry> = self
            .pointer_devices
            .values()
            .filter_map(|device| device.mouse_entry(&self.config.pointer))
            .collect();
        mice.sort_by(|a, b| a.key.cmp(&b.key));
        mice.dedup_by(|a, b| a.key == b.key);
        mice
    }

    /// Tell the session interface that the appearance or the accessibility settings changed; called by every path
    /// that changes them.
    pub fn publish_session_settings(&self) {
        if let Some(signals) = &self.quit.signals {
            if signals
                .try_send(BusOut::Settings(self.session_settings()))
                .is_err()
            {
                tracing::warn!(
                    "the session bus thread has stopped: the settings properties are not updated"
                );
            }
        }
    }

    /// `SetAppearance`: store `appearance` in `compositor.json`, then apply it.
    pub fn set_session_appearance(&mut self, appearance: &str) -> Result<(), Refusal> {
        use cupertino_defaults::compositor::{AppearanceName, CompositorSettings};
        let (name, value) = match appearance {
            "light" => (AppearanceName::Light, cupertino_theme::Appearance::Light),
            "dark" => (AppearanceName::Dark, cupertino_theme::Appearance::Dark),
            "auto" => return Err(Refusal::NotSupported("the automatic appearance needs the time of day or a location, which the session has no source for".into())),
            other => return Err(Refusal::InvalidArgs(format!("unknown appearance {other:?}: light, dark or auto"))),
        };
        match self.live.paths.compositor.clone() {
            Some(path) => {
                let (mut file, _) = CompositorSettings::load(&path, None).map_err(|e| Refusal::Failed(format!("reading {}: {e}", path.display())))?;
                file.appearance = Some(name);
                file.store(&path).map_err(|e| Refusal::Failed(format!("writing {}: {e}", path.display())))?;
                // the watcher sees the key as it is now: the change is applied here, once
                self.live.files.compositor.appearance = Some(name);
            }
            // store, then apply: what cannot be stored is not applied either
            None => return Err(Refusal::Failed("no settings directory (neither XDG_CONFIG_HOME nor HOME is set): the appearance cannot be stored".into())),
        }
        self.set_appearance(value);
        Ok(())
    }

    /// `SetAccessibility`: set one key of `accessibility.conf`, apply and store it.
    pub fn set_session_accessibility(&mut self, key: &str, value: &str) -> Result<(), Refusal> {
        let mut settings = self.accessibility.settings;
        settings.set(key, value).map_err(Refusal::InvalidArgs)?;
        self.set_accessibility(settings);
        Ok(())
    }

    fn on_quit_call(&mut self, call: Call) {
        // a caller that went away before the answer is not an error of ours
        match call {
            Call::QuitNext(reply) => drop(reply.try_send(self.quit_next_application())),
            Call::ForceQuit(identity, reply) => {
                let _ = reply.try_send(self.force_quit_application(&identity));
            }
            Call::SetAppearance(appearance, reply) => {
                let _ = reply.try_send(self.set_session_appearance(&appearance));
            }
            Call::SetAccessibility(key, value, reply) => {
                let _ = reply.try_send(self.set_session_accessibility(&key, &value));
            }
        }
    }
}

struct Session1 {
    calls: std::sync::Mutex<channel::Sender<Call>>,
    /// What the properties read; the bus thread updates it from [`BusOut::Settings`].
    settings: Arc<Mutex<SessionSettings>>,
}

fn refused(r: Refusal) -> zbus::fdo::Error {
    match r {
        Refusal::InvalidArgs(m) => zbus::fdo::Error::InvalidArgs(m),
        Refusal::NotSupported(m) => zbus::fdo::Error::NotSupported(m),
        Refusal::Failed(m) => zbus::fdo::Error::Failed(m),
    }
}

impl Session1 {
    async fn ask<T>(
        &self,
        make: impl FnOnce(async_channel::Sender<T>) -> Call,
    ) -> zbus::fdo::Result<T> {
        let (tx, rx) = async_channel::bounded(1);
        self.calls
            .lock()
            .expect("no thread panics while sending a call")
            .send(make(tx))
            .map_err(|_| zbus::fdo::Error::Failed("the compositor is shutting down".into()))?;
        rx.recv()
            .await
            .map_err(|_| zbus::fdo::Error::Failed("the compositor dropped the call".into()))
    }
}

#[zbus::interface(name = "org.cupertino.Session1")]
impl Session1 {
    async fn quit_next_application(&self) -> zbus::fdo::Result<(String, u32)> {
        self.ask(Call::QuitNext).await
    }

    async fn force_quit_application(&self, app_id: String) -> zbus::fdo::Result<u32> {
        self.ask(|tx| Call::ForceQuit(app_id, tx)).await
    }

    async fn set_appearance(&self, appearance: String) -> zbus::fdo::Result<()> {
        self.ask(|tx| Call::SetAppearance(appearance, tx))
            .await?
            .map_err(refused)
    }

    async fn set_accessibility(&self, key: String, value: String) -> zbus::fdo::Result<()> {
        self.ask(|tx| Call::SetAccessibility(key, value, tx))
            .await?
            .map_err(refused)
    }

    #[zbus(property)]
    async fn appearance(&self) -> String {
        self.settings
            .lock()
            .expect("no thread panics while holding the settings")
            .appearance
            .clone()
    }

    #[zbus(property)]
    async fn accessibility(&self) -> HashMap<String, String> {
        self.settings
            .lock()
            .expect("no thread panics while holding the settings")
            .accessibility
            .clone()
    }

    /// Read-only on purpose: the resolution a user stores goes through `compositor.json`.
    #[zbus(property)]
    async fn mice(&self) -> Vec<(String, String, u32, bool)> {
        self.settings
            .lock()
            .expect("no thread panics while holding the settings")
            .mice
            .iter()
            .map(|m| (m.key.clone(), m.name.clone(), m.dpi, m.stored))
            .collect()
    }

    #[zbus(signal)]
    async fn application_quit(emitter: &SignalEmitter<'_>, app_id: &str) -> zbus::Result<()>;
}

/// Serve `org.cupertino.Session1` on the session bus from a thread of its own. Failures (no bus, the name owned by
/// another compositor) are logged: the session agent then logs out without quitting the applications first.
pub fn start_session_service(state: &mut Cupertino) {
    let (calls, rx) = channel::channel::<Call>();
    if let Err(e) = state.loop_handle.insert_source(rx, |event, _, state| {
        if let Event::Msg(call) = event {
            state.on_quit_call(call);
        }
    }) {
        tracing::warn!(error = %e, "cannot serve the session interface");
        return;
    }
    let (signals, signals_rx) = async_channel::unbounded::<BusOut>();
    state.quit.signals = Some(signals);
    let settings = Arc::new(Mutex::new(state.session_settings()));
    let spawned = std::thread::Builder::new().name("cupertino-session".into()).spawn(move || {
        let served = zbus::block_on(async {
            let iface = Session1 { calls: std::sync::Mutex::new(calls), settings: Arc::clone(&settings) };
            let conn = zbus::connection::Builder::session()?.serve_at(OBJECT_PATH, iface)?.name(BUS_NAME)?.build().await?;
            let emitter = SignalEmitter::new(&conn, OBJECT_PATH)?;
            let served = conn.object_server().interface::<_, Session1>(OBJECT_PATH).await?;
            while let Ok(out) = signals_rx.recv().await {
                match out {
                    BusOut::ApplicationQuit(app_id) => Session1::application_quit(&emitter, &app_id).await?,
                    BusOut::Settings(now) => {
                        let before = std::mem::replace(&mut *settings.lock().expect("no thread panics while holding the settings"), now.clone());
                        let iface = served.get().await;
                        if before.appearance != now.appearance {
                            iface.appearance_changed(&emitter).await?;
                        }
                        if before.accessibility != now.accessibility {
                            iface.accessibility_changed(&emitter).await?;
                        }
                        if before.mice != now.mice {
                            iface.mice_changed(&emitter).await?;
                        }
                    }
                }
            }
            Ok::<(), zbus::Error>(())
        });
        if let Err(e) = served {
            tracing::warn!(error = %e, "the session interface {BUS_NAME} is not served: a log out does not quit the applications first");
        }
    });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot start the session interface thread");
    }
}
