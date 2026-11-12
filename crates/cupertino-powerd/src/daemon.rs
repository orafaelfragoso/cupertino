//! The daemon: owns the bus names, keeps the latest [`Snapshot`], and turns the difference
//! between two snapshots into D-Bus signals.
//!
//! The host calls [`Daemon::refresh`] when the kernel reports a change (or on a timer) and
//! [`Daemon::set_lid`] when the lid switch moves. Both are synchronous; the zbus executor thread
//! serves calls in between.

use std::borrow::Cow;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zbus::blocking::connection::Builder;
use zbus::fdo::{RequestNameFlags, RequestNameReply};
use zbus::names::InterfaceName;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{ObjectPath, Value};
use zbus::Connection;

use crate::auth::Authorizer;
use crate::error::{start_failed, Error, Result};
use crate::history::{History, Sample};
use crate::interfaces::{
    device_path, device_properties, keyboard_path, manager_properties, DeviceObject,
    KeyboardBacklightObject, Manager, PowerObject, Property, Target, DEVICE_INTERFACE,
    DISPLAY_DEVICE_PATH, KEYBOARD_BACKLIGHT_PATH, MANAGER_INTERFACE, POWER_NAME, POWER_PATH,
    UPOWER_NAME, UPOWER_PATH,
};
use crate::sysfs::{self, Lid, Snapshot};
use crate::view::DeviceView;
use crate::warning::{Level, Reading, Warnings};

/// Which bus to serve on.
pub enum Bus {
    /// The system bus, where the daemon runs as root.
    System,
    /// The session bus, to run unprivileged against a fake sysfs root.
    Session,
    /// A bus at an explicit address (tests).
    Address(String),
}

/// Seconds since the epoch.
fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64)
}

/// Everything the daemon needs to start.
pub struct Config {
    pub bus: Bus,
    /// `/sys`, or a tree that looks like it.
    pub sysfs_root: PathBuf,
    /// The lid as read from the switch device before starting.
    pub lid: Lid,
    /// Decides who may write a brightness.
    pub authorizer: Box<dyn Authorizer>,
    /// Where the charge history is kept between runs; `None` keeps it in memory only.
    pub history_path: Option<PathBuf>,
}

/// State shared between the host thread and the bus objects.
pub struct Shared {
    sysfs_root: PathBuf,
    snapshot: Mutex<Snapshot>,
    authorizer: Box<dyn Authorizer>,
    /// The low-battery warning state, updated with every snapshot.
    warnings: Mutex<Warnings>,
    /// Time base of the warning rules.
    start: Instant,
    /// The battery's recorded charge.
    history: Mutex<History>,
    history_path: Option<PathBuf>,
}

/// Why a brightness write did not happen.
#[derive(Debug)]
pub enum SetError {
    /// There is no such backlight.
    NoDevice,
    /// The value is above the device's maximum.
    OutOfRange { value: u32, max: u32 },
    /// Writing or reading sysfs failed.
    Io(Error),
}

impl fmt::Display for SetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoDevice => write!(f, "there is no such backlight"),
            Self::OutOfRange { value, max } => {
                write!(f, "brightness {value} is above the maximum {max}")
            }
            Self::Io(source) => write!(f, "{source}"),
        }
    }
}

