//! Removable media through udisks2 on the system bus (feature `app`). The sidebar lists every file system of a drive
//! that udisks neither hides nor counts as the system's, and every encrypted container that is still locked
//! ([`devices`]); a click mounts a file system, or unlocks a container with the passphrase the window asked for and
//! mounts what it holds; the eject button unmounts the drive's file systems, locks the containers they came from and
//! ejects or powers off the drive. The calls never set `auth.no_user_interaction`, so polkit may ask through the
//! session's authentication agent for permission; the only secret Files asks for itself is a container's passphrase,
//! which lives in a [`Passphrase`] that is wiped when dropped and never printed. A call that fails is reported as one
//! notification ([`notice`]), naming the programs that keep a busy file system in use ([`holders`]); a prompt the
//! person dismissed is not.
//!
//! Nothing here waits: the reads and the calls are futures on the system bus connection the program's loop ticks
//! (`cupertino_kit::bus`), and what reads files (the process table, [`holders`]) is for the program's I/O tasks.

use std::collections::{HashMap, HashSet};
use std::ffi::OsStr;
use std::future::Future;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::pin::{pin, Pin};
use std::sync::Arc;
use std::task::Poll;
use std::time::{Duration, Instant};

use zbus::export::futures_core::Stream;
use zbus::fdo::{ManagedObjects, ObjectManagerProxy};
use zbus::message::Type;
use zbus::zvariant::{OwnedObjectPath, OwnedValue, Value};
use zbus::{Connection, MatchRule, MessageStream};

use crate::format::human_size;
use cupertino_kit::files::natural::compare_names;
use cupertino_kit::files::notice::{escape, shown, Notice};
use cupertino_kit::files::places::{Medium, Place};

pub const UDISKS_NAME: &str = "org.freedesktop.UDisks2";
pub const MANAGER_PATH: &str = "/org/freedesktop/UDisks2";
pub const BLOCK: &str = "org.freedesktop.UDisks2.Block";
pub const FILESYSTEM: &str = "org.freedesktop.UDisks2.Filesystem";
pub const DRIVE: &str = "org.freedesktop.UDisks2.Drive";
pub const ENCRYPTED: &str = "org.freedesktop.UDisks2.Encrypted";
const ERROR: &str = "org.freedesktop.UDisks2.Error.";
/// The notification's icon: the freedesktop name the device symbol maps to first.
const DEVICE_ICON: &str = "drive-harddisk-symbolic";

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
    /// The kind of medium the drive takes, for its symbol.
    pub medium: Medium,
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
                medium: medium_of(drive),
            })
        })
        .collect();
    out.sort_by(|a, b| compare_names(&a.name, &b.name).then_with(|| a.object.cmp(&b.object)));
    out
}

