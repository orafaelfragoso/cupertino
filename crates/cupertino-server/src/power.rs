//! Display and system power: runs the idle policy of `display_power` on the model clock, draws the dim and sleep overlay,
//! turns the outputs off on the TTY backend, locks the session after display sleep, and puts the system to sleep through
//! logind.
//!
//! The facts the policy needs from outside come from a system bus watcher ([`start_power_bus`]): the power source
//! (`OnBattery` of `org.freedesktop.UPower`, served by `cupertino-powerd`), logind's `idle` block inhibitors and the end of
//! a system sleep (`PrepareForSleep(false)`, which counts as input). Without a system bus or a power daemon the mains
//! timers apply, and the server says so once.
//!
//! What a sleeping display looks like depends on the backend: on DRM every CRTC is switched off (`DrmCompositor::clear`,
//! the DPMS-off path) and nothing is rendered until input; the other backends draw an opaque black overlay and keep
//! taking input. The dim is a black overlay whose alpha shows the dim factor (`display_power::overlay_alpha`).

use std::sync::mpsc;

use smithay::reexports::calloop::channel::{self, Event};

use crate::display_power::{overlay_alpha, ActivationFade, DisplayState, Effect, Facts, IdlePolicy};
use crate::render::programs::u4;
use crate::render::shader_rect::{ElementKey, ShaderRect, Sig};
use crate::render::CupertinoElement;
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
        Self { policy, on_battery: None, idle_blocked: false, bus: None, fade: ActivationFade::default() }
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
    /// The system woke from sleep.
    Resumed,
    /// The watcher stopped; the facts it reported stay as they were.
    Stopped(String),
}

/// What the main loop asks of the bus.
#[derive(Debug)]
enum BusRequest {
    Suspend,
}

impl Cupertino {
    /// Input arrived (any key, pointer or touch event): restart the idle countdowns and wake the display.
    pub fn input_activity(&mut self) {
        self.idle.notifier.notify_activity(&self.seat);
        let now = self.now();
        let effects = self.power.policy.input(now);
        self.apply_power_effects(effects);
    }

    /// Advance the idle policy; called from [`Cupertino::tick`].
    pub fn tick_display_power(&mut self) {
        let now = self.now();
        let facts = Facts { on_battery: self.power.on_battery.unwrap_or(false), inhibited: self.idle.inhibited() || self.power.idle_blocked, locked: self.session_lock.is_active() };
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
                            }
                        }
                        None => tracing::warn!("no system bus: cannot put the system to sleep"),
                    }
                }
            }
        }
    }

    fn set_outputs_asleep(&mut self, asleep: bool) {
        #[cfg(feature = "udev")]
        if let Some(crate::backend::Backend::Udev(u)) = self.backend.as_mut() {
            u.set_asleep(asleep);
        }
        #[cfg(not(feature = "udev"))]
        let _ = asleep;
    }

    /// The black overlay over output `idx` while the display is dimmed, asleep or ramping; front-most element.
    pub fn power_overlay(&mut self, idx: usize) -> Option<CupertinoElement> {
        let alpha = overlay_alpha(self.power.policy.factor(self.now()));
        // the sRGB curve leaves a rounding residue at factor 1
        if alpha < 1e-6 {
            return None;
        }
        const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
        let program = self.render.as_ref()?.programs.wallpaper.clone();
        let out = &mut self.outputs[idx];
        let area = smithay::utils::Rectangle::new((0.0, 0.0).into(), (out.rect.w, out.rect.h).into());
        let (id, commit) = out.cache.slot(ElementKey::new("power-overlay", 0), Sig::new().rect(area).f(alpha).finish());
        let opaque = alpha >= 1.0;
        Some(CupertinoElement::Shader(ShaderRect::new(program, id, commit, area, alpha as f32, vec![u4("c0", BLACK), u4("c1", BLACK), u4("c2", BLACK), u4("c3", BLACK)], opaque)))
    }

    /// The black overlay over output `idx` while the screen fades in after the session became active; front-most element.
    pub fn activation_overlay(&mut self, idx: usize) -> Option<CupertinoElement> {
        let alpha = self.power.fade.alpha(self.now());
        if alpha < 1e-6 {
            return None;
        }
        const BLACK: [f32; 4] = [0.0, 0.0, 0.0, 1.0];
        let program = self.render.as_ref()?.programs.wallpaper.clone();
        let out = &mut self.outputs[idx];
        let area = smithay::utils::Rectangle::new((0.0, 0.0).into(), (out.rect.w, out.rect.h).into());
        let (id, commit) = out.cache.slot(ElementKey::new("activation-fade", 0), Sig::new().rect(area).f(alpha).finish());
        Some(CupertinoElement::Shader(ShaderRect::new(program, id, commit, area, alpha as f32, vec![u4("c0", BLACK), u4("c1", BLACK), u4("c2", BLACK), u4("c3", BLACK)], alpha >= 1.0)))
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
            BusEvent::Resumed => {
                tracing::info!("the system woke from sleep");
                self.input_activity();
            }
            BusEvent::Stopped(reason) => tracing::warn!(reason, "the system bus watcher stopped"),
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
        Err(e) => tracing::warn!(error = %e, "no system bus: power source, idle inhibitors and system sleep are unavailable"),
    }
}

mod bus {
    //! The watcher threads. The blocking `zbus` API is used as in `cupertino-login`: one reader thread per signal stream.

