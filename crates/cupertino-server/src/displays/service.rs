//! `org.cupertino.Displays1` on the session bus (D14): the display listing and configuration
//! entry point for Settings and other authorized programs, replacing raw `zwlr_output_management_v1`
//! connections from applications.
//!
//! Reading (`ListDisplays`) is open to every caller. Mutating methods (`SetScale`, `SetRefreshRate`,
//! `SetPosition`, `Configure`, `TestConfigure`) require `Need::SettingsClient` (Settings, the
//! session agent, or the server itself) and are refused while the session is locked.

use smithay::reexports::calloop::channel::{self, Event};
use smithay::utils::Transform;
use zbus::object_server::SignalEmitter;

use crate::caller::{identify, Caller, Denial, Need, Sandbox};
use crate::displays::{HeadMode, HeadState};
use crate::quit::{Refusal, Reject};
use crate::state::Cupertino;

/// Well-known session-bus name for the displays service.
pub const BUS_NAME: &str = "org.cupertino.Displays1";
/// Object path served on the session bus.
pub const OBJECT_PATH: &str = "/org/cupertino/Displays1";

/// One connected display on the wire:
/// `(connector, description, enabled, x, y, width, height, refresh_mhz, scale, transform, modes, physical_mm)`
/// where each mode is `(width, height, refresh_mhz)` and `physical_mm` is the picture's width and height in
/// millimetres, `(0, 0)` when the display does not report it.
pub type DisplayRecord = (
    String,
    String,
    bool,
    i32,
    i32,
    i32,
    i32,
    u32,
    f64,
    String,
    Vec<(i32, i32, u32)>,
    (u32, u32),
);

/// Signals emitted by the compositor to the `Displays1` bus thread.
#[derive(Debug)]
pub enum DisplaysBusOut {
    Changed,
}

/// Calls dispatched from the `Displays1` bus thread onto the compositor loop.
pub enum DisplaysCall {
    List(async_channel::Sender<Result<Vec<DisplayRecord>, Reject>>),
    SetScale {
        connector: String,
        scale: f64,
        caller: Caller,
        reply: async_channel::Sender<Result<(), Reject>>,
    },
    SetRefreshRate {
        connector: String,
        refresh_mhz: u32,
        caller: Caller,
        reply: async_channel::Sender<Result<(), Reject>>,
    },
    SetPosition {
        connector: String,
        x: i32,
        y: i32,
        caller: Caller,
        reply: async_channel::Sender<Result<(), Reject>>,
    },
    Configure {
        connector: String,
        enabled: bool,
        width: i32,
        height: i32,
        refresh_mhz: u32,
        x: i32,
        y: i32,
        scale: f64,
        transform: String,
        test_only: bool,
        caller: Caller,
        reply: async_channel::Sender<Result<(), Reject>>,
    },
}

/// A display's physical size on the wire: `(0, 0)` when either side is unknown.
fn physical_size((width, height): (i32, i32)) -> (u32, u32) {
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(w), Ok(h)) if w > 0 && h > 0 => (w, h),
        _ => (0, 0),
    }
}

/// Wire name for a Smithay `Transform`.
pub fn transform_name(transform: Transform) -> &'static str {
    match transform {
        Transform::Normal => "normal",
        Transform::_90 => "90",
        Transform::_180 => "180",
        Transform::_270 => "270",
        Transform::Flipped => "flipped",
        Transform::Flipped90 => "flipped-90",
        Transform::Flipped180 => "flipped-180",
        Transform::Flipped270 => "flipped-270",
    }
}

/// Parse a wire transform name into a Smithay `Transform`.
pub fn parse_transform(name: &str) -> Option<Transform> {
    match name {
        "normal" => Some(Transform::Normal),
        "90" => Some(Transform::_90),
        "180" => Some(Transform::_180),
        "270" => Some(Transform::_270),
        "flipped" => Some(Transform::Flipped),
        "flipped-90" => Some(Transform::Flipped90),
        "flipped-180" => Some(Transform::Flipped180),
        "flipped-270" => Some(Transform::Flipped270),
        _ => None,
    }
}

impl Cupertino {
    /// Snapshot every connected display head for `org.cupertino.Displays1.ListDisplays`.
    pub fn list_displays(&self) -> Vec<DisplayRecord> {
        self.displays
            .heads
            .iter()
            .map(|h| {
                let description = match (h.info.make.is_empty(), h.info.model.is_empty()) {
                    (true, true) => h.info.connector.clone(),
                    (false, true) => h.info.make.clone(),
                    (true, false) => h.info.model.clone(),
                    (false, false) => format!("{} {}", h.info.make, h.info.model),
                };
                let modes: Vec<(i32, i32, u32)> = h
                    .info
                    .modes
                    .iter()
                    .map(|m| (m.width, m.height, m.refresh_mhz.max(0) as u32))
                    .collect();
                (
                    h.info.connector.clone(),
                    description,
                    h.state.enabled,
                    h.state.position.0,
                    h.state.position.1,
                    h.state.mode.width,
                    h.state.mode.height,
                    h.state.mode.refresh_mhz.max(0) as u32,
                    h.state.scale,
                    transform_name(h.state.transform).to_owned(),
                    modes,
                    physical_size(h.info.physical_mm),
                )
            })
            .collect()
    }

