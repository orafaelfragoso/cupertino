//! Display and system power: runs the idle policy of `display_power` on the model clock, draws the dim and sleep overlay,
//! turns the outputs off on the TTY backend, locks the session after display sleep, and puts the system to sleep through
//! logind.
//!
//! The facts the policy needs from outside come from a system bus watcher ([`start_power_bus`]): the power source
//! (`OnBattery` of `org.freedesktop.UPower`, served by `cupertino-powerd`), logind's `idle` block inhibitors and the end of
//! a system sleep (`PrepareForSleep(false)`, which counts as input). Without a system bus or a power daemon the mains
//! timers apply, and the server says so once.
//!
//! The server holds a logind `sleep` delay inhibitor of its own. At `PrepareForSleep(true)` it covers the session
//! (`session_lock.rs`, [`Cupertino::system_going_to_sleep`]) and releases the inhibitor once every output shows a covered
//! frame, so what the screens show when the system wakes is never the session; it takes a new one after the wake.
//!
//! What a sleeping display looks like depends on the backend: on DRM every CRTC is switched off (`DrmCompositor::clear`,
//! the DPMS-off path) and nothing is rendered until input; the other backends draw an opaque black overlay and keep
//! taking input. The dim is a black overlay whose alpha shows the dim factor (`display_power::overlay_alpha`).

use std::sync::mpsc;

use cupertino_diagnostic_events::{self as diag, Severity, Value};

use smithay::reexports::calloop::channel::{self, Event};

use crate::display_power::{
    overlay_alpha, ActivationFade, DisplayState, Effect, Facts, IdlePolicy,
};
use crate::state::Cupertino;

/// The idle policy and the facts it is fed.
pub struct Power {
    policy: IdlePolicy,
    /// `None` until the power daemon answered (and while there is none): the mains timers apply.
    on_battery: Option<bool>,
    /// logind holds an `idle` block inhibitor.
    idle_blocked: bool,
    /// Asks logind to suspend; `None` without a system bus.
    bus: Option<mpsc::Sender<BusRequest>>,
    /// The fade from black after logind resumes this session.
    fade: ActivationFade,
}

impl Power {
    pub fn new(policy: IdlePolicy) -> Self {
        Self {
            policy,
            on_battery: None,
            idle_blocked: false,
            bus: None,
            fade: ActivationFade::default(),
        }
    }

    pub fn policy(&self) -> &IdlePolicy {
        &self.policy
    }

    /// Take new timers (`power.conf` changed).
    pub fn set_settings(&mut self, settings: cupertino_defaults::power::PowerSettings) {
        self.policy.set_settings(settings);
    }

    /// The display is asleep: the TTY backend renders nothing.
    pub fn asleep(&self) -> bool {
        self.policy.state() == DisplayState::Asleep
    }

    /// Record the power source (tests set it directly; the bus watcher reports it).
    pub fn set_on_battery(&mut self, on_battery: bool) {
        self.on_battery = Some(on_battery);
    }
}

/// What the watcher thread reports.
#[derive(Debug)]
pub enum BusEvent {
    OnBattery(bool),
    /// No power daemon answers; the mains timers apply.
    NoPowerSource(String),
    /// logind's `BlockInhibited` names `idle` (or no longer does).
    IdleBlocked(bool),
    /// The system is about to sleep (`PrepareForSleep(true)`).
    Sleeping,
    /// The system woke from sleep.
    Resumed,
    /// The watcher stopped; the facts it reported stay as they were.
    Stopped(String),
}

/// Records `power.suspend.failed`: the idle policy's system sleep did not happen, for `reason` (`no_system_bus`,
/// `watcher_stopped`, `logind_refused`), with the name of logind's error when it gave one ([`error_name`]).
fn suspend_failed(reason: &'static str, error: Option<&str>) {
    let mut event = diag::event(
        Severity::Error,
        "power.suspend.failed",
        "the system could not be put to sleep",
    )
    .field("reason", Value::Public(reason));
    if let Some(error) = error {
        event = event.field("error", Value::Public(error));
    }
    event.emit();
}

/// What an event keeps of a failed call to logind: the D-Bus error's name, else the kind of failure. Never logind's
/// text: a refusal for an inhibitor names the program and the user that hold it and the reason they gave.
fn error_name(error: &zbus::Error) -> String {
    match error {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(error) => zbus::DBusError::name(error.as_ref()).to_string(),
        zbus::Error::InputOutput(_) => "io".to_owned(),
        zbus::Error::Variant(_) => "reply".to_owned(),
        _ => "bus".to_owned(),
    }
}

