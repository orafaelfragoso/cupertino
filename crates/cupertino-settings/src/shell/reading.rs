//! What the window reads from the services, off its thread: the readings of the shown pane ([`State::refresh`]),
//! asked of the services on the I/O pool and shown when they come back, one reading at a time; the change signals
//! that ask for them, watched by tasks of the window's loop; the appearance and the accessibility settings the server
//! holds; and the Sound pane's input level meter.

use std::collections::HashMap;
use std::sync::Arc;

use super::desk::Done;
use super::state::{
    reads, Background, Result, State, HISTORY_REFRESH, REWATCH_FIRST, REWATCH_MAX, SCAN,
};
use super::Message;
use crate::agent::Agent;
use crate::live::{BatteryExtras, Bluetooth, Live, Reading, Wifi};
use crate::meter::Meter;
use crate::services::{self, Backends, Reach, Services};
use crate::sidebar;
use crate::watch::{Changed, Which};
use crate::PaneId;

/// The buses the tasks of the window's loop reach: the watches and the Sound pane's client.
pub(crate) struct Buses {
    pub(crate) session: Reach,
    pub(crate) system: Reach,
}

/// The watches of the two buses: which run (connecting, subscribing or watching), the buses whose watch could not
/// start and how long until the next try, in seconds.
pub(crate) struct Watches {
    running: [bool; 2],
    lost: Vec<Which>,
    rewatch_in: f64,
}

impl Default for Watches {
    fn default() -> Self {
        Watches {
            running: [false; 2],
            lost: Vec::new(),
            rewatch_in: REWATCH_FIRST,
        }
    }
}

/// What a reading of the shown pane asks the services for, with the readings it carries over.
struct Ask {
    pane: PaneId,
    battery: bool,
    history: bool,
    extras: BatteryExtras,
    brightness: bool,
    scan: bool,
}

/// What a reading found, for the window to show.
struct Found {
    pane: PaneId,
    backends: Backends,
    battery: Option<Reading<Option<crate::live::Battery>>>,
    extras: Option<BatteryExtras>,
    brightness: Option<Reading<f64>>,
    wifi: Option<Reading<Wifi>>,
    bluetooth: Option<Reading<Bluetooth>>,
    /// The other readings of the pane.
    set: Box<dyn FnOnce(&mut Live) + Send>,
    scanned: bool,
}

impl Ask {
    /// Read what the pane shows, on the I/O pool.
    fn read(self, services: &mut Services) -> Found {
        let backends = services.detect();
        let battery = self.battery.then(|| services.battery());
        let extras = (self.pane == PaneId::Battery)
            .then(|| services.battery_extras(&self.extras, self.history));
        let (mut brightness, mut wifi, mut bluetooth, mut scanned) = (None, None, None, false);
        let set: Box<dyn FnOnce(&mut Live) + Send> = match self.pane {
            PaneId::Focus => {
                let focus = services.focus();
                Box::new(move |live| live.focus = focus)
            }
            PaneId::Notifications => {
                let apps = services.notifying_apps();
                Box::new(move |live| live.notifying_apps = apps)
            }
            PaneId::Keyboard => {
                let backlight = services.keyboard_backlight();
                Box::new(move |live| live.keyboard_backlight = backlight)
            }
            PaneId::Vpn => {
                let vpns = services.vpns();
                Box::new(move |live| live.vpns = vpns)
            }
            PaneId::Displays => {
                let displays = services.displays();
                brightness = self.brightness.then(|| services.brightness());
                Box::new(move |live| live.displays = displays)
            }
            PaneId::Wifi | PaneId::Network => {
                let found = services.wifi();
                let interfaces = crate::netinfo::read();
                let radio_on = found
                    .ready()
                    .is_some_and(|w| w.enabled && w.has_wifi_device());
                if self.scan && radio_on {
                    scanned = true;
                    if let Err(e) = services.scan() {
                        eprintln!("cupertino-settings: Wi-Fi scan: {e}");
                    }
                }
                wifi = Some(found);
                Box::new(move |live| live.interfaces = interfaces)
            }
            PaneId::Mouse => {
                let mice = services.mice();
                Box::new(move |live| live.mice = mice)
            }
            PaneId::Bluetooth => {
                bluetooth = Some(services.bluetooth());
                Box::new(|_| {})
            }
            _ => Box::new(|_| {}),
        };
        Found {
            pane: self.pane,
            backends,
            battery,
            extras,
            brightness,
            wifi,
            bluetooth,
            set,
            scanned,
        }
    }
}

