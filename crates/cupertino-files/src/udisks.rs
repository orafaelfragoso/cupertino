//! Removable media through udisks2 on the system bus (feature `app`). The sidebar lists every file system of a drive
//! that udisks neither hides nor counts as the system's, and every encrypted container that is still locked
//! ([`devices`]); a click mounts a file system, or unlocks a container with the passphrase the window asked for and
//! mounts what it holds; the eject button unmounts the drive's file systems, locks the containers they came from and
//! ejects or powers off the drive. The calls never set `auth.no_user_interaction`, so polkit may ask through the
//! session's authentication agent for permission; the only secret Files asks for itself is a container's passphrase,
//! which lives in a [`Passphrase`] that is wiped when dropped and never printed. A call that fails is reported as one
//! notification ([`notice`]), naming the programs that keep a busy file system in use ([`holders`]); a prompt the
//! person dismissed is not.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use zbus::blocking::fdo::ObjectManagerProxy;
use zbus::blocking::{Connection, MessageIterator};
use zbus::fdo::ManagedObjects;
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

use crate::format::human_size;
use crate::natural::compare_names;
use crate::notice::{escape, shown, Notice, Notices};
use crate::places::Place;

pub const UDISKS_NAME: &str = "org.freedesktop.UDisks2";
pub const MANAGER_PATH: &str = "/org/freedesktop/UDisks2";
pub const BLOCK: &str = "org.freedesktop.UDisks2.Block";
pub const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
pub const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
pub const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
const ERROR: &str = "org.freedesktop.UDisks2.Error.";
/// The notification's icon: the freedesktop name the device symbol maps to first.
const DEVICE_ICON: &str = "drive-removable-media-symbolic";

/// A file system the sidebar lists, of a drive udisks reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Device {
    /// The block object, which has the file system.
    pub object: OwnedObjectPath,
    pub drive: OwnedObjectPath,
    pub name: String,
    /// The first mount point; `None` while not mounted.
    pub mount: Option<PathBuf>,
    pub ejectable: bool,
    pub can_power_off: bool,
    /// An encrypted container that is not unlocked: a click asks for its passphrase.
    pub locked: bool,
    /// The medium can't be written.
    pub read_only: bool,
    /// The encrypted container this file system was unlocked from, which an eject locks again.
    pub backing: Option<OwnedObjectPath>,
}

/// A container's passphrase: wiped from memory when dropped, and never shown by `Debug`.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Passphrase(zeroize::Zeroizing<String>);

impl Passphrase {
    pub fn new(text: &str) -> Passphrase {
        Passphrase(zeroize::Zeroizing::new(text.to_owned()))
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn text(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for Passphrase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Passphrase([secret])")
    }
}

/// The listed file systems of `objects` (udisks' managed objects), sorted by name: a block with a file system whose
/// usage is `filesystem`, or an encrypted container (usage `crypto`) that is not unlocked, which udisks neither hides
/// nor counts as the system's, and which belongs to a drive (a loop device belongs to none). An unlocked container is
/// left out: the file system it holds is listed in its place, on the container's drive. A name is [`shown`] text.
pub fn devices(objects: &ManagedObjects) -> Vec<Device> {
    let mut out: Vec<Device> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let block = interfaces.get(BLOCK)?;
            // `None` for a locked container
            let filesystem = match (
                text(block, "IdUsage").as_deref(),
                interfaces.get(FILESYSTEM),
                interfaces.get(ENCRYPTED),
            ) {
                (Some("filesystem"), Some(filesystem), _) => Some(filesystem),
                (Some("crypto"), _, Some(encrypted))
                    if object(encrypted, "CleartextDevice").is_none() =>
                {
                    None
                }
                _ => return None,
            };
            if flag(block, "HintIgnore") || flag(block, "HintSystem") {
                return None;
            }
            let backing = object(block, "CryptoBackingDevice");
            let drive_path = object(block, "Drive").or_else(|| {
                let backing = objects.get(backing.as_ref()?)?.get(BLOCK)?;
                object(backing, "Drive")
            })?;
            let drive = objects.get(&drive_path)?.get(DRIVE)?;
            let vendor_model = format!(
                "{} {}",
                text(drive, "Vendor").unwrap_or_default().trim(),
                text(drive, "Model").unwrap_or_default().trim()
            );
            let name = [
                text(block, "HintName"),
                text(block, "IdLabel"),
                Some(vendor_model.trim().to_string()),
            ]
            .into_iter()
            .flatten()
            .map(|n| shown(n.trim()).trim().to_string())
            .find(|n| !n.is_empty())
            .unwrap_or_else(|| {
                let size = block
                    .get("Size")
                    .and_then(|v| u64::try_from(&**v).ok())
                    .unwrap_or(0);
                format!("{} Volume", human_size(size))
            });
            Some(Device {
                object: path.clone(),
                drive: drive_path,
                name,
                mount: filesystem.and_then(first_mount),
                ejectable: flag(drive, "Ejectable"),
                can_power_off: flag(drive, "CanPowerOff"),
                locked: filesystem.is_none(),
                read_only: flag(block, "ReadOnly"),
                backing,
            })
        })
        .collect();
    out.sort_by(|a, b| compare_names(&a.name, &b.name).then_with(|| a.object.cmp(&b.object)));
    out
}

