//! What the panel reads and changes, behind traits so the controller and the tests run on in-memory stand-ins and the
//! process can run without touching the machine (`--fake-backends`).
//!
//! The real implementations over the system and session buses are in `system` (feature `dbus`); here are the ones
//! that need no bus: the default PipeWire sink and source through `cupertino-pipewire` (else the kernel's sound control
//! interface through `cupertino-alsa`), the dock's hiding switch in its settings file, and the settings application.

use std::error::Error;
use std::fmt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use cupertino_alsa::{Master, OpenMode};
use cupertino_defaults::control_panel::ControlPanelSettings;
use cupertino_defaults::dock::DockSettings;
use cupertino_pipewire::{Target, VolumeWorker, Wpctl};

use crate::model::{Device, Media, MediaCommand, Network, Output, VolumeState, WifiStatus};

/// Why an effect failed; it disables the tile and goes to the log.
pub type BackendError = Box<dyn Error + Send + Sync>;

/// The Wi-Fi radio.
pub trait Wifi: Send {
    fn read(&mut self) -> Result<WifiStatus, BackendError>;
    /// Switch the radio on or off. The radio may take a moment to follow.
    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError>;
    /// The networks of the last scan, strongest first; asks for a new scan as well.
    fn networks(&mut self) -> Result<Vec<Network>, BackendError>;
    /// Join a network of the last scan with the credentials the service holds.
    fn join(&mut self, ssid: &str) -> Result<(), BackendError>;
}

/// The Bluetooth adapter.
pub trait Bluetooth: Send {
    /// Whether the adapter is powered.
    fn read(&mut self) -> Result<bool, BackendError>;
    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError>;
    /// The paired devices.
    fn devices(&mut self) -> Result<Vec<Device>, BackendError>;
    fn set_connected(&mut self, id: &str, connected: bool) -> Result<(), BackendError>;
}

/// A setting that is on or off: focus mode (do not disturb), the microphone, the dock's hiding, the keyboard
/// backlight.
pub trait Switch: Send {
    fn read(&mut self) -> Result<bool, BackendError>;
    fn set(&mut self, on: bool) -> Result<(), BackendError>;
}

/// Display brightness.
pub trait Brightness: Send {
    /// The level, 0 to 1.
    fn read(&mut self) -> Result<f64, BackendError>;
    fn set(&mut self, level: f64) -> Result<(), BackendError>;
}

/// The output volume.
pub trait Volume: Send {
    fn read(&mut self) -> Result<VolumeState, BackendError>;
    /// Set the level and the mute state together; returns the state the output takes (a background write may still
    /// be running).
    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError>;
    /// The outputs applications can play to.
    fn outputs(&mut self) -> Result<Vec<Output>, BackendError>;
    /// Play to output `id` from now on.
    fn select_output(&mut self, id: u32) -> Result<(), BackendError>;
}

/// The media player that is playing, or was last.
pub trait MediaPlayer: Send {
    /// `None` when no player has anything.
    fn read(&mut self) -> Result<Option<Media>, BackendError>;
    fn command(&mut self, command: MediaCommand) -> Result<(), BackendError>;
}

/// Locking the session.
pub trait SessionLock: Send {
    fn lock(&mut self) -> Result<(), BackendError>;
}

/// Opening the settings application.
pub trait Launcher: Send {
    /// Open the settings application on the page with identifier `pane`.
    fn open_settings(&mut self, pane: &str) -> Result<(), BackendError>;
}

/// Where the person's choice of controls is kept.
pub trait LayoutStore: Send {
    /// The names of the controls shown, in order, as stored; `None` when nothing is stored.
    fn load(&mut self) -> Result<Option<Vec<String>>, BackendError>;
    /// Keep `controls` as the names of the controls shown, in order.
    fn store(&mut self, controls: &[&str]) -> Result<(), BackendError>;
}

/// The overlay the panel tells about a level it changed itself.
pub trait OverlayNotifier: Send {
    fn show(&mut self, kind: OverlayKind, level: f64) -> Result<(), BackendError>;
}

/// What the overlay shows (the `Show` kinds of its bus interface).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayKind {
    Volume,
    Mute,
    Brightness,
}

impl OverlayKind {
    /// The name on the bus.
    pub fn name(self) -> &'static str {
        match self {
            OverlayKind::Volume => "volume",
            OverlayKind::Mute => "mute",
            OverlayKind::Brightness => "brightness",
        }
    }
}

/// A backend that always fails with one reason: a service that does not exist, or a stand-in for a broken one.
#[derive(Clone, Debug)]
pub struct Unavailable {
    reason: String,
}