/// What the main loop asks of the bus.
#[derive(Debug)]
enum BusRequest {
    Suspend,
    /// Take the sleep delay inhibitor (after a wake); the watcher takes one as it starts.
    HoldSleep,
    /// Let the sleep go on: drop the delay inhibitor.
    ReleaseSleep,
}

impl Cupertino {
    /// Input arrived (any key, pointer or touch event): restart the idle countdowns and wake the display.
    pub fn input_activity(&mut self) {
        self.idle.notifier.notify_activity(&self.seat);
        let now = self.now();
        let effects = self.power.policy.input(now);
        self.apply_power_effects(effects);
    }

    fn display_power_facts(&self) -> Facts {
        Facts {
            on_battery: self.power.on_battery.unwrap_or(false),
            inhibited: self.idle.inhibited() || self.power.idle_blocked,
            locked: self.session_lock.is_active(),
        }
    }

    /// When the idle policy next has something to do.
    pub fn display_power_deadline(&self) -> Option<f64> {
        self.power.policy.next_deadline(self.display_power_facts())
    }

    /// Advance the idle policy; called from [`Cupertino::tick`].
    pub fn tick_display_power(&mut self) {
        let now = self.now();
        let facts = self.display_power_facts();
        let effects = self.power.policy.tick(now, facts);
        self.apply_power_effects(effects);
        if self.power.policy.animating(now) || self.power.fade.animating(now) {
            self.request_redraw();
        }
    }

    /// logind made this session the active one (the console returned to it): the screen fades in from black.
    pub fn session_activated(&mut self) {
        let now = self.now();
        tracing::info!("fading in from black");
        self.power.fade.begin(now);
        self.request_redraw();
    }

    fn apply_power_effects(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Display(state) => {
                    tracing::info!(state = state_name(state), "display power");
                    self.set_outputs_asleep(state == DisplayState::Asleep);
                    self.request_redraw();
                }
                Effect::Lock => {
                    tracing::info!("the display slept for the password delay: locking the session");
                    self.lock_session();
                }
                Effect::SystemSleep => {
                    tracing::info!("idle: putting the system to sleep");
                    match &self.power.bus {
                        Some(bus) => {
                            if bus.send(BusRequest::Suspend).is_err() {
                                tracing::warn!("the system bus watcher has stopped: cannot put the system to sleep");
                                suspend_failed("watcher_stopped", None);
                            }
                        }
                        None => {
                            tracing::warn!("no system bus: cannot put the system to sleep");
                            suspend_failed("no_system_bus", None);
                        }
                    }
                }
            }
        }
    }

    fn set_outputs_asleep(&mut self, asleep: bool) {
        #[cfg(feature = "udev")]
        if let Some(crate::backend::Backend::Udev(u)) = self.backend.as_mut() {
            u.set_asleep(asleep);
            if asleep {
                // the cleared CRTCs dropped any flip in flight
                for o in &mut self.outputs {
                    o.scheduler.reset();
                }
            }
        }
        #[cfg(not(feature = "udev"))]
        let _ = asleep;
    }

    /// Opacity of the black overlay while the display is dimmed, asleep or ramping.
    pub fn power_overlay_alpha(&self) -> Option<f64> {
        let alpha = overlay_alpha(self.power.policy.factor(self.now()));
        // the sRGB curve leaves a rounding residue at factor 1
        (alpha >= 1e-6).then_some(alpha)
    }

    /// Opacity of the black overlay while the screen fades in after the session became active.
    pub fn activation_overlay_alpha(&self) -> Option<f64> {
        let alpha = self.power.fade.alpha(self.now());
        (alpha >= 1e-6).then_some(alpha)
    }

    fn on_power_bus(&mut self, event: BusEvent) {
        match event {
            BusEvent::OnBattery(on_battery) => {
                if self.power.on_battery != Some(on_battery) {
                    tracing::info!(on_battery, "power source");
                }
                self.power.on_battery = Some(on_battery);
            }
            BusEvent::NoPowerSource(reason) => {
                tracing::info!(reason, "power source unknown: the mains timers apply");
                self.power.on_battery = None;
            }
            BusEvent::IdleBlocked(blocked) => {
                tracing::info!(blocked, "logind idle inhibitor");
                self.power.idle_blocked = blocked;
            }
            BusEvent::Sleeping => {
                tracing::info!("the system is about to sleep");
                self.system_going_to_sleep();
            }
            BusEvent::Resumed => {
                tracing::info!("the system woke from sleep");
                self.power_request(BusRequest::HoldSleep);
                // a display plugged or swapped while the system slept sends no uevent when the kernel did not see it
                #[cfg(feature = "udev")]
                crate::backend::udev::scan_connectors(self);
                self.input_activity();
            }
            BusEvent::Stopped(reason) => tracing::warn!(reason, "the system bus watcher stopped"),
        }
    }
}

