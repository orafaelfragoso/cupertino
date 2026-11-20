//! The panel's state machine: what the person does on the panel, what the services answer and the clock become the
//! readings the panel shows and the effects to carry out ([`Effect`]). Nothing here waits or talks to a service: the
//! component carries the effects out off the UI thread and hands their outcomes back ([`Controller::read`],
//! [`Controller::changed`]). Time is injected as seconds on any monotonic clock.
//!
//! A reading or a change that fails disables its tile with the reason and is logged on stderr; nothing falls back to a
//! default. A failure is logged when it starts or its reason changes and once more when the service recovers, not on
//! every retry. An open panel reads its sources again every [`REFRESH_INTERVAL`], since the sound service sends no
//! change signal; the radios' signals and do not disturb's publication are followed as they come.
//!
//! One reading of a source is under way at a time: one asked for meanwhile is taken once more when it ends. A change
//! is shown at once; a reading of its source that started before it, or that is asked for while it is under way, is
//! put off until it has ended, so the panel never shows what the service had before the change. A level that is
//! dragged is written at most every [`WRITE_INTERVAL`], one write at a time, the latest value winning.

use std::collections::{HashMap, HashSet};

use cupertino_geometry::Point;

use crate::arrangement::Arrangement;
use crate::lifecycle::{Lifecycle, Phase};
use crate::model::{
    Controls, Detail, Device, Media, MediaCommand, Module, Network, Output, Reading, Slider,
    TileState, VolumeState, WifiStatus, MUTE_BELOW,
};

/// Shortest time between two writes of a level while it is dragged, in seconds; the latest value wins (a frame at
/// 60 Hz, approximate).
pub const WRITE_INTERVAL: f64 = 0.0167;

/// How often an open panel reads its sources again, in seconds, so a level changed by a key or another program shows
/// up: the sound service tells no change (approximate).
pub const REFRESH_INTERVAL: f64 = 1.0;

/// The level an unmuted output takes when it was muted by dragging its level to the end.
const UNMUTED_LEVEL: f64 = 0.25;

/// What the panel reads from a service.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Source {
    Wifi,
    Bluetooth,
    Microphone,
    DockHiding,
    Keyboard,
    Brightness,
    Volume,
    Media,
    /// The Wi-Fi view's networks.
    Networks,
    /// The Bluetooth view's devices.
    Devices,
    /// The Sound view's outputs.
    Outputs,
    /// The stored choice of controls.
    Layout,
}

impl Source {
    /// The name a failure is logged with.
    pub fn what(self) -> &'static str {
        match self {
            Source::Wifi => "wi-fi",
            Source::Bluetooth => "bluetooth",
            Source::Microphone => "microphone",
            Source::DockHiding => "dock hiding",
            Source::Keyboard => "keyboard backlight",
            Source::Brightness => "display brightness",
            Source::Volume => "volume",
            Source::Media => "media",
            Source::Networks => "wi-fi networks",
            Source::Devices => "bluetooth devices",
            Source::Outputs => "sound outputs",
            Source::Layout => "controls shown",
        }
    }
}

/// A network of the Wi-Fi view and the service's key for it, which joining it takes.
pub type Listed = (Network, String);

/// What a media player plays, and the player's bus name, which the media tile's buttons go to.
#[derive(Clone, Debug, PartialEq)]
pub struct Playing {
    pub player: String,
    pub media: Media,
}

/// A reading a service gave, or why it gave none.
#[derive(Clone, Debug, PartialEq)]
pub enum Read {
    Wifi(Result<WifiStatus, String>),
    Bluetooth(Result<bool, String>),
    Microphone(Result<bool, String>),
    DockHiding(Result<bool, String>),
    Keyboard(Result<bool, String>),
    Brightness(Result<f64, String>),
    Volume(Result<VolumeState, String>),
    /// What the playing player plays, else the first that has a title; `None` when none has anything.
    Media(Result<Option<Playing>, String>),
    Networks(Result<Vec<Listed>, String>),
    Devices(Result<Vec<Device>, String>),
    Outputs(Result<Vec<Output>, String>),
    /// The names of the controls shown, in order, as stored; `None` when nothing is stored.
    Layout(Result<Option<Vec<String>>, String>),
}

