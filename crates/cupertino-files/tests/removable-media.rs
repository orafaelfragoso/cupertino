//! Removable media against a fake udisks2 and a fake notification service on a private bus (as system and session bus):
//! which file systems are listed, the calls a mount and an eject make in order, and what a failure reports.
#![cfg(feature = "app")]

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_files::udisks::{Device, Report, Volumes, MANAGER_PATH, UDISKS_NAME};
use cupertino_test_bus::{missing_tool, PrivateBus};
use zbus::blocking::Connection;
use zbus::object_server::SignalEmitter;
use zbus::zvariant::{OwnedObjectPath, OwnedValue};

#[derive(Default)]
struct State {
    /// The calls udisks and the notification service received, in order.
    log: Vec<String>,
    /// File systems whose unmount fails as busy.
    busy: HashSet<String>,
    /// The error a mount fails with.
    refuse_mount: Option<Fail>,
}

type Shared = Arc<Mutex<State>>;

#[derive(Debug, Clone, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.UDisks2.Error")]
enum Fail {
    #[zbus(error)]
    ZBus(zbus::Error),
    DeviceBusy(String),
    NotAuthorizedDismissed(String),
}

struct Block {
    usage: &'static str,
    label: &'static str,
    hint_name: &'static str,
    ignore: bool,
    system: bool,
    drive: OwnedObjectPath,
    size: u64,
}

impl Default for Block {
    fn default() -> Self {
        Block {
            usage: "filesystem",
            label: "",
            hint_name: "",
            ignore: false,
            system: false,
            drive: OwnedObjectPath::try_from("/").unwrap(),
            size: 0,
        }
    }
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Block")]
impl Block {
    #[zbus(property)]
    fn id_usage(&self) -> String {
        self.usage.into()
    }
    #[zbus(property)]
    fn id_label(&self) -> String {
        self.label.into()
    }
    #[zbus(property)]
    fn hint_name(&self) -> String {
        self.hint_name.into()
    }
    #[zbus(property)]
    fn hint_ignore(&self) -> bool {
        self.ignore
    }
    #[zbus(property)]
    fn hint_system(&self) -> bool {
        self.system
    }
    #[zbus(property)]
    fn drive(&self) -> OwnedObjectPath {
        self.drive.clone()
    }
    #[zbus(property)]
    fn size(&self) -> u64 {
        self.size
    }
}

struct Filesystem {
    id: String,
    mount: Option<String>,
    state: Shared,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Filesystem")]
impl Filesystem {
    #[zbus(property)]
    fn mount_points(&self) -> Vec<Vec<u8>> {
        self.mount
            .iter()
            .map(|m| m.bytes().chain([0]).collect())
            .collect()
    }