impl Unavailable {
    pub fn new(reason: &str) -> Self {
        Self {
            reason: reason.to_string(),
        }
    }

    fn fail<T>(&self) -> Result<T, BackendError> {
        Err(self.reason.clone().into())
    }
}

impl Wifi for Unavailable {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        self.fail()
    }
    fn set_enabled(&mut self, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
    fn networks(&mut self) -> Result<Vec<Network>, BackendError> {
        self.fail()
    }
    fn join(&mut self, _: &str) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Bluetooth for Unavailable {
    fn read(&mut self) -> Result<bool, BackendError> {
        self.fail()
    }
    fn set_powered(&mut self, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
    fn devices(&mut self) -> Result<Vec<Device>, BackendError> {
        self.fail()
    }
    fn set_connected(&mut self, _: &str, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Switch for Unavailable {
    fn read(&mut self) -> Result<bool, BackendError> {
        self.fail()
    }
    fn set(&mut self, _: bool) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Brightness for Unavailable {
    fn read(&mut self) -> Result<f64, BackendError> {
        self.fail()
    }
    fn set(&mut self, _: f64) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Volume for Unavailable {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        self.fail()
    }
    fn set(&mut self, _: f64, _: bool) -> Result<VolumeState, BackendError> {
        self.fail()
    }
    fn outputs(&mut self) -> Result<Vec<Output>, BackendError> {
        self.fail()
    }
    fn select_output(&mut self, _: u32) -> Result<(), BackendError> {
        self.fail()
    }
}

impl MediaPlayer for Unavailable {
    fn read(&mut self) -> Result<Option<Media>, BackendError> {
        self.fail()
    }
    fn command(&mut self, _: MediaCommand) -> Result<(), BackendError> {
        self.fail()
    }
}

impl SessionLock for Unavailable {
    fn lock(&mut self) -> Result<(), BackendError> {
        self.fail()
    }
}

impl Launcher for Unavailable {
    fn open_settings(&mut self, _: &str) -> Result<(), BackendError> {
        self.fail()
    }
}

impl OverlayNotifier for Unavailable {
    fn show(&mut self, _: OverlayKind, _: f64) -> Result<(), BackendError> {
        self.fail()
    }
}

/// An in-memory Wi-Fi radio and the networks of its last scan; joining one connects it.
#[derive(Clone, Debug)]
pub struct MemoryWifi(pub WifiStatus, pub Vec<Network>);

impl Wifi for MemoryWifi {
    fn read(&mut self) -> Result<WifiStatus, BackendError> {
        Ok(self.0.clone())
    }
    fn set_enabled(&mut self, enabled: bool) -> Result<(), BackendError> {
        self.0.enabled = enabled;
        if !enabled {
            self.0.network = None;
            self.1.iter_mut().for_each(|n| n.connected = false);
        }
        Ok(())
    }
    fn networks(&mut self) -> Result<Vec<Network>, BackendError> {
        Ok(if self.0.enabled {
            self.1.clone()
        } else {
            Vec::new()
        })
    }
    fn join(&mut self, ssid: &str) -> Result<(), BackendError> {
        if !self.1.iter().any(|n| n.ssid == ssid) {
            return Err(format!("no network {ssid:?} in the last scan").into());
        }
        for network in &mut self.1 {
            network.connected = network.ssid == ssid;
        }
        self.0.network = Some(ssid.to_owned());
        Ok(())
    }
}

/// An in-memory Bluetooth adapter and its paired devices.
#[derive(Clone, Debug)]
pub struct MemoryBluetooth(pub bool, pub Vec<Device>);

impl Bluetooth for MemoryBluetooth {
    fn read(&mut self) -> Result<bool, BackendError> {
        Ok(self.0)
    }
    fn set_powered(&mut self, powered: bool) -> Result<(), BackendError> {
        self.0 = powered;
        Ok(())
    }
    fn devices(&mut self) -> Result<Vec<Device>, BackendError> {
        Ok(self.1.clone())
    }
    fn set_connected(&mut self, id: &str, connected: bool) -> Result<(), BackendError> {
        let device = self
            .1
            .iter_mut()
            .find(|d| d.id == id)
            .ok_or_else(|| format!("no device {id}"))?;
        device.connected = connected;
        Ok(())
    }
}

/// An in-memory switch.
#[derive(Clone, Copy, Debug)]
pub struct MemorySwitch(pub bool);

impl Switch for MemorySwitch {
    fn read(&mut self) -> Result<bool, BackendError> {
        Ok(self.0)
    }
    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        self.0 = on;
        Ok(())
    }
}

/// An in-memory backlight.
#[derive(Clone, Copy, Debug)]
pub struct MemoryBrightness(pub f64);

impl Brightness for MemoryBrightness {
    fn read(&mut self) -> Result<f64, BackendError> {
        Ok(self.0)
    }
    fn set(&mut self, level: f64) -> Result<(), BackendError> {
        self.0 = level.clamp(0.0, 1.0);
        Ok(())
    }
}

/// An in-memory output volume and its outputs.
#[derive(Clone, Debug)]
pub struct MemoryVolume(pub VolumeState, pub Vec<Output>);

impl Volume for MemoryVolume {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        Ok(self.0)
    }
    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError> {
        self.0 = VolumeState {
            level: level.clamp(0.0, 1.0),
            muted,
        };
        Ok(self.0)
    }
    fn outputs(&mut self) -> Result<Vec<Output>, BackendError> {
        Ok(self.1.clone())
    }
    fn select_output(&mut self, id: u32) -> Result<(), BackendError> {
        if !self.1.iter().any(|o| o.id == id) {
            return Err(format!("no output {id}").into());
        }
        for output in &mut self.1 {
            output.current = output.id == id;
        }
        Ok(())
    }
}

/// An in-memory media player: `None` plays nothing; play/pause flips what it has.
#[derive(Clone, Debug)]
pub struct MemoryMedia(pub Option<Media>);

impl MediaPlayer for MemoryMedia {
    fn read(&mut self) -> Result<Option<Media>, BackendError> {
        Ok(self.0.clone())
    }
    fn command(&mut self, command: MediaCommand) -> Result<(), BackendError> {
        let media = self.0.as_mut().ok_or("nothing is playing")?;
        if command == MediaCommand::PlayPause {
            media.playing = !media.playing;
        }
        Ok(())
    }
}

/// What a stand-in for the session, the settings application and the overlay was asked, shared with its clones.
#[derive(Clone, Debug)]
pub struct Recording<T> {
    calls: Arc<Mutex<Vec<T>>>,
}

impl<T> Default for Recording<T> {
    fn default() -> Self {
        Self {
            calls: Arc::default(),
        }
    }
}

impl<T: Clone> Recording<T> {
    pub fn calls(&self) -> Vec<T> {
        self.calls
            .lock()
            .expect("the lock is never poisoned")
            .clone()
    }

    fn push(&self, call: T) {
        self.calls
            .lock()
            .expect("the lock is never poisoned")
            .push(call);
    }
}

/// Records each lock it is asked for.
pub type RecordingLock = Recording<()>;

impl SessionLock for RecordingLock {
    fn lock(&mut self) -> Result<(), BackendError> {
        self.push(());
        Ok(())
    }
}

/// Records each settings page it is asked to open.
pub type RecordingLauncher = Recording<String>;

impl Launcher for RecordingLauncher {
    fn open_settings(&mut self, pane: &str) -> Result<(), BackendError> {
        self.push(pane.to_owned());
        Ok(())
    }
}

/// A choice of controls kept in memory, shared with its clones; nothing stored at first.
#[derive(Clone, Debug, Default)]
pub struct MemoryLayout {
    stored: Arc<Mutex<Option<Vec<String>>>>,
}

impl MemoryLayout {
    /// What is stored now.
    pub fn stored(&self) -> Option<Vec<String>> {
        self.stored
            .lock()
            .expect("the lock is never poisoned")
            .clone()
    }

    /// Store `controls`, as another process writing the layout would.
    pub fn set(&self, controls: &[&str]) {
        *self.stored.lock().expect("the lock is never poisoned") =
            Some(controls.iter().map(|name| name.to_string()).collect());
    }
}

impl LayoutStore for MemoryLayout {
    fn load(&mut self) -> Result<Option<Vec<String>>, BackendError> {
        Ok(self.stored())
    }

    fn store(&mut self, controls: &[&str]) -> Result<(), BackendError> {
        self.set(controls);
        Ok(())
    }
}

/// Records what the overlay was asked to show.
pub type RecordingOverlay = Recording<(OverlayKind, f64)>;

impl RecordingOverlay {
    /// Everything shown so far, oldest first.
    pub fn shown(&self) -> Vec<(OverlayKind, f64)> {
        self.calls()
    }
}

impl OverlayNotifier for RecordingOverlay {
    fn show(&mut self, kind: OverlayKind, level: f64) -> Result<(), BackendError> {
        self.push((kind, level));
        Ok(())
    }
}

/// The default PipeWire source: on while it records, off while muted. Read and written in the background, as the
/// output volume is.
pub struct PipewireMicrophone {
    worker: VolumeWorker,
    /// The level last read, written back with a change of the mute state.
    level: f64,
}

impl PipewireMicrophone {
    pub fn new(wpctl: Wpctl) -> Self {
        let worker = VolumeWorker::new(wpctl, Target::DefaultSource);
        worker.request_read(false);
        Self { worker, level: 1.0 }
    }
}

impl Switch for PipewireMicrophone {
    fn read(&mut self) -> Result<bool, BackendError> {
        self.worker.request_read(false);
        let reading = self
            .worker
            .latest()
            .ok_or("the sound server has not answered yet")?;
        let volume = reading.volume?;
        self.level = volume.level;
        Ok(!volume.muted)
    }

    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        Ok(self.worker.set(cupertino_pipewire::Volume {
            level: self.level,
            muted: !on,
        })?)
    }
}

/// The dock's hiding, the `autohide` key of its settings file, which the dock follows as it changes.
#[derive(Clone, Debug, Default)]
pub struct DockHiding {
    /// The file; `None` takes the dock's own (`DockSettings::path`).
    pub path: Option<PathBuf>,
}

impl DockHiding {
    fn path(&self) -> Result<PathBuf, BackendError> {
        self.path
            .clone()
            .or_else(DockSettings::path)
            .ok_or_else(|| "there is no settings directory".into())
    }
}

impl Switch for DockHiding {
    fn read(&mut self) -> Result<bool, BackendError> {
        let (settings, _) = DockSettings::load(&self.path()?, None)?;
        Ok(settings.autohide.unwrap_or(false))
    }

    fn set(&mut self, on: bool) -> Result<(), BackendError> {
        let path = self.path()?;
        let (mut settings, _) = DockSettings::load(&path, None)?;
        settings.autohide = Some(on);
        settings.store(&path)?;
        Ok(())
    }
}

/// The choice of controls in the panel's settings file, the `controls` key, read each time it is asked for.
#[derive(Clone, Debug, Default)]
pub struct StoredLayout {
    /// The file; `None` takes the panel's own (`ControlPanelSettings::path`).
    pub path: Option<PathBuf>,
}

impl StoredLayout {
    fn path(&self) -> Result<PathBuf, BackendError> {
        self.path
            .clone()
            .or_else(ControlPanelSettings::path)
            .ok_or_else(|| "there is no settings directory".into())
    }
}

impl LayoutStore for StoredLayout {
    fn load(&mut self) -> Result<Option<Vec<String>>, BackendError> {
        let (settings, _) = ControlPanelSettings::load(&self.path()?, None)?;
        Ok(settings.controls)
    }

    fn store(&mut self, controls: &[&str]) -> Result<(), BackendError> {
        let settings = ControlPanelSettings {
            controls: Some(controls.iter().map(|name| name.to_string()).collect()),
        };
        settings.store(&self.path()?)?;
        Ok(())
    }
}

/// The settings application, started as its own process (`cupertino-settings --pane <page>`).
#[derive(Clone, Debug)]
pub struct SettingsLauncher {
    pub program: PathBuf,
}

impl Default for SettingsLauncher {
    fn default() -> Self {
        Self {
            program: PathBuf::from("cupertino-settings"),
        }
    }
}

impl Launcher for SettingsLauncher {
    fn open_settings(&mut self, pane: &str) -> Result<(), BackendError> {
        let mut child = Command::new(&self.program)
            .args(["--pane", pane])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", self.program.display()))?;
        // reap it when it exits, so a long-running panel leaves no zombie behind
        std::thread::spawn(move || child.wait());
        Ok(())
    }
}

/// The playback master volume through the kernel's sound control interface. The device is opened on the first use and
/// again after a failure, so a card that appears later (or went away) is picked up.
pub struct AlsaVolume {
    dir: PathBuf,
    card: Option<u32>,
    master: Option<Master>,
}

impl fmt::Debug for AlsaVolume {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AlsaVolume")
            .field("dir", &self.dir)
            .field("card", &self.card)
            .field("open", &self.master.is_some())
            .finish()
    }
}

impl AlsaVolume {
    /// The master of card `card`, or the best master of all cards (see `cupertino_alsa::master`), below `dir`.
    pub fn new(dir: PathBuf, card: Option<u32>) -> Self {
        Self {
            dir,
            card,
            master: None,
        }
    }

    fn master(&mut self) -> Result<&Master, cupertino_alsa::Error> {
        if self.master.is_none() {
            self.master = Some(match self.card {
                Some(card) => Master::open_card(&self.dir, card, OpenMode::ReadWrite)?,
                None => Master::open_default(&self.dir, OpenMode::ReadWrite)?,
            });
        }
        Ok(self.master.as_ref().expect("opened above"))
    }

    /// Run `action` on the master; a failure drops the open device so the next call opens it again.
    fn with_master<T>(
        &mut self,
        action: impl FnOnce(&Master) -> Result<T, cupertino_alsa::Error>,
    ) -> Result<T, BackendError> {
        let result = self.master().and_then(action);
        if result.is_err() {
            self.master = None;
        }
        Ok(result?)
    }
}

/// The level and mute state of `master`; a card without a mute switch is muted exactly at level 0, as in the overlay.
fn state(master: &Master) -> Result<VolumeState, cupertino_alsa::Error> {
    let level = master.level()?;
    Ok(VolumeState {
        level,
        muted: master.muted()?.unwrap_or(level <= 0.0),
    })
}

impl Volume for AlsaVolume {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        self.with_master(state)
    }

    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError> {
        self.with_master(|master| {
            master.set_level(level)?;
            // a card without a mute switch is silenced by its level alone
            if master.muted()?.is_some_and(|current| current != muted) {
                master.set_muted(muted)?;
            }
            state(master)
        })
    }

    fn outputs(&mut self) -> Result<Vec<Output>, BackendError> {
        Err("without a sound server the card's master is the only output".into())
    }

    fn select_output(&mut self, _: u32) -> Result<(), BackendError> {
        Err("without a sound server the card's master is the only output".into())
    }
}