impl Read {
    /// The source the reading is of.
    pub fn source(&self) -> Source {
        match self {
            Read::Wifi(_) => Source::Wifi,
            Read::Bluetooth(_) => Source::Bluetooth,
            Read::Microphone(_) => Source::Microphone,
            Read::DockHiding(_) => Source::DockHiding,
            Read::Keyboard(_) => Source::Keyboard,
            Read::Brightness(_) => Source::Brightness,
            Read::Volume(_) => Source::Volume,
            Read::Media(_) => Source::Media,
            Read::Networks(_) => Source::Networks,
            Read::Devices(_) => Source::Devices,
            Read::Outputs(_) => Source::Outputs,
            Read::Layout(_) => Source::Layout,
        }
    }

    /// The reading of `source` that failed for `reason`.
    pub fn failed(source: Source, reason: String) -> Read {
        match source {
            Source::Wifi => Read::Wifi(Err(reason)),
            Source::Bluetooth => Read::Bluetooth(Err(reason)),
            Source::Microphone => Read::Microphone(Err(reason)),
            Source::DockHiding => Read::DockHiding(Err(reason)),
            Source::Keyboard => Read::Keyboard(Err(reason)),
            Source::Brightness => Read::Brightness(Err(reason)),
            Source::Volume => Read::Volume(Err(reason)),
            Source::Media => Read::Media(Err(reason)),
            Source::Networks => Read::Networks(Err(reason)),
            Source::Devices => Read::Devices(Err(reason)),
            Source::Outputs => Read::Outputs(Err(reason)),
            Source::Layout => Read::Layout(Err(reason)),
        }
    }
}

/// A change the panel asks a service for.
#[derive(Clone, Debug, PartialEq)]
pub enum Change {
    /// Switch the Wi-Fi radio on or off; it may take a moment to follow.
    Wifi(bool),
    /// Power the Bluetooth adapter on or off.
    Bluetooth(bool),
    /// Let the default input record (`true`) or mute it.
    Microphone(bool),
    DockHiding(bool),
    /// The keyboard backlight on (at its full level) or off.
    Keyboard(bool),
    /// The display brightness, 0 to 1.
    Brightness(f64),
    /// The output volume: level and mute together.
    Volume(VolumeState),
    /// A button of the media tile, for the player of the last reading.
    Media {
        player: String,
        command: MediaCommand,
    },
    /// Join the network of this key with the credentials the service holds.
    Join(String),
    /// Connect the Bluetooth device of this identifier (`true`), or disconnect it.
    Connect(String, bool),
    /// Play to the output of this identifier from now on.
    Output(u32),
    /// Keep these names of controls as the controls shown, in order.
    Layout(Vec<String>),
    /// Lock the session.
    Lock,
}

impl Change {
    /// The source the change changes; `None` for the lock.
    pub fn source(&self) -> Option<Source> {
        Some(match self {
            Change::Wifi(_) => Source::Wifi,
            Change::Bluetooth(_) => Source::Bluetooth,
            Change::Microphone(_) => Source::Microphone,
            Change::DockHiding(_) => Source::DockHiding,
            Change::Keyboard(_) => Source::Keyboard,
            Change::Brightness(_) => Source::Brightness,
            Change::Volume(_) => Source::Volume,
            Change::Media { .. } => Source::Media,
            Change::Join(_) => Source::Networks,
            Change::Connect(..) => Source::Devices,
            Change::Output(_) => Source::Outputs,
            Change::Layout(_) => Source::Layout,
            Change::Lock => return None,
        })
    }

    /// The name a failure is logged with.
    pub fn what(&self) -> &'static str {
        match self {
            Change::Join(_) => "wi-fi join",
            Change::Connect(..) => "bluetooth device",
            Change::Output(_) => "sound output",
            Change::Lock => "lock screen",
            change => change.source().map_or("lock screen", Source::what),
        }
    }
}

/// What the on-screen display shows of a level the panel changed itself.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Volume,
    Mute,
    Brightness,
}

/// What the component is to carry out for the panel.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    /// Read a source; the reading goes to [`Controller::read`].
    Read(Source),
    /// Ask a service for a change; its outcome goes to [`Controller::changed`].
    Change(Change),
    /// Open the settings application on the page of this identifier; whether it started goes to
    /// [`Controller::settings_opened`].
    OpenSettings(&'static str),
    /// Show a level on the on-screen display.
    Overlay(OverlayKind, f64),
    /// Switch do not disturb on (`true`) or off, as the focus capsule asks.
    FocusMode(bool),
}