impl State {
    /// Read the live services of the pane shown again; it shows what they say when the reading comes back. One
    /// reading runs at a time: one asked for meanwhile follows it. The battery is read on the first refresh too,
    /// whatever the pane: whether there is one names the Battery row.
    pub(crate) fn refresh(&mut self, now: f64) -> Result<()> {
        self.refreshed = now;
        self.display = self.settings.accessibility.display;
        if self.content_pane == PaneId::Sound {
            self.refresh_sound(now);
        }
        if self.reading {
            self.read_again = true;
            return Ok(());
        }
        self.reading = true;
        let ask = Ask {
            pane: self.content_pane,
            battery: self.content_pane == PaneId::Battery || self.live.battery == Reading::Unknown,
            history: self.content_pane == PaneId::Battery
                && now - self.history_read >= HISTORY_REFRESH,
            extras: self.live.battery_extras.clone(),
            brightness: self.level.is_none(),
            scan: self.content_pane == PaneId::Wifi && now - self.scanned >= SCAN,
        };
        if ask.history {
            self.history_read = now;
        }
        self.desk.serve(
            move |services| ask.read(services),
            move |state, found| state.found(found, now),
        );
        Ok(())
    }

    /// Show what a reading asked at `asked` found; the readings of a pane no longer shown are dropped.
    fn found(&mut self, found: Found, asked: f64) {
        self.reading = false;
        self.live.hardware = Some(found.backends.hardware);
        let hidden = found.backends.hidden();
        if hidden != self.hidden {
            self.groups = sidebar::groups(&hidden);
            self.hidden = hidden;
        }
        if self.hidden.contains(&self.pane()) {
            self.read_again = false;
            if let Err(e) = self.go(self.rows()[0]) {
                eprintln!("cupertino-settings: {e}");
            }
            return;
        }
        if let Some(battery) = found.battery {
            self.live.battery = battery;
        }
        if found.pane == self.content_pane {
            if let Some(extras) = found.extras {
                self.live.battery_extras = extras;
            }
            if let Some(wifi) = found.wifi {
                self.live.wifi = services::carry_wifi(wifi, self.live.wifi.ready());
            }
            if let Some(bluetooth) = found.bluetooth {
                self.live.bluetooth =
                    services::carry_bluetooth(bluetooth, self.live.bluetooth.ready());
            }
            (found.set)(&mut self.live);
            if let (Some(brightness), None) = (found.brightness, &self.level) {
                self.live.brightness = brightness;
            }
            if found.scanned {
                self.scanned = asked;
            }
            self.start_bluetooth();
        }
        if std::mem::take(&mut self.read_again) {
            let now = self.now();
            if let Err(e) = self.refresh(now) {
                eprintln!("cupertino-settings: {e}");
            }
        }
    }

    /// Once the Bluetooth pane's first reading shows an adapter: serve the pairing agent and discover devices while the
    /// pane shows (discovery needs a powered adapter; the pane says why otherwise).
    fn start_bluetooth(&mut self) {
        let Some(b) = self.live.bluetooth.ready() else {
            return;
        };
        if !self.agent_wanted || self.content_pane != PaneId::Bluetooth || !b.available {
            return;
        }
        self.agent_wanted = false;
        let powered = b.powered;
        let notify = self.notify.clone();
        let agent = Agent {
            on_event: Box::new(move |event| notify(Background::Agent(event))),
        };
        self.desk.serve(
            move |services| {
                let registered = services.register_agent(agent);
                let discovery = if powered {
                    services.discovery(true)
                } else {
                    Ok(())
                };
                (registered, discovery)
            },
            move |state, (registered, discovery)| {
                match registered {
                    Ok(ours) => {
                        state.agent_registered = ours;
                        if let Reading::Ready(b) = &mut state.live.bluetooth {
                            b.agent_elsewhere = !ours;
                        }
                        // the pane was left meanwhile: its agent goes again
                        if ours && state.content_pane != PaneId::Bluetooth {
                            state.stop_bluetooth(powered);
                        }
                    }
                    Err(e) => eprintln!("cupertino-settings: Bluetooth pairing agent: {e}"),
                }
                if let Err(e) = discovery {
                    eprintln!("cupertino-settings: Bluetooth discovery: {e}");
                }
            },
        );
    }

