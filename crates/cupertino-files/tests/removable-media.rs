//! Removable media against a fake udisks2 and a fake notification service on a private bus (as system and session bus):
//! which file systems are listed, the calls a mount and an eject make in order, and what a failure reports.
#![cfg(feature = "app")]

use crate::common;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use cupertino_files::notice::Notices;
use cupertino_files::places::{window_title, PlaceKind};
use cupertino_files::udisks::{Device, Report, Volumes, MANAGER_PATH, UDISKS_NAME};
use cupertino_kit::core::Role;
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
    /// Where file systems are mounted, each in a folder named by its block; `/run/media/ada` when empty.
    mount_root: String,
    /// The passphrase that unlocks every container.
    passphrase: String,
}

type Shared = Arc<Logged>;

/// The fake's state, and what wakes a test waiting for its log: every lock of it tells the waiters once let go.
#[derive(Default)]
struct Logged {
    state: Mutex<State>,
    changed: Condvar,
}

/// A lock of [`Logged`], which tells the waiters once let go.
struct Guard<'a> {
    state: MutexGuard<'a, State>,
    changed: &'a Condvar,
}

impl std::ops::Deref for Guard<'_> {
    type Target = State;

    fn deref(&self) -> &State {
        &self.state
    }
}

impl std::ops::DerefMut for Guard<'_> {
    fn deref_mut(&mut self) -> &mut State {
        &mut self.state
    }
}

impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.changed.notify_all();
    }
}

impl Logged {
    fn lock(&self) -> Result<Guard<'_>, std::convert::Infallible> {
        Ok(Guard {
            state: self.state.lock().unwrap_or_else(PoisonError::into_inner),
            changed: &self.changed,
        })
    }

    /// The log once `done` holds of it; panics after `guard`, which only guards against a hang.
    fn log_once(&self, guard: Duration, done: impl Fn(&[String]) -> bool) -> Vec<String> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let (state, waited) = self
            .changed
            .wait_timeout_while(state, guard, |s| !done(&s.log))
            .unwrap_or_else(PoisonError::into_inner);
        assert!(!waited.timed_out(), "log {:?}", state.log);
        state.log.clone()
    }
}

#[derive(Debug, Clone, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.UDisks2.Error")]
enum Fail {
    #[zbus(error)]
    ZBus(zbus::Error),
    DeviceBusy(String),
    NotAuthorizedDismissed(String),
    Failed(String),
}

struct Block {
    usage: &'static str,
    label: &'static str,
    hint_name: &'static str,
    ignore: bool,
    system: bool,
    drive: OwnedObjectPath,
    size: u64,
    read_only: bool,
    backing: OwnedObjectPath,
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
            read_only: false,
            backing: OwnedObjectPath::try_from("/").unwrap(),
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
    #[zbus(property)]
    fn read_only(&self) -> bool {
        self.read_only
    }
    #[zbus(property)]
    fn crypto_backing_device(&self) -> OwnedObjectPath {
        self.backing.clone()
    }
}

/// An encrypted container whose cleartext block, once unlocked, is `<id>_clear` with the label `label`.
struct Encrypted {
    id: String,
    label: &'static str,
    cleartext: OwnedObjectPath,
    state: Shared,
}

#[zbus::interface(name = "org.freedesktop.UDisks2.Encrypted")]
impl Encrypted {
    #[zbus(property)]
    fn cleartext_device(&self) -> OwnedObjectPath {
        self.cleartext.clone()
    }