/// The kind of medium of a drive, from its `Media` and `ConnectionBus` properties: a memory card for a flash card
/// format other than plain flash (`flash_sd`, `flash_cf`, …) or a drive on the card-reader bus, a flash drive for a
/// stick (`thumb`) or plain flash on USB, else a disk.
pub fn medium_of(drive: &HashMap<String, OwnedValue>) -> Medium {
    let media = text(drive, "Media").unwrap_or_default();
    let bus = text(drive, "ConnectionBus").unwrap_or_default();
    if (media.starts_with("flash_") && media != "flash") || bus == "sdio" {
        Medium::Card
    } else if media == "thumb" || (media == "flash" && bus == "usb") {
        Medium::Stick
    } else {
        Medium::Disk
    }
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

/// How long reading which programs hold a busy file system should take: a link read per open descriptor of every
/// process.
pub const HOLDERS_DEADLINE: Duration = Duration::from_secs(5);

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
/// `proc` (`/proc`): sorted, each once. Processes whose table can't be read, as another user's, are left out. It reads
/// every process's descriptors: run it on an I/O task ([`HOLDERS_DEADLINE`]).
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
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
    // a service text that holds the passphrase is not shown at all: one with the passphrase cut out would show where it
    // was, which gives a short one away
    let message = message
        .filter(|m| secret.is_none_or(|s| s.is_empty() || !m.contains(s.text())))
        .map(str::to_owned);
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

/// How a call went.
#[derive(Debug)]
pub enum Called {
    /// It ended: its report, and the notification of its failure.
    Ended(Report, Option<Notice>),
    /// The file system to mount is not there yet: [`Volumes::mount_again`] after [`CLEARTEXT_RETRY`].
    Later(Pending),
    /// An eject found a file system busy: [`Busy::ended`] with the programs that hold it ([`holders`]).
    Busy(Busy),
}

/// A mount to try again: the device the window asked for, the block whose file system is mounted, and until when.
#[derive(Debug)]
pub struct Pending {
    device: Device,
    block: OwnedObjectPath,
    until: Instant,
}

/// An eject that found the file system `name`, mounted at `mount`, busy.
#[derive(Debug)]
pub struct Busy {
    device: OwnedObjectPath,
    name: String,
    mount: PathBuf,
    error: zbus::Error,
}

impl Busy {
    /// Where the busy file system is mounted, which [`holders`] looks under.
    pub fn mount(&self) -> &Path {
        &self.mount
    }

    /// The eject's end, its notification naming `holders`, the programs that keep the file system in use.
    pub fn ended(self, holders: &[String]) -> Called {
        Called::Ended(
            Report::Done {
                object: self.device,
            },
            notice_of(Operation::Eject, &self.name, &self.error, holders, None),
        )
    }
}

/// The signals that tell udisks changed: its own, and its name's owner coming and going.
struct Signals {
    changes: MessageStream,
    owners: MessageStream,
}

/// The removable media of a system bus: what reports the devices whenever udisks changes them, and the calls.
pub struct Volumes {
    system: Connection,
    signals: Option<Signals>,
    sender: async_channel::Sender<Report>,
    reports: async_channel::Receiver<Report>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Volumes {
    /// The removable media of `system`, listening for udisks' changes from now on; [`Volumes::follow`] reads them.
    /// `wake` runs after each report.
    pub async fn watch(
        system: Connection,
        wake: Arc<dyn Fn() + Send + Sync>,
    ) -> zbus::Result<Volumes> {
        let changes = MessageStream::for_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender(UDISKS_NAME)?
                .path_namespace(MANAGER_PATH)?
                .build(),
            &system,
            None,
        )
        .await?;
        let owners = MessageStream::for_match_rule(
            MatchRule::builder()
                .msg_type(Type::Signal)
                .sender("org.freedesktop.DBus")?
                .member("NameOwnerChanged")?
                .arg(0, UDISKS_NAME)?
                .build(),
            &system,
            None,
        )
        .await?;
        let (sender, reports) = async_channel::unbounded();
        Ok(Volumes {
            system,
            signals: Some(Signals { changes, owners }),
            sender,
            reports,
            wake,
        })
    }

    /// The future that reads the devices now and whenever udisks changes them or comes to the bus, reporting each read,
    /// which the program runs on its loop until the connection closes; `None` once taken. A signal only asks for a
    /// read, with at most one asked for while one runs, so a later report is never older than an earlier one. The
    /// signals are taken at once whatever a read costs: a stream that fills stops the connection, the reply to the
    /// read included, and the bus hands this process any signal another user sends it. The devices are read from
    /// udisks itself, so such a signal costs a read at most.
    pub fn follow(&mut self) -> Option<impl Future<Output = ()> + Send + 'static> {
        let Signals {
            mut changes,
            mut owners,
        } = self.signals.take()?;
        let (system, sender, wake) = (
            self.system.clone(),
            self.sender.clone(),
            Arc::clone(&self.wake),
        );
        Some(async move {
            let (again, asked) = async_channel::bounded::<()>(1);
            let _ = again.try_send(());
            let listen = async move {
                loop {
                    let signal = std::future::poll_fn(|cx| {
                        for stream in [&mut changes, &mut owners] {
                            if let Poll::Ready(signal) = Pin::new(stream).poll_next(cx) {
                                return Poll::Ready(signal);
                            }
                        }
                        Poll::Pending
                    })
                    .await;
                    match signal {
                        Some(Ok(_)) => {
                            let _ = again.try_send(());
                        }
                        _ => break,
                    }
                }
            };
            let read = async move {
                while asked.recv().await.is_ok() {
                    if sender
                        .send(Report::Devices(read_devices(&system).await))
                        .await
                        .is_err()
                    {
                        break;
                    }
                    wake();
                }
            };
            both(listen, read).await;
        })
    }

    /// The reports that came since the last call.
    pub fn poll(&self) -> Vec<Report> {
        std::iter::from_fn(|| self.reports.try_recv().ok()).collect()
    }

    /// Mount `device`: [`Report::Mounted`], or a notification and [`Report::Done`].
    pub fn mount(&self, device: &Device) -> impl Future<Output = Called> + Send + 'static {
        let pending = Pending {
            block: device.object.clone(),
            device: device.clone(),
            until: Instant::now() + CLEARTEXT_WAIT,
        };
        self.mount_again(pending)
    }

    /// Try the mount of `pending` again.
    pub fn mount_again(&self, pending: Pending) -> impl Future<Output = Called> + Send + 'static {
        let system = self.system.clone();
        async move { mounted(&system, pending).await }
    }

    /// Unlock the encrypted container `device` with `passphrase`, then mount the file system it holds:
    /// [`Report::Mounted`] for `device`, or a notification and [`Report::Done`].
    pub fn unlock(
        &self,
        device: &Device,
        passphrase: Passphrase,
    ) -> impl Future<Output = Called> + Send + 'static {
        let (system, device) = (self.system.clone(), device.clone());
        async move {
            match unlock(&system, &device, &passphrase).await {
                Ok(cleartext) => {
                    let pending = Pending {
                        device,
                        block: cleartext,
                        until: Instant::now() + CLEARTEXT_WAIT,
                    };
                    mounted(&system, pending).await
                }
                Err(e) => {
                    let notice =
                        notice_of(Operation::Unlock, &device.name, &e, &[], Some(&passphrase));
                    Called::Ended(
                        Report::Done {
                            object: device.object,
                        },
                        notice,
                    )
                }
            }
        }
    }

    /// Eject the drive of `device`: unmount `filesystems` (the drive's mounted ones) in order, stopping at the first
    /// that fails, lock the containers they were unlocked from, then eject the drive, else power it off.
    /// [`Report::Done`] for `device`, or [`Called::Busy`] for a file system that is in use.
    pub fn eject(
        &self,
        device: &Device,
        filesystems: Vec<Device>,
    ) -> impl Future<Output = Called> + Send + 'static {
        let (system, device) = (self.system.clone(), device.clone());
        async move { eject(&system, &device, &filesystems).await }
    }
}

