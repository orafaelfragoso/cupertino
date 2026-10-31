//! Removable media through udisks2 on the system bus (feature `app`). The sidebar lists every file system of a drive
//! that udisks neither hides nor counts as the system's ([`devices`]); a click mounts one, the eject button unmounts the
//! drive's file systems and ejects or powers off the drive. The calls never set `auth.no_user_interaction`, so polkit
//! may ask through the session's authentication agent; Files never asks for a password itself. A call that fails is
//! reported as one notification ([`notice`]); a prompt the person dismissed is not.

use std::collections::HashMap;
use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};

use zbus::blocking::fdo::ObjectManagerProxy;
use zbus::blocking::{Connection, MessageIterator};
use zbus::fdo::ManagedObjects;
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::MatchRule;

use crate::natural::compare_names;
use crate::notice::{escape, shown, Notice, Notices};
use crate::render::human_size;

pub const UDISKS_NAME: &str = "org.freedesktop.UDisks2";
pub const MANAGER_PATH: &str = "/org/freedesktop/UDisks2";
pub const BLOCK: &str = "org.freedesktop.UDisks2.Block";
pub const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
pub const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
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
}

/// The listed file systems of `objects` (udisks' managed objects), sorted by name: a block with a file system whose
/// usage is `filesystem`, which udisks neither hides nor counts as the system's, and which belongs to a drive (a loop
/// device belongs to none). A name is [`shown`] text.
pub fn devices(objects: &ManagedObjects) -> Vec<Device> {
    let mut out: Vec<Device> = objects
        .iter()
        .filter_map(|(path, interfaces)| {
            let block = interfaces.get(BLOCK)?;
            let filesystem = interfaces.get(FILESYSTEM)?;
            if text(block, "IdUsage").as_deref() != Some("filesystem")
                || flag(block, "HintIgnore")
                || flag(block, "HintSystem")
            {
                return None;
            }
            let drive_path = block
                .get("Drive")
                .and_then(|v| OwnedObjectPath::try_from(v.try_clone().ok()?).ok())
                .filter(|p| p.as_str() != "/")?;
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
                mount: first_mount(filesystem),
                ejectable: flag(drive, "Ejectable"),
                can_power_off: flag(drive, "CanPowerOff"),
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
}

/// The notification for a failed call on the device `name`; `None` when the person dismissed the polkit prompt.
pub fn notice(operation: Operation, name: &str, e: &zbus::Error) -> Option<Notice> {
    let (error, message) = match e {
        zbus::Error::MethodError(error, message, _) => (Some(error.as_str()), message.as_deref()),
        _ => (None, None),
    };
    let failed = match operation {
        Operation::Mount => format!("“{name}” couldn't be mounted"),
        Operation::Eject => format!("“{name}” couldn't be ejected"),
    };
    let (summary, body) = match error.and_then(|n| n.strip_prefix(ERROR)) {
        Some("NotAuthorizedDismissed") => return None,
        Some("DeviceBusy") => (
            format!("“{name}” wasn't ejected"),
            "One or more applications are using it. Quit them and try again.".to_string(),
        ),
        Some("NotAuthorizedCanObtain") => (failed, "Couldn't ask for permission.".to_string()),
        Some("NotAuthorized") => (failed, "You aren't allowed to do this.".to_string()),
        _ => (
            failed,
            escape(&shown(
                &message
                    .filter(|m| !m.is_empty())
                    .map_or_else(|| e.to_string(), str::to_string),
            )),
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

    /// Eject the drive of `device`: unmount `filesystems` (the drive's mounted ones) in order, stopping at the first
    /// that fails, then eject the drive, else power it off. [`Report::Done`] for `device` follows.
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
    let at: String = system
        .call_method(
            Some(UDISKS_NAME),
            device.object.as_str(),
            Some(FILESYSTEM),
            "Mount",
            &(no_options(),),
        )?
        .body()
        .deserialize()?;
    Ok(PathBuf::from(at))
}

/// Unmount `filesystems`, then eject or power off the drive; the notice of a failed call.
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
            return notice(Operation::Eject, &fs.name, &e);
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
