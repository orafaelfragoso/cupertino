//! Carries out what the user does on the panel on the backends and keeps the model in step.
//!
//! A reading or write that fails disables its tile with the reason and logs it on stderr; nothing falls back to a
//! default. The panel reads again every [`REFRESH_INTERVAL`], so a failure is logged when it starts or its reason
//! changes and once more when the backend recovers, not on every retry. Time is injected as seconds on any monotonic
//! clock.

use std::collections::HashMap;

use crate::backend::{BackendError, BatteryReader, Bluetooth, Brightness, FocusMode, OverlayKind, OverlayNotifier, Volume, Wifi};
use crate::lifecycle::{Lifecycle, Phase};
use crate::model::{Controls, Module, Reading, Slider, TileState, MUTE_BELOW};

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
    pub focus_mode: Box<dyn FocusMode>,
    pub brightness: Box<dyn Brightness>,
    pub volume: Box<dyn Volume>,
    pub battery: Box<dyn BatteryReader>,
    pub overlay: Box<dyn OverlayNotifier>,
}

/// A slider value waiting for the write throttle.
#[derive(Clone, Copy, Debug)]
struct PendingWrite {
    slider: Slider,
    fraction: f64,
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
            Ok(()) => self.reasons.remove(what).map(|_| format!("cupertino-controlcenter: {what}: available again")),
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
        Self { lifecycle: Lifecycle::new(), controls: Controls::unread(), backends, failures: FailureLog::default(), dragging: None, pending: None, last_write: None, last_refresh: None }
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

    /// Open the panel and read every backend, or dismiss it when it is shown (the `Toggle` call).
    pub fn toggle(&mut self, now: f64) {
        let was_shown = self.lifecycle.phase(now) == Phase::Shown;
        self.lifecycle.toggle(now);
        if !was_shown {
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

    fn stop_dragging(&mut self) {
        self.dragging = None;
        self.pending = None;
    }

    /// Read every backend. A slider being dragged keeps its value.
    pub fn refresh(&mut self, now: f64) {
        self.last_refresh = Some(now);
        let (backends, failures) = (&mut self.backends, &mut self.failures);
        self.controls.wifi = failures.reading("wi-fi", backends.wifi.read());
        self.controls.bluetooth = failures.reading("bluetooth", backends.bluetooth.read());
        self.controls.focus_mode = failures.reading("focus mode", backends.focus_mode.read());
        if self.dragging.is_none() {
            self.controls.brightness = failures.reading("display brightness", backends.brightness.read());
            self.controls.volume = failures.reading("volume", backends.volume.read());
        }
        self.controls.battery = match backends.battery.read() {
            Ok(battery) => {
                failures.log("battery", Ok(()));
                battery
            }
            Err(error) => {
                failures.log("battery", Err(&error));
                None
            }
        };
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
                let Reading::Ready(status) = &self.controls.wifi else { return false };
                let enabled = !status.enabled;
                match backends.wifi.set_enabled(enabled) {
                    // the radio follows the switch later; the next reading shows the network again
                    Ok(()) => {
                        if let Reading::Ready(status) = &mut self.controls.wifi {
                            status.enabled = enabled;
                            status.network = status.network.take().filter(|_| enabled);
                        }
                    }
                    Err(error) => self.controls.wifi = self.failures.reading::<crate::model::WifiStatus>("wi-fi", Err(error)),
                }
            }
            Module::Bluetooth => {
                let Reading::Ready(on) = self.controls.bluetooth else { return false };
                self.controls.bluetooth = match backends.bluetooth.set_powered(!on) {
                    Ok(()) => Reading::Ready(!on),
                    Err(error) => self.failures.reading("bluetooth", Err(error)),
                };
            }
            Module::FocusMode => {
                let Reading::Ready(on) = self.controls.focus_mode else { return false };
                self.controls.focus_mode = match backends.focus_mode.set(!on) {
                    Ok(()) => Reading::Ready(!on),
                    Err(error) => self.failures.reading("focus mode", Err(error)),
                };
            }
            Module::Display | Module::Sound | Module::FileSharing | Module::Stage | Module::ScreenMirroring | Module::NowPlaying => return false,
        }
        true
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
        if self.pending.is_some() && self.last_write.is_none_or(|last| now - last >= WRITE_INTERVAL) {
            self.write_pending(now);
        }
    }

    /// The pointer let go of the slider: write the last value and tell the overlay what the level is.
    pub fn release(&mut self, now: f64) {
        self.write_pending(now);
        let Some(slider) = self.dragging.take() else { return };
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
        let Some(PendingWrite { slider, fraction }) = self.pending.take() else { return };
        self.last_write = Some(now);
        match slider {
            Slider::Display => {
                if let Err(error) = self.backends.brightness.set(fraction) {
                    self.controls.brightness = self.failures.reading::<f64>("display brightness", Err(error));
                }
            }
            Slider::Sound => {
                self.controls.volume = self.failures.reading("volume", self.backends.volume.set(fraction, fraction < MUTE_BELOW));
            }
        }
    }

    /// Read the backends again when the panel has been open for [`REFRESH_INTERVAL`] since the last reading.
    pub fn tick(&mut self, now: f64) {
        self.flush(now);
        if self.lifecycle.phase(now) == Phase::Shown && self.dragging.is_none() && self.last_refresh.is_none_or(|last| now - last >= REFRESH_INTERVAL) {
            self.refresh(now);
        }
    }