    fn find_head_by_connector(
        &self,
        connector: &str,
    ) -> Result<(super::HeadId, HeadState), Refusal> {
        self.displays
            .heads
            .iter()
            .find(|h| h.info.connector == connector)
            .map(|h| (h.id, h.state))
            .ok_or_else(|| Refusal::InvalidArgs(format!("unknown display connector {connector:?}")))
    }

    fn check_displays_unlocked(&self) -> Result<(), Refusal> {
        if self.session_lock.is_active() {
            return Err(Refusal::Failed(
                "cannot configure displays while the session is locked".into(),
            ));
        }
        Ok(())
    }

    /// Set the scale of display `connector`, keeping its mode, position, and transform unchanged.
    pub fn display_set_scale(&mut self, connector: &str, scale: f64) -> Result<(), Refusal> {
        self.check_displays_unlocked()?;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Refusal::InvalidArgs(format!(
                "scale {scale} must be finite and above zero"
            )));
        }
        let (id, mut state) = self.find_head_by_connector(connector)?;
        state.scale = scale;
        self.configure_heads(&[(id, state)], false)
            .map_err(Refusal::Failed)?;
        self.heads_changed();
        Ok(())
    }

    /// Set the refresh rate (millihertz) of display `connector` at its current resolution.
    pub fn display_set_refresh_rate(
        &mut self,
        connector: &str,
        refresh_mhz: u32,
    ) -> Result<(), Refusal> {
        self.check_displays_unlocked()?;
        let (id, mut state) = self.find_head_by_connector(connector)?;
        let refresh = i32::try_from(refresh_mhz).map_err(|_| {
            Refusal::InvalidArgs(format!("refresh rate {refresh_mhz} out of range"))
        })?;
        state.mode = HeadMode {
            width: state.mode.width,
            height: state.mode.height,
            refresh_mhz: refresh,
        };
        self.configure_heads(&[(id, state)], false)
            .map_err(Refusal::Failed)?;
        self.heads_changed();
        Ok(())
    }

    /// Move display `connector` to logical position `(x, y)`.
    pub fn display_set_position(&mut self, connector: &str, x: i32, y: i32) -> Result<(), Refusal> {
        self.check_displays_unlocked()?;
        let (id, mut state) = self.find_head_by_connector(connector)?;
        state.position = (x, y);
        self.configure_heads(&[(id, state)], false)
            .map_err(Refusal::Failed)?;
        self.heads_changed();
        Ok(())
    }

    /// Apply (or validate when `test_only`) a full head state for `connector`.
    #[allow(clippy::too_many_arguments)]
    pub fn display_configure(
        &mut self,
        connector: &str,
        enabled: bool,
        width: i32,
        height: i32,
        refresh_mhz: u32,
        x: i32,
        y: i32,
        scale: f64,
        transform: &str,
        test_only: bool,
    ) -> Result<(), Refusal> {
        self.check_displays_unlocked()?;
        if !(scale.is_finite() && scale > 0.0) {
            return Err(Refusal::InvalidArgs(format!(
                "scale {scale} must be finite and above zero"
            )));
        }
        let parsed_transform = parse_transform(transform)
            .ok_or_else(|| Refusal::InvalidArgs(format!("unknown transform {transform:?}")))?;
        let refresh = i32::try_from(refresh_mhz).map_err(|_| {
            Refusal::InvalidArgs(format!("refresh rate {refresh_mhz} out of range"))
        })?;
        let (id, _) = self.find_head_by_connector(connector)?;
        let wanted = HeadState {
            enabled,
            mode: HeadMode {
                width,
                height,
                refresh_mhz: refresh,
            },
            position: (x, y),
            scale,
            transform: parsed_transform,
        };
        self.configure_heads(&[(id, wanted)], test_only)
            .map_err(Refusal::Failed)?;
        if !test_only {
            self.heads_changed();
        }
        Ok(())
    }

    /// Notify the `Displays1` bus thread that connected heads or their state changed.
    pub(crate) fn notify_displays_bus_changed(&self) {
        if let Some(signals) = &self.displays.bus_signals {
            let _ = signals.try_send(DisplaysBusOut::Changed);
        }
    }

    /// Handle one call from the `Displays1` bus thread on the compositor loop.
    pub fn on_displays_call(&mut self, call: DisplaysCall) {
        match call {
            DisplaysCall::List(reply) => {
                let _ = reply.try_send(Ok(self.list_displays()));
            }
            DisplaysCall::SetScale {
                connector,
                scale,
                caller,
                reply,
            } => {
                let answer = self
                    .authorize_call("Displays1.SetScale", &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.display_set_scale(&connector, scale)
                            .map_err(Reject::Refusal)
                    });
                let _ = reply.try_send(answer);
            }
            DisplaysCall::SetRefreshRate {
                connector,
                refresh_mhz,
                caller,
                reply,
            } => {
                let answer = self
                    .authorize_call("Displays1.SetRefreshRate", &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.display_set_refresh_rate(&connector, refresh_mhz)
                            .map_err(Reject::Refusal)
                    });
                let _ = reply.try_send(answer);
            }
            DisplaysCall::SetPosition {
                connector,
                x,
                y,
                caller,
                reply,
            } => {
                let answer = self
                    .authorize_call("Displays1.SetPosition", &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.display_set_position(&connector, x, y)
                            .map_err(Reject::Refusal)
                    });
                let _ = reply.try_send(answer);
            }
            DisplaysCall::Configure {
                connector,
                enabled,
                width,
                height,
                refresh_mhz,
                x,
                y,
                scale,
                transform,
                test_only,
                caller,
                reply,
            } => {
                let method = if test_only {
                    "Displays1.TestConfigure"
                } else {
                    "Displays1.Configure"
                };
                let answer = self
                    .authorize_call(method, &caller, Need::SettingsClient)
                    .map_err(Reject::Caller)
                    .and_then(|()| {
                        self.display_configure(
                            &connector,
                            enabled,
                            width,
                            height,
                            refresh_mhz,
                            x,
                            y,
                            scale,
                            &transform,
                            test_only,
                        )
                        .map_err(Reject::Refusal)
                    });
                let _ = reply.try_send(answer);
            }
        }
    }
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.cupertino.Displays1.Error")]
enum DisplaysCallError {
    #[zbus(error)]
    ZBus(zbus::Error),
    AnonymousCaller(String),
    FlatpakCaller(String),
    SnapCaller(String),
    UntrustedCaller(String),
}