    async fn mount(
        &mut self,
        options: HashMap<String, OwnedValue>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<String, Fail> {
        let refused = {
            let mut state = self.state.lock().unwrap();
            let mut keys: Vec<&String> = options.keys().collect();
            keys.sort();
            state.log.push(format!("Mount {} {keys:?}", self.id));
            state.refuse_mount.clone()
        };
        if let Some(e) = refused {
            return Err(e);
        }
        let at = format!("/run/media/ada/{}", self.id);
        self.mount = Some(at.clone());
        self.mount_points_changed(&emitter).await?;
        Ok(at)
    }

    async fn unmount(
        &mut self,
        options: HashMap<String, OwnedValue>,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Fail> {
        let busy = {
            let mut state = self.state.lock().unwrap();
            state
                .log
                .push(format!("Unmount {} {}", self.id, options.len()));
            state.busy.contains(&self.id)
        };
        if busy {
            return Err(Fail::DeviceBusy("target is busy".into()));
        }
        self.mount = None;
        self.mount_points_changed(&emitter).await?;
        Ok(())
    }
}

struct Drive {
    id: String,
    vendor: &'static str,
    model: &'static str,
    ejectable: bool,
    can_power_off: bool,
    state: Shared,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Drive")]
impl Drive {
    #[zbus(property)]
    fn vendor(&self) -> String {
        self.vendor.into()
    }
    #[zbus(property)]
    fn model(&self) -> String {
        self.model.into()
    }
    #[zbus(property)]
    fn ejectable(&self) -> bool {
        self.ejectable
    }
    #[zbus(property)]
    fn can_power_off(&self) -> bool {
        self.can_power_off
    }
    fn eject(&self, options: HashMap<String, OwnedValue>) {
        let line = format!("Eject {} {}", self.id, options.len());
        self.state.lock().unwrap().log.push(line);
    }
    fn power_off(&self, options: HashMap<String, OwnedValue>) {
        let line = format!("PowerOff {} {}", self.id, options.len());
        self.state.lock().unwrap().log.push(line);
    }
}

struct Notifications(Shared);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let transient = hints
            .get("transient")
            .and_then(|v| bool::try_from(&**v).ok());
        self.0.lock().unwrap().log.push(format!(
            "Notify {app_name}: {summary} / {body} (transient {transient:?})"
        ));
        1
    }
}

/// A fake udisks2 and notification service on a private bus.
struct Fake {
    bus: PrivateBus,
    udisks: Connection,
    _notifications: Connection,
    state: Shared,
}

fn drive_path(id: &str) -> String {
    format!("{MANAGER_PATH}/drives/{id}")
}

fn block_path(id: &str) -> String {
    format!("{MANAGER_PATH}/block_devices/{id}")
}

impl Fake {
    fn start() -> Option<Fake> {
        let Some(bus) = PrivateBus::start() else {
            missing_tool("dbus-daemon");
            return None;
        };
        let state = Shared::default();
        let udisks = bus
            .blocking_builder()
            .name(UDISKS_NAME)
            .unwrap()
            .serve_at(MANAGER_PATH, zbus::fdo::ObjectManager)
            .unwrap()
            .build()
            .unwrap();
        let notifications = bus
            .blocking_builder()
            .name("org.freedesktop.Notifications")
            .unwrap()
            .serve_at(
                "/org/freedesktop/Notifications",
                Notifications(state.clone()),
            )
            .unwrap()
            .build()
            .unwrap();
        Some(Fake {
            bus,
            udisks,
            _notifications: notifications,
            state,
        })
    }

    fn drive(&self, id: &str, vendor: &'static str, model: &'static str, ejectable: bool) {
        let drive = Drive {
            id: id.into(),
            vendor,
            model,
            ejectable,
            can_power_off: true,
            state: self.state.clone(),
        };
        self.udisks
            .object_server()
            .at(drive_path(id), drive)
            .unwrap();
    }

    /// A block `id` with a file system, mounted at `mount`, on the drive `drive` (`None`: a loop device).
    fn block(&self, id: &str, drive: Option<&str>, mount: Option<&str>, block: Block) {
        let block = Block {
            drive: OwnedObjectPath::try_from(drive.map_or("/".into(), drive_path)).unwrap(),
            ..block
        };
        let server = self.udisks.object_server();
        server.at(block_path(id), block).unwrap();
        server
            .at(
                block_path(id),
                Filesystem {
                    id: id.into(),
                    mount: mount.map(str::to_string),
                    state: self.state.clone(),
                },
            )
            .unwrap();
    }

    fn remove(&self, id: &str) {
        let server = self.udisks.object_server();
        server.remove::<Filesystem, _>(block_path(id)).unwrap();
        server.remove::<Block, _>(block_path(id)).unwrap();
    }

    fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }

    fn watch(&self) -> Watch {
        let (woke, woken) = mpsc::channel();
        let woke = Mutex::new(woke);
        let volumes = Volumes::watch(
            self.bus.connect(),
            Some(self.bus.connect()),
            Arc::new(move || {
                let _ = woke.lock().unwrap().send(());
            }),
        )
        .expect("subscribing to udisks");
        Watch {
            volumes,
            woken,
            seen: Vec::new(),
        }
    }
}

struct Watch {
    volumes: Volumes,
    woken: mpsc::Receiver<()>,
    seen: Vec<Report>,
}

impl Watch {
    /// The first report not taken yet that `matches`, waiting up to 5 s.
    fn until(&mut self, matches: impl Fn(&Report) -> bool) -> Report {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            for report in self.volumes.poll() {
                self.seen.push(report);
            }
            if let Some(i) = self.seen.iter().position(&matches) {
                return self.seen.remove(i);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                self.woken.recv_timeout(left).is_ok(),
                "no matching report within 5 s; seen {:?}",
                self.seen
            );
        }
    }