    /// The Bluetooth pane is left: discovery stops when it runs, and the agent goes.
    pub(crate) fn stop_bluetooth(&mut self, discovering: bool) {
        self.agent_wanted = false;
        let agent = std::mem::take(&mut self.agent_registered);
        if !discovering && !agent {
            return;
        }
        self.desk.serve(
            move |services| {
                if discovering {
                    if let Err(e) = services.discovery(false) {
                        eprintln!("cupertino-settings: Bluetooth discovery: {e}");
                    }
                }
                if agent {
                    if let Err(e) = services.unregister_agent() {
                        eprintln!("cupertino-settings: Bluetooth pairing agent: {e}");
                    }
                }
            },
            |_, ()| {},
        );
    }

    /// Services sent change signals: read again what they changed, once, if the window shows it.
    pub(crate) fn changed(&mut self, changes: &[Changed]) -> Result<()> {
        if changes
            .iter()
            .any(|c| matches!(c, Changed::Owner(Which::Session) | Changed::Session))
        {
            self.read_appearance();
        }
        if changes.iter().any(|c| self.shows(*c)) {
            self.refresh(self.now())?;
        }
        Ok(())
    }

    /// Whether the shown pane shows a reading `changed` is about.
    fn shows(&self, changed: Changed) -> bool {
        match changed {
            // the backends and the hardware name the sidebar's rows, whatever the pane
            Changed::Owner(_) | Changed::Hardware => true,
            Changed::Session => self.content_pane == PaneId::Mouse,
            changed => reads(self.content_pane).contains(&changed),
        }
    }

    /// Watch the change signals of the services on both buses, and the devices coming and going when `devices`
    /// ([`crate::hotplug`]), as tasks of the window's loop that tell the loop through [`Background::Changed`].
    pub(crate) fn start_watching(&mut self, devices: bool) {
        if devices {
            let on = self.on_change();
            self.desk.run(async move {
                crate::hotplug::watch(on).await;
                std::future::pending().await
            });
        }
        for bus in [Which::Session, Which::System] {
            self.watch(bus);
        }
    }

    fn on_change(&self) -> Arc<dyn Fn(Changed) + Send + Sync> {
        let to_loop = self.notify.clone();
        Arc::new(move |changed| to_loop(Background::Changed(changed)))
    }

    /// Watch `bus` unless its watch runs; its end comes back to [`State::watch_ended`].
    fn watch(&mut self, bus: Which) {
        if std::mem::replace(&mut self.watches.running[bus as usize], true) {
            return;
        }
        let reach = match bus {
            Which::Session => &self.buses.session,
            Which::System => &self.buses.system,
        };
        let connect = reach.connect();
        let (interest, on, told) = (self.interest.clone(), self.on_change(), self.notify.clone());
        self.desk.run(async move {
            let subscribed = crate::watch::watch(bus, connect, interest, on, move || {
                told(Background::Watching(bus))
            })
            .await;
            Message::Done(Done::new(move |state: &mut State| {
                state.watch_ended(bus, subscribed)
            }))
        });
    }

    /// The watch of `bus` has the daemon's rules: it is no longer lost.
    pub(crate) fn watch_started(&mut self, bus: Which) {
        self.watches.lost.retain(|lost| *lost != bus);
        if self.watches.lost.is_empty() {
            self.watches.rewatch_in = REWATCH_FIRST;
        }
    }

    /// The watch of `bus` ended: one that had subscribed ended with its connection and watches a new one at once; one
    /// that could not start is tried again later ([`State::rewatch_after`]).
    fn watch_ended(&mut self, bus: Which, subscribed: bool) {
        self.watches.running[bus as usize] = false;
        if subscribed {
            self.watch(bus);
        } else if !self.watches.lost.contains(&bus) {
            self.watches.lost.push(bus);
        }
    }