fn text(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    properties
        .get(key)
        .and_then(|v| String::try_from(v.try_clone().ok()?).ok())
}

/// An object path property; `None` for `/`, udisks' "no object".
fn object(properties: &HashMap<String, OwnedValue>, key: &str) -> Option<OwnedObjectPath> {
    properties
        .get(key)
        .and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok())
        .filter(|p| p.as_str() != "/")
}

fn flag(properties: &HashMap<String, OwnedValue>, key: &str) -> bool {
    properties
        .get(key)
        .and_then(|v| bool::try_from(&**v).ok())
        .unwrap_or(false)
}

/// The first of `MountPoints` (`aay`, each NUL-terminated).
fn first_mount(filesystem: &HashMap<String, OwnedValue>) -> Option<PathBuf> {
    let points = Vec::<Vec<u8>>::try_from(filesystem.get("MountPoints")?.try_clone().ok()?).ok()?;
    let first = points.into_iter().next()?;
    let bytes = first.strip_suffix(&[0]).unwrap_or(&first);
    (!bytes.is_empty()).then(|| PathBuf::from(OsStr::from_bytes(bytes)))
}

/// What a call was for, which words its failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Mount,
    Eject,
    Unlock,
}

/// The notification for a failed call on the device `name`; `None` when the person dismissed the polkit prompt.
pub fn notice(operation: Operation, name: &str, e: &zbus::Error) -> Option<Notice> {
    notice_of(operation, name, e, &[], None)
}

/// The names of the programs that hold a file or a working folder under `mount`, read from the process table at
/// `proc` (`/proc`): sorted, each once. Processes whose table can't be read, as another user's, are left out.
pub fn holders(proc: &Path, mount: &Path) -> Vec<String> {
    let Ok(processes) = std::fs::read_dir(proc) else {
        return Vec::new();
    };
    let under = |link: PathBuf| std::fs::read_link(link).is_ok_and(|to| to.starts_with(mount));
    let mut names: Vec<String> = processes
        .flatten()
        .filter(|p| {
            p.file_name()
                .to_str()
                .is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
        })
        .map(|p| p.path())
        .filter(|process| {
            under(process.join("cwd"))
                || std::fs::read_dir(process.join("fd"))
                    .is_ok_and(|fds| fds.flatten().any(|fd| under(fd.path())))
        })
        .filter_map(|process| std::fs::read_to_string(process.join("comm")).ok())
        .map(|name| shown(name.trim()).trim().to_owned())
        .filter(|name| !name.is_empty())
        .collect();
    names.sort();
    names.dedup();
    names
}

/// How a busy file system's notice names the programs `holders` that use it.
fn using(holders: &[String]) -> String {
    let quoted = |name: &String| format!("“{}”", escape(name));
    match holders {
        [] => "One or more applications are using it. Quit them and try again.".to_owned(),
        [one] => format!("{} is using it. Quit it and try again.", quoted(one)),
        [first, second] => format!(
            "{} and {} are using it. Quit them and try again.",
            quoted(first),
            quoted(second)
        ),
        [first, rest @ ..] => format!(
            "{} and {} other applications are using it. Quit them and try again.",
            quoted(first),
            rest.len()
        ),
    }
}

