//! Carries out what the user does on the panel on the backends and keeps the model in step.
//!
//! A reading or write that fails disables its tile with the reason and logs it on stderr; nothing falls back to a
//! default. The panel reads again every [`REFRESH_INTERVAL`], so a failure is logged when it starts or its reason
//! changes and once more when the backend recovers, not on every retry. Time is injected as seconds on any monotonic
//! clock.

use std::collections::HashMap;

use cupertino_geometry::Point;

use crate::arrangement::Arrangement;
use crate::backend::{
    BackendError, Bluetooth, Brightness, Launcher, LayoutStore, MediaPlayer, OverlayKind,
    OverlayNotifier, SessionLock, Switch, Volume, Wifi,
};
use crate::lifecycle::{Lifecycle, Phase};
use crate::model::{
    Controls, Detail, MediaCommand, Module, Reading, Slider, TileState, MUTE_BELOW,
};

/// Shortest time between two backend writes while a slider is dragged; the latest value wins
/// (write throttle 0.0167 s for the volume; applied to the brightness as well, approximate).
pub const WRITE_INTERVAL: f64 = 0.0167;

/// How often an open panel reads the backends again, so a radio that follows a switch late, or a level changed by a key,
/// shows up (approximate).
pub const REFRESH_INTERVAL: f64 = 1.0;

/// The backends the controller drives.
pub struct Backends {
    pub wifi: Box<dyn Wifi>,
    pub bluetooth: Box<dyn Bluetooth>,
    pub microphone: Box<dyn Switch>,
    pub dock_hiding: Box<dyn Switch>,
    pub keyboard: Box<dyn Switch>,
    pub focus_mode: Box<dyn Switch>,
    pub brightness: Box<dyn Brightness>,
    pub volume: Box<dyn Volume>,
    pub media: Box<dyn MediaPlayer>,
    pub lock: Box<dyn SessionLock>,
    pub launcher: Box<dyn Launcher>,
    pub overlay: Box<dyn OverlayNotifier>,
    /// Where the choice of controls is kept.
    pub layout: Box<dyn LayoutStore>,
}

/// A slider value waiting for the write throttle.
#[derive(Clone, Copy, Debug)]
struct PendingWrite {
    slider: Slider,
    fraction: f64,
}

/// The level an unmuted output takes when it was muted by dragging its level to the end.
const UNMUTED_LEVEL: f64 = 0.25;

/// Switch `reading` the other way in `backend`; a refused write disables the tile with the reason.
fn flip(
    reading: &mut Reading<bool>,
    backend: &mut dyn Switch,
    what: &'static str,
    failures: &mut FailureLog,
) {
    let Reading::Ready(on) = *reading else {
        return;
    };
    *reading = match backend.set(!on) {
        Ok(()) => Reading::Ready(!on),
        Err(error) => failures.reading(what, Err(error)),
    };
}

/// The panel's model, lifetime and backends.
pub struct Controller {
    lifecycle: Lifecycle,
    controls: Controls,
    backends: Backends,
    failures: FailureLog,
    /// The slider being dragged and the value not written yet.
    dragging: Option<Slider>,
    pending: Option<PendingWrite>,
    last_write: Option<f64>,
    last_refresh: Option<f64>,
    /// The detail view open, whose list each reading takes too.
    detail: Option<Detail>,
    /// The controls shown and their order, as stored.
    arrangement: Arrangement,
    /// The controls are being edited: the arrangement as changed so far, which nothing has stored yet.
    draft: Option<Arrangement>,
}

/// The last failure of each backend, to log a failure only when it changes.
#[derive(Default)]
struct FailureLog {
    reasons: HashMap<&'static str, String>,
}