    /// Seconds from `now` until [`tick`](Self::tick) or the end of the fade-out has something to do.
    pub fn next_wakeup(&self, now: f64) -> Option<f64> {
        let write = self.pending.map(|_| self.last_write.map_or(0.0, |last| (last + WRITE_INTERVAL - now).max(0.0)));
        let refresh = (self.lifecycle.phase(now) == Phase::Shown && self.dragging.is_none()).then(|| self.last_refresh.map_or(0.0, |last| (last + REFRESH_INTERVAL - now).max(0.0)));
        [write, refresh, self.lifecycle.until_hidden(now)].into_iter().flatten().min_by(f64::total_cmp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::*;
    use crate::model::{Battery, VolumeState, WifiStatus};

    fn backends(overlay: RecordingOverlay) -> Backends {
        Backends {
            wifi: Box::new(MemoryWifi(WifiStatus { enabled: true, network: Some("Home".into()) })),
            bluetooth: Box::new(MemoryBluetooth(false)),
            focus_mode: Box::new(MemoryFocusMode(false)),
            brightness: Box::new(MemoryBrightness(0.5)),
            volume: Box::new(MemoryVolume(VolumeState { level: 0.5, muted: false })),
            battery: Box::new(MemoryBattery(Some(Battery { percent: 80, charging: false }))),
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
        assert_eq!((controls.fraction(Slider::Display), controls.fraction(Slider::Sound)), (Some(0.5), Some(0.5)));
        assert_eq!(controls.battery, Some(Battery { percent: 80, charging: false }));
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
        assert_eq!((controller.controls().tile(Module::Bluetooth), controller.controls().tile(Module::WiFi)), (TileState::On, TileState::Off));
    }

    #[test]
    fn a_failing_backend_disables_its_tile_with_the_reason() {
        let mut backends = backends(RecordingOverlay::default());
        backends.bluetooth = Box::new(Unavailable::new("no adapter"));
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::Disabled("no adapter".into()));
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
        }
        let mut backends = backends(RecordingOverlay::default());
        backends.bluetooth = Box::new(RefusesWrites);
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        assert!(controller.press(Module::Bluetooth));
        assert_eq!(controller.controls().tile(Module::Bluetooth), TileState::Disabled("not authorized".into()));
    }

    #[test]
    fn a_failure_is_logged_once_until_its_reason_changes_or_it_recovers() {
        let mut log = FailureLog::default();
        let unknown: BackendError = "the name is not activatable".into();
        let refused: BackendError = "access denied".into();
        assert_eq!(log.record("wi-fi", Err(&unknown)).as_deref(), Some("cupertino-controlcenter: wi-fi: the name is not activatable"));
        assert_eq!(log.record("wi-fi", Err(&unknown)), None);
        // another backend has its own state
        assert_eq!(log.record("bluetooth", Err(&unknown)).as_deref(), Some("cupertino-controlcenter: bluetooth: the name is not activatable"));
        assert_eq!(log.record("wi-fi", Err(&refused)).as_deref(), Some("cupertino-controlcenter: wi-fi: access denied"));
        assert_eq!(log.record("wi-fi", Ok(())).as_deref(), Some("cupertino-controlcenter: wi-fi: available again"));
        assert_eq!(log.record("wi-fi", Ok(())), None);
        assert_eq!(log.record("wi-fi", Err(&refused)).as_deref(), Some("cupertino-controlcenter: wi-fi: access denied"));
    }

    #[test]
    fn a_missing_battery_is_not_a_failure() {
        let mut backends = backends(RecordingOverlay::default());
        backends.battery = Box::new(MemoryBattery(None));
        let mut controller = Controller::new(backends);
        controller.toggle(0.0);
        assert_eq!(controller.controls().battery, None);
    }

    #[test]
    fn modules_without_a_backend_ignore_presses() {
        let (mut controller, _) = opened();
        assert!(!controller.press(Module::Stage));
        assert!(!controller.press(Module::NowPlaying));
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
        assert_eq!(controller.controls().fraction(Slider::Display), Some(0.6), "no refresh during a drag");
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
        assert_eq!(overlay.shown(), [(OverlayKind::Brightness, 0.75), (OverlayKind::Volume, 0.25), (OverlayKind::Mute, 0.0)]);
    }

    #[test]
    fn dragging_the_volume_to_zero_mutes_and_back_up_unmutes() {
        let (mut controller, _) = opened();
        controller.slide(1.0, Slider::Sound, 0.0);
        controller.release(1.1);
        controller.refresh(1.2);
        assert_eq!(controller.controls().volume, Reading::Ready(VolumeState { level: 0.0, muted: true }));
        controller.slide(2.0, Slider::Sound, 0.5);
        controller.release(2.1);
        controller.refresh(2.2);
        assert_eq!(controller.controls().volume, Reading::Ready(VolumeState { level: 0.5, muted: false }));
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
        assert_eq!(controller.controls().tile(Module::Display), TileState::Disabled("no backlight".into()));
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
    fn an_open_panel_reads_again_every_second() {
        let (mut controller, _) = opened();
        assert_eq!(controller.next_wakeup(0.4), Some(0.6));
        controller.tick(0.5);
        assert_eq!(controller.next_wakeup(0.5), Some(0.5));
        controller.dismiss(0.6);
        assert!((controller.next_wakeup(0.7).unwrap() - 0.138).abs() < 1e-9);
    }
}