    /// How soon to try again to watch a bus whose watch could not start; `None` while none waits for a try.
    pub(crate) fn rewatch_after(&self) -> Option<f64> {
        let waiting = self
            .watches
            .lost
            .iter()
            .any(|bus| !self.watches.running[*bus as usize]);
        waiting.then_some(self.watches.rewatch_in)
    }

    /// Try again to watch the buses whose watch could not start; the next try waits twice as long.
    pub(crate) fn rewatch(&mut self) {
        for bus in self.watches.lost.clone() {
            self.watch(bus);
        }
        self.watches.rewatch_in = (self.watches.rewatch_in * 2.0).min(REWATCH_MAX);
    }

    /// Read the appearance the session is in; it changes with the session interface's change signal.
    pub(crate) fn read_appearance(&mut self) {
        self.desk.serve(Services::dark, |state, dark| {
            if let Some(dark) = dark {
                state.dark = dark;
            }
        });
    }

    /// Read the accessibility settings as the server holds them (`org.cupertino.Session1.Accessibility`) and show
    /// them over the files': the server is their only writer, and it keeps them in memory only when it has no file to
    /// store them in, so the file is not the truth. Without the property the file's values stay.
    pub(crate) fn sync_accessibility(&mut self) {
        self.desk.serve(Services::accessibility, |state, reading| {
            let problems = match reading {
                Reading::Ready(held) => {
                    state.held_accessibility = Some(held);
                    return state.apply_held_accessibility();
                }
                Reading::Unavailable(reason) => {
                    vec![format!(
                        "the session's accessibility settings cannot be read: {reason}"
                    )]
                }
                Reading::Unknown => Vec::new(),
            };
            state.report(problems);
        });
    }

    /// Put the accessibility settings the server held at the last reading over the files' just read.
    pub(crate) fn apply_held_accessibility(&mut self) {
        let held: &HashMap<String, String> = match &self.held_accessibility {
            Some(held) => held,
            None => return,
        };
        let problems = self.settings.apply_session_accessibility(held);
        self.report(problems);
    }

    /// Log each of `problems` the first time it comes.
    fn report(&mut self, problems: Vec<String>) {
        for problem in problems {
            if self.reported.insert(problem.clone()) {
                eprintln!("cupertino-settings: {problem}");
            }
        }
    }

    /// Whether the input level meter listens and shows now.
    pub(crate) fn meter_running(&self) -> bool {
        self.content_pane == PaneId::Sound && self.live.tab == 1 && !self.live.inputs.is_empty()
    }

    /// Read how loud the input is now; true when it changed.
    pub(crate) fn refresh_meter(&mut self) -> bool {
        let level = self.meter_level();
        let changed = (level - self.live.input_level).abs() > 0.005;
        self.live.input_level = level;
        changed
    }

    fn meter_level(&self) -> f64 {
        self.meter.as_ref().map_or(0.0, Meter::level)
    }

    pub(super) fn refresh_sound(&mut self, now: f64) {
        self.refreshed = now;
        if self.level.is_none() {
            self.audio.read(self.live.tab == 1);
        }
        if self.live.tab == 1 {
            self.start_meter();
            self.live.input_level = self.meter_level();
        } else {
            self.stop_meter();
        }
    }

    /// Listen to the input to measure how loud it is, unless the meter listens already; the recording that ends by
    /// itself lets the next refresh start another.
    fn start_meter(&mut self) {
        if self.meter.is_some() {
            return;
        }
        let Some((meter, reader)) = Meter::start() else {
            return;
        };
        self.meter = Some(meter);
        self.meter_generation += 1;
        let generation = self.meter_generation;
        self.desk.run(async move {
            reader.await;
            // the recording ended, by itself or stopped: the meter it read goes, unless another has started since
            Message::Done(Done::new(move |state: &mut State| {
                if state.meter_generation == generation {
                    state.meter = None;
                }
            }))
        });
    }

    /// Stop listening to the input.
    pub(crate) fn stop_meter(&mut self) {
        self.meter = None;
        self.live.input_level = 0.0;
    }
}