    /// The devices once they are those `names`.
    fn devices(&mut self, names: &[&str]) -> Vec<Device> {
        match self.until(|r| {
            matches!(r, Report::Devices(d) if d.iter().map(|d| d.name.as_str()).eq(names.iter().copied()))
        }) {
            Report::Devices(devices) => devices,
            _ => unreachable!(),
        }
    }

    /// The devices once `name` is mounted (`true`) or not.
    fn device_once(&mut self, name: &str, mounted: bool) -> (Device, Vec<Device>) {
        let has = |d: &[Device]| {
            d.iter()
                .find(|d| d.name == name && d.mount.is_some() == mounted)
                .cloned()
        };
        match self.until(|r| matches!(r, Report::Devices(d) if has(d).is_some())) {
            Report::Devices(devices) => (has(&devices).unwrap(), devices),
            _ => unreachable!(),
        }
    }
}

fn done(object: &OwnedObjectPath) -> impl Fn(&Report) -> bool + '_ {
    move |r| matches!(r, Report::Done { object: o } if o == object)
}

#[test]
fn removable_file_systems_are_listed_by_name_and_system_hidden_and_loop_ones_are_not() {
    let Some(fake) = Fake::start() else { return };
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.drive("card", "", "", true);
    fake.drive("ssd", "Samsung", "SSD 980", false);
    let at = |hint_name, label| Block {
        hint_name,
        label,
        size: 16_000_000_000,
        ..Block::default()
    };
    fake.block("sdb1", Some("usb"), None, at("", "KINGSTON"));
    fake.block("sdb2", Some("usb"), None, at("", ""));
    fake.block(
        "sdc1",
        Some("card"),
        Some("/run/media/ada/CAM"),
        at("Camera Card", "CAM"),
    );
    fake.block("sdc2", Some("card"), None, at("", ""));
    fake.block(
        "nvme0n1p2",
        Some("ssd"),
        Some("/"),
        Block {
            system: true,
            ..at("", "root")
        },
    );
    fake.block(
        "sdb3",
        Some("usb"),
        None,
        Block {
            ignore: true,
            ..at("", "hidden")
        },
    );
    fake.block(
        "sdb4",
        Some("usb"),
        None,
        Block {
            usage: "other",
            ..at("", "swap")
        },
    );
    fake.block(
        "loop0",
        None,
        Some("/run/media/ada/disk"),
        at("", "Disk Image"),
    );
    fake.block(
        "sdd1",
        Some("card"),
        None,
        at("", "\u{202e}Spoof\ned\u{2069}"),
    );

    let mut watch = fake.watch();
    let devices = watch.devices(&[
        "16.0 GB Volume",
        "Camera Card",
        "KINGSTON",
        "Kingston DataTraveler",
        "Spoofed",
    ]);
    let card = &devices[1];
    assert_eq!(card.mount, Some(PathBuf::from("/run/media/ada/CAM")));
    assert_eq!(card.drive.as_str(), drive_path("card"));
    assert!(card.ejectable);
    assert_eq!(devices[3].mount, None);
}

#[test]
fn a_click_mounts_without_refusing_a_prompt_and_reports_where() {
    let Some(fake) = Fake::start() else { return };
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.block(
        "sdb1",
        Some("usb"),
        None,
        Block {
            label: "KINGSTON",
            ..Block::default()
        },
    );
    let mut watch = fake.watch();
    let (stick, _) = watch.device_once("KINGSTON", false);

    watch.volumes.mount(&stick);
    assert_eq!(
        watch.until(|r| matches!(r, Report::Mounted { .. })),
        Report::Mounted {
            object: stick.object.clone(),
            at: PathBuf::from("/run/media/ada/sdb1")
        }
    );
    assert_eq!(fake.log(), ["Mount sdb1 []"]);
    let (mounted, _) = watch.device_once("KINGSTON", true);
    assert_eq!(mounted.mount, Some(PathBuf::from("/run/media/ada/sdb1")));
}

