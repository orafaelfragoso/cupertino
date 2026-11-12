//! Carries out what the user does on the panel on the backends and keeps the model in step.
//!
//! A reading or write that fails disables its tile with the reason and logs it on stderr; nothing falls back to a
//! default. The panel reads again every [`REFRESH_INTERVAL`], so a failure is logged when it starts or its reason
//! changes and once more when the backend recovers, not on every retry. Time is injected as seconds on any monotonic
//! clock.

use std::collections::HashMap;

use crate::backend::{
    BackendError, Bluetooth, Brightness, Launcher, MediaPlayer, OverlayKind, OverlayNotifier,
    SessionLock, Switch, Volume, Wifi,
};
use crate::lifecycle::{Lifecycle, Phase};
use crate::model::{
    Controls, Detail, MediaCommand, Module, Reading, Slider, TileState, MUTE_BELOW,
};

/// The settings page the "Edit Controls" button opens.
pub const SETTINGS_PANE: &str = "control-panel";

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
            // the panel opens on its modules, never on the detail it closed with
            self.detail = None;
            self.refresh(now);
        } else {
            self.stop_dragging();
        }
    }

    /// Start the fade-out; a drag in progress ends without a write.
    pub fn dismiss(&mut self, now: f64) {
        self.lifecycle.dismiss(now);
        self.stop_dragging();
    }

    /// The panel's surfaces are gone with their output: it is hidden at once, without the fade, and a drag in progress
    /// ends without a write, as with a dismissal. The next toggle opens it again and reads every backend.
    pub fn vanish(&mut self) {
        self.lifecycle.hide();
        self.stop_dragging();
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
        self.read_detail();
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

    /// Open the settings page of the panel ("Edit Controls"); the panel closes so the window it opens is in front.
    pub fn edit_controls(&mut self, now: f64) -> bool {
        self.open_settings(now, SETTINGS_PANE)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::*;
    use crate::model::{Device, Media, Network, Output, VolumeState, WifiStatus};

    fn network(ssid: &str, secured: bool, known: bool) -> Network {
        Network {
            ssid: ssid.into(),
            bars: Some(3),
            secured,
            known,
            connected: false,
        }
    }

    fn backends(overlay: RecordingOverlay) -> Backends {
        Backends {
            wifi: Box::new(MemoryWifi(
                WifiStatus {
                    enabled: true,
                    network: Some("Home".into()),
                },
                vec![network("Home", true, true), network("Cafe", false, false)],
            )),
            bluetooth: Box::new(MemoryBluetooth(
                false,
                vec![Device {
                    id: "AA:BB:CC:DD:EE:FF".into(),
                    name: "Headphones".into(),
                    connected: false,
                }],
            )),
            microphone: Box::new(MemorySwitch(true)),
            dock_hiding: Box::new(MemorySwitch(false)),
            keyboard: Box::new(Unavailable::new("there is no keyboard backlight")),
            focus_mode: Box::new(MemorySwitch(false)),
            brightness: Box::new(MemoryBrightness(0.5)),
            volume: Box::new(MemoryVolume(
                VolumeState {
                    level: 0.5,
                    muted: false,
                },
                vec![
                    Output {
                        id: 31,
                        name: "Speakers".into(),
                        current: true,
                    },
                    Output {
                        id: 47,
                        name: "Headphones".into(),
                        current: false,
                    },
                ],
            )),
            media: Box::new(MemoryMedia(Some(Media {
                title: "Song".into(),
                artist: Some("Band".into()),
                playing: false,
                can_previous: true,
                can_next: true,
                can_play_pause: true,
            }))),
            lock: Box::new(RecordingLock::default()),
            launcher: Box::new(RecordingLauncher::default()),
            overlay: Box::new(overlay),
        }
    }

    fn opened() -> (Controller, RecordingOverlay) {
        let overlay = RecordingOverlay::default();
        let mut controller = Controller::new(backends(overlay.clone()));
        controller.toggle(0.0);
        (controller, overlay)
    }

    #[test]
    fn opening_reads_every_backend() {
        let (controller, _) = opened();
        let controls = controller.controls();
        assert_eq!(controls.tile(Module::WiFi), TileState::On);
        assert_eq!(controls.tile(Module::Bluetooth), TileState::Off);
        assert_eq!(
            (
                controls.fraction(Slider::Display),
                controls.fraction(Slider::Sound)
            ),
            (Some(0.5), Some(0.5))
        );
        assert_eq!(controls.tile(Module::Microphone), TileState::On);
        assert_eq!(controls.tile(Module::DockHiding), TileState::Off);
        assert_eq!(
            controls.tile(Module::KeyboardBrightness),
            TileState::Disabled("there is no keyboard backlight".into())
        );
        assert_eq!(
            controls.media.ready().unwrap().as_ref().unwrap().title,
            "Song"
        );
        assert_eq!(controller.phase(0.0), Phase::Shown);
    }

    #[test]
    fn pressing_a_toggle_flips_it_in_the_backend() {
        let (mut controller, _) = opened();
        assert!(controller.press(Module::Bluetooth));
        assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::On);
        assert!(controller.press(Module::WiFi));
        assert_eq!(controller.controls().tile(Module::WiFi), TileState::Off);
        // the next reading comes from the backend and agrees
        controller.refresh(1.0);
        assert_eq!(
            (
                controller.controls().tile(Module::Bluetooth),
                controller.controls().tile(Module::WiFi)
            ),
            (TileState::On, TileState::Off)
        );
    }

    #[test]
    fn a_failing_backend_disables_its_tile_with_the_reason() {
        let mut backends = backends(RecordingOverlay::default());
        backends.bluetooth = Box::new(Unavailable::new("no adapter"));
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        assert_eq!(
            controller.controls().tile(Module::Bluetooth),
            TileState::Disabled("no adapter".into())
        );
        assert_eq!(controller.controls().tile(Module::WiFi), TileState::On);
        assert!(!controller.press(Module::Bluetooth));
    }

    #[test]
    fn a_failing_write_disables_the_tile_with_the_reason() {
        struct RefusesWrites;
        impl Bluetooth for RefusesWrites {
            fn read(&mut self) -> Result<bool, BackendError> {
                Ok(false)
            }
            fn set_powered(&mut self, _: bool) -> Result<(), BackendError> {
                Err("not authorized".into())
            }
            fn devices(&mut self) -> Result<Vec<Device>, BackendError> {
                Ok(Vec::new())
            }
            fn set_connected(&mut self, _: &str, _: bool) -> Result<(), BackendError> {
                Err("not authorized".into())
            }
        }
        let mut backends = backends(RecordingOverlay::default());
        backends.bluetooth = Box::new(RefusesWrites);
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        assert!(controller.press(Module::Bluetooth));
        assert_eq!(
            controller.controls().tile(Module::Bluetooth),
            TileState::Disabled("not authorized".into())
        );
    }

    #[test]
    fn a_failure_is_logged_once_until_its_reason_changes_or_it_recovers() {
        let mut log = FailureLog::default();
        let unknown: BackendError = "the name is not activatable".into();
        let refused: BackendError = "access denied".into();
        assert_eq!(
            log.record("wi-fi", Err(&unknown)).as_deref(),
            Some("cupertino-controlcenter: wi-fi: the name is not activatable")
        );
        assert_eq!(log.record("wi-fi", Err(&unknown)), None);
        // another backend has its own state
        assert_eq!(
            log.record("bluetooth", Err(&unknown)).as_deref(),
            Some("cupertino-controlcenter: bluetooth: the name is not activatable")
        );
        assert_eq!(
            log.record("wi-fi", Err(&refused)).as_deref(),
            Some("cupertino-controlcenter: wi-fi: access denied")
        );
        assert_eq!(
            log.record("wi-fi", Ok(())).as_deref(),
            Some("cupertino-controlcenter: wi-fi: available again")
        );
        assert_eq!(log.record("wi-fi", Ok(())), None);
        assert_eq!(
            log.record("wi-fi", Err(&refused)).as_deref(),
            Some("cupertino-controlcenter: wi-fi: access denied")
        );
    }

    #[test]
    fn the_switches_flip_in_their_backends_and_a_missing_one_ignores_presses() {
        let (mut controller, _) = opened();
        assert!(controller.press(Module::Microphone));
        assert!(controller.press(Module::DockHiding));
        assert!(controller.press(Module::FocusMode));
        assert!(!controller.press(Module::KeyboardBrightness));
        assert!(
            !controller.press(Module::NowPlaying),
            "the media tile has its own buttons"
        );
        controller.refresh(1.0);
        let controls = controller.controls();
        assert_eq!(
            [Module::Microphone, Module::DockHiding, Module::FocusMode].map(|m| controls.tile(m)),
            [TileState::Off, TileState::On, TileState::On]
        );
    }

    #[test]
    fn mute_silences_the_output_and_unmuting_a_level_at_the_end_brings_back_a_level() {
        let (mut controller, _) = opened();
        assert!(controller.press(Module::Mute));
        assert_eq!(
            controller.controls().volume,
            Reading::Ready(VolumeState {
                level: 0.5,
                muted: true
            })
        );
        controller.press(Module::Mute);
        controller.slide(1.0, Slider::Sound, 0.0);
        controller.release(1.1);
        controller.press(Module::Mute);
        assert_eq!(
            controller.controls().volume,
            Reading::Ready(VolumeState {
                level: UNMUTED_LEVEL,
                muted: false
            })
        );
    }

    #[test]
    fn lock_screen_asks_the_session_and_edit_controls_opens_the_settings_page() {
        let overlay = RecordingOverlay::default();
        let (lock, launcher) = (RecordingLock::default(), RecordingLauncher::default());
        let mut backends = backends(overlay);
        backends.lock = Box::new(lock.clone());
        backends.launcher = Box::new(launcher.clone());
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        assert!(controller.press(Module::LockScreen));
        assert_eq!(lock.calls().len(), 1);
        assert!(controller.edit_controls(1.0));
        assert_eq!(launcher.calls(), [SETTINGS_PANE]);
        assert_eq!(
            controller.phase(1.0),
            Phase::Hiding,
            "the panel steps aside"
        );
    }

    #[test]
    fn a_media_button_reaches_the_player_and_the_tile_reads_it_again() {
        let (mut controller, _) = opened();
        assert_eq!(
            controller.controls().tile(Module::NowPlaying),
            TileState::Off
        );
        assert!(controller.media(MediaCommand::PlayPause));
        assert_eq!(
            controller.controls().tile(Module::NowPlaying),
            TileState::On
        );
    }

    #[test]
    fn a_detail_reads_its_list_when_it_opens_and_with_every_reading() {
        let (mut controller, _) = opened();
        assert!(
            controller.controls().networks.ready().is_none(),
            "no list until asked"
        );
        controller.show_detail(Some(Detail::WiFi));
        let names = |c: &Controller| {
            c.controls()
                .networks
                .ready()
                .unwrap()
                .iter()
                .map(|n| (n.ssid.clone(), n.connected))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            names(&controller),
            [("Home".into(), false), ("Cafe".into(), false)]
        );
        assert!(controller.join("Cafe"));
        assert_eq!(names(&controller)[1], ("Cafe".into(), true));
        assert_eq!(
            controller.controls().subtitle(Module::WiFi),
            Some("Cafe"),
            "the tile names the network joined"
        );
        controller.show_detail(Some(Detail::Bluetooth));
        assert!(controller.connect("AA:BB:CC:DD:EE:FF"));
        assert!(controller.controls().devices.ready().unwrap()[0].connected);
        assert!(!controller.connect("unknown"));
        controller.show_detail(Some(Detail::Sound));
        assert!(controller.select_output(47));
        let current: Vec<_> = controller
            .controls()
            .outputs
            .ready()
            .unwrap()
            .iter()
            .map(|o| o.current)
            .collect();
        assert_eq!(current, [false, true]);
        controller.dismiss(2.0);
        controller.toggle(3.0);
        assert_eq!(
            controller.detail(),
            None,
            "the panel opens again on its modules"
        );
    }

    #[test]
    fn a_refused_join_shows_the_reason_in_the_list() {
        let (mut controller, _) = opened();
        controller.show_detail(Some(Detail::WiFi));
        assert!(controller.join("Nowhere"));
        assert_eq!(
            controller.controls().networks,
            Reading::Unavailable("no network \"Nowhere\" in the last scan".into())
        );
    }

    #[test]
    fn dragging_writes_at_most_every_16_7_milliseconds_and_the_latest_value_wins() {
        let (mut controller, _) = opened();
        assert!(controller.slide(1.0, Slider::Display, 0.2));
        controller.slide(1.005, Slider::Display, 0.4);
        controller.slide(1.010, Slider::Display, 0.6);
        // the model follows at once, the backend only got the first value
        assert_eq!(controller.controls().fraction(Slider::Display), Some(0.6));
        controller.refresh(1.011);
        assert_eq!(
            controller.controls().fraction(Slider::Display),
            Some(0.6),
            "no refresh during a drag"
        );
        assert!((controller.next_wakeup(1.010).unwrap() - 0.0067).abs() < 1e-9);
        controller.tick(1.02);
        controller.release(1.021);
        controller.refresh(1.5);
        assert_eq!(controller.controls().fraction(Slider::Display), Some(0.6));
    }

    #[test]
    fn releasing_a_slider_tells_the_overlay_the_level() {
        let (mut controller, overlay) = opened();
        controller.slide(1.0, Slider::Display, 0.75);
        controller.release(1.5);
        controller.slide(2.0, Slider::Sound, 0.25);
        controller.release(2.5);
        controller.slide(3.0, Slider::Sound, 0.0);
        controller.release(3.5);
        assert_eq!(
            overlay.shown(),
            [
                (OverlayKind::Brightness, 0.75),
                (OverlayKind::Volume, 0.25),
                (OverlayKind::Mute, 0.0)
            ]
        );
    }

    #[test]
    fn dragging_the_volume_to_zero_mutes_and_back_up_unmutes() {
        let (mut controller, _) = opened();
        controller.slide(1.0, Slider::Sound, 0.0);
        controller.release(1.1);
        controller.refresh(1.2);
        assert_eq!(
            controller.controls().volume,
            Reading::Ready(VolumeState {
                level: 0.0,
                muted: true
            })
        );
        controller.slide(2.0, Slider::Sound, 0.5);
        controller.release(2.1);
        controller.refresh(2.2);
        assert_eq!(
            controller.controls().volume,
            Reading::Ready(VolumeState {
                level: 0.5,
                muted: false
            })
        );
    }

    #[test]
    fn a_failing_slider_write_disables_the_slider() {
        let mut backends = backends(RecordingOverlay::default());
        struct RefusesWrites;
        impl Brightness for RefusesWrites {
            fn read(&mut self) -> Result<f64, BackendError> {
                Ok(0.5)
            }
            fn set(&mut self, _: f64) -> Result<(), BackendError> {
                Err("no backlight".into())
            }
        }
        backends.brightness = Box::new(RefusesWrites);
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        controller.slide(1.0, Slider::Display, 0.8);
        assert_eq!(
            controller.controls().tile(Module::Display),
            TileState::Disabled("no backlight".into())
        );
    }

    #[test]
    fn dismissing_stops_a_drag_without_a_write() {
        let (mut controller, overlay) = opened();
        controller.slide(1.0, Slider::Display, 0.2);
        controller.slide(1.001, Slider::Display, 0.9);
        controller.dismiss(1.002);
        controller.release(1.003);
        assert!(overlay.shown().is_empty());
        controller.toggle(2.0);
        assert_eq!(controller.controls().fraction(Slider::Display), Some(0.2));
    }

    #[test]
    fn vanishing_hides_at_once_drops_the_drag_and_the_next_toggle_reads_again() {
        let (mut controller, overlay) = opened();
        controller.slide(1.0, Slider::Display, 0.2);
        controller.slide(1.001, Slider::Display, 0.9);
        controller.vanish();
        assert_eq!(controller.phase(1.002), Phase::Hidden);
        assert_eq!(
            controller.next_wakeup(1.002),
            None,
            "no fade, no refresh, no write"
        );
        controller.release(1.003);
        assert!(overlay.shown().is_empty());
        controller.toggle(2.0);
        assert_eq!(controller.phase(2.0), Phase::Shown);
        assert_eq!(controller.controls().fraction(Slider::Display), Some(0.2));
        assert!(
            controller.press(Module::Bluetooth),
            "the reopened panel acts on the backends"
        );
    }

    #[test]
    fn an_open_panel_reads_again_every_second() {
        let (mut controller, _) = opened();
        assert_eq!(controller.next_wakeup(0.4), Some(0.6));
        controller.tick(0.5);
        assert_eq!(controller.next_wakeup(0.5), Some(0.5));
        controller.dismiss(0.6);
        assert!((controller.next_wakeup(0.7).unwrap() - 0.08).abs() < 1e-9);
    }
}