    use std::sync::mpsc::Receiver;

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
        let conn = Connection::system()?;
        let changes = |interface: &'static str, path: &'static str| -> zbus::Result<MessageIterator> {
            let rule = MatchRule::builder().msg_type(Type::Signal).interface(PROPERTIES)?.member("PropertiesChanged")?.path(path)?.arg(0, interface)?.build();
            MessageIterator::for_match_rule(rule, &conn, None)
        };
        let upower_changes = changes(UPOWER, UPOWER_PATH)?;
        let logind_changes = changes(LOGIND_MANAGER, LOGIND_PATH)?;
        let owner_rule = MatchRule::builder().msg_type(Type::Signal).sender("org.freedesktop.DBus")?.member("NameOwnerChanged")?.arg(0, UPOWER)?.build();
        let owners = MessageIterator::for_match_rule(owner_rule, &conn, None)?;
        let sleep_rule = MatchRule::builder().msg_type(Type::Signal).sender(LOGIND)?.path(LOGIND_PATH)?.interface(LOGIND_MANAGER)?.member("PrepareForSleep")?.build();
        let sleeps = MessageIterator::for_match_rule(sleep_rule, &conn, None)?;

        // the subscriptions are up: read the current values
        report_on_battery(&conn, &events);
        match get(&conn, LOGIND, LOGIND_PATH, LOGIND_MANAGER, "BlockInhibited").and_then(|v| String::try_from(v).map_err(zbus::Error::from)) {
            Ok(what) => send(&events, BusEvent::IdleBlocked(names_idle(&what))),
            Err(e) => send(&events, BusEvent::Stopped(format!("reading logind's BlockInhibited: {e}"))),
        }

        let e = events.clone();
        reader("power-source", upower_changes, e, |message, events| {
            let (_, changed, _): (String, std::collections::HashMap<String, OwnedValue>, Vec<String>) = message.body().deserialize()?;
            if let Some(v) = changed.get("OnBattery") {
                send(events, BusEvent::OnBattery(bool::try_from(v).map_err(zbus::Error::from)?));
            }
            Ok(())
        })?;
        let e = events.clone();
        let c = conn.clone();
        reader("power-daemon", owners, e, move |message, events| {
            let (_, _, new_owner): (String, String, String) = message.body().deserialize()?;
            if new_owner.is_empty() {
                send(events, BusEvent::NoPowerSource(format!("{UPOWER} left the bus")));
            } else {
                report_on_battery(&c, events);
            }
            Ok(())
        })?;
        let e = events.clone();
        reader("idle-inhibitors", logind_changes, e, |message, events| {
            let (_, changed, _): (String, std::collections::HashMap<String, OwnedValue>, Vec<String>) = message.body().deserialize()?;
            if let Some(v) = changed.get("BlockInhibited") {
                send(events, BusEvent::IdleBlocked(names_idle(&String::try_from(v.try_clone()?).map_err(zbus::Error::from)?)));
            }
            Ok(())
        })?;
        reader("system-sleep", sleeps, events.clone(), |message, events| {
            let going_to_sleep: bool = message.body().deserialize()?;
            if !going_to_sleep {
                send(events, BusEvent::Resumed);
            }
            Ok(())
        })?;
        std::thread::Builder::new()
            .name("power-requests".into())
            .spawn(move || {
                for request in requests {
                    match request {
                        BusRequest::Suspend => {
                            if let Err(e) = conn.call_method(Some(LOGIND), LOGIND_PATH, Some(LOGIND_MANAGER), "Suspend", &false) {
                                tracing::warn!(error = %e, "logind refused to put the system to sleep");
                            }
                        }
                    }
                }
            })
            .map_err(|e| zbus::Error::Failure(format!("starting the power request thread: {e}")))?;
        Ok(())
    }

    /// `BlockInhibited` is a colon-separated list of inhibitor kinds.
    pub(super) fn names_idle(what: &str) -> bool {
        what.split(':').any(|w| w == "idle")
    }

    fn get(conn: &Connection, name: &str, path: &str, interface: &str, property: &str) -> zbus::Result<OwnedValue> {
        let reply = conn.call_method(Some(name), path, Some(PROPERTIES), "Get", &(interface, property))?;
        let value: OwnedValue = reply.body().deserialize()?;
        Ok(value)
    }

    fn report_on_battery(conn: &Connection, events: &Sender<BusEvent>) {
        match get(conn, UPOWER, UPOWER_PATH, UPOWER, "OnBattery").and_then(|v| bool::try_from(&*v).map_err(zbus::Error::from)) {
            Ok(on_battery) => send(events, BusEvent::OnBattery(on_battery)),
            Err(e) => send(events, BusEvent::NoPowerSource(format!("reading {UPOWER}.OnBattery: {e}"))),
        }
    }

    fn send(events: &Sender<BusEvent>, event: BusEvent) {
        // the receiver goes away only when the event loop ends
        let _ = events.send(event);
    }

    fn reader(name: &str, messages: MessageIterator, events: Sender<BusEvent>, handle: impl Fn(&zbus::Message, &Sender<BusEvent>) -> zbus::Result<()> + Send + 'static) -> zbus::Result<()> {
        let label = name.to_owned();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                for message in messages {
                    if let Err(e) = message.and_then(|m| handle(&m, &events)) {
                        return send(&events, BusEvent::Stopped(format!("{label}: {e}")));
                    }
                }
                send(&events, BusEvent::Stopped(format!("{label}: the signal stream ended")));
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
        assert!(!super::bus::names_idle("handle-power-key:handle-idle"), "handle-idle is not idle");
        assert!(!super::bus::names_idle(""));
    }
}