impl Shared {
    /// The current snapshot, locked. Keep the guard short: bus calls wait on it.
    pub fn snapshot(&self) -> MutexGuard<'_, Snapshot> {
        self.snapshot
            .lock()
            .expect("no thread panics while holding the snapshot")
    }

    /// The machine's low-battery warning level.
    pub fn warning_level(&self) -> Level {
        self.warnings
            .lock()
            .expect("no thread panics while holding the warnings")
            .level()
    }

    fn update_warnings(&self, supplies: &[crate::supply::Supply]) -> Level {
        let now = self.start.elapsed().as_secs_f64();
        self.warnings
            .lock()
            .expect("no thread panics while holding the warnings")
            .update(now, Reading::of(supplies))
    }

    pub fn authorizer(&self) -> &dyn Authorizer {
        self.authorizer.as_ref()
    }

    /// The charge samples of the last `timespan` seconds, thinned to one per `resolution` seconds.
    pub fn history(&self, timespan: u32, resolution: u32) -> Vec<Sample> {
        self.history
            .lock()
            .expect("no thread panics while holding the history")
            .query(unix_now(), timespan, resolution)
    }

    /// Record the battery's charge when a sample is due, and keep the file.
    fn record_charge(&self, supplies: &[crate::supply::Supply]) {
        let view = DeviceView::display(supplies);
        if !view.is_present {
            return;
        }
        let mut history = self
            .history
            .lock()
            .expect("no thread panics while holding the history");
        if history.record(unix_now(), view.percentage, view.state) {
            if let Some(path) = &self.history_path {
                if let Err(e) = history.save(path) {
                    eprintln!("cupertino-powerd: the charge history is not saved: {e}");
                }
            }
        }
    }

    pub fn sysfs_root(&self) -> &Path {
        &self.sysfs_root
    }

    /// Writes the display brightness and returns the level the device reports afterwards.
    pub fn set_display(&self, value: u32) -> std::result::Result<u32, SetError> {
        let mut snapshot = self.snapshot();
        let backlight = snapshot.backlight().ok_or(SetError::NoDevice)?.clone();
        if value > backlight.max_brightness {
            return Err(SetError::OutOfRange {
                value,
                max: backlight.max_brightness,
            });
        }
        sysfs::write_brightness(
            &sysfs::backlight_dir(&self.sysfs_root, &backlight.name),
            value,
        )
        .map_err(SetError::Io)?;
        snapshot.backlights = sysfs::read_backlights(&self.sysfs_root).map_err(SetError::Io)?;
        snapshot
            .backlight()
            .map(|backlight| backlight.brightness)
            .ok_or(SetError::NoDevice)
    }

    /// Writes the brightness of the keyboard backlight `name` and returns the level afterwards.
    pub fn set_keyboard(&self, name: &str, value: u32) -> std::result::Result<u32, SetError> {
        let mut snapshot = self.snapshot();
        let keyboard = snapshot
            .keyboards
            .iter()
            .find(|keyboard| keyboard.name == name)
            .ok_or(SetError::NoDevice)?
            .clone();
        if value > keyboard.max_brightness {
            return Err(SetError::OutOfRange {
                value,
                max: keyboard.max_brightness,
            });
        }
        sysfs::write_brightness(&sysfs::led_dir(&self.sysfs_root, name), value)
            .map_err(SetError::Io)?;
        snapshot.keyboards =
            sysfs::read_keyboard_backlights(&self.sysfs_root).map_err(SetError::Io)?;
        snapshot
            .keyboards
            .iter()
            .find(|keyboard| keyboard.name == name)
            .map(|keyboard| keyboard.brightness)
            .ok_or(SetError::NoDevice)
    }
}

/// The running daemon.
pub struct Daemon {
    connection: zbus::blocking::Connection,
    shared: Arc<Shared>,
}

impl Daemon {
    /// Reads the hardware, serves every object and takes both bus names. Fails with
    /// [`Error::NameTaken`] when another daemon (UPower, for instance) owns one of them. A failure
    /// is recorded as `power.backend.start_failed` (step `bus`).
    pub fn start(config: Config) -> Result<Self> {
        Self::serve(config).inspect_err(|error| start_failed("bus", error))
    }