impl Cupertino {
    /// Let a system sleep the server held back go on (the session is covered, `session_lock.rs`).
    pub(crate) fn release_sleep(&mut self) {
        self.power_request(BusRequest::ReleaseSleep);
    }

    fn power_request(&mut self, request: BusRequest) {
        if let Some(bus) = &self.power.bus {
            if bus.send(request).is_err() {
                tracing::warn!("the system bus watcher has stopped");
            }
        }
    }
}

fn state_name(state: DisplayState) -> &'static str {
    match state {
        DisplayState::On => "on",
        DisplayState::Dimmed => "dimmed",
        DisplayState::Asleep => "asleep",
    }
}

/// Watch the power source and logind on the system bus and serve the suspend request. Failures are logged; the policy then
/// runs on the mains timers without logind inhibitors.
pub fn start_power_bus(state: &mut Cupertino) {
    let (events, rx) = channel::channel::<BusEvent>();
    let inserted = state.loop_handle.insert_source(rx, |event, _, state| {
        if let Event::Msg(event) = event {
            state.on_power_bus(event);
        }
    });
    if let Err(e) = inserted {
        tracing::warn!(error = %e, "cannot watch the system bus");
        return;
    }
    let (requests, requests_rx) = mpsc::channel();
    match bus::start(events, requests_rx) {
        Ok(()) => state.power.bus = Some(requests),
        Err(e) => {
            tracing::warn!(error = %e, "no system bus: power source, idle inhibitors and system sleep are unavailable")
        }
    }
}

mod bus {
    //! The watcher threads. The blocking `zbus` API is used as in `cupertino-login`: one reader thread per signal stream.
    //! Each stream passes on a signal only from its service, the owner of the service's name (`from_owner`) or the bus daemon
    //! ([`only_from`]), checked when the signal arrives: the bus delivers a signal addressed to this process whatever its
    //! rules say, so another user could otherwise hold off the idle lock with a forged `BlockInhibited` or `PrepareForSleep`.

    use std::sync::mpsc::Receiver;

    use cupertino_diagnostic_events::{self as diag, Severity, Value};
    use cupertino_peer::signal::{from_owner, sent_by_bus, FromOwner};
    use smithay::reexports::calloop::channel::Sender;
    use zbus::blocking::{Connection, MessageIterator};
    use zbus::message::Type;
    use zbus::zvariant::OwnedValue;
    use zbus::MatchRule;

    use super::{BusEvent, BusRequest};

    const UPOWER: &str = "org.freedesktop.UPower";
    const UPOWER_PATH: &str = "/org/freedesktop/UPower";
    const LOGIND: &str = "org.freedesktop.login1";
    const LOGIND_PATH: &str = "/org/freedesktop/login1";
    const LOGIND_MANAGER: &str = "org.freedesktop.login1.Manager";
    const PROPERTIES: &str = "org.freedesktop.DBus.Properties";

    pub fn start(events: Sender<BusEvent>, requests: Receiver<BusRequest>) -> zbus::Result<()> {
        start_on(Connection::system()?, events, requests)
    }