#[test]
fn eject_unmounts_every_file_system_of_the_drive_then_ejects_it_or_powers_it_off() {
    let Some(fake) = Fake::start() else { return };
    fake.drive("card", "", "", true);
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.block(
        "sdc1",
        Some("card"),
        Some("/run/media/ada/a"),
        Block {
            label: "A",
            ..Block::default()
        },
    );
    fake.block(
        "sdc2",
        Some("card"),
        Some("/run/media/ada/b"),
        Block {
            label: "B",
            ..Block::default()
        },
    );
    fake.block(
        "sdb1",
        Some("usb"),
        Some("/run/media/ada/k"),
        Block {
            label: "KINGSTON",
            ..Block::default()
        },
    );
    let mut watch = fake.watch();
    let devices = watch.devices(&["A", "B", "KINGSTON"]);

    let card: Vec<Device> = devices[..2].to_vec();
    watch.volumes.eject(&devices[0], card);
    watch.until(done(&devices[0].object));
    watch.volumes.eject(&devices[2], vec![devices[2].clone()]);
    watch.until(done(&devices[2].object));
    assert_eq!(
        fake.log(),
        [
            "Unmount sdc1 0",
            "Unmount sdc2 0",
            "Eject card 0",
            "Unmount sdb1 0",
            "PowerOff usb 0"
        ]
    );
    assert!(fake.log().iter().all(|l| !l.starts_with("Notify")));
}

#[test]
fn a_busy_file_system_stops_the_eject_with_one_notification_and_keeps_what_was_unmounted() {
    let Some(fake) = Fake::start() else { return };
    fake.drive("card", "", "", true);
    fake.block(
        "sdc1",
        Some("card"),
        Some("/run/media/ada/a"),
        Block {
            label: "A",
            ..Block::default()
        },
    );
    fake.block(
        "sdc2",
        Some("card"),
        Some("/run/media/ada/b"),
        Block {
            label: "B",
            ..Block::default()
        },
    );
    fake.state.lock().unwrap().busy.insert("sdc2".into());
    let mut watch = fake.watch();
    let devices = watch.devices(&["A", "B"]);

    watch.volumes.eject(&devices[0], devices.clone());
    watch.until(done(&devices[0].object));
    assert_eq!(
        fake.log(),
        [
            "Unmount sdc1 0",
            "Unmount sdc2 0",
            "Notify Files: “B” wasn't ejected / One or more applications are using it. Quit them and try again. (transient Some(true))"
        ]
    );
    let (a, now) = watch.device_once("A", false);
    assert_eq!(a.mount, None);
    assert_eq!(
        now.iter().find(|d| d.name == "B").unwrap().mount,
        Some(PathBuf::from("/run/media/ada/b"))
    );
}

#[test]
fn a_dismissed_prompt_fails_the_mount_without_a_notification() {
    let Some(fake) = Fake::start() else { return };
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.block(
        "sdb1",
        Some("usb"),
        None,
        Block {
            label: "KINGSTON",
            ..Block::default()
        },
    );
    fake.state.lock().unwrap().refuse_mount =
        Some(Fail::NotAuthorizedDismissed("dismissed".into()));
    let mut watch = fake.watch();
    let (stick, _) = watch.device_once("KINGSTON", false);

    watch.volumes.mount(&stick);
    watch.until(done(&stick.object));
    assert_eq!(fake.log(), ["Mount sdb1 []"]);
}

#[test]
fn a_removed_device_leaves_the_list() {
    let Some(fake) = Fake::start() else { return };
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.block(
        "sdb1",
        Some("usb"),
        Some("/run/media/ada/k"),
        Block {
            label: "KINGSTON",
            ..Block::default()
        },
    );
    let mut watch = fake.watch();
    watch.devices(&["KINGSTON"]);

    fake.remove("sdb1");
    assert!(watch.devices(&[]).is_empty());
}

#[test]
fn without_udisks_on_the_bus_there_are_no_devices_until_it_comes() {
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (woke, woken) = mpsc::channel();
    let woke = Mutex::new(woke);
    let volumes = Volumes::watch(
        bus.connect(),
        None,
        Arc::new(move || {
            let _ = woke.lock().unwrap().send(());
        }),
    )
    .unwrap();
    woken.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(volumes.poll(), [Report::Devices(Vec::new())]);
    assert!(woken.recv_timeout(Duration::from_millis(200)).is_err());

    let udisks = bus
        .blocking_builder()
        .name(UDISKS_NAME)
        .unwrap()
        .serve_at(MANAGER_PATH, zbus::fdo::ObjectManager)
        .unwrap()
        .build()
        .unwrap();
    woken.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(volumes.poll(), [Report::Devices(Vec::new())]);
    drop(udisks);
}