    fn serve(config: Config) -> Result<Self> {
        let snapshot = Snapshot::read(&config.sysfs_root, config.lid)?;
        let shared = Arc::new(Shared {
            sysfs_root: config.sysfs_root,
            snapshot: Mutex::new(snapshot.clone()),
            authorizer: config.authorizer,
            warnings: Mutex::new(Warnings::new()),
            start: Instant::now(),
            history: Mutex::new(
                config
                    .history_path
                    .as_deref()
                    .map(History::load)
                    .unwrap_or_default(),
            ),
            history_path: config.history_path,
        });
        shared.update_warnings(&snapshot.supplies);
        shared.record_charge(&snapshot.supplies);
        let bus = Error::bus("connecting to the bus");
        let builder = match &config.bus {
            Bus::System => Builder::system(),
            Bus::Session => Builder::session(),
            Bus::Address(address) => Builder::address(address.as_str()),
        }
        .map_err(bus)?;
        let serve = Error::bus("exporting an object");
        let mut builder = builder
            .serve_at(UPOWER_PATH, Manager(shared.clone()))
            .and_then(|builder| {
                builder.serve_at(
                    DISPLAY_DEVICE_PATH,
                    DeviceObject {
                        shared: shared.clone(),
                        target: Target::Display,
                    },
                )
            })
            .and_then(|builder| {
                builder.serve_at(
                    KEYBOARD_BACKLIGHT_PATH,
                    KeyboardBacklightObject {
                        shared: shared.clone(),
                        name: None,
                    },
                )
            })
            .and_then(|builder| builder.serve_at(POWER_PATH, PowerObject(shared.clone())))
            .map_err(serve)?;
        for supply in &snapshot.supplies {
            let object = DeviceObject {
                shared: shared.clone(),
                target: Target::Supply(supply.name.clone()),
            };
            builder = builder
                .serve_at(device_path(supply), object)
                .map_err(serve)?;
        }
        for keyboard in &snapshot.keyboards {
            let object = KeyboardBacklightObject {
                shared: shared.clone(),
                name: Some(keyboard.name.clone()),
            };
            builder = builder
                .serve_at(keyboard_path(keyboard), object)
                .map_err(serve)?;
        }
        let connection = builder
            .build()
            .map_err(Error::bus("building the connection"))?;
        for name in [UPOWER_NAME, POWER_NAME] {
            let reply = connection
                .request_name_with_flags(name, RequestNameFlags::DoNotQueue.into())
                .map_err(Error::bus("requesting a bus name"))?;
            match reply {
                RequestNameReply::PrimaryOwner | RequestNameReply::AlreadyOwner => {}
                RequestNameReply::Exists | RequestNameReply::InQueue => {
                    return Err(Error::NameTaken(name))
                }
            }
        }
        Ok(Self { connection, shared })
    }

    /// Reads sysfs again and signals what changed: devices added and removed, changed
    /// properties, brightness changes. On `Err` the previous snapshot stays.
    pub fn refresh(&self) -> Result<()> {
        self.apply(|current| Snapshot::read(&self.shared.sysfs_root, current.lid))
    }

    /// Records a new lid state and signals the manager's property changes.
    pub fn set_lid(&self, lid: Lid) -> Result<()> {
        self.apply(|current| {
            Ok(Snapshot {
                lid,
                ..current.clone()
            })
        })
    }

    /// How long until the warning rules need a fresh reading (the end of the ignore window), if they do.
    pub fn next_deadline(&self) -> Option<Duration> {
        let until = self
            .shared
            .warnings
            .lock()
            .expect("no thread panics while holding the warnings")
            .deadline()?;
        Some(Duration::from_secs_f64(
            (until - self.shared.start.elapsed().as_secs_f64()).max(0.0),
        ))
    }

    pub fn connection(&self) -> &zbus::blocking::Connection {
        &self.connection
    }

    fn apply(&self, next: impl FnOnce(&Snapshot) -> Result<Snapshot>) -> Result<()> {
        let (old, new) = {
            let mut current = self.shared.snapshot();
            let new = next(&current)?;
            (std::mem::replace(&mut *current, new.clone()), new)
        };
        let level_before = self.shared.warning_level();
        let level = self.shared.update_warnings(&new.supplies);
        self.shared.record_charge(&new.supplies);
        zbus::block_on(async {
            announce(self.connection.inner(), &self.shared, &old, &new).await?;
            if level != level_before {
                announce_warning(self.connection.inner(), &new, level).await?;
            }
            Ok(())
        })
    }
}