    /// The watchers and the request thread on `conn`, a connection to the system bus outside tests: the services are
    /// found by their well-known names on whatever bus `conn` is on.
    pub(super) fn start_on(
        conn: Connection,
        events: Sender<BusEvent>,
        requests: Receiver<BusRequest>,
    ) -> zbus::Result<()> {
        let ignored = |_: &zbus::Message| {
            tracing::warn!("ignored a power signal that its service did not send")
        };
        let changes = |name: &str, interface: &str, path: &str| {
            let rule = MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(name)?
                .interface(PROPERTIES)?
                .member("PropertiesChanged")?
                .path(path)?
                .arg(0, interface)?
                .build();
            from_owner(&conn, name, rule, ignored)
        };
        let upower_changes = changes(UPOWER, UPOWER, UPOWER_PATH)?;
        let logind_changes = changes(LOGIND, LOGIND_MANAGER, LOGIND_PATH)?;
        let owner_rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender("org.freedesktop.DBus")?
            .member("NameOwnerChanged")?
            .arg(0, UPOWER)?
            .build();
        let owners = only_from(
            MessageIterator::for_match_rule(owner_rule, &conn, None)?,
            sent_by_bus,
        );
        let sleeps = sleep_signals(&conn, LOGIND)?;

        // the subscriptions are up: read the current values
        report_on_battery(&conn, &events);
        match get(&conn, LOGIND, LOGIND_PATH, LOGIND_MANAGER, "BlockInhibited")
            .and_then(|v| String::try_from(v).map_err(zbus::Error::from))
        {
            Ok(what) => send(&events, BusEvent::IdleBlocked(names_idle(&what))),
            Err(e) => send(
                &events,
                BusEvent::Stopped(format!("reading logind's BlockInhibited: {e}")),
            ),
        }

        let e = events.clone();
        reader("power-source", upower_changes, e, |message, events| {
            let (_, changed, _): (
                String,
                std::collections::HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize()?;
            if let Some(v) = changed.get("OnBattery") {
                send(
                    events,
                    BusEvent::OnBattery(bool::try_from(v).map_err(zbus::Error::from)?),
                );
            }
            Ok(())
        })?;
        let e = events.clone();
        let c = conn.clone();
        reader("power-daemon", owners, e, move |message, events| {
            let (_, _, new_owner): (String, String, String) = message.body().deserialize()?;
            if new_owner.is_empty() {
                send(
                    events,
                    BusEvent::NoPowerSource(format!("{UPOWER} left the bus")),
                );
            } else {
                report_on_battery(&c, events);
            }
            Ok(())
        })?;
        let e = events.clone();
        reader("idle-inhibitors", logind_changes, e, |message, events| {
            let (_, changed, _): (
                String,
                std::collections::HashMap<String, OwnedValue>,
                Vec<String>,
            ) = message.body().deserialize()?;
            if let Some(v) = changed.get("BlockInhibited") {
                send(
                    events,
                    BusEvent::IdleBlocked(names_idle(
                        &String::try_from(v.try_clone()?).map_err(zbus::Error::from)?,
                    )),
                );
            }
            Ok(())
        })?;
        // the delay is held before a sleep can be announced
        let mut delay = hold_sleep(&conn);
        reader(
            "system-sleep",
            sleeps,
            events.clone(),
            move |message, events| {
                let going_to_sleep: bool = message.body().deserialize()?;
                send(
                    events,
                    if going_to_sleep {
                        BusEvent::Sleeping
                    } else {
                        BusEvent::Resumed
                    },
                );
                Ok(())
            },
        )?;
        std::thread::Builder::new()
            .name("power-requests".into())
            .spawn(move || {
                for request in requests {
                    match request {
                        BusRequest::Suspend => {
                            if let Err(e) = conn.call_method(Some(LOGIND), LOGIND_PATH, Some(LOGIND_MANAGER), "Suspend", &false) {
                                tracing::warn!(error = %e, "logind refused to put the system to sleep");
                                super::suspend_failed("logind_refused", Some(&super::error_name(&e)));
                            }
                        }
                        BusRequest::HoldSleep => {
                            if delay.is_none() {
                                delay = hold_sleep(&conn);
                            }
                        }
                        BusRequest::ReleaseSleep => drop(delay.take()),
                    }
                }
            })
            .map_err(|e| zbus::Error::Failure(format!("starting the power request thread: {e}")))?;
        Ok(())
    }

    /// A logind `sleep` delay inhibitor: the system sleeps once it is dropped (or logind's delay limit passed). `None`
    /// when logind refuses; the session is then covered as the system sleeps, without waiting for the frame.
    fn hold_sleep(conn: &Connection) -> Option<zbus::zvariant::OwnedFd> {
        let reply = conn.call_method(
            Some(LOGIND),
            LOGIND_PATH,
            Some(LOGIND_MANAGER),
            "Inhibit",
            &(
                "sleep",
                "cupertino",
                "Cover the screen before the system sleeps",
                "delay",
            ),
        );
        match reply.and_then(|r| r.body().deserialize::<zbus::zvariant::OwnedFd>()) {
            Ok(fd) => Some(fd),
            Err(e) => {
                tracing::warn!(error = %e, "logind gave no sleep delay: the screen may still show the session as the system sleeps");
                diag::event(
                    Severity::Warn,
                    "power.inhibit.failed",
                    "logind gave no sleep delay inhibitor",
                )
                .field("what", Value::Public("sleep"))
                .field("mode", Value::Public("delay"))
                .field("error", Value::Public(&super::error_name(&e)))
                .emit();
                None
            }
        }
    }

    /// logind's `PrepareForSleep`, taken only from the connection that owns `name` (logind's name outside tests): a peer
    /// may send this connection a signal of the same name, and one not from logind covers nothing.
    pub(super) fn sleep_signals(conn: &Connection, name: &str) -> zbus::Result<FromOwner> {
        let rule = MatchRule::builder()
            .msg_type(Type::Signal)
            .sender(name)?
            .path(LOGIND_PATH)?
            .interface(LOGIND_MANAGER)?
            .member("PrepareForSleep")?
            .build();
        from_owner(
            conn,
            name,
            rule,
            |message| tracing::warn!(sender = ?message.header().sender(), "PrepareForSleep ignored: it did not come from logind"),
        )
    }

    /// `BlockInhibited` is a colon-separated list of inhibitor kinds.
    pub(super) fn names_idle(what: &str) -> bool {
        what.split(':').any(|w| w == "idle")
    }

    fn get(
        conn: &Connection,
        name: &str,
        path: &str,
        interface: &str,
        property: &str,
    ) -> zbus::Result<OwnedValue> {
        let reply = conn.call_method(
            Some(name),
            path,
            Some(PROPERTIES),
            "Get",
            &(interface, property),
        )?;
        let value: OwnedValue = reply.body().deserialize()?;
        Ok(value)
    }

    fn report_on_battery(conn: &Connection, events: &Sender<BusEvent>) {
        match get(conn, UPOWER, UPOWER_PATH, UPOWER, "OnBattery")
            .and_then(|v| bool::try_from(&*v).map_err(zbus::Error::from))
        {
            Ok(on_battery) => send(events, BusEvent::OnBattery(on_battery)),
            Err(e) => send(
                events,
                BusEvent::NoPowerSource(format!("reading {UPOWER}.OnBattery: {e}")),
            ),
        }
    }

    fn send(events: &Sender<BusEvent>, event: BusEvent) {
        // the receiver goes away only when the event loop ends
        let _ = events.send(event);
    }

    /// The messages of `messages` that `from_service` accepts; any other is logged and dropped.
    fn only_from(
        messages: MessageIterator,
        from_service: impl Fn(&zbus::Message) -> bool + Send + 'static,
    ) -> impl Iterator<Item = zbus::Result<zbus::Message>> + Send + 'static {
        messages.filter(move |message| match message {
            Ok(message) if !from_service(message) => {
                tracing::warn!("ignored a power signal that its service did not send");
                false
            }
            _ => true,
        })
    }