impl FailureLog {
    /// Record the outcome of `what` and return the line to log, if the outcome differs from the last one: a new or
    /// changed failure, or a recovery after one.
    fn record(&mut self, what: &'static str, outcome: Result<(), &BackendError>) -> Option<String> {
        match outcome {
            Ok(()) => self
                .reasons
                .remove(what)
                .map(|_| format!("cupertino-controlcenter: {what}: available again")),
            Err(error) => {
                let reason = error.to_string();
                if self.reasons.get(what) == Some(&reason) {
                    return None;
                }
                let line = format!("cupertino-controlcenter: {what}: {reason}");
                self.reasons.insert(what, reason);
                Some(line)
            }
        }
    }

    /// Log the outcome of `what` on stderr when it changed.
    fn log(&mut self, what: &'static str, outcome: Result<(), &BackendError>) {
        if let Some(line) = self.record(what, outcome) {
            eprintln!("{line}");
        }
    }

    /// A reading from a backend result; a failure is logged and becomes the reason.
    fn reading<T>(&mut self, what: &'static str, result: Result<T, BackendError>) -> Reading<T> {
        match result {
            Ok(value) => {
                self.log(what, Ok(()));
                Reading::Ready(value)
            }
            Err(error) => {
                self.log(what, Err(&error));
                Reading::Unavailable(error.to_string())
            }
        }
    }
}