    async fn unlock(
        &mut self,
        passphrase: String,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<OwnedObjectPath, Fail> {
        let right = {
            let mut state = self.state.lock().unwrap();
            let right = passphrase == state.passphrase;
            state.log.push(format!("Unlock {} right={right}", self.id));
            right
        };
        if !right {
            // a service that echoes what it was sent: the window must not pass that on
            return Err(Fail::Failed(format!(
                "Error unlocking /dev/{}: wrong passphrase {passphrase}",
                self.id
            )));
        }
        let clear = format!("{}_clear", self.id);
        let block = Block {
            label: self.label,
            backing: OwnedObjectPath::try_from(block_path(&self.id)).unwrap(),
            ..Block::default()
        };
        server.at(block_path(&clear), block).await?;
        let filesystem = Filesystem {
            id: clear.clone(),
            mount: None,
            state: self.state.clone(),
        };
        server.at(block_path(&clear), filesystem).await?;
        self.cleartext = OwnedObjectPath::try_from(block_path(&clear)).unwrap();
        self.cleartext_device_changed(&emitter).await?;
        Ok(self.cleartext.clone())
    }

    async fn lock(
        &mut self,
        _options: HashMap<String, OwnedValue>,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        #[zbus(signal_emitter)] emitter: SignalEmitter<'_>,
    ) -> Result<(), Fail> {
        self.state
            .lock()
            .unwrap()
            .log
            .push(format!("Lock {}", self.id));
        let clear = block_path(&format!("{}_clear", self.id));
        server.remove::<Filesystem, _>(clear.as_str()).await?;
        server.remove::<Block, _>(clear.as_str()).await?;
        self.cleartext = OwnedObjectPath::try_from("/").unwrap();
        self.cleartext_device_changed(&emitter).await?;
        Ok(())
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
        let root = self.state.lock().unwrap().mount_root.clone();
        let root = if root.is_empty() {
            "/run/media/ada".to_owned()
        } else {
            root
        };
        let at = format!("{root}/{}", self.id);
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

    /// A locked encrypted container `id` on the drive `drive`, which holds a file system labelled `label`.
    fn encrypted(&self, id: &str, drive: &str, label: &'static str) {
        let block = Block {
            usage: "crypto",
            drive: OwnedObjectPath::try_from(drive_path(drive)).unwrap(),
            size: 8_000_000_000,
            ..Block::default()
        };
        let server = self.udisks.object_server();
        server.at(block_path(id), block).unwrap();
        let encrypted = Encrypted {
            id: id.into(),
            label,
            cleartext: OwnedObjectPath::try_from("/").unwrap(),
            state: self.state.clone(),
        };
        server.at(block_path(id), encrypted).unwrap();
    }

    fn remove(&self, id: &str) {
        let server = self.udisks.object_server();
        server.remove::<Filesystem, _>(block_path(id)).unwrap();
        server.remove::<Block, _>(block_path(id)).unwrap();
    }

    fn log(&self) -> Vec<String> {
        self.state.lock().unwrap().log.clone()
    }

    /// The log once a notification came: notices are posted on a thread of their own. The five seconds only guard
    /// against a hang.
    fn log_once_notified(&self) -> Vec<String> {
        self.state.log_once(Duration::from_secs(5), |log| {
            log.iter().any(|l| l.starts_with("Notify"))
        })
    }

    fn watch(&self) -> Watch {
        self.watch_on(self.bus.connect())
    }

    /// [`Fake::watch`] on `system`.
    fn watch_on(&self, system: Connection) -> Watch {
        let (woke, woken) = mpsc::channel();
        let woke = Mutex::new(woke);
        let volumes = Volumes::watch(
            system,
            Notices::start(Some(self.bus.connect())),
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
fn eject_from_the_window_on_a_card_goes_home_and_ejects_its_drive() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let card = home.path().join("CARD");
    std::fs::create_dir(&card).unwrap();
    fake.drive("card", "", "", true);
    fake.block(
        "sdc1",
        Some("card"),
        Some(card.to_str().unwrap()),
        Block {
            label: "CARD",
            ..Block::default()
        },
    );
    let documents = common::place("Documents", home.path(), PlaceKind::Documents);
    let mut run = common::Run::start(
        home,
        card.clone(),
        vec![documents],
        Notices::start(None),
        Some(fake.bus.connect()),
    );
    run.wait_until(|run| row(run, "CARD", "Removable"));
    assert!(!run.disabled().contains(&common::action("eject")));

    run.menu("eject");
    wait_for_log(&fake, 2);
    assert_eq!(fake.log(), ["Unmount sdc1 0", "Eject card 0"]);
    assert_eq!(run.title(), window_title(&run.home(), &[]));
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
        fake.log_once_notified(),
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
fn a_flood_of_changes_another_connection_sends_does_not_stop_the_list() {
    let Some(fake) = Fake::start() else { return };
    let system = fake.bus.connect();
    let to = system.unique_name().unwrap().to_string();
    let mut watch = fake.watch_on(system);
    watch.devices(&[]);

    // another user's program on the system bus, to this process directly, many times what a stream holds
    let peer = fake.bus.connect();
    let removed = (
        OwnedObjectPath::try_from(block_path("sda1")).unwrap(),
        Vec::<String>::new(),
    );
    for _ in 0..1000 {
        peer.emit_signal(
            Some(to.as_str()),
            MANAGER_PATH,
            "org.freedesktop.DBus.ObjectManager",
            "InterfacesRemoved",
            &removed,
        )
        .unwrap();
    }
    // udisks is still heard, and its devices still read
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
    watch.devices(&["KINGSTON"]);
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
        Notices::start(None),
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

/// A Kit window on `folder` of `home`, with Documents (`home`) in the sidebar and the fake's removable media.
fn kit(fake: &Fake, home: tempfile::TempDir, folder: PathBuf) -> common::Run {
    let documents = common::place("Documents", home.path(), PlaceKind::Documents);
    common::Run::start(
        home,
        folder,
        vec![documents],
        Notices::start(Some(fake.bus.connect())),
        Some(fake.bus.connect()),
    )
}

/// Whether the sidebar has a row named `name` described as `description`.
fn row(run: &common::Run, name: &str, description: &str) -> bool {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    snapshot.accessibility.iter().any(|(_, node)| {
        node.role == Role::ListItem
            && node.name.as_deref() == Some(name)
            && node.description.as_deref() == Some(description)
    })
}

/// The fake's log once it holds `count` lines; the five seconds only guard against a hang.
fn wait_for_log(fake: &Fake, count: usize) -> Vec<String> {
    fake.state
        .log_once(Duration::from_secs(5), |log| log.len() >= count)
}

#[test]
fn a_click_on_a_device_mounts_it_and_takes_the_window_that_asked_there() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let media = home.path().join("media");
    std::fs::create_dir_all(media.join("sdb1")).unwrap();
    std::fs::write(media.join("sdb1/photo.jpg"), "").unwrap();
    fake.state.lock().unwrap().mount_root = media.to_string_lossy().into_owned();
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
    let folder = home.path().to_path_buf();
    let mut run = kit(&fake, home, folder);
    run.wait_until(|run| row(run, "KINGSTON", "Removable, not mounted"));
    let first = run.window;
    run.menu("new");
    let second = run.windows()[1];
    run.activate(second);
    run.wait_until(|run| row(run, "KINGSTON", "Removable, not mounted"));

    run.click_named(Role::ListItem, "KINGSTON");
    assert_eq!(wait_for_log(&fake, 1), ["Mount sdb1 []"]);
    run.wait_until(|run| run.title() == "sdb1");
    run.wait_until(|run| run.rows() == ["photo.jpg"]);
    assert!(row(&run, "KINGSTON", "Removable"));
    assert_eq!(
        run.named(Role::Button)
            .iter()
            .filter(|b| *b == "Eject KINGSTON")
            .count(),
        1
    );
    run.driver.look(run.window, "files-device-mounted").unwrap();

    // the other window stayed where it was
    run.activate(first);
    assert_ne!(run.title(), "sdb1");
}

#[test]
fn the_eject_button_unmounts_every_partition_and_the_window_goes_home() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let (a, b) = (home.path().join("A"), home.path().join("B"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    fake.drive("card", "", "", true);
    let labelled = |label| Block {
        label,
        ..Block::default()
    };
    fake.block(
        "sdc1",
        Some("card"),
        Some(a.to_str().unwrap()),
        labelled("A"),
    );
    fake.block(
        "sdc2",
        Some("card"),
        Some(b.to_str().unwrap()),
        labelled("B"),
    );
    let mut run = kit(&fake, home, b.clone());
    run.wait_until(|run| row(run, "A", "Removable") && row(run, "B", "Removable"));
    assert_eq!(run.title(), "B");

    run.click_named(Role::Button, "Eject A");
    assert_eq!(
        wait_for_log(&fake, 3),
        ["Unmount sdc1 0", "Unmount sdc2 0", "Eject card 0"]
    );
    assert_eq!(run.title(), window_title(&run.home(), &[]), "off the card");
}

/// Primary+E ejects the device of the sidebar row the keyboard is on, here the card picked in the sidebar while the
/// window had shown the stick.
#[test]
fn primary_e_ejects_the_device_of_the_sidebar_row_the_keyboard_is_on() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let (a, b) = (home.path().join("A"), home.path().join("B"));
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    fake.drive("card", "", "", true);
    fake.drive("stick", "", "", false);
    let labelled = |label| Block {
        label,
        ..Block::default()
    };
    fake.block(
        "sdc1",
        Some("card"),
        Some(a.to_str().unwrap()),
        labelled("A"),
    );
    fake.block(
        "sdd1",
        Some("stick"),
        Some(b.to_str().unwrap()),
        labelled("B"),
    );
    let mut run = kit(&fake, home, b.clone());
    run.wait_until(|run| row(run, "A", "Removable") && row(run, "B", "Removable"));
    assert_eq!(run.title(), "B");

    run.click_named(Role::ListItem, "A");
    assert_eq!(run.title(), "A");
    run.key_with(0x65, Some('e'), cupertino_kit::platform::Mods::PRIMARY);
    assert_eq!(wait_for_log(&fake, 2), ["Unmount sdc1 0", "Eject card 0"]);
    assert_eq!(run.title(), window_title(&run.home(), &[]), "off the card");
}

/// Primary+E leaves a drive that also holds the home folder, as a portable disk the session runs from: its eject would
/// unmount the home folder with it. Another drive still ejects.
#[test]
fn primary_e_never_ejects_the_drive_that_holds_the_home_folder() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let (data, b) = (home.path().join("Data"), home.path().join("B"));
    std::fs::create_dir_all(&data).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    fake.drive("portable", "", "", true);
    fake.drive("stick", "", "", true);
    let labelled = |label| Block {
        label,
        ..Block::default()
    };
    fake.block(
        "sde1",
        Some("portable"),
        Some(home.path().to_str().unwrap()),
        labelled("Home"),
    );
    fake.block(
        "sde2",
        Some("portable"),
        Some(data.to_str().unwrap()),
        labelled("Data"),
    );
    fake.block(
        "sdd1",
        Some("stick"),
        Some(b.to_str().unwrap()),
        labelled("B"),
    );
    let mut run = kit(&fake, home, data.clone());
    run.wait_until(|run| row(run, "Data", "Removable") && row(run, "B", "Removable"));
    assert_eq!(run.title(), "Data");
    run.key_with(0x65, Some('e'), cupertino_kit::platform::Mods::PRIMARY);
    assert_eq!(
        run.title(),
        "Data",
        "the window stays on the home folder's drive"
    );

    run.click_named(Role::ListItem, "B");
    run.key_with(0x65, Some('e'), cupertino_kit::platform::Mods::PRIMARY);
    // the log is in order: the stick's eject is the first thing the fake was asked
    assert_eq!(wait_for_log(&fake, 2), ["Unmount sdd1 0", "Eject stick 0"]);
}

/// A read-only medium takes nothing new: no new folder, nothing moved off it, and no paste though a copied file waits
/// on the pasteboard.
#[test]
fn a_read_only_medium_is_described_so_and_takes_nothing_new() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let disc = home.path().join("DISC");
    std::fs::create_dir_all(&disc).unwrap();
    std::fs::write(home.path().join("x.txt"), "x").unwrap();
    let folder = home.path().to_path_buf();
    fake.drive("optical", "", "", true);
    fake.block(
        "sr0",
        Some("optical"),
        Some(disc.to_str().unwrap()),
        Block {
            label: "DISC",
            read_only: true,
            ..Block::default()
        },
    );
    let mut run = kit(&fake, home, folder);
    run.wait_until(|run| {
        row(run, "DISC", "Removable, read-only") && run.rows().contains(&"x.txt".to_owned())
    });
    run.click_row("x.txt");
    run.menu("copy");
    assert!(
        !run.disabled().contains(&common::action("paste")),
        "a copied file pastes at home"
    );

    run.click_named(Role::ListItem, "DISC");
    assert_eq!(run.title(), "DISC");
    let disabled = run.disabled();
    for name in ["new-folder", "move-to-trash", "duplicate", "paste"] {
        assert!(disabled.contains(&common::action(name)), "{name}");
    }
}

#[test]
fn a_device_pulled_out_leaves_the_sidebar_and_its_window_goes_home_still_answering() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let stick = home.path().join("STICK");
    std::fs::create_dir_all(stick.join("Photos")).unwrap();
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.block(
        "sdb1",
        Some("usb"),
        Some(stick.to_str().unwrap()),
        Block {
            label: "STICK",
            ..Block::default()
        },
    );
    let mut run = kit(&fake, home, stick.clone());
    run.wait_until(|run| row(run, "STICK", "Removable") && run.rows() == ["Photos"]);

    // no eject: the device just goes
    fake.remove("sdb1");
    run.wait_until(|run| !run.named(Role::ListItem).contains(&"STICK".to_owned()));
    assert_eq!(run.title(), window_title(&run.home(), &[]));
    assert!(run
        .named(Role::Button)
        .iter()
        .all(|b| !b.starts_with("Eject")));
    // the window still answers
    run.menu("toggle-status-bar");
    assert!(run
        .named(Role::StaticText)
        .iter()
        .any(|t| t.ends_with("items") || t.contains("item")));
    assert!(
        fake.log().is_empty(),
        "nothing was asked of udisks: {:?}",
        fake.log()
    );
}

#[test]
fn a_locked_container_asks_for_its_passphrase_and_never_shows_or_reports_it() {
    const WRONG: &str = "hunter2-wrong";
    const RIGHT: &str = "correct horse";
    const RETURN: u32 = 0xff0d;
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let media = home.path().join("media");
    std::fs::create_dir_all(media.join("sdb1_clear")).unwrap();
    {
        let mut state = fake.state.lock().unwrap();
        state.mount_root = media.to_string_lossy().into_owned();
        state.passphrase = RIGHT.into();
    }
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.encrypted("sdb1", "usb", "Secrets");
    let folder = home.path().to_path_buf();
    let mut run = kit(&fake, home, folder);
    run.wait_until(|run| row(run, "Kingston DataTraveler", "Removable, locked"));

    run.click_named(Role::ListItem, "Kingston DataTraveler");
    run.wait_until(|run| run.named(Role::PasswordInput) == ["Passphrase"]);
    run.driver.look(run.window, "files-unlock-sheet").unwrap();
    run.click("passphrase");
    run.type_text(WRONG);
    for text in run.read_aloud() {
        assert!(!text.contains(WRONG), "assistive technology reads {text:?}");
    }
    run.key(RETURN, None);
    let log = fake.log_once_notified();
    assert_eq!(log[0], "Unlock sdb1 right=false");
    assert!(
        log[1].starts_with(
            "Notify Files: “Kingston DataTraveler” couldn't be unlocked / The call failed."
        ),
        "{log:?}"
    );
    assert!(
        log.iter().all(|l| !l.contains(WRONG)),
        "the passphrase leaked: {log:?}"
    );
    run.wait_until(|run| run.named(Role::PasswordInput).is_empty());
    run.wait_until(|run| row(run, "Kingston DataTraveler", "Removable, locked"));

    // again, with the right one: unlocked, mounted and shown
    run.click_named(Role::ListItem, "Kingston DataTraveler");
    run.wait_until(|run| run.named(Role::PasswordInput) == ["Passphrase"]);
    run.click("passphrase");
    run.type_text(RIGHT);
    run.click_named(Role::Button, "Unlock");
    run.wait_until(|run| run.title() == "sdb1_clear");
    run.wait_until(|run| row(run, "Secrets", "Removable"));
    assert!(!run
        .named(Role::ListItem)
        .contains(&"Kingston DataTraveler".to_owned()));
    for text in run.read_aloud() {
        assert!(!text.contains(RIGHT), "assistive technology reads {text:?}");
    }

    // an eject unmounts, locks the container again and ejects the drive
    run.click_named(Role::Button, "Eject Secrets");
    let log = wait_for_log(&fake, 7);
    assert_eq!(
        log[2..],
        [
            "Unlock sdb1 right=true",
            "Mount sdb1_clear []",
            "Unmount sdb1_clear 0",
            "Lock sdb1",
            "PowerOff usb 0",
        ][..]
    );
    assert!(log.iter().all(|l| !l.contains(RIGHT)));
    run.wait_until(|run| row(run, "Kingston DataTraveler", "Removable, locked"));
}

#[test]
fn cancel_closes_the_passphrase_sheet_without_a_call() {
    const ESCAPE: u32 = 0xff1b;
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    fake.drive("usb", "Kingston", "DataTraveler", false);
    fake.encrypted("sdb1", "usb", "Secrets");
    let folder = home.path().to_path_buf();
    let mut run = kit(&fake, home, folder);
    run.wait_until(|run| row(run, "Kingston DataTraveler", "Removable, locked"));
    run.click_named(Role::ListItem, "Kingston DataTraveler");
    run.wait_until(|run| run.named(Role::PasswordInput) == ["Passphrase"]);
    run.key(ESCAPE, None);
    run.wait_until(|run| run.named(Role::PasswordInput).is_empty());
    run.click_named(Role::ListItem, "Kingston DataTraveler");
    run.wait_until(|run| run.named(Role::PasswordInput) == ["Passphrase"]);
    run.click_named(Role::Button, "Cancel");
    run.wait_until(|run| run.named(Role::PasswordInput).is_empty());
    // a call starts on the window's thread, marking its device busy until its end is taken in: none did
    assert!(!busy(&run, "Kingston DataTraveler"), "a call started");
    assert!(fake.log().is_empty(), "{:?}", fake.log());
}

/// Whether the sidebar's row named `name` is busy.
fn busy(run: &common::Run, name: &str) -> bool {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    snapshot.accessibility.iter().any(|(_, node)| {
        node.role == Role::ListItem && node.name.as_deref() == Some(name) && node.states.busy
    })
}

#[test]
fn a_busy_file_system_names_the_program_that_keeps_it() {
    let Some(fake) = Fake::start() else { return };
    let home = tempfile::tempdir().unwrap();
    let card = home.path().join("CARD");
    std::fs::create_dir_all(&card).unwrap();
    // a program whose working folder is on the card
    let mut holder = std::process::Command::new("sleep")
        .arg("30")
        .current_dir(&card)
        .spawn()
        .unwrap();
    fake.drive("card", "", "", true);
    fake.block(
        "sdc1",
        Some("card"),
        Some(card.to_str().unwrap()),
        Block {
            label: "CARD",
            ..Block::default()
        },
    );
    fake.state.lock().unwrap().busy.insert("sdc1".into());
    let mut watch = fake.watch();
    let devices = watch.devices(&["CARD"]);
    watch.volumes.eject(&devices[0], devices.clone());
    let log = fake.log_once_notified();
    holder.kill().unwrap();
    let _ = holder.wait();
    assert_eq!(
        log,
        [
            "Unmount sdc1 0",
            "Notify Files: “CARD” wasn't ejected / “sleep” is using it. Quit it and try again. (transient Some(true))"
        ]
    );
}

/// Set in the copy of the test binary that plays the window read over AT-SPI.
const ATSPI_WINDOW: &str = "CUPERTINO_TEST_FILES_ATSPI_WINDOW";
const ATSPI_ROOT: &str = "/org/a11y/atspi/accessible/root";
/// AT-SPI's role numbers (`AtspiRole`) and state bits (`AtspiStateType`).
const ATSPI_LIST: u32 = 31;
const ATSPI_LIST_ITEM: u32 = 32;
const ATSPI_PUSH_BUTTON: u32 = 43;
const ATSPI_SELECTED: u32 = 23;

struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The window read over AT-SPI: Documents shown, a mounted card `A` in the sidebar. Once the card's eject reached the
/// fake it waits for the test to close its input and exits successfully; it fails when no eject arrives.
fn atspi_window() {
    use std::io::Read;
    let fake = Fake::start().expect("a private bus for the window's udisks");
    let home = tempfile::tempdir().unwrap();
    let card = home.path().join("A");
    std::fs::create_dir_all(&card).unwrap();
    fake.drive("card", "", "", true);
    fake.block(
        "sdc1",
        Some("card"),
        Some(card.to_str().unwrap()),
        Block {
            label: "A",
            ..Block::default()
        },
    );
    let documents = common::place("Documents", home.path(), PlaceKind::Documents);
    let folder = home.path().to_path_buf();
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let host = common::Host {
        accessibility: Some(Box::new(cupertino_ui_atspi::AtspiAdapter::new(waker))),
        ..common::Host::default()
    };
    let preferences = cupertino_kit::theme::ThemePreferences {
        appearance: cupertino_kit::theme::Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = common::Run::start_hosted(
        home,
        folder,
        vec![documents],
        Notices::start(None),
        Some(fake.bus.connect()),
        preferences,
        |start| start,
        host,
    );
    run.send(cupertino_kit::platform::WindowEvent::Focus(true));
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        run.driver
            .window_system_mut()
            .enqueue(cupertino_kit::platform::PlatformEvent::Wake)
            .unwrap();
        run.settle();
        if fake.log().iter().any(|line| line.starts_with("Eject card")) {
            std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
            return;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    panic!(
        "the eject button's click never reached udisks: {:?}",
        fake.log()
    );
}

struct A11yStatus;

#[zbus::interface(name = "org.a11y.Status")]
impl A11yStatus {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
    }
}

/// The registry: it records which bus name embedded its application root.
struct Socket(mpsc::Sender<String>);

#[zbus::interface(name = "org.a11y.atspi.Socket")]
impl Socket {
    fn embed(
        &self,
        plug: (String, OwnedObjectPath),
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> (String, OwnedObjectPath) {
        let _ = self.0.send(plug.0);
        let registry = conn
            .unique_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        (registry, OwnedObjectPath::try_from(ATSPI_ROOT).unwrap())
    }
}

fn accessible<'a>(conn: &Connection, peer: &str, path: &str) -> zbus::blocking::Proxy<'a> {
    zbus::blocking::Proxy::new(
        conn,
        peer.to_owned(),
        path.to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap()
}

/// The nodes under `path`, depth first, with their role and name.
fn descendants(conn: &Connection, peer: &str, path: &str) -> Vec<(OwnedObjectPath, u32, String)> {
    let found: zbus::Result<Vec<(String, OwnedObjectPath)>> =
        accessible(conn, peer, path).call("GetChildren", &());
    let mut out = Vec::new();
    for (_, child) in found.unwrap_or_default() {
        let node = accessible(conn, peer, child.as_str());
        // a node that went between the two calls, as the media arrive, is not there yet for the polling caller
        let role: zbus::Result<u32> = node.call("GetRole", &());
        let Ok(role) = role else {
            continue;
        };
        let name: String = node.get_property("Name").unwrap_or_default();
        out.push((child.clone(), role, name));
        out.extend(descendants(conn, peer, child.as_str()));
    }
    out
}

/// An assistive technology reads the window's sidebar over AT-SPI as a list of its places by name, the folder shown
/// selected and the card described as removable, and the card's eject button by its name; that button's click ejects
/// the card.
#[test]
fn assistive_technology_reads_the_sidebar_over_atspi_and_its_eject_button_ejects() {
    const NAME: &str =
        "assistive_technology_reads_the_sidebar_over_atspi_and_its_eject_button_ejects";
    if std::env::var_os(ATSPI_WINDOW).is_some() {
        return atspi_window();
    }
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (embedded, embeds) = mpsc::channel();
    let _status = bus
        .blocking_builder()
        .name("org.a11y.Bus")
        .unwrap()
        .serve_at("/org/a11y/bus", A11yStatus)
        .unwrap()
        .build()
        .unwrap();
    let _registry = bus
        .blocking_builder()
        .name("org.a11y.atspi.Registry")
        .unwrap()
        .serve_at(ATSPI_ROOT, Socket(embedded))
        .unwrap()
        .build()
        .unwrap();
    let mut window = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            cupertino_test_bus::test_path(NAME).as_str(),
            "--exact",
            "--nocapture",
        ])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(ATSPI_WINDOW, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let peer = embeds
        .recv_timeout(Duration::from_secs(20))
        .expect("the window never embedded its root");
    let conn = bus.connect();
    let deadline = Instant::now() + Duration::from_secs(20);
    let (items, eject) = loop {
        let nodes = descendants(&conn, &peer, ATSPI_ROOT);
        let sidebar = nodes
            .iter()
            .find(|(_, role, name)| *role == ATSPI_LIST && name == "Sidebar")
            .map(|(path, _, _)| path.clone());
        let eject = nodes
            .iter()
            .find(|(_, role, name)| *role == ATSPI_PUSH_BUTTON && name == "Eject A")
            .map(|(path, _, _)| path.clone());
        if let (Some(sidebar), Some(eject)) = (sidebar, eject) {
            let items: Vec<(OwnedObjectPath, String)> = descendants(&conn, &peer, sidebar.as_str())
                .into_iter()
                .filter(|(_, role, _)| *role == ATSPI_LIST_ITEM)
                .map(|(path, _, name)| (path, name))
                .collect();
            if items.iter().any(|(_, name)| name == "A") {
                break (items, eject);
            }
        }
        assert!(
            Instant::now() < deadline,
            "no sidebar with the card appeared"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let names: Vec<&str> = items.iter().map(|(_, name)| name.as_str()).collect();
    assert_eq!(names, ["Documents", "A"], "the sidebar's rows by name");
    let read = |path: &OwnedObjectPath| {
        let node = accessible(&conn, &peer, path.as_str());
        let state: Vec<u32> = node.call("GetState", &()).unwrap();
        let description: String = node.get_property("Description").unwrap_or_default();
        (state[0] & (1 << ATSPI_SELECTED) != 0, description)
    };
    assert_eq!(read(&items[0].0), (true, String::new()), "the folder shown");
    assert_eq!(read(&items[1].0), (false, "Removable".to_owned()));
    let action = zbus::blocking::Proxy::new(
        &conn,
        peer.clone(),
        eject.as_str().to_owned(),
        "org.a11y.atspi.Action",
    )
    .unwrap();
    let name: String = action.call("GetName", &(0i32,)).unwrap();
    assert_eq!(name, "click");
    let _: bool = action.call("DoAction", &(0i32,)).unwrap();
    drop(window.stdin.take());
    assert!(
        window.wait().unwrap().success(),
        "the eject button's click did not eject the card"
    );
}