/// The default PipeWire sink through `wpctl`, so the slider shows the level `wpctl get-volume` reports. A read that
/// no PipeWire daemon answers goes to the ALSA master instead, and so do the changes after it until the next read.
/// The sink is read and written in the background ([`VolumeWorker`]): a drag or the panel's periodic reading never
/// waits for `wpctl`. A read gives the last reading done (asking for the next one), or the level asked since; only the
/// first, before any reading is done, waits for `wpctl`.
pub struct SessionVolume {
    wpctl: Wpctl,
    worker: VolumeWorker,
    alsa: AlsaVolume,
    pipewire: bool,
}

impl SessionVolume {
    pub fn new(wpctl: Wpctl, alsa: AlsaVolume) -> Self {
        let worker = VolumeWorker::new(wpctl.clone(), Target::DefaultSink);
        worker.request_read(false);
        Self {
            wpctl,
            worker,
            alsa,
            pipewire: true,
        }
    }

    fn sink(&self) -> Option<cupertino_pipewire::Volume> {
        self.worker.request_read(false);
        match self.worker.latest() {
            Some(reading) => reading.volume.ok(),
            None => self.wpctl.volume(Target::DefaultSink).ok(),
        }
    }
}

impl Volume for SessionVolume {
    fn read(&mut self) -> Result<VolumeState, BackendError> {
        match self.sink() {
            Some(sink) => {
                self.pipewire = true;
                Ok(VolumeState {
                    level: sink.level,
                    muted: sink.muted,
                })
            }
            None => {
                self.pipewire = false;
                self.alsa.read()
            }
        }
    }

    fn set(&mut self, level: f64, muted: bool) -> Result<VolumeState, BackendError> {
        if !self.pipewire {
            return self.alsa.set(level, muted);
        }
        let level = level.clamp(0.0, 1.0);
        self.worker
            .set(cupertino_pipewire::Volume { level, muted })?;
        Ok(VolumeState { level, muted })
    }

    /// The sinks of the last reading, asking for the next one with them.
    fn outputs(&mut self) -> Result<Vec<Output>, BackendError> {
        if !self.pipewire {
            return self.alsa.outputs();
        }
        self.worker.request_read(true);
        let sinks = match self.worker.latest().and_then(|r| r.sinks) {
            Some(sinks) => sinks?,
            None => self.wpctl.sinks()?,
        };
        Ok(sinks
            .into_iter()
            .map(|sink| Output {
                id: sink.id,
                name: sink.name,
                current: sink.default,
            })
            .collect())
    }

    fn select_output(&mut self, id: u32) -> Result<(), BackendError> {
        if !self.pipewire {
            return self.alsa.select_output(id);
        }
        self.wpctl.set_default_sink(id)?;
        self.worker.request_read(true);
        Ok(())
    }
}