/// [`notice`] for a file system that `holders` keep busy, and with every copy of `secret` cut from the service's
/// words.
fn notice_of(
    operation: Operation,
    name: &str,
    e: &zbus::Error,
    holders: &[String],
    secret: Option<&Passphrase>,
) -> Option<Notice> {
    let (error, message) = match e {
        zbus::Error::MethodError(error, message, _) => (Some(error.as_str()), message.as_deref()),
        _ => (None, None),
    };
    let failed = match operation {
        Operation::Mount => format!("“{name}” couldn't be mounted"),
        Operation::Eject => format!("“{name}” couldn't be ejected"),
        Operation::Unlock => format!("“{name}” couldn't be unlocked"),
    };
    let message = message.map(|m| match secret {
        Some(secret) if !secret.is_empty() => m.replace(secret.text(), "…"),
        _ => m.to_owned(),
    });
    let (summary, body) = match error.and_then(|n| n.strip_prefix(ERROR)) {
        Some("NotAuthorizedDismissed") => return None,
        Some("DeviceBusy") => (format!("“{name}” wasn't ejected"), using(holders)),
        Some("NotAuthorizedCanObtain") => (failed, "Couldn't ask for permission.".to_string()),
        Some("NotAuthorized") => (failed, "You aren't allowed to do this.".to_string()),
        _ => (
            failed,
            escape(&shown(&message.filter(|m| !m.is_empty()).unwrap_or_else(
                || match secret {
                    // the error's own text could hold what the call sent
                    Some(_) => "The call failed.".to_owned(),
                    None => e.to_string(),
                },
            ))),
        ),
    };
    Some(Notice {
        summary,
        body,
        icon: DEVICE_ICON,
    })
}

/// What the reader and the calls report to the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Report {
    /// The listed file systems now ([`devices`]); none while udisks is not on the bus.
    Devices(Vec<Device>),
    /// A mount of `object` finished at `at`.
    Mounted {
        object: OwnedObjectPath,
        at: PathBuf,
    },
    /// A call on `object` ended without a mount to go to: an eject, done or failed, or a failed mount.
    Done { object: OwnedObjectPath },
}

/// The removable media of a system bus: a reader that reports the devices whenever udisks changes them, and the calls.
pub struct Volumes {
    system: Connection,
    notices: Notices,
    sender: Sender<Report>,
    reports: Receiver<Report>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Volumes {
    /// Read the devices of `system` now and whenever udisks changes them or comes to the bus; `notices` takes the
    /// failures. `wake` runs on other threads after each report.
    pub fn watch(
        system: Connection,
        notices: Notices,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> zbus::Result<Volumes> {
        let (sender, reports) = mpsc::channel();
        let changes = MessageIterator::for_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(UDISKS_NAME)?
                .path_namespace(MANAGER_PATH)?
                .build(),
            &system,
            None,
        )?;
        let owners = MessageIterator::for_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender("org.freedesktop.DBus")?
                .member("NameOwnerChanged")?
                .arg(0, UDISKS_NAME)?
                .build(),
            &system,
            None,
        )?;
        // one read at a time, so a later report is never older than an earlier one
        let read = Arc::new(Mutex::new(()));
        let report = {
            let (system, sender, wake) = (system.clone(), sender.clone(), Arc::clone(&wake));
            move || {
                let _one = read.lock().expect("no reader panics while reading");
                let _ = sender.send(Report::Devices(read_devices(&system)));
                wake();
            }
        };
        report();
        for (name, messages) in [("udisks-changes", changes), ("udisks-owner", owners)] {
            let report = report.clone();
            std::thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    for message in messages {
                        if message.is_err() {
                            break;
                        }
                        report();
                    }
                })
                .map_err(|e| zbus::Error::Failure(e.to_string()))?;
        }
        Ok(Volumes {
            system,
            notices,
            sender,
            reports,
            wake,
        })
    }

    /// The reports that came since the last call.
    pub fn poll(&self) -> Vec<Report> {
        self.reports.try_iter().collect()
    }

    /// Mount `device`: [`Report::Mounted`] follows, or a notification and [`Report::Done`].
    pub fn mount(&self, device: &Device) {
        let device = device.clone();
        self.call(device.object.clone(), move |system, notices| {
            match mount(system, &device) {
                Ok(at) => Report::Mounted {
                    object: device.object,
                    at,
                },
                Err(e) => {
                    if let Some(notice) = notice(Operation::Mount, &device.name, &e) {
                        notices.post(notice);
                    }
                    Report::Done {
                        object: device.object,
                    }
                }
            }
        });
    }

    /// Unlock the encrypted container `device` with `passphrase`, then mount the file system it holds:
    /// [`Report::Mounted`] for `device` follows, or a notification and [`Report::Done`].
    pub fn unlock(&self, device: &Device, passphrase: Passphrase) {
        let device = device.clone();
        self.call(device.object.clone(), move |system, notices| {
            let mounted = unlock(system, &device, &passphrase)
                .map_err(|e| (Operation::Unlock, e))
                .and_then(|cleartext| {
                    mount_object(system, &cleartext).map_err(|e| (Operation::Mount, e))
                });
            match mounted {
                Ok(at) => Report::Mounted {
                    object: device.object,
                    at,
                },
                Err((operation, e)) => {
                    let secret = (operation == Operation::Unlock).then_some(&passphrase);
                    if let Some(notice) = notice_of(operation, &device.name, &e, &[], secret) {
                        notices.post(notice);
                    }
                    Report::Done {
                        object: device.object,
                    }
                }
            }
        });
    }

    /// Eject the drive of `device`: unmount `filesystems` (the drive's mounted ones) in order, stopping at the first
    /// that fails, lock the containers they were unlocked from, then eject the drive, else power it off.
    /// [`Report::Done`] for `device` follows.
    pub fn eject(&self, device: &Device, filesystems: Vec<Device>) {
        let device = device.clone();
        self.call(device.object.clone(), move |system, notices| {
            if let Some(notice) = eject(system, &device, &filesystems) {
                notices.post(notice);
            }
            Report::Done {
                object: device.object,
            }
        });
    }

    fn call(
        &self,
        object: OwnedObjectPath,
        run: impl FnOnce(&Connection, &Notices) -> Report + Send + 'static,
    ) {
        let (system, notices) = (self.system.clone(), self.notices.clone());
        let (sender, wake) = (self.sender.clone(), Arc::clone(&self.wake));
        let spawned = std::thread::Builder::new()
            .name("udisks-call".into())
            .spawn(move || {
                let _ = sender.send(run(&system, &notices));
                wake();
            });
        if let Err(e) = spawned {
            eprintln!(
                "cupertino-files: cannot start a call on {}: {e}",
                object.as_str()
            );
            let _ = self.sender.send(Report::Done { object });
        }
    }
}