/// Run `a` and `b` until both ended.
async fn both(a: impl Future<Output = ()>, b: impl Future<Output = ()>) {
    let (mut a, mut b) = (pin!(a), pin!(b));
    let (mut a_done, mut b_done) = (false, false);
    std::future::poll_fn(|cx| {
        a_done = a_done || a.as_mut().poll(cx).is_ready();
        b_done = b_done || b.as_mut().poll(cx).is_ready();
        match a_done && b_done {
            true => Poll::Ready(()),
            false => Poll::Pending,
        }
    })
    .await;
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
        self.devices.iter().map(|d| {
            Place::removable(
                &d.name,
                d.mount.as_deref(),
                self.busy.contains(&d.object),
                d.medium,
            )
        })
    }

    /// The device of row `index` and whether a call on it is under way.
    pub fn device(&self, index: usize) -> Option<(&Device, bool)> {
        let device = self.devices.get(index)?;
        Some((device, self.busy.contains(&device.object)))
    }

    /// Whether the drive of device `index` has `folder` on one of its mounted file systems: an eject of that drive
    /// unmounts them all.
    pub fn drive_holds(&self, index: usize, folder: &Path) -> bool {
        let Some(device) = self.devices.get(index) else {
            return false;
        };
        self.devices
            .iter()
            .filter(|d| d.drive == device.drive)
            .any(|d| d.mount.as_deref().is_some_and(|m| folder.starts_with(m)))
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
async fn read_devices(system: &Connection) -> Vec<Device> {
    let objects = async {
        ObjectManagerProxy::builder(system)
            .destination(UDISKS_NAME)?
            .path(MANAGER_PATH)?
            .build()
            .await?
            .get_managed_objects()
            .await
            .map_err(zbus::Error::from)
    };
    match objects.await {
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

/// How long the file system of a container just unlocked may take to appear, and the wait between two tries.
const CLEARTEXT_WAIT: Duration = Duration::from_secs(2);
pub const CLEARTEXT_RETRY: Duration = Duration::from_millis(50);

/// Mount the file system of `pending`'s block, once. A block udisks has not probed yet has no file system interface:
/// that is [`Called::Later`] until [`CLEARTEXT_WAIT`] has passed since the first try.
async fn mounted(system: &Connection, pending: Pending) -> Called {
    let called = system
        .call_method(
            Some(UDISKS_NAME),
            pending.block.as_str(),
            Some(FILESYSTEM),
            "Mount",
            &(no_options(),),
        )
        .await
        .and_then(|reply| reply.body().deserialize::<String>());
    let object = pending.device.object.clone();
    match called {
        Ok(at) => Called::Ended(
            Report::Mounted {
                object,
                at: PathBuf::from(at),
            },
            None,
        ),
        Err(e) if not_there_yet(&e) && Instant::now() < pending.until => Called::Later(pending),
        Err(e) => Called::Ended(
            Report::Done { object },
            notice(Operation::Mount, &pending.device.name, &e),
        ),
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
async fn unlock(
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
        )
        .await?
        .body()
        .deserialize()
}

/// Unmount `filesystems`, lock the containers they came from, then eject or power off the drive; a failed call is
/// its notification, and a busy file system [`Called::Busy`].
async fn eject(system: &Connection, device: &Device, filesystems: &[Device]) -> Called {
    let done = |notice| {
        Called::Ended(
            Report::Done {
                object: device.object.clone(),
            },
            notice,
        )
    };
    for fs in filesystems {
        let unmounted = system
            .call_method(
                Some(UDISKS_NAME),
                fs.object.as_str(),
                Some(FILESYSTEM),
                "Unmount",
                &(no_options(),),
            )
            .await;
        if let Err(e) = unmounted {
            let busy = matches!(&e, zbus::Error::MethodError(name, _, _)
                if name.as_str().strip_prefix(ERROR) == Some("DeviceBusy"));
            return match &fs.mount {
                Some(mount) if busy => Called::Busy(Busy {
                    device: device.object.clone(),
                    name: fs.name.clone(),
                    mount: mount.clone(),
                    error: e,
                }),
                _ => done(notice_of(Operation::Eject, &fs.name, &e, &[], None)),
            };
        }
        if let Some(container) = &fs.backing {
            let locked = system
                .call_method(
                    Some(UDISKS_NAME),
                    container.as_str(),
                    Some(ENCRYPTED),
                    "Lock",
                    &(no_options(),),
                )
                .await;
            if let Err(e) = locked {
                return done(notice(Operation::Eject, &fs.name, &e));
            }
        }
    }
    let method = if device.ejectable {
        "Eject"
    } else if device.can_power_off {
        "PowerOff"
    } else {
        return done(None);
    };
    let ejected = system
        .call_method(
            Some(UDISKS_NAME),
            device.drive.as_str(),
            Some(DRIVE),
            method,
            &(no_options(),),
        )
        .await;
    done(
        ejected
            .err()
            .and_then(|e| notice(Operation::Eject, &device.name, &e)),
    )
}