/// The last failure of each source and change, to log a failure only when it changes.
#[derive(Debug, Default)]
struct FailureLog {
    reasons: HashMap<&'static str, String>,
}

impl FailureLog {
    /// Log the outcome of `what` on stderr when it differs from the last one: a new or changed failure, or a recovery
    /// after one.
    fn log(&mut self, what: &'static str, outcome: Result<(), &str>) {
        match outcome {
            Ok(()) => {
                if self.reasons.remove(what).is_some() {
                    eprintln!("cupertino-controlcenter: {what}: available again");
                }
            }
            Err(reason) => {
                if self.reasons.get(what).map(String::as_str) != Some(reason) {
                    eprintln!("cupertino-controlcenter: {what}: {reason}");
                    self.reasons.insert(what, reason.to_owned());
                }
            }
        }
    }

    /// A reading from a service's answer; a failure is logged and becomes the reason.
    fn reading<T>(&mut self, what: &'static str, result: Result<T, String>) -> Reading<T> {
        match result {
            Ok(value) => {
                self.log(what, Ok(()));
                Reading::Ready(value)
            }
            Err(reason) => {
                self.log(what, Err(&reason));
                Reading::Unavailable(reason)
            }
        }
    }
}

/// A reading under way.
#[derive(Clone, Copy, Debug)]
struct Flight {
    /// The changes of the source asked for when it started.
    changes: u64,
    /// It was asked for again meanwhile.
    again: bool,
}

/// The changes of a source asked for so far, and how many are under way.
#[derive(Clone, Copy, Debug, Default)]
struct Changes {
    asked: u64,
    under_way: u32,
}

/// The panel's model and lifetime, and the effects they ask for.
#[derive(Debug)]
pub struct Controller {
    lifecycle: Lifecycle,
    controls: Controls,
    failures: FailureLog,
    /// The level being dragged, and the value not written yet.
    dragging: Option<Slider>,
    pending: Option<(Slider, f64)>,
    /// The level a write is under way for.
    writing: Option<Slider>,
    last_write: Option<f64>,
    last_refresh: Option<f64>,
    /// The detail view open, whose list each reading takes too.
    detail: Option<Detail>,
    /// The controls shown and their order, as stored.
    arrangement: Arrangement,
    /// The controls are being edited: the arrangement as changed so far, which nothing has stored yet.
    draft: Option<Arrangement>,
    flights: HashMap<Source, Flight>,
    changes: HashMap<Source, Changes>,
    /// The sources whose reading waits for their changes to end.
    stale: HashSet<Source>,
    /// The service's key of each network of the last list, by the name the list shows.
    keys: HashMap<String, String>,
    /// The player of the last media reading.
    player: Option<String>,
}

impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}

impl Controller {
    /// A hidden panel; nothing is read until it opens.
    pub fn new() -> Self {
        Self {
            lifecycle: Lifecycle::new(),
            controls: Controls::unread(),
            failures: FailureLog::default(),
            dragging: None,
            pending: None,
            writing: None,
            last_write: None,
            last_refresh: None,
            detail: None,
            arrangement: Arrangement::default(),
            draft: None,
            flights: HashMap::new(),
            changes: HashMap::new(),
            stale: HashSet::new(),
            keys: HashMap::new(),
            player: None,
        }
    }

    pub fn controls(&self) -> &Controls {
        &self.controls
    }

    pub fn phase(&self, now: f64) -> Phase {
        self.lifecycle.phase(now)
    }

    pub fn opacity(&self, now: f64) -> f64 {
        self.lifecycle.opacity(now)
    }

    /// Whether the panel is still fading in at `now`.
    pub fn opening(&self, now: f64) -> bool {
        self.lifecycle.opening(now)
    }

    /// With Reduce Motion the panel opens and closes without a fade.
    pub fn set_reduce_motion(&mut self, on: bool) {
        self.lifecycle.set_reduce_motion(on);
    }

    fn shown(&self, now: f64) -> bool {
        self.lifecycle.phase(now) == Phase::Shown
    }

    /// Open the panel and read every source, or dismiss it when it is shown.
    pub fn toggle(&mut self, now: f64) -> Vec<Effect> {
        let was_shown = self.shown(now);
        self.lifecycle.toggle(now);
        self.stop_dragging();
        self.draft = None;
        if was_shown {
            return Vec::new();
        }
        // the panel opens on its modules, never on the detail or the editing it closed with
        self.detail = None;
        self.refresh(now)
    }