impl From<zbus::fdo::Error> for DisplaysCallError {
    fn from(e: zbus::fdo::Error) -> Self {
        DisplaysCallError::ZBus(e.into())
    }
}

impl From<Reject> for DisplaysCallError {
    fn from(reject: Reject) -> Self {
        match reject {
            Reject::Caller(Denial::Anonymous) => {
                DisplaysCallError::AnonymousCaller("the bus gave no process for this caller".into())
            }
            Reject::Caller(Denial::Sandboxed(Sandbox::Flatpak)) => {
                DisplaysCallError::FlatpakCaller("a Flatpak may not configure displays".into())
            }
            Reject::Caller(Denial::Sandboxed(Sandbox::Snap)) => {
                DisplaysCallError::SnapCaller("a Snap may not configure displays".into())
            }
            Reject::Caller(Denial::Untrusted) => {
                DisplaysCallError::UntrustedCaller("this process may not configure displays".into())
            }
            Reject::Refusal(Refusal::InvalidArgs(m)) => zbus::fdo::Error::InvalidArgs(m).into(),
            Reject::Refusal(Refusal::NotSupported(m)) => zbus::fdo::Error::NotSupported(m).into(),
            Reject::Refusal(Refusal::Failed(m)) => zbus::fdo::Error::Failed(m).into(),
        }
    }
}

struct Displays1 {
    calls: std::sync::Mutex<channel::Sender<DisplaysCall>>,
}

impl Displays1 {
    async fn ask<T>(
        &self,
        make: impl FnOnce(async_channel::Sender<Result<T, Reject>>) -> DisplaysCall,
    ) -> Result<T, DisplaysCallError> {
        let (tx, rx) = async_channel::bounded(1);
        self.calls
            .lock()
            .expect("no thread panics while sending a displays call")
            .send(make(tx))
            .map_err(|_| zbus::fdo::Error::Failed("the compositor is shutting down".into()))?;
        rx.recv()
            .await
            .map_err(|_| zbus::fdo::Error::Failed("the compositor dropped the call".into()))?
            .map_err(DisplaysCallError::from)
    }
}

async fn caller_of(header: &zbus::message::Header<'_>, connection: &zbus::Connection) -> Caller {
    match header.sender() {
        Some(sender) => identify(connection, sender).await,
        None => Caller::Anonymous,
    }
}

#[zbus::interface(name = "org.cupertino.Displays1")]
impl Displays1 {
    async fn list_displays(&self) -> Result<Vec<DisplayRecord>, DisplaysCallError> {
        self.ask(DisplaysCall::List).await
    }