/// The removable media a window lists: the devices udisks last reported and those a call is under way on.
#[derive(Debug, Default)]
pub struct Media {
    devices: Vec<Device>,
    busy: HashSet<OwnedObjectPath>,
}

impl Media {
    /// Take `report`; the folder a finished mount goes to.
    pub fn take(&mut self, report: Report) -> Option<PathBuf> {
        match report {
            Report::Devices(devices) => {
                self.busy
                    .retain(|object| devices.iter().any(|d| &d.object == object));
                self.devices = devices;
                None
            }
            Report::Mounted { object, at } => {
                self.busy.remove(&object);
                Some(at)
            }
            Report::Done { object } => {
                let drive = self
                    .devices
                    .iter()
                    .find(|d| d.object == object)
                    .map(|d| d.drive.clone());
                let devices = &self.devices;
                self.busy.retain(|o| {
                    *o != object
                        && !devices
                            .iter()
                            .any(|d| &d.object == o && Some(&d.drive) == drive.as_ref())
                });
                None
            }
        }
    }

    /// The sidebar's rows of the devices, in order.
    pub fn places(&self) -> impl Iterator<Item = Place> + '_ {
        self.devices
            .iter()
            .map(|d| Place::removable(&d.name, d.mount.as_deref(), self.busy.contains(&d.object)))
    }

    /// The device of row `index` and whether a call on it is under way.
    pub fn device(&self, index: usize) -> Option<(&Device, bool)> {
        let device = self.devices.get(index)?;
        Some((device, self.busy.contains(&device.object)))
    }

    /// The row of the device whose block is `object`.
    pub fn index_of(&self, object: &OwnedObjectPath) -> Option<usize> {
        self.devices.iter().position(|d| &d.object == object)
    }

    /// Start a mount or an unlock of device `index`: the device, busy from now on; `None` while a call on it is under
    /// way.
    pub fn start(&mut self, index: usize) -> Option<Device> {
        let device = self
            .devices
            .get(index)
            .filter(|d| !self.busy.contains(&d.object))?
            .clone();
        self.busy.insert(device.object.clone());
        Some(device)
    }

    /// Whether a device is mounted on `dir` or a folder holding it.
    pub fn holds(&self, dir: &Path) -> bool {
        self.holding(dir).is_some()
    }

    /// The device mounted on `dir` or the folder nearest it that holds it.
    pub fn holding(&self, dir: &Path) -> Option<&Device> {
        self.devices
            .iter()
            .filter(|d| d.mount.as_deref().is_some_and(|m| dir.starts_with(m)))
            .max_by_key(|d| d.mount.as_ref().map_or(0, |m| m.components().count()))
    }

    /// Start the eject of the drive of device `index`: the device and the drive's mounted file systems, busy from
    /// now on, for [`Volumes::eject`]; `None` while a call on the device is under way.
    pub fn eject(&mut self, index: usize) -> Option<(Device, Vec<Device>)> {
        let device = self
            .devices
            .get(index)
            .filter(|d| !self.busy.contains(&d.object))?
            .clone();
        let filesystems: Vec<Device> = self
            .devices
            .iter()
            .filter(|d| d.drive == device.drive && d.mount.is_some())
            .cloned()
            .collect();
        self.busy
            .extend(filesystems.iter().map(|d| d.object.clone()));
        self.busy.insert(device.object.clone());
        Some((device, filesystems))
    }
}