    /// Start the fade-out; a drag in progress ends without a write.
    pub fn dismiss(&mut self, now: f64) {
        self.lifecycle.dismiss(now);
        self.stop_dragging();
        self.draft = None;
    }

    /// The panel's windows are gone with their output: it is hidden at once, without the fade, and a drag in progress
    /// ends without a write, as with a dismissal. The next toggle opens it again and reads every source.
    pub fn vanish(&mut self) {
        self.lifecycle.hide();
        self.stop_dragging();
        self.draft = None;
    }

    fn stop_dragging(&mut self) {
        self.dragging = None;
        self.pending = None;
    }

    /// Read every source, and the list of the detail view that is open. A level being dragged keeps its value.
    pub fn refresh(&mut self, now: f64) -> Vec<Effect> {
        self.last_refresh = Some(now);
        let mut sources = vec![
            Source::Wifi,
            Source::Bluetooth,
            Source::Microphone,
            Source::DockHiding,
            Source::Keyboard,
            Source::Media,
            Source::Layout,
        ];
        if self.dragging.is_none() {
            sources.extend([Source::Brightness, Source::Volume]);
        }
        sources.extend(self.detail_source());
        sources.into_iter().filter_map(|s| self.ask(s)).collect()
    }

    /// The list the open detail view shows.
    fn detail_source(&self) -> Option<Source> {
        match self.detail? {
            Detail::WiFi => Some(Source::Networks),
            Detail::Bluetooth => Some(Source::Devices),
            Detail::Sound => Some(Source::Outputs),
            Detail::Display => None,
        }
    }

    /// Whether a change of `source` is under way or waits to be written.
    fn changing(&self, source: Source) -> bool {
        let level = |slider: Slider| match slider {
            Slider::Display => Source::Brightness,
            Slider::Sound => Source::Volume,
        };
        self.changes.get(&source).is_some_and(|c| c.under_way > 0)
            || self
                .pending
                .is_some_and(|(slider, _)| level(slider) == source)
    }

    /// Ask for a reading of `source`: at once, unless one is under way (it is taken again when that one ends) or a
    /// change of the source is (it is taken when the change has ended).
    fn ask(&mut self, source: Source) -> Option<Effect> {
        if self.changing(source) {
            self.stale.insert(source);
            return None;
        }
        if let Some(flight) = self.flights.get_mut(&source) {
            flight.again = true;
            return None;
        }
        let changes = self.changes.get(&source).map_or(0, |c| c.asked);
        self.flights.insert(
            source,
            Flight {
                changes,
                again: false,
            },
        );
        Some(Effect::Read(source))
    }

    /// Ask a service for `change`; readings of its source wait for it.
    fn change(&mut self, change: Change) -> Effect {
        if let Some(source) = change.source() {
            let changes = self.changes.entry(source).or_default();
            changes.asked += 1;
            changes.under_way += 1;
        }
        Effect::Change(change)
    }

    /// A reading came back. It is shown unless a change of its source was asked for since it started; a reading asked
    /// for while it was under way is taken now.
    pub fn read(&mut self, now: f64, read: Read) -> Vec<Effect> {
        let source = read.source();
        let Some(flight) = self.flights.remove(&source) else {
            return Vec::new();
        };
        let changes = self.changes.get(&source).copied().unwrap_or_default();
        if changes.asked != flight.changes || changes.under_way > 0 {
            self.stale.insert(source);
            return self.retake(now, source).into_iter().collect();
        }
        self.apply(read);
        if (flight.again || self.stale.remove(&source)) && self.shown(now) {
            return self.ask(source).into_iter().collect();
        }
        Vec::new()
    }

    /// Take the reading of `source` that waited for its changes, once none is under way.
    fn retake(&mut self, now: f64, source: Source) -> Option<Effect> {
        if self.changing(source) || !self.shown(now) || !self.stale.contains(&source) {
            return None;
        }
        let effect = self.ask(source);
        if effect.is_some() {
            self.stale.remove(&source);
        }
        effect
    }