fn emitter<'a>(
    connection: &Connection,
    path: impl TryInto<ObjectPath<'a>, Error = zbus::zvariant::Error>,
) -> Result<SignalEmitter<'a>> {
    SignalEmitter::new(connection, path).map_err(Error::bus("creating a signal emitter"))
}

/// Emits `PropertiesChanged` with the properties whose value differs between `old` and `new`
/// (both in the same order).
async fn emit_changes(
    connection: &Connection,
    path: &str,
    interface: &'static str,
    old: Vec<Property>,
    new: Vec<Property>,
) -> Result<()> {
    let changed: HashMap<&str, Value<'_>> = new
        .into_iter()
        .zip(old)
        .filter(|((_, new), (_, old))| new != old)
        .map(|((name, new), _)| (name, new))
        .collect();
    if changed.is_empty() {
        return Ok(());
    }
    let signal = Error::bus("emitting PropertiesChanged");
    let interface = InterfaceName::try_from(interface).map_err(|e| signal(e.into()))?;
    zbus::fdo::Properties::properties_changed(
        &emitter(connection, path)?,
        interface,
        changed,
        Cow::Borrowed(&[]),
    )
    .await
    .map_err(signal)
}

/// Signals a new warning level on the display device and on each power-supply battery, which share it.
async fn announce_warning(
    connection: &Connection,
    snapshot: &Snapshot,
    level: Level,
) -> Result<()> {
    let signal = Error::bus("emitting PropertiesChanged");
    let interface = InterfaceName::try_from(DEVICE_INTERFACE).map_err(|e| signal(e.into()))?;
    let paths = std::iter::once(DISPLAY_DEVICE_PATH.to_owned()).chain(
        snapshot
            .supplies
            .iter()
            .filter(|s| shares_warning(s))
            .map(device_path),
    );
    for path in paths {
        let changed = HashMap::from([("WarningLevel", Value::from(level.upower()))]);
        zbus::fdo::Properties::properties_changed(
            &emitter(connection, path.as_str())?,
            interface.clone(),
            changed,
            Cow::Borrowed(&[]),
        )
        .await
        .map_err(signal)?;
    }
    Ok(())
}

/// The machine's warning level is the level of every power-supply battery; other devices report none.
pub fn shares_warning(supply: &crate::supply::Supply) -> bool {
    supply.power_supply && supply.battery.is_some()
}

/// Signals a display brightness change.
pub async fn announce_display(connection: &Connection, brightness: u32) -> Result<()> {
    PowerObject::brightness_changed(&emitter(connection, POWER_PATH)?, brightness)
        .await
        .map_err(Error::bus("emitting BrightnessChanged"))
}

/// Signals a keyboard backlight change on its own path and, for the first keyboard backlight, on the
/// bare path too. `source` is `external` for a change made through this interface and `internal`
/// for one the hardware made.
pub async fn announce_keyboard(
    connection: &Connection,
    shared: &Shared,
    name: &str,
    brightness: u32,
    source: &str,
) -> Result<()> {
    let signal = Error::bus("emitting BrightnessChanged");
    let value = i32::try_from(brightness).map_err(|_| {
        signal(zbus::Error::Failure(format!(
            "brightness {brightness} does not fit the interface"
        )))
    })?;
    let (path, is_first) = {
        let snapshot = shared.snapshot();
        let keyboard = snapshot
            .keyboards
            .iter()
            .find(|keyboard| keyboard.name == name);
        (
            keyboard.map(keyboard_path),
            snapshot
                .keyboards
                .first()
                .is_some_and(|first| first.name == name),
        )
    };
    let Some(path) = path else {
        return Ok(());
    };
    let mut paths = vec![path];
    if is_first {
        paths.push(KEYBOARD_BACKLIGHT_PATH.to_owned());
    }
    for path in &paths {
        let signal_emitter = emitter(connection, path.as_str())?;
        KeyboardBacklightObject::brightness_changed(&signal_emitter, value)
            .await
            .map_err(signal)?;
        KeyboardBacklightObject::brightness_changed_with_source(&signal_emitter, value, source)
            .await
            .map_err(signal)?;
    }
    Ok(())
}