impl Controller {
    /// A hidden panel; nothing is read until it opens.
    pub fn new(backends: Backends) -> Self {
        Self {
            lifecycle: Lifecycle::new(),
            controls: Controls::unread(),
            backends,
            failures: FailureLog::default(),
            dragging: None,
            pending: None,
            last_write: None,
            last_refresh: None,
            detail: None,
            arrangement: Arrangement::default(),
            draft: None,
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

    /// Open the panel and read every backend, or dismiss it when it is shown (the `Toggle` call).
    pub fn toggle(&mut self, now: f64) {
        let was_shown = self.lifecycle.phase(now) == Phase::Shown;
        self.lifecycle.toggle(now);
        if !was_shown {
            // the panel opens on its modules, never on the detail or the editing it closed with
            self.detail = None;
            self.draft = None;
            self.refresh(now);
        } else {
            self.stop_dragging();
            self.draft = None;
        }
    }

    /// Start the fade-out; a drag in progress ends without a write.
    pub fn dismiss(&mut self, now: f64) {
        self.lifecycle.dismiss(now);
        self.stop_dragging();
        self.draft = None;
    }

    /// The panel's surfaces are gone with their output: it is hidden at once, without the fade, and a drag in progress
    /// ends without a write, as with a dismissal. The next toggle opens it again and reads every backend.
    pub fn vanish(&mut self) {
        self.lifecycle.hide();
        self.stop_dragging();
        self.draft = None;
    }

    fn stop_dragging(&mut self) {
        self.dragging = None;
        self.pending = None;
    }

    /// Read every backend, and the list of the detail view that is open. A slider being dragged keeps its value.
    pub fn refresh(&mut self, now: f64) {
        self.last_refresh = Some(now);
        let (backends, failures) = (&mut self.backends, &mut self.failures);
        let controls = &mut self.controls;
        controls.wifi = failures.reading("wi-fi", backends.wifi.read());
        controls.bluetooth = failures.reading("bluetooth", backends.bluetooth.read());
        controls.microphone = failures.reading("microphone", backends.microphone.read());
        controls.dock_hiding = failures.reading("dock hiding", backends.dock_hiding.read());
        controls.keyboard = failures.reading("keyboard backlight", backends.keyboard.read());
        controls.focus_mode = failures.reading("focus mode", backends.focus_mode.read());
        controls.media = failures.reading("media", backends.media.read());
        if self.dragging.is_none() {
            controls.brightness =
                failures.reading("display brightness", backends.brightness.read());
            controls.volume = failures.reading("volume", backends.volume.read());
        }
        self.read_layout();
        self.read_detail();
    }

    /// Read the stored choice of controls, so one changed while the panel runs is followed. Nothing stored is every
    /// control in reading order. A reading that fails keeps the controls shown and is logged. While the controls are
    /// edited the panel shows the edit, and the store is read again when it ends.
    fn read_layout(&mut self) {
        if self.draft.is_some() {
            return;
        }
        let stored = self.backends.layout.load();
        self.failures
            .log("controls shown", stored.as_ref().map(|_| ()));
        if let Ok(stored) = stored {
            self.arrangement =
                stored.map_or_else(Arrangement::default, |ids| Arrangement::from_ids(&ids));
            // a view whose module is no longer shown has no tile to close into
            if self
                .detail
                .is_some_and(|detail| !self.arrangement.contains(detail.module()))
            {
                self.detail = None;
            }
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

    /// End editing and keep the edit ("Done"): it is stored and shown from now on. A store that fails is logged and
    /// the edit is dropped, so the panel never shows controls the next opening would not. Returns whether an edit
    /// was kept.
    pub fn finish_edit(&mut self) -> bool {
        let Some(draft) = self.draft.take() else {
            return false;
        };
        let stored = self.backends.layout.store(&draft.ids());
        self.failures
            .log("controls shown", stored.as_ref().map(|_| ()));
        if stored.is_ok() {
            self.arrangement = draft;
        }
        stored.is_ok()
    }

    /// End editing and drop the edit ("Cancel", Escape): the panel shows the stored controls again. Returns whether
    /// the controls were being edited.
    pub fn cancel_edit(&mut self) -> bool {
        self.draft.take().is_some()
    }

    /// A backend said its state changed: a shown panel reads again at once, without waiting for
    /// [`REFRESH_INTERVAL`]. Returns whether it read.
    pub fn backends_changed(&mut self, now: f64) -> bool {
        let shown = self.lifecycle.phase(now) == Phase::Shown;
        if shown {
            self.refresh(now);
        }
        shown
    }

    /// The Wi-Fi radio (`wifi`) or the Bluetooth adapter (`bluetooth`) changed: a shown panel reads that radio again
    /// at once, with its list when its detail view is open, and no other backend. Returns whether it read.
    pub fn radios_changed(&mut self, now: f64, wifi: bool, bluetooth: bool) -> bool {
        if self.lifecycle.phase(now) != Phase::Shown || !(wifi || bluetooth) {
            return false;
        }
        let (backends, failures) = (&mut self.backends, &mut self.failures);
        if wifi {
            self.controls.wifi = failures.reading("wi-fi", backends.wifi.read());
        }
        if bluetooth {
            self.controls.bluetooth = failures.reading("bluetooth", backends.bluetooth.read());
        }
        match self.detail {
            Some(Detail::WiFi) if wifi => self.read_detail(),
            Some(Detail::Bluetooth) if bluetooth => self.read_detail(),
            _ => {}
        }
        true
    }

    /// Read the list of the open detail view.
    fn read_detail(&mut self) {
        let (backends, failures) = (&mut self.backends, &mut self.failures);
        let controls = &mut self.controls;
        match self.detail {
            Some(Detail::WiFi) => {
                controls.networks = failures.reading("wi-fi networks", backends.wifi.networks())
            }
            Some(Detail::Bluetooth) => {
                controls.devices =
                    failures.reading("bluetooth devices", backends.bluetooth.devices())
            }
            Some(Detail::Sound) => {
                controls.outputs = failures.reading("sound outputs", backends.volume.outputs())
            }
            Some(Detail::Display) | None => {}
        }
    }

    /// The detail view that is open.
    pub fn detail(&self) -> Option<Detail> {
        self.detail
    }

    /// Open `detail` and read its list at once; `None` closes the detail view.
    pub fn show_detail(&mut self, detail: Option<Detail>) {
        self.detail = detail;
        self.read_detail();
    }

    /// Press a toggle: switch the radio, adapter or focus mode the other way. A disabled tile ignores the press.
    /// Returns whether anything changed on screen.
    pub fn press(&mut self, module: Module) -> bool {
        if matches!(self.controls.tile(module), TileState::Disabled(_)) {
            return false;
        }
        let backends = &mut self.backends;
        match module {
            Module::WiFi => {
                let Reading::Ready(status) = &self.controls.wifi else {
                    return false;
                };
                let enabled = !status.enabled;
                match backends.wifi.set_enabled(enabled) {
                    // the radio follows the switch later; the next reading shows the network again
                    Ok(()) => {
                        if let Reading::Ready(status) = &mut self.controls.wifi {
                            status.enabled = enabled;
                            status.network = status.network.take().filter(|_| enabled);
                        }
                    }
                    Err(error) => {
                        self.controls.wifi = self
                            .failures
                            .reading::<crate::model::WifiStatus>("wi-fi", Err(error))
                    }
                }
            }
            Module::Bluetooth => {
                let Reading::Ready(on) = self.controls.bluetooth else {
                    return false;
                };
                self.controls.bluetooth = match backends.bluetooth.set_powered(!on) {
                    Ok(()) => Reading::Ready(!on),
                    Err(error) => self.failures.reading("bluetooth", Err(error)),
                };
            }
            Module::Microphone => flip(
                &mut self.controls.microphone,
                &mut *backends.microphone,
                "microphone",
                &mut self.failures,
            ),
            Module::DockHiding => flip(
                &mut self.controls.dock_hiding,
                &mut *backends.dock_hiding,
                "dock hiding",
                &mut self.failures,
            ),
            Module::KeyboardBrightness => flip(
                &mut self.controls.keyboard,
                &mut *backends.keyboard,
                "keyboard backlight",
                &mut self.failures,
            ),
            Module::FocusMode => flip(
                &mut self.controls.focus_mode,
                &mut *backends.focus_mode,
                "focus mode",
                &mut self.failures,
            ),
            Module::Mute => {
                let Reading::Ready(volume) = self.controls.volume else {
                    return false;
                };
                // unmuting a level dragged to the end brings back an audible level
                let level = if volume.muted && volume.level < MUTE_BELOW {
                    UNMUTED_LEVEL
                } else {
                    volume.level
                };
                self.controls.volume = self
                    .failures
                    .reading("volume", backends.volume.set(level, !volume.muted));
            }
            Module::LockScreen => {
                let locked = backends.lock.lock();
                self.failures
                    .log("lock screen", locked.as_ref().map(|_| ()));
                if locked.is_err() {
                    return false;
                }
            }
            Module::Display | Module::Sound | Module::NowPlaying => return false,
        }
        true
    }

    /// A button of the media tile.
    pub fn media(&mut self, command: MediaCommand) -> bool {
        let sent = self.backends.media.command(command);
        self.failures.log("media", sent.as_ref().map(|_| ()));
        if sent.is_err() {
            return false;
        }
        self.controls.media = self.failures.reading("media", self.backends.media.read());
        true
    }

    /// Join a network of the Wi-Fi detail; the list shows the outcome at a later reading.
    pub fn join(&mut self, ssid: &str) -> bool {
        let joined = self.backends.wifi.join(ssid);
        self.failures.log("wi-fi join", joined.as_ref().map(|_| ()));
        if let Err(error) = joined {
            self.controls.networks = Reading::Unavailable(error.to_string());
            return true;
        }
        self.controls.wifi = self.failures.reading("wi-fi", self.backends.wifi.read());
        self.read_detail();
        true
    }

    /// Connect a device of the Bluetooth detail, or disconnect it when it is connected.
    pub fn connect(&mut self, id: &str) -> bool {
        let Some(connected) = self
            .controls
            .devices
            .ready()
            .and_then(|devices| devices.iter().find(|d| d.id == id))
            .map(|d| d.connected)
        else {
            return false;
        };
        let done = self.backends.bluetooth.set_connected(id, !connected);
        self.failures
            .log("bluetooth device", done.as_ref().map(|_| ()));
        if let Err(error) = done {
            self.controls.devices = Reading::Unavailable(error.to_string());
            return true;
        }
        self.read_detail();
        true
    }

    /// Play to an output of the Sound detail.
    pub fn select_output(&mut self, id: u32) -> bool {
        let done = self.backends.volume.select_output(id);
        self.failures.log("sound output", done.as_ref().map(|_| ()));
        if let Err(error) = done {
            self.controls.outputs = Reading::Unavailable(error.to_string());
            return true;
        }
        self.controls.volume = self.failures.reading("volume", self.backends.volume.read());
        self.read_detail();
        true
    }

    /// Opens the settings application at `pane` and, when it starts, dismisses the panel.
    pub fn open_settings(&mut self, now: f64, pane: &str) -> bool {
        let opened = self.backends.launcher.open_settings(pane);
        self.failures.log("settings", opened.as_ref().map(|_| ()));
        if opened.is_ok() {
            self.dismiss(now);
        }
        opened.is_ok()
    }

    /// The pointer put `slider` at `fraction`: the model follows at once and the backend is written at most every
    /// [`WRITE_INTERVAL`], the latest value winning. Returns whether anything changed on screen.
    pub fn slide(&mut self, now: f64, slider: Slider, fraction: f64) -> bool {
        if !self.controls.set_fraction(slider, fraction) {
            return false;
        }
        self.dragging = Some(slider);
        self.pending = Some(PendingWrite { slider, fraction });
        self.flush(now);
        true
    }

    /// Write the waiting slider value when the throttle allows it.
    pub fn flush(&mut self, now: f64) {
        if self.pending.is_some()
            && self
                .last_write
                .is_none_or(|last| now - last >= WRITE_INTERVAL)
        {
            self.write_pending(now);
        }
    }

    /// The pointer let go of the slider: write the last value and tell the overlay what the level is.
    pub fn release(&mut self, now: f64) {
        self.write_pending(now);
        let Some(slider) = self.dragging.take() else {
            return;
        };
        let (kind, level) = match (slider, &self.controls.volume, &self.controls.brightness) {
            (Slider::Display, _, Reading::Ready(level)) => (OverlayKind::Brightness, *level),
            (Slider::Sound, Reading::Ready(volume), _) if volume.muted => (OverlayKind::Mute, 0.0),
            (Slider::Sound, Reading::Ready(volume), _) => (OverlayKind::Volume, volume.level),
            // the backend failed during the drag: there is no level to show
            _ => return,
        };
        let shown = self.backends.overlay.show(kind, level);
        self.failures.log("overlay", shown.as_ref().map(|_| ()));
    }

    fn write_pending(&mut self, now: f64) {
        let Some(PendingWrite { slider, fraction }) = self.pending.take() else {
            return;
        };
        self.last_write = Some(now);
        match slider {
            Slider::Display => {
                if let Err(error) = self.backends.brightness.set(fraction) {
                    self.controls.brightness = self
                        .failures
                        .reading::<f64>("display brightness", Err(error));
                }
            }
            Slider::Sound => {
                self.controls.volume = self.failures.reading(
                    "volume",
                    self.backends.volume.set(fraction, fraction < MUTE_BELOW),
                );
            }
        }
    }

    /// Read the backends again when the panel has been open for [`REFRESH_INTERVAL`] since the last reading.
    pub fn tick(&mut self, now: f64) {
        self.flush(now);
        if self.lifecycle.phase(now) == Phase::Shown
            && self.dragging.is_none()
            && self
                .last_refresh
                .is_none_or(|last| now - last >= REFRESH_INTERVAL)
        {
            self.refresh(now);
        }
    }

    /// Seconds from `now` until [`tick`](Self::tick) or the end of the fade-out has something to do.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        let write = self.pending.map(|_| {
            self.last_write
                .map_or(0.0, |last| (last + WRITE_INTERVAL - now).max(0.0))
        });
        let refresh =
            (self.lifecycle.phase(now) == Phase::Shown && self.dragging.is_none()).then(|| {
                self.last_refresh
                    .map_or(0.0, |last| (last + REFRESH_INTERVAL - now).max(0.0))
            });
        [write, refresh, self.lifecycle.until_hidden(now)]
            .into_iter()
            .flatten()
            .min_by(f64::total_cmp)
    }
}