    /// Show a reading.
    fn apply(&mut self, read: Read) {
        let what = read.source().what();
        let (failures, controls) = (&mut self.failures, &mut self.controls);
        match read {
            Read::Wifi(r) => controls.wifi = failures.reading(what, r),
            Read::Bluetooth(r) => controls.bluetooth = failures.reading(what, r),
            Read::Microphone(r) => controls.microphone = failures.reading(what, r),
            Read::DockHiding(r) => controls.dock_hiding = failures.reading(what, r),
            Read::Keyboard(r) => controls.keyboard = failures.reading(what, r),
            // a level being dragged shows where the pointer has it
            Read::Brightness(_) if self.dragging == Some(Slider::Display) => {}
            Read::Volume(_) if self.dragging == Some(Slider::Sound) => {}
            Read::Brightness(r) => controls.brightness = failures.reading(what, r),
            Read::Volume(r) => controls.volume = failures.reading(what, r),
            Read::Media(r) => {
                let reading = failures.reading(what, r);
                self.player = reading
                    .ready()
                    .and_then(|playing| playing.as_ref().map(|p| p.player.clone()));
                controls.media = match reading {
                    Reading::Ready(playing) => Reading::Ready(playing.map(|p| p.media)),
                    Reading::Unavailable(reason) => Reading::Unavailable(reason),
                };
            }
            Read::Networks(r) => {
                let reading = failures.reading(what, r);
                if let Reading::Ready(listed) = &reading {
                    self.keys = listed
                        .iter()
                        .map(|(network, key)| (network.ssid.clone(), key.clone()))
                        .collect();
                }
                controls.networks = match reading {
                    Reading::Ready(listed) => {
                        Reading::Ready(listed.into_iter().map(|(network, _)| network).collect())
                    }
                    Reading::Unavailable(reason) => Reading::Unavailable(reason),
                };
            }
            Read::Devices(r) => controls.devices = failures.reading(what, r),
            Read::Outputs(r) => controls.outputs = failures.reading(what, r),
            Read::Layout(r) => self.stored_layout(r),
        }
    }

    /// The stored choice of controls, so one changed while the panel runs is followed. Nothing stored is every control
    /// in reading order. A reading that fails keeps the controls shown and is logged. While the controls are edited
    /// the panel shows the edit.
    fn stored_layout(&mut self, stored: Result<Option<Vec<String>>, String>) {
        self.failures.log(
            Source::Layout.what(),
            stored.as_ref().map(|_| ()).map_err(String::as_str),
        );
        let Ok(stored) = stored else {
            return;
        };
        self.arrangement =
            stored.map_or_else(Arrangement::default, |ids| Arrangement::from_ids(&ids));
        // a view whose module is no longer shown has no tile to close into
        if self.draft.is_none()
            && self
                .detail
                .is_some_and(|detail| !self.arrangement.contains(detail.module()))
        {
            self.detail = None;
        }
    }

    /// A change came back: `Ok` with the volume it reached for a volume change, or why it failed. A failed change
    /// disables its tile with the reason, or for a list its view.
    pub fn changed(
        &mut self,
        now: f64,
        change: Change,
        result: Result<Option<VolumeState>, String>,
    ) -> Vec<Effect> {
        let source = change.source();
        if let Some(changes) = source.and_then(|s| self.changes.get_mut(&s)) {
            changes.under_way = changes.under_way.saturating_sub(1);
        }
        self.failures.log(
            change.what(),
            result.as_ref().map(|_| ()).map_err(String::as_str),
        );
        let mut effects = Vec::new();
        match (change, result) {
            (Change::Brightness(_), result) => {
                if self.writing == Some(Slider::Display) {
                    self.writing = None;
                }
                if let Err(reason) = result {
                    self.controls.brightness = Reading::Unavailable(reason);
                }
            }
            (Change::Volume(_), result) => {
                if self.writing == Some(Slider::Sound) {
                    self.writing = None;
                }
                match result {
                    // the level the output took, unless the pointer has moved it since
                    Ok(Some(reached)) if self.dragging.is_none() && self.pending.is_none() => {
                        self.controls.volume = Reading::Ready(reached);
                    }
                    Ok(_) => {}
                    Err(reason) => self.controls.volume = Reading::Unavailable(reason),
                }
            }
            (change, Err(reason)) => self.refused(change, reason),
            // the readings that show what the change did
            (Change::Media { .. }, Ok(_)) => effects.extend(self.ask(Source::Media)),
            (Change::Join(_), Ok(_)) => {
                effects.extend(self.ask(Source::Wifi));
                effects.extend(self.ask(Source::Networks));
            }
            (Change::Output(_), Ok(_)) => effects.extend(self.ask(Source::Volume)),
            (_, Ok(_)) => {}
        }
        effects.extend(source.and_then(|s| self.retake(now, s)));
        // a device's link is read again whatever its outcome: the call may answer late
        if source == Some(Source::Devices) && self.shown(now) {
            effects.extend(self.ask(Source::Devices));
        }
        effects.extend(self.flush(now));
        effects
    }