    async fn set_scale(
        &self,
        connector: String,
        scale: f64,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), DisplaysCallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|reply| DisplaysCall::SetScale {
            connector,
            scale,
            caller,
            reply,
        })
        .await
    }

    async fn set_refresh_rate(
        &self,
        connector: String,
        refresh_mhz: u32,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), DisplaysCallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|reply| DisplaysCall::SetRefreshRate {
            connector,
            refresh_mhz,
            caller,
            reply,
        })
        .await
    }

    async fn set_position(
        &self,
        connector: String,
        x: i32,
        y: i32,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), DisplaysCallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|reply| DisplaysCall::SetPosition {
            connector,
            x,
            y,
            caller,
            reply,
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn configure(
        &self,
        connector: String,
        enabled: bool,
        width: i32,
        height: i32,
        refresh_mhz: u32,
        x: i32,
        y: i32,
        scale: f64,
        transform: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), DisplaysCallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|reply| DisplaysCall::Configure {
            connector,
            enabled,
            width,
            height,
            refresh_mhz,
            x,
            y,
            scale,
            transform,
            test_only: false,
            caller,
            reply,
        })
        .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn test_configure(
        &self,
        connector: String,
        enabled: bool,
        width: i32,
        height: i32,
        refresh_mhz: u32,
        x: i32,
        y: i32,
        scale: f64,
        transform: String,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(connection)] connection: &zbus::Connection,
    ) -> Result<(), DisplaysCallError> {
        let caller = caller_of(&header, connection).await;
        self.ask(|reply| DisplaysCall::Configure {
            connector,
            enabled,
            width,
            height,
            refresh_mhz,
            x,
            y,
            scale,
            transform,
            test_only: true,
            caller,
            reply,
        })
        .await
    }

    #[zbus(signal)]
    async fn displays_changed(emitter: &SignalEmitter<'_>) -> zbus::Result<()>;
}

/// Serve `org.cupertino.Displays1` on the session bus from a dedicated thread.
pub fn start_displays_service(state: &mut Cupertino) {
    start_displays_service_on(state, None);
}

/// Serve `org.cupertino.Displays1` on the bus at `address` (the session bus when `None`).
pub fn start_displays_service_on(state: &mut Cupertino, address: Option<String>) {
    let (calls, rx) = channel::channel::<DisplaysCall>();
    if let Err(e) = state.loop_handle.insert_source(rx, |event, _, state| {
        if let Event::Msg(call) = event {
            state.on_displays_call(call);
        }
    }) {
        tracing::warn!(error = %e, "cannot serve the displays interface");
        return;
    }
    let (signals, signals_rx) = async_channel::unbounded::<DisplaysBusOut>();
    state.displays.bus_signals = Some(signals);
    let spawned = std::thread::Builder::new()
        .name("displays-service".into())
        .spawn(move || {
            let mut pause = crate::dbus::RECONNECT_FIRST;
            let mut served_before = false;
            loop {
                let ended = zbus::block_on(serve_displays(&address, &calls, &signals_rx));
                match ended {
                    Ok(true) => break,
                    Ok(false) => {
                        tracing::warn!(
                            "the session bus connection dropped: serving {BUS_NAME} again"
                        );
                        pause = crate::dbus::RECONNECT_FIRST;
                        served_before = true;
                    }
                    Err(e) => {
                        if served_before {
                            tracing::debug!(error = %e, "the displays interface {BUS_NAME} is not served yet");
                        } else {
                            tracing::warn!(error = %e, "the displays interface {BUS_NAME} is not served");
                            served_before = true;
                        }
                        pause = (pause * 2).min(crate::dbus::RECONNECT_LONGEST);
                    }
                }
                std::thread::sleep(pause);
            }
        });
    if let Err(e) = spawned {
        tracing::warn!(error = %e, "cannot start the displays interface thread");
    }
}

async fn serve_displays(
    address: &Option<String>,
    calls: &channel::Sender<DisplaysCall>,
    signals_rx: &async_channel::Receiver<DisplaysBusOut>,
) -> zbus::Result<bool> {
    let iface = Displays1 {
        calls: std::sync::Mutex::new(calls.clone()),
    };
    let conn = crate::dbus::bus(address.as_deref())?
        .serve_at(OBJECT_PATH, iface)?
        .name(BUS_NAME)?
        .build()
        .await?;
    let emitter = SignalEmitter::new(&conn, OBJECT_PATH)?;
    loop {
        let out = match crate::dbus::either(signals_rx.recv(), conn.closed()).await {
            crate::dbus::Either::First(Ok(out)) => out,
            crate::dbus::Either::First(Err(_)) => return Ok(true),
            crate::dbus::Either::Second(()) => return Ok(false),
        };
        match out {
            DisplaysBusOut::Changed => Displays1::displays_changed(&emitter).await?,
        }
    }
}