    fn reader(
        name: &str,
        messages: impl Iterator<Item = zbus::Result<zbus::Message>> + Send + 'static,
        events: Sender<BusEvent>,
        handle: impl Fn(&zbus::Message, &Sender<BusEvent>) -> zbus::Result<()> + Send + 'static,
    ) -> zbus::Result<()> {
        let label = name.to_owned();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for message in messages {
                    if let Err(e) = message.and_then(|m| handle(&m, &events)) {
                        return send(&events, BusEvent::Stopped(format!("{label}: {e}")));
                    }
                }
                send(
                    &events,
                    BusEvent::Stopped(format!("{label}: the signal stream ended")),
                );
            })
            .map(drop)
            .map_err(|e| zbus::Error::Failure(format!("starting the {name} reader: {e}")))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn block_inhibited_names_idle_only_as_a_whole_item() {
        assert!(super::bus::names_idle("shutdown:sleep:idle"));
        assert!(super::bus::names_idle("idle"));
        assert!(
            !super::bus::names_idle("handle-power-key:handle-idle"),
            "handle-idle is not idle"
        );
        assert!(!super::bus::names_idle(""));
    }

    #[test]
    fn prepare_for_sleep_is_taken_only_from_the_owner_of_the_name() {
        use std::time::Duration;
        const OWNER: &str = "org.example.SleepOwner";
        const PATH: &str = "/org/freedesktop/login1";
        const MANAGER: &str = "org.freedesktop.login1.Manager";
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let owner = bus.blocking_builder().name(OWNER).unwrap().build().unwrap();
        let impostor = bus.connect();
        let watcher = bus.connect();
        let sleeps = super::bus::sleep_signals(&watcher, OWNER).unwrap();
        let (heard, hearing) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            for message in sleeps {
                let going_to_sleep: bool = message.unwrap().body().deserialize().unwrap();
                if heard.send(going_to_sleep).is_err() {
                    return;
                }
            }
        });
        let to = watcher.unique_name().map(|n| n.to_owned());
        // a peer that does not own the name sends the signal straight to the watcher, many times what a stream holds:
        // the bus delivers each one, and none passes
        for _ in 0..200 {
            impostor
                .emit_signal(to.clone(), PATH, MANAGER, "PrepareForSleep", &(true,))
                .unwrap();
        }
        owner
            .emit_signal(None::<&str>, PATH, MANAGER, "PrepareForSleep", &(false,))
            .unwrap();
        assert_eq!(
            hearing.recv_timeout(Duration::from_secs(10)),
            Ok(false),
            "only the owner's signal is heard"
        );
        // the connection still reads: a call on it is answered after the flood
        let bus_daemon = zbus::blocking::fdo::DBusProxy::new(&watcher).unwrap();
        assert!(bus_daemon
            .name_has_owner(OWNER.try_into().unwrap())
            .unwrap());
        owner
            .emit_signal(to, PATH, MANAGER, "PrepareForSleep", &(true,))
            .unwrap();
        assert_eq!(
            hearing.recv_timeout(Duration::from_secs(10)),
            Ok(true),
            "the owner is heard also when it sends to the watcher alone"
        );
        assert!(hearing.recv_timeout(Duration::from_millis(300)).is_err());
    }

    /// How logind refuses for an inhibitor: it names the program and the user that hold it and their reason.
    const INHIBITED: &str = "Operation inhibited by \"Mail\" (PID 4242 \"mail\", user mallory), reason is \"Sending taxes.pdf\".";

    /// logind as the watcher sees it: the manager object with no inhibitor blocking anything, no sleep delay given, and
    /// a sleep refused for an inhibitor.
    struct Logind;

    #[zbus::interface(name = "org.freedesktop.login1.Manager")]
    impl Logind {
        #[zbus(property)]
        fn block_inhibited(&self) -> String {
            String::new()
        }

        fn inhibit(
            &self,
            _what: &str,
            _who: &str,
            _why: &str,
            _mode: &str,
        ) -> zbus::fdo::Result<zbus::zvariant::OwnedFd> {
            Err(zbus::fdo::Error::AccessDenied(INHIBITED.into()))
        }

        fn suspend(&self, _interactive: bool) -> zbus::fdo::Result<()> {
            Err(zbus::fdo::Error::AccessDenied(INHIBITED.into()))
        }
    }

    /// logind refuses the sleep delay the watcher takes as it starts (`power.inhibit.failed`) and the idle policy's sleep
    /// (`power.suspend.failed`), each for an inhibitor: the events keep the error's name, not logind's text with the
    /// user, the program and its reason. Without a bus the sleep fails before it is asked for.
    #[test]
    fn a_refused_sleep_delay_and_a_failed_suspend_are_recorded() {
        use cupertino_diagnostic_events::{self as diag, Record};
        use std::time::{Duration, Instant};
        const PATH: &str = "/org/freedesktop/login1";
        diag::init(diag::component!(), "server", diag::Sink::Memory);
        super::suspend_failed("no_system_bus", None);
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        let _logind = bus
            .blocking_builder()
            .serve_at(PATH, Logind)
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .build()
            .unwrap();
        let watcher = bus.blocking_builder().build().unwrap();
        let (events, _received) = smithay::reexports::calloop::channel::channel();
        let (requests, requested) = std::sync::mpsc::channel();
        super::bus::start_on(watcher, events, requested).unwrap();
        requests.send(super::BusRequest::Suspend).unwrap();
        let recorded = |id: &str| -> Vec<Record> {
            diag::recent()
                .iter()
                .map(|line| Record::parse(line.trim_end()).unwrap())
                .filter(|record| record.id == id)
                .collect()
        };
        let until = Instant::now() + Duration::from_secs(60);
        while recorded("power.suspend.failed").len() < 2 {
            assert!(
                Instant::now() < until,
                "no failed suspend: {:?}",
                diag::recent()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let suspends = recorded("power.suspend.failed");
        assert_eq!(suspends[0].fields.get("reason").unwrap(), "no_system_bus");
        assert!(suspends[0].fields.get("error").is_none());
        assert_eq!(suspends[1].fields.get("reason").unwrap(), "logind_refused");
        assert_eq!(
            suspends[1].fields.get("error").unwrap(),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
        assert!(suspends.iter().all(|r| r.severity == diag::Severity::Error));
        let inhibits = recorded("power.inhibit.failed");
        assert_eq!(inhibits.len(), 1, "{inhibits:?}");
        assert_eq!(inhibits[0].severity, diag::Severity::Warn);
        assert_eq!(inhibits[0].fields.get("what").unwrap(), "sleep");
        assert_eq!(inhibits[0].fields.get("mode").unwrap(), "delay");
        assert_eq!(
            inhibits[0].fields.get("error").unwrap(),
            "org.freedesktop.DBus.Error.AccessDenied"
        );
        for line in diag::recent() {
            for private in ["mallory", "taxes"] {
                assert!(!line.contains(private), "{private} in {line}");
            }
        }
    }

    #[test]
    fn the_watcher_ignores_a_forged_prepare_for_sleep_and_covers_the_session_for_logind() {
        use super::BusEvent;
        use std::time::{Duration, Instant};
        const PATH: &str = "/org/freedesktop/login1";
        const MANAGER: &str = "org.freedesktop.login1.Manager";
        let Some(bus) = cupertino_test_bus::PrivateBus::start() else {
            return;
        };
        // logind's name on a private bus: the watcher never opens the system bus
        let logind = bus
            .blocking_builder()
            .serve_at(PATH, Logind)
            .unwrap()
            .name("org.freedesktop.login1")
            .unwrap()
            .build()
            .unwrap();
        let watcher = bus.blocking_builder().build().unwrap();
        let to = watcher.unique_name().map(|n| n.to_owned());
        let (events, received) = smithay::reexports::calloop::channel::channel();
        let (_requests, requests) = std::sync::mpsc::channel();
        super::bus::start_on(watcher, events, requests).unwrap();
        let (heard, hearing) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(event) = received.recv() {
                if heard.send(event).is_err() {
                    return;
                }
            }
        });
        // the next sleep or wake the watcher reports within `wait`; the other facts are skipped
        let next_sleep = |wait: Duration| {
            let until = Instant::now() + wait;
            loop {
                let left = until.checked_duration_since(Instant::now())?;
                match hearing.recv_timeout(left).ok()? {
                    event @ (BusEvent::Sleeping | BusEvent::Resumed) => return Some(event),
                    _ => continue,
                }
            }
        };

        // another connection sends PrepareForSleep straight to the watcher and to everyone: nothing is covered
        let impostor = bus.connect();
        for _ in 0..100 {
            impostor
                .emit_signal(to.clone(), PATH, MANAGER, "PrepareForSleep", &(true,))
                .unwrap();
            impostor
                .emit_signal(None::<&str>, PATH, MANAGER, "PrepareForSleep", &(true,))
                .unwrap();
        }
        let ignored = next_sleep(Duration::from_millis(500));
        assert!(ignored.is_none(), "a forged signal was taken: {ignored:?}");

        // logind's own signals are taken, in order
        logind
            .emit_signal(None::<&str>, PATH, MANAGER, "PrepareForSleep", &(true,))
            .unwrap();
        assert!(matches!(
            next_sleep(Duration::from_secs(10)),
            Some(BusEvent::Sleeping)
        ));
        impostor
            .emit_signal(to, PATH, MANAGER, "PrepareForSleep", &(false,))
            .unwrap();
        logind
            .emit_signal(None::<&str>, PATH, MANAGER, "PrepareForSleep", &(false,))
            .unwrap();
        assert!(matches!(
            next_sleep(Duration::from_secs(10)),
            Some(BusEvent::Resumed)
        ));
        assert!(next_sleep(Duration::from_millis(300)).is_none());
    }
}