    /// A change the service refused: the tile or the view shows why.
    fn refused(&mut self, change: Change, reason: String) {
        let controls = &mut self.controls;
        match change {
            Change::Wifi(_) => controls.wifi = Reading::Unavailable(reason),
            Change::Bluetooth(_) => controls.bluetooth = Reading::Unavailable(reason),
            Change::Microphone(_) => controls.microphone = Reading::Unavailable(reason),
            Change::DockHiding(_) => controls.dock_hiding = Reading::Unavailable(reason),
            Change::Keyboard(_) => controls.keyboard = Reading::Unavailable(reason),
            Change::Join(_) => controls.networks = Reading::Unavailable(reason),
            Change::Output(_) => controls.outputs = Reading::Unavailable(reason),
            // the stored controls come back: the panel never shows controls the next opening would not
            Change::Layout(_) => {
                self.stale.insert(Source::Layout);
            }
            // nothing on screen changed for these; the log has the reason
            Change::Media { .. }
            | Change::Connect(..)
            | Change::Lock
            | Change::Brightness(_)
            | Change::Volume(_) => {}
        }
    }

    /// The controls the panel shows and their order: the stored ones, or while they are edited the edit so far.
    pub fn arrangement(&self) -> &Arrangement {
        self.draft.as_ref().unwrap_or(&self.arrangement)
    }

    /// Whether the controls are being edited.
    pub fn editing(&self) -> bool {
        self.draft.is_some()
    }

    /// Start editing the controls ("Edit Controls"): the panel stays open on its grid, and nothing is stored until
    /// [`finish_edit`](Self::finish_edit). Returns false when they are edited already.
    pub fn edit_controls(&mut self) -> bool {
        if self.draft.is_some() {
            return false;
        }
        self.stop_dragging();
        self.detail = None;
        self.draft = Some(self.arrangement.clone());
        true
    }

    /// Change the edit; false when the controls are not being edited or nothing changed.
    fn edit(&mut self, change: impl FnOnce(&mut Arrangement) -> bool) -> bool {
        self.draft.as_mut().is_some_and(change)
    }

    /// While editing: stop showing `module`.
    pub fn edit_remove(&mut self, module: Module) -> bool {
        self.edit(|draft| draft.remove(module))
    }

    /// While editing: show `module`, after the others.
    pub fn edit_add(&mut self, module: Module) -> bool {
        self.edit(|draft| draft.add(module))
    }

    /// While editing: move `module` one place later (`forward`) or earlier.
    pub fn edit_step(&mut self, module: Module, forward: bool) -> bool {
        self.edit(|draft| draft.step(module, forward))
    }

    /// While editing: move `module` to the place nearest to `origin`, the corner of its tile as the pointer holds
    /// it, from the grid's corner.
    pub fn edit_move_near(&mut self, module: Module, origin: Point) -> bool {
        self.edit(|draft| draft.move_near(module, origin))
    }

    /// End editing and keep the edit ("Done"): it is shown from now on and stored. A store that fails is logged and
    /// the stored controls are read again, so the panel never shows controls the next opening would not.
    pub fn finish_edit(&mut self) -> Vec<Effect> {
        let Some(draft) = self.draft.take() else {
            return Vec::new();
        };
        let ids = draft.ids().iter().map(|id| (*id).to_owned()).collect();
        self.arrangement = draft;
        vec![self.change(Change::Layout(ids))]
    }

    /// End editing and drop the edit ("Cancel", Escape): the panel shows the stored controls again. Returns whether
    /// the controls were being edited.
    pub fn cancel_edit(&mut self) -> bool {
        self.draft.take().is_some()
    }

    /// The Wi-Fi radio (`wifi`) or the Bluetooth adapter (`bluetooth`) changed: a shown panel reads that radio again
    /// at once, with its list when its detail view is open, and no other source.
    pub fn radios_changed(&mut self, now: f64, wifi: bool, bluetooth: bool) -> Vec<Effect> {
        if !self.shown(now) {
            return Vec::new();
        }
        let mut sources = Vec::new();
        if wifi {
            sources.push(Source::Wifi);
            if self.detail == Some(Detail::WiFi) {
                sources.push(Source::Networks);
            }
        }
        if bluetooth {
            sources.push(Source::Bluetooth);
            if self.detail == Some(Detail::Bluetooth) {
                sources.push(Source::Devices);
            }
        }
        sources.into_iter().filter_map(|s| self.ask(s)).collect()
    }