/// The devices udisks reports; none when it is not on the bus. Another failure is logged.
fn read_devices(system: &Connection) -> Vec<Device> {
    let objects = ObjectManagerProxy::builder(system)
        .destination(UDISKS_NAME)
        .and_then(|b| b.path(MANAGER_PATH))
        .and_then(|b| b.build())
        .and_then(|proxy| Ok(proxy.get_managed_objects()?));
    match objects {
        Ok(objects) => devices(&objects),
        Err(e) if absent(&e) => Vec::new(),
        Err(e) => {
            eprintln!("cupertino-files: cannot read the removable media: {e}");
            Vec::new()
        }
    }
}

/// Whether `e` says udisks is not on the bus and can't be started.
fn absent(e: &zbus::Error) -> bool {
    match e {
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.ServiceUnknown"
                | "org.freedesktop.DBus.Error.NameHasNoOwner"
        ),
        zbus::Error::FDO(e) => matches!(
            e.as_ref(),
            zbus::fdo::Error::ServiceUnknown(_) | zbus::fdo::Error::NameHasNoOwner(_)
        ),
        _ => false,
    }
}

/// The options of every call: none, so polkit may ask.
fn no_options() -> HashMap<&'static str, Value<'static>> {
    HashMap::new()
}

fn mount(system: &Connection, device: &Device) -> zbus::Result<PathBuf> {
    mount_object(system, &device.object)
}

/// How long the file system of a container just unlocked may take to appear, and the wait between two tries.
const CLEARTEXT_WAIT: std::time::Duration = std::time::Duration::from_secs(2);
const CLEARTEXT_RETRY: std::time::Duration = std::time::Duration::from_millis(50);

/// Mount the file system of `object`. A block udisks has not probed yet has no file system interface: that is tried
/// again for up to [`CLEARTEXT_WAIT`].
fn mount_object(system: &Connection, object: &OwnedObjectPath) -> zbus::Result<PathBuf> {
    let deadline = std::time::Instant::now() + CLEARTEXT_WAIT;
    loop {
        let called = system.call_method(
            Some(UDISKS_NAME),
            object.as_str(),
            Some(FILESYSTEM),
            "Mount",
            &(no_options(),),
        );
        match called {
            Ok(reply) => return Ok(PathBuf::from(reply.body().deserialize::<String>()?)),
            Err(e) if not_there_yet(&e) && std::time::Instant::now() < deadline => {
                std::thread::sleep(CLEARTEXT_RETRY)
            }
            Err(e) => return Err(e),
        }
    }
}

/// Whether `e` says the object or its file system interface is not exported (yet).
fn not_there_yet(e: &zbus::Error) -> bool {
    let name = match e {
        zbus::Error::MethodError(name, _, _) => name.as_str(),
        _ => return false,
    };
    matches!(
        name,
        "org.freedesktop.DBus.Error.UnknownInterface"
            | "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownObject"
    )
}