/// Brings the exported objects and the signals in line with the change from `old` to `new`.
async fn announce(
    connection: &Connection,
    shared: &Arc<Shared>,
    old: &Snapshot,
    new: &Snapshot,
) -> Result<()> {
    let objects = connection.object_server();
    let serve = Error::bus("exporting an object");
    let manager = emitter(connection, UPOWER_PATH)?;

    for supply in &new.supplies {
        let path = device_path(supply);
        match old
            .supplies
            .iter()
            .find(|before| before.name == supply.name)
        {
            None => {
                let object = DeviceObject {
                    shared: shared.clone(),
                    target: Target::Supply(supply.name.clone()),
                };
                if !objects.at(path.as_str(), object).await.map_err(serve)? {
                    return Err(serve(zbus::Error::Failure(format!(
                        "the object {path} exists already"
                    ))));
                }
                Manager::device_added(
                    &manager,
                    ObjectPath::try_from(path.as_str()).map_err(|e| serve(e.into()))?,
                )
                .await
                .map_err(Error::bus("emitting DeviceAdded"))?;
            }
            Some(before) => {
                let (before, now) = (DeviceView::of_supply(before), DeviceView::of_supply(supply));
                emit_changes(
                    connection,
                    &path,
                    DEVICE_INTERFACE,
                    device_properties(&before),
                    device_properties(&now),
                )
                .await?;
            }
        }
    }
    for gone in old
        .supplies
        .iter()
        .filter(|before| !new.supplies.iter().any(|supply| supply.name == before.name))
    {
        let path = device_path(gone);
        Manager::device_removed(
            &manager,
            ObjectPath::try_from(path.as_str()).map_err(|e| serve(e.into()))?,
        )
        .await
        .map_err(Error::bus("emitting DeviceRemoved"))?;
        objects
            .remove::<DeviceObject, _>(path.as_str())
            .await
            .map_err(serve)?;
    }

    let (before, now) = (
        DeviceView::display(&old.supplies),
        DeviceView::display(&new.supplies),
    );
    emit_changes(
        connection,
        DISPLAY_DEVICE_PATH,
        DEVICE_INTERFACE,
        device_properties(&before),
        device_properties(&now),
    )
    .await?;
    emit_changes(
        connection,
        UPOWER_PATH,
        MANAGER_INTERFACE,
        manager_properties(old),
        manager_properties(new),
    )
    .await?;

    let level = |snapshot: &Snapshot| {
        snapshot
            .backlight()
            .map(|backlight| (backlight.name.clone(), backlight.brightness))
    };
    if let Some((_, brightness)) = level(new).filter(|now| Some(now) != level(old).as_ref()) {
        announce_display(connection, brightness).await?;
    }

    for keyboard in &new.keyboards {
        match old
            .keyboards
            .iter()
            .find(|before| before.name == keyboard.name)
        {
            None => {
                let object = KeyboardBacklightObject {
                    shared: shared.clone(),
                    name: Some(keyboard.name.clone()),
                };
                let path = keyboard_path(keyboard);
                if !objects.at(path.as_str(), object).await.map_err(serve)? {
                    return Err(serve(zbus::Error::Failure(format!(
                        "the object {path} exists already"
                    ))));
                }
            }
            Some(before) if before.brightness != keyboard.brightness => {
                announce_keyboard(
                    connection,
                    shared,
                    &keyboard.name,
                    keyboard.brightness,
                    "internal",
                )
                .await?;
            }
            Some(_) => {}
        }
    }
    for gone in old.keyboards.iter().filter(|before| {
        !new.keyboards
            .iter()
            .any(|keyboard| keyboard.name == before.name)
    }) {
        objects
            .remove::<KeyboardBacklightObject, _>(keyboard_path(gone).as_str())
            .await
            .map_err(serve)?;
    }
    Ok(())
}