    /// Do not disturb as its owner last published it, or why it cannot be switched: the focus capsule shows it.
    pub fn set_focus_mode(&mut self, reading: Reading<bool>) {
        self.controls.focus_mode = reading;
    }

    /// The detail view that is open.
    pub fn detail(&self) -> Option<Detail> {
        self.detail
    }

    /// Open `detail` and read its list at once; `None` closes the detail view.
    pub fn show_detail(&mut self, detail: Option<Detail>) -> Vec<Effect> {
        self.detail = detail;
        self.detail_source()
            .and_then(|source| self.ask(source))
            .into_iter()
            .collect()
    }

    /// Press a toggle: switch the radio, adapter, setting or focus mode the other way, or lock the session. A disabled
    /// tile ignores the press. The tile shows the change at once.
    pub fn press(&mut self, module: Module) -> Vec<Effect> {
        if matches!(self.controls.tile(module), TileState::Disabled(_)) {
            return Vec::new();
        }
        let flip = |reading: &mut Reading<bool>| match reading {
            Reading::Ready(on) => {
                *on = !*on;
                Some(*on)
            }
            Reading::Unavailable(_) => None,
        };
        let controls = &mut self.controls;
        let change = match module {
            Module::WiFi => {
                let Reading::Ready(status) = &mut controls.wifi else {
                    return Vec::new();
                };
                // the radio follows the switch later; the next reading shows the network again
                status.enabled = !status.enabled;
                if !status.enabled {
                    status.network = None;
                }
                Change::Wifi(status.enabled)
            }
            Module::Bluetooth => match flip(&mut controls.bluetooth) {
                Some(on) => Change::Bluetooth(on),
                None => return Vec::new(),
            },
            Module::Microphone => match flip(&mut controls.microphone) {
                Some(on) => Change::Microphone(on),
                None => return Vec::new(),
            },
            Module::DockHiding => match flip(&mut controls.dock_hiding) {
                Some(on) => Change::DockHiding(on),
                None => return Vec::new(),
            },
            Module::KeyboardBrightness => match flip(&mut controls.keyboard) {
                Some(on) => Change::Keyboard(on),
                None => return Vec::new(),
            },
            Module::FocusMode => {
                return flip(&mut controls.focus_mode)
                    .map(Effect::FocusMode)
                    .into_iter()
                    .collect()
            }
            Module::Mute => {
                let Reading::Ready(volume) = &mut controls.volume else {
                    return Vec::new();
                };
                // unmuting a level dragged to the end brings back an audible level
                if volume.muted && volume.level < MUTE_BELOW {
                    volume.level = UNMUTED_LEVEL;
                }
                volume.muted = !volume.muted;
                Change::Volume(*volume)
            }
            Module::LockScreen => Change::Lock,
            Module::Display | Module::Sound | Module::NowPlaying => return Vec::new(),
        };
        vec![self.change(change)]
    }

    /// A button of the media tile, for the player the tile shows.
    pub fn media(&mut self, command: MediaCommand) -> Vec<Effect> {
        match self.player.clone() {
            Some(player) => vec![self.change(Change::Media { player, command })],
            None => {
                self.failures
                    .log(Source::Media.what(), Err("no media player is open"));
                Vec::new()
            }
        }
    }

    /// Join a network of the Wi-Fi view by the name it shows; the list shows the outcome.
    pub fn join(&mut self, ssid: &str) -> Vec<Effect> {
        let key = self.keys.get(ssid).map_or(ssid, String::as_str).to_owned();
        vec![self.change(Change::Join(key))]
    }

    /// Connect a device of the Bluetooth view, or disconnect it when it is connected.
    pub fn connect(&mut self, id: &str) -> Vec<Effect> {
        let Some(connected) = self
            .controls
            .devices
            .ready()
            .and_then(|devices| devices.iter().find(|d| d.id == id))
            .map(|d| d.connected)
        else {
            return Vec::new();
        };
        vec![self.change(Change::Connect(id.to_owned(), !connected))]
    }