/// Unlock the container `device` with `passphrase`; the object of the block it opens.
fn unlock(
    system: &Connection,
    device: &Device,
    passphrase: &Passphrase,
) -> zbus::Result<OwnedObjectPath> {
    system
        .call_method(
            Some(UDISKS_NAME),
            device.object.as_str(),
            Some(ENCRYPTED),
            "Unlock",
            &(passphrase.text(), no_options()),
        )?
        .body()
        .deserialize()
}

/// Unmount `filesystems`, lock the containers they came from, then eject or power off the drive; the notice of a
/// failed call, naming what keeps a busy file system in use.
fn eject(system: &Connection, device: &Device, filesystems: &[Device]) -> Option<Notice> {
    for fs in filesystems {
        let unmounted = system.call_method(
            Some(UDISKS_NAME),
            fs.object.as_str(),
            Some(FILESYSTEM),
            "Unmount",
            &(no_options(),),
        );
        if let Err(e) = unmounted {
            let busy = matches!(&e, zbus::Error::MethodError(name, _, _)
                if name.as_str().strip_prefix(ERROR) == Some("DeviceBusy"));
            let holders = match &fs.mount {
                Some(mount) if busy => holders(Path::new("/proc"), mount),
                _ => Vec::new(),
            };
            return notice_of(Operation::Eject, &fs.name, &e, &holders, None);
        }
        if let Some(container) = &fs.backing {
            let locked = system.call_method(
                Some(UDISKS_NAME),
                container.as_str(),
                Some(ENCRYPTED),
                "Lock",
                &(no_options(),),
            );
            if let Err(e) = locked {
                return notice(Operation::Eject, &fs.name, &e);
            }
        }
    }
    let method = if device.ejectable {
        "Eject"
    } else if device.can_power_off {
        "PowerOff"
    } else {
        return None;
    };
    system
        .call_method(
            Some(UDISKS_NAME),
            device.drive.as_str(),
            Some(DRIVE),
            method,
            &(no_options(),),
        )
        .err()
        .and_then(|e| notice(Operation::Eject, &device.name, &e))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(name: &str, message: &str) -> zbus::Error {
        zbus::Error::MethodError(
            zbus::names::OwnedErrorName::try_from(name).unwrap(),
            Some(message.into()),
            zbus::message::Message::method_call("/", "M")
                .unwrap()
                .build(&())
                .unwrap(),
        )
    }

    #[test]
    fn a_failure_is_worded_by_its_udisks_error_and_a_dismissed_prompt_says_nothing() {
        let busy = notice(
            Operation::Eject,
            "Backup Disk",
            &error("org.freedesktop.UDisks2.Error.DeviceBusy", "target is busy"),
        )
        .unwrap();
        assert_eq!(busy.summary, "“Backup Disk” wasn't ejected");
        assert_eq!(
            busy.body,
            "One or more applications are using it. Quit them and try again."
        );
        assert_eq!(
            notice(
                Operation::Mount,
                "KINGSTON",
                &error(
                    "org.freedesktop.UDisks2.Error.NotAuthorizedDismissed",
                    "dismissed"
                ),
            ),
            None
        );
        let no_agent = notice(
            Operation::Mount,
            "KINGSTON",
            &error(
                "org.freedesktop.UDisks2.Error.NotAuthorizedCanObtain",
                "not authorized",
            ),
        )
        .unwrap();
        assert_eq!(
            (no_agent.summary.as_str(), no_agent.body.as_str()),
            (
                "“KINGSTON” couldn't be mounted",
                "Couldn't ask for permission."
            )
        );
        assert_eq!(
            notice(
                Operation::Eject,
                "KINGSTON",
                &error("org.freedesktop.UDisks2.Error.NotAuthorized", "no"),
            )
            .unwrap()
            .body,
            "You aren't allowed to do this."
        );
        assert_eq!(
            notice(
                Operation::Mount,
                "KINGSTON",
                &error("org.freedesktop.UDisks2.Error.Failed", "wrong fs type"),
            )
            .unwrap()
            .body,
            "wrong fs type"
        );
        let link = notice(
            Operation::Mount,
            "KINGSTON",
            &error(
                "org.freedesktop.UDisks2.Error.Failed",
                "Error mounting /dev/sdb1 at /run/media/ada/<a href=x>y</a>: bad superblock",
            ),
        )
        .unwrap();
        assert_eq!(
            link.body,
            "Error mounting /dev/sdb1 at /run/media/ada/&lt;a href=x&gt;y&lt;/a&gt;: bad superblock"
        );
    }
}