    /// Play to an output of the Sound view; the list shows it at once.
    pub fn select_output(&mut self, id: u32) -> Vec<Effect> {
        let Reading::Ready(outputs) = &mut self.controls.outputs else {
            return Vec::new();
        };
        if !outputs.iter().any(|o| o.id == id) {
            return Vec::new();
        }
        for output in outputs.iter_mut() {
            output.current = output.id == id;
        }
        vec![self.change(Change::Output(id))]
    }

    /// Open the settings application at `pane`.
    pub fn open_settings(&mut self, pane: &'static str) -> Vec<Effect> {
        vec![Effect::OpenSettings(pane)]
    }

    /// Whether the settings application started; when it did, the panel goes.
    pub fn settings_opened(&mut self, now: f64, opened: Result<(), String>) {
        self.failures.log(
            "settings",
            opened.as_ref().map(|_| ()).map_err(String::as_str),
        );
        if opened.is_ok() {
            self.dismiss(now);
        }
    }

    /// The pointer put `slider` at `fraction`: the model follows at once and the service is written at most every
    /// [`WRITE_INTERVAL`], one write at a time, the latest value winning.
    pub fn slide(&mut self, now: f64, slider: Slider, fraction: f64) -> Vec<Effect> {
        if !self.controls.set_fraction(slider, fraction) {
            return Vec::new();
        }
        self.dragging = Some(slider);
        self.pending = Some((slider, fraction.clamp(0.0, 1.0)));
        self.flush(now)
    }

    /// Write the waiting level when no write is under way and the throttle allows it.
    pub fn flush(&mut self, now: f64) -> Vec<Effect> {
        let due = self
            .last_write
            .is_none_or(|last| now - last >= WRITE_INTERVAL);
        if self.writing.is_some() || !due {
            return Vec::new();
        }
        self.write_pending(now).into_iter().collect()
    }

    /// The pointer let go of the level: the last value is written as soon as the write under way has ended, and the
    /// on-screen display shows the level.
    pub fn release(&mut self, now: f64) -> Vec<Effect> {
        let mut effects: Vec<Effect> = if self.writing.is_none() {
            self.write_pending(now).into_iter().collect()
        } else {
            Vec::new()
        };
        let Some(slider) = self.dragging.take() else {
            return effects;
        };
        let shown = match (slider, &self.controls.volume, &self.controls.brightness) {
            (Slider::Display, _, Reading::Ready(level)) => (OverlayKind::Brightness, *level),
            (Slider::Sound, Reading::Ready(volume), _) if volume.muted => (OverlayKind::Mute, 0.0),
            (Slider::Sound, Reading::Ready(volume), _) => (OverlayKind::Volume, volume.level),
            // the service failed during the drag: there is no level to show
            _ => return effects,
        };
        effects.push(Effect::Overlay(shown.0, shown.1));
        effects
    }

    fn write_pending(&mut self, now: f64) -> Option<Effect> {
        let (slider, fraction) = self.pending.take()?;
        self.last_write = Some(now);
        self.writing = Some(slider);
        Some(self.change(match slider {
            Slider::Display => Change::Brightness(fraction),
            Slider::Sound => Change::Volume(VolumeState {
                level: fraction,
                muted: fraction < MUTE_BELOW,
            }),
        }))
    }

    /// Write a waiting level, and read the sources again when the panel has been open for [`REFRESH_INTERVAL`] since
    /// the last reading.
    pub fn tick(&mut self, now: f64) -> Vec<Effect> {
        let mut effects = self.flush(now);
        if self.shown(now)
            && self.dragging.is_none()
            && self
                .last_refresh
                .is_none_or(|last| now - last >= REFRESH_INTERVAL)
        {
            effects.extend(self.refresh(now));
        }
        effects
    }

    /// Seconds from `now` until [`tick`](Self::tick) or the end of the fade-out has something to do.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        let write = self.pending.filter(|_| self.writing.is_none()).map(|_| {
            self.last_write
                .map_or(0.0, |last| (last + WRITE_INTERVAL - now).max(0.0))
        });
        let refresh = (self.shown(now) && self.dragging.is_none()).then(|| {
            self.last_refresh
                .map_or(0.0, |last| (last + REFRESH_INTERVAL - now).max(0.0))
        });
        [write, refresh, self.lifecycle.until_hidden(now)]
            .into_iter()
            .flatten()
            .min_by(f64::total_cmp)
    }
}
