//! The portal's dialogs (`cupertino-shell-dialogs`) as `cupertino-shell` runs them, against the real server,
//! headless, on a session bus of the test's own and in a runtime directory and a home of its own
//! (`docs/architecture/shell-host.md`, D16, D20, D21). The server starts this test binary twice over: as
//! `cupertino-shell`, which with [`host::MARK`] among its arguments runs the host with the test dock of
//! `shell/components.rs` and the real dialogs component (`cupertino_shell::DIALOGS`); and as the portal, hard-linked
//! as `cupertino-portal` under an install root of the test's own ([`as_portal`]), which serves the real portal's
//! `FileChooser` and `AppChooser` and asks the dialogs through `cupertino_portal::dialogs`. The test is
//! xdg-desktop-portal: it owns `org.freedesktop.portal.Desktop` and calls the portal.
//! - A portal `OpenFile` shows the open panel; the keys pick a file and the call answers its URI. `SaveFile` takes the
//!   typed name, and `ChooseApplication` the app chosen. Escape cancels.
//! - A caller of `org.cupertino.Dialogs1` other than the portal is refused and shown nothing.
//! - A panic injected as it paints, as its loop dispatches and on a helper thread restarts the component alone, fifty
//!   times, while the dock keeps drawing and the host's anonymous memory stays flat; a dialog open when it fails ends
//!   its call with nothing chosen, which is what a restart loses, and the last instance shows dialogs again.

#[path = "shell/components.rs"]
#[allow(dead_code)]
mod components;
#[path = "shell/memory.rs"]
mod memory;
// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{mpsc, Once};
use std::time::{Duration, Instant};

use cupertino_portal::app_chooser::AppChooser;
use cupertino_portal::dialogs::Dialogs;
use cupertino_portal::file_chooser::FileChooser;
use cupertino_portal::{BUS_NAME as PORTAL_NAME, PORTAL_PATH};
use cupertino_server::config::BuiltIn;
use cupertino_server::host::HostKind;
use cupertino_server::quit::start_session_service_on;
use cupertino_shell_dialogs::service::{BUS_NAME, INTERFACE, OBJECT_PATH};
use wayland_client::protocol::wl_registry;
use zbus::blocking::Connection;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use components::{BLUE, DOCK_SIZE, RED};
use memory::{steady_allocator, HostMemory, CRASHES_IN_A_ROW, RESTARTS};
use rig::{count, Area, Rig, OUTPUT_SIZE};

/// The host program of this test: `cupertino-shell`'s own (`cupertino_shell::run`) with the test dock and the real
/// dialogs component.
mod host {
    use std::path::PathBuf;

    use cupertino_kit_shell::Entry;

    use super::components::{report_to, DOCK};

    /// The argument that makes this test binary the host: a filter after `--` that names no test.
    pub const MARK: &str = "cupertino-shell-dialogs-host";

    /// The components this host was built with.
    static ENTRIES: [&Entry; 2] = [&DOCK, &cupertino_shell::DIALOGS];

    /// The report file and the host's own arguments that follow [`MARK`], when this process is the host.
    pub fn arguments() -> Option<(PathBuf, Vec<String>)> {
        let args: Vec<String> = std::env::args().collect();
        let at = args.iter().position(|a| a == MARK)?;
        let report = PathBuf::from(args.get(at + 1)?);
        Some((report, args[at + 2..].to_vec()))
    }

    /// Be the host, the dock reporting to `report`.
    pub fn run(report: PathBuf, args: Vec<String>) -> ! {
        report_to(report);
        cupertino_shell::run(&ENTRIES, args)
    }
}

/// The file name the portal stand-in is linked as, which the supervisor starts it by.
const PORTAL_FILE: &str = "cupertino-portal";
/// The name xdg-desktop-portal owns, which the portal takes calls from.
const FRONTEND: &str = "org.freedesktop.portal.Desktop";
/// The file the open panel picks, alone in its folder, and the name a save is given.
const PICKED: &str = "report.txt";
const TYPED: &str = "Minutes";
/// The applications the app chooser offers, the app's last choice, and the activation token the app sends.
const APPS: [&str; 3] = [
    "org.example.Viewer",
    "org.example.Writer",
    "org.example.Editor",
];
const LAST_CHOICE: &str = "org.example.Writer";
const TOKEN: &str = "token-of-the-app";
/// Pixels above the dock a dialog's window changes at least: a fraction of its 720 x 460 points.
const DIALOG_PIXELS: usize = 40_000;
/// How far a channel may move before a pixel counts as changed, out of 255.
const PIXEL_TOLERANCE: i32 = 8;
/// How long the test lets a window's look settle between two looks at it.
const LOOK: Duration = Duration::from_millis(300);

/// The host itself, when the supervisor started this binary with [`host::MARK`]; a test run does nothing here.
#[test]
fn host_program() {
    as_portal();
    if let Some((report, args)) = host::arguments() {
        host::run(report, args);
    }
}

#[derive(Default)]
struct Plain;

impl wayland_client::Dispatch<wl_registry::WlRegistry, ()> for Plain {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &(),
        _: &wayland_client::Connection,
        _: &wayland_client::QueueHandle<Self>,
    ) {
    }
}

/// In the portal stand-in, be the portal: serve the real `FileChooser` and `AppChooser`, which ask the dialogs, for
/// as long as the server's connection lasts; then end the process.
fn as_portal() {
    let exe = std::env::current_exe().expect("the test binary");
    if exe.file_name().is_none_or(|name| name != PORTAL_FILE) {
        return;
    }
    static SERVED: Once = Once::new();
    SERVED.call_once(|| {
        let bus = Connection::session().expect("the test's bus");
        let dialogs = Dialogs::new(bus.inner().clone());
        let files = dialogs.clone();
        bus.object_server()
            .at(
                PORTAL_PATH,
                FileChooser::new(move |event| files.file(event)),
            )
            .expect("the file chooser is served");
        bus.object_server()
            .at(
                PORTAL_PATH,
                AppChooser::new(Vec::new(), move |event| dialogs.app(event)),
            )
            .expect("the app chooser is served");
        bus.request_name(PORTAL_NAME).expect("the portal's name");
        let conn =
            wayland_client::Connection::connect_to_env().expect("started with a WAYLAND_SOCKET");
        let mut queue = conn.new_event_queue::<Plain>();
        let _registry = conn.display().get_registry(&queue.handle(), ());
        while queue.blocking_dispatch(&mut Plain).is_ok() {}
    });
    std::process::exit(0);
}

/// An install root of the test's own whose `bin/cupertino-portal` is this binary, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new() -> Root {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("shell-dialogs-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        for dir in [&root, &bin] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let portal = bin.join(PORTAL_FILE);
        std::fs::hard_link(std::env::current_exe().unwrap(), &portal).unwrap();
        std::fs::set_permissions(&portal, std::fs::Permissions::from_mode(0o755)).unwrap();
        Root(root)
    }

    fn portal(&self) -> PathBuf {
        self.0.join("bin").join(PORTAL_FILE)
    }
}

impl Drop for Root {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A session bus of the test's own, whose address the server's programs inherit for both their buses.
struct Bus(Child, String);

/// A bus that starts no service: nothing the programs call reaches a program of the machine.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

impl Bus {
    fn start() -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-dialogs-bus.conf");
        std::fs::write(&config, BUS_CONFIG).expect("the bus's configuration");
        let mut daemon = Process::new("dbus-daemon")
            .arg(format!("--config-file={}", config.display()))
            .args(["--nofork", "--print-address=1"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("dbus-daemon runs");
        let mut address = String::new();
        BufReader::new(daemon.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let address = address.trim().to_owned();
        assert!(!address.is_empty(), "dbus-daemon gives its address");
        std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &address);
        std::env::set_var("DBUS_SYSTEM_BUS_ADDRESS", &address);
        Bus(daemon, address)
    }

    /// A connection of the test's own, which owns `name` when one is given.
    fn connect(&self, name: Option<&str>) -> Connection {
        let mut builder =
            zbus::blocking::connection::Builder::address(self.1.as_str()).expect("the test's bus");
        if let Some(name) = name {
            builder = builder.name(name).expect("a well-known name");
        }
        builder.build().expect("a connection to the test's bus")
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A directory of the test's own, new and empty, of mode 0700.
fn private_dir(name: &str) -> PathBuf {
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("shell-dialogs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(&dir)
        .expect("a private directory");
    dir
}

/// A home of the test's own, never the person's: its settings and data, and the folder the dialogs open on, with
/// [`PICKED`] alone in it.
fn private_home() -> PathBuf {
    let home = private_dir("home");
    for (variable, dir) in [
        ("XDG_CONFIG_HOME", ".config"),
        ("XDG_DATA_HOME", ".local/share"),
        ("XDG_STATE_HOME", ".local/state"),
        ("XDG_CACHE_HOME", ".cache"),
    ] {
        let dir = home.join(dir);
        std::fs::create_dir_all(&dir).expect("a folder of the home");
        std::env::set_var(variable, dir);
    }
    std::env::set_var("HOME", &home);
    let documents = home.join("Documents");
    std::fs::create_dir_all(&documents).expect("the folder the dialogs open on");
    std::fs::write(documents.join(PICKED), b"minutes").expect("the file to pick");
    documents
}

/// What a call of the portal answers: the response and the results.
type Answer = zbus::Result<(u32, HashMap<String, OwnedValue>)>;

/// Call `method` of the portal's `interface` with `body` as the frontend; the answer comes on the channel.
fn call<B>(
    frontend: &Connection,
    interface: &'static str,
    method: &'static str,
    body: B,
) -> mpsc::Receiver<Answer>
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let frontend = frontend.clone();
    let (done, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let interface = format!("org.freedesktop.impl.portal.{interface}");
        let result = frontend
            .call_method(
                Some(PORTAL_NAME),
                PORTAL_PATH,
                Some(interface.as_str()),
                method,
                &body,
            )
            .and_then(|reply| reply.body().deserialize());
        let _ = done.send(result);
    });
    answer
}

/// The frontend's `OpenFile` or `SaveFile` (`method`) for request `n`, opening on `folder`.
fn choose_file(
    frontend: &Connection,
    method: &'static str,
    n: u32,
    folder: &Path,
) -> mpsc::Receiver<Answer> {
    let handle = ObjectPath::try_from(format!("/org/freedesktop/portal/desktop/request/1_1/t{n}"))
        .unwrap()
        .into_owned();
    let mut bytes = folder.as_os_str().as_encoded_bytes().to_vec();
    bytes.push(0);
    let mut options: HashMap<&str, Value<'static>> =
        HashMap::from([("current_folder", Value::from(bytes))]);
    if method == "SaveFile" {
        options.insert("current_name", Value::from("Untitled.txt"));
    }
    call(
        frontend,
        "FileChooser",
        method,
        (handle, "org.example.Editor", "", "Choose", options),
    )
}

/// The frontend's `ChooseApplication` for request `n`: [`APPS`], the app's [`LAST_CHOICE`] and its [`TOKEN`].
fn choose_application(frontend: &Connection, n: u32) -> mpsc::Receiver<Answer> {
    let handle = ObjectPath::try_from(format!("/org/freedesktop/portal/desktop/request/1_1/t{n}"))
        .unwrap()
        .into_owned();
    let options: HashMap<&str, Value<'static>> = HashMap::from([
        ("last_choice", Value::from(LAST_CHOICE)),
        ("filename", Value::from(PICKED)),
        ("activation_token", Value::from(TOKEN)),
    ]);
    call(
        frontend,
        "AppChooser",
        "ChooseApplication",
        (handle, "org.example.Mail", "", APPS.to_vec(), options),
    )
}

/// The answer `asked` gets, while the server turns.
fn answer(rig: &mut Rig, asked: &mpsc::Receiver<Answer>, what: &str) -> Answer {
    let mut got = None;
    rig.until(what, |_| {
        got = asked.try_recv().ok();
        got.is_some()
    });
    got.unwrap()
}

/// The `uris` of a chosen answer.
fn uris(results: &HashMap<String, OwnedValue>) -> Vec<String> {
    let value = results["uris"].try_clone().expect("the uris");
    Vec::<String>::try_from(value).expect("a list of strings")
}

/// The output above the dock, where the dialogs show.
fn above_dock() -> Area {
    Area::new(0, 0, OUTPUT_SIZE.0, OUTPUT_SIZE.1 - DOCK_SIZE.height as i32)
}

/// How many pixels above the dock differ from `before`.
fn changed(now: &image::RgbaImage, before: &image::RgbaImage) -> usize {
    count(now, above_dock(), |_| true) - count_same(now, before)
}

fn count_same(now: &image::RgbaImage, before: &image::RgbaImage) -> usize {
    let area = above_dock();
    let mut same = 0;
    for y in area.y..area.y + area.h {
        for x in area.x..area.x + area.w {
            let (a, b) = (
                now.get_pixel(x as u32, y as u32),
                before.get_pixel(x as u32, y as u32),
            );
            if (0..3).all(|c| (i32::from(a[c]) - i32::from(b[c])).abs() <= PIXEL_TOLERANCE) {
                same += 1;
            }
        }
    }
    same
}

/// Turn the server for `time`.
fn wait(rig: &mut Rig, time: Duration) {
    let until = Instant::now() + time;
    rig.until("the time passes", |_| Instant::now() >= until);
}

/// Turn until a dialog shows over `bare`, the output without one, and its look holds still for a moment: its folder is
/// read and its window has the keyboard. A window whose caret blinks never holds still, so `steady` is false for it.
fn dialog_up(rig: &mut Rig, bare: &image::RgbaImage, steady: bool) {
    rig.until("a dialog shows", |rig| {
        changed(&rig.image(), bare) >= DIALOG_PIXELS
    });
    if !steady {
        wait(rig, LOOK * 3);
        return;
    }
    let mut last = rig.image();
    let mut next = Instant::now() + LOOK;
    rig.until("the dialog holds still", |rig| {
        if Instant::now() < next {
            return false;
        }
        next = Instant::now() + LOOK;
        let now = rig.image();
        let still = changed(&now, &last) == 0;
        last = now;
        still
    });
}

/// `work` on a thread of its own while the server turns, so a call that waits for the server is answered.
fn aside<T: Send + 'static>(
    rig: &mut Rig,
    what: &str,
    work: impl FnOnce() -> T + Send + 'static,
) -> T {
    let work = std::thread::spawn(work);
    rig.until(what, |_| work.is_finished());
    work.join().unwrap()
}

/// Turn until no dialog shows over `bare`.
fn dialog_gone(rig: &mut Rig, bare: &image::RgbaImage) {
    rig.until("the dialog goes", |rig| {
        changed(&rig.image(), bare) < DIALOG_PIXELS / 10
    });
}

/// The colour the dock shows now.
fn dock_colour(rig: &mut Rig) -> Option<&'static str> {
    let image = rig.image();
    let (w, h) = (DOCK_SIZE.width as i32, DOCK_SIZE.height as i32);
    let area = Area::new(0, OUTPUT_SIZE.1 - h, w, h);
    let shows = |colour: cupertino_kit::theme::Color| {
        let want = [colour.r, colour.g, colour.b].map(|c| (c * 255.0) as i32);
        count(&image, area, |rgb| {
            (0..3).all(|c| (i32::from(rgb[c]) - want[c]).abs() < 40)
        }) >= area.covered()
    };
    if shows(RED) {
        Some("red")
    } else if shows(BLUE) {
        Some("blue")
    } else {
        None
    }
}

/// The connection that owns `org.cupertino.Dialogs1` now, and its process.
fn owner(session: &Connection) -> Option<(String, u32)> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(session).ok()?;
    let name = zbus::names::BusName::try_from(BUS_NAME).ok()?;
    let owner = dbus.get_name_owner(name.clone()).ok()?;
    let pid = dbus.get_connection_unix_process_id(name).ok()?;
    Some((owner.to_string(), pid))
}

/// The server, the host, the portal stand-in and the frontend, the dock drawn and both services owned.
struct Session {
    rig: Rig,
    bus: Bus,
    frontend: Connection,
    folder: PathBuf,
    _root: Root,
}

impl Session {
    fn start() -> Session {
        let bus = Bus::start();
        let runtime = private_dir("runtime");
        std::env::set_var("XDG_RUNTIME_DIR", &runtime);
        let folder = private_home();
        steady_allocator();
        let root = Root::new();
        let mut rig = Rig::new("dialogs", &["dock", "dialogs"], &[]);
        start_session_service_on(&mut rig.state, Some(bus.1.clone()));
        let session = bus.connect(None);
        let has_owner = |name: &str| {
            zbus::blocking::fdo::DBusProxy::new(&session)
                .ok()
                .and_then(|d| d.name_has_owner(name.try_into().unwrap()).ok())
                .unwrap_or(false)
        };
        rig.until("the server serves its session service", |_| {
            has_owner("org.cupertino.Session1")
        });
        rig.state
            .supervisor
            .add_built_ins([(BuiltIn::Portal, Some(root.portal()))], Instant::now());
        rig.state.supervise_shell(Instant::now());
        rig.until(
            "the dock draws, the dialogs and the portal own their names",
            |rig| dock_colour(rig).is_some() && has_owner(BUS_NAME) && has_owner(PORTAL_NAME),
        );
        let frontend = bus.connect(Some(FRONTEND));
        Session {
            rig,
            bus,
            frontend,
            folder,
            _root: root,
        }
    }
}

#[test]
fn the_portals_dialogs_answer_what_the_person_chose() {
    as_portal();
    let mut s = Session::start();
    let rig = &mut s.rig;
    let bare = rig.image();

    // a caller other than the portal is refused, and nothing shows for it
    let stranger = s.bus.connect(None);
    let refused = aside(rig, "the stranger is answered", move || {
        let handle = ObjectPath::try_from("/request/stranger").unwrap();
        stranger
            .call_method(
                Some(BUS_NAME),
                OBJECT_PATH,
                Some(INTERFACE),
                "ChooseFile",
                &(
                    handle,
                    "open",
                    "org.example.Spy",
                    "",
                    "",
                    HashMap::<&str, Value>::new(),
                ),
            )
            .map(drop)
    })
    .expect_err("a stranger is refused");
    assert!(
        refused
            .to_string()
            .contains("org.freedesktop.DBus.Error.AccessDenied"),
        "{refused}"
    );
    wait(rig, LOOK);
    assert!(
        changed(&rig.image(), &bare) < DIALOG_PIXELS / 10,
        "nothing shows for the stranger"
    );

    // OpenFile: the open panel, its items keyed; the file picked is the answer
    let asked = choose_file(&s.frontend, "OpenFile", 1, &s.folder);
    dialog_up(rig, &bare, true);
    rig.state.scripted_key("Down").unwrap();
    rig.state.scripted_key("Return").unwrap();
    let (response, results) = answer(rig, &asked, "OpenFile is answered").expect("an answer");
    assert_eq!(response, 0, "the file is chosen");
    let picked = format!("file://{}", s.folder.join(PICKED).display());
    assert_eq!(uris(&results), vec![picked]);
    dialog_gone(rig, &bare);

    // SaveFile: the typed name replaces the suggested one's stem
    let asked = choose_file(&s.frontend, "SaveFile", 2, &s.folder);
    dialog_up(rig, &bare, false);
    rig.state.scripted_type(TYPED);
    rig.state.scripted_key("Return").unwrap();
    let (response, results) = answer(rig, &asked, "SaveFile is answered").expect("an answer");
    assert_eq!(response, 0, "the name is taken");
    let saved = format!("file://{}", s.folder.join(format!("{TYPED}.txt")).display());
    assert_eq!(uris(&results), vec![saved]);
    dialog_gone(rig, &bare);

    // ChooseApplication: the last choice is selected; Down and Return open the next, with the app's own token
    let asked = choose_application(&s.frontend, 3);
    dialog_up(rig, &bare, true);
    rig.state.scripted_key("Down").unwrap();
    rig.state.scripted_key("Return").unwrap();
    let (response, results) =
        answer(rig, &asked, "ChooseApplication is answered").expect("an answer");
    assert_eq!(response, 0, "an app is chosen");
    let choice = String::try_from(results["choice"].try_clone().unwrap()).unwrap();
    assert_eq!(choice, APPS[2]);
    let token = String::try_from(results["activation_token"].try_clone().unwrap()).unwrap();
    assert_eq!(token, TOKEN, "the app's own token is handed on");
    dialog_gone(rig, &bare);

    // Escape cancels
    let asked = choose_file(&s.frontend, "OpenFile", 4, &s.folder);
    dialog_up(rig, &bare, true);
    rig.state.scripted_key("Escape").unwrap();
    let (response, _) =
        answer(rig, &asked, "the cancelled OpenFile is answered").expect("an answer");
    assert_eq!(response, 1, "cancelled");
    dialog_gone(rig, &bare);
}

#[test]
fn the_dialogs_restart_alone_after_injected_panics_and_end_the_dialog_they_showed() {
    as_portal();
    let mut s = Session::start();
    let session = s.bus.connect(None);
    let rig = &mut s.rig;
    let bare = rig.image();
    let host = rig.host_pid(HostKind::Shared).unwrap();
    let (mut connection, pid) = owner(&session).unwrap();
    assert_eq!(pid, host, "the name is owned in the host's process");

    let mut colours = HashSet::new();
    let first_instance = rig.state.hosts.component_instance("dialogs");
    let mut host_memory = HostMemory::of(host);
    for crash in 0..RESTARTS {
        let at = ["paint", "dispatch", "helper"][crash as usize % 3];
        let (name, bus_call) = (connection.clone(), session.clone());
        aside(rig, "the crash is taken", move || {
            bus_call
                .call_method(
                    Some(name.as_str()),
                    "/org/cupertino/ShellComponentDebug1",
                    Some("org.cupertino.ShellComponentDebug1"),
                    "Crash",
                    &(at,),
                )
                .map(drop)
        })
        .unwrap_or_else(|e| panic!("the dialogs take a crash at {at}: {e}"));
        if at == "paint" {
            // what it draws next panics: a dialog the portal asked for, whose call then ends with nothing chosen
            let asked = choose_file(&s.frontend, "OpenFile", 100 + crash, &s.folder);
            let (response, _) =
                answer(rig, &asked, "the dialog of the failed instance ends").expect("an answer");
            assert_eq!(
                response, 2,
                "a dialog open as the component fails ends its call"
            );
        }
        if (crash + 1) % CRASHES_IN_A_ROW == 0 {
            rig.until(
                &format!("the server gives the dialogs up after a crash at {at}"),
                |rig| {
                    colours.extend(dock_colour(rig));
                    rig.state.hosts.component_gave_up("dialogs")
                },
            );
            let rx = rig.restart("dialogs");
            rig.answer(&rx).expect("the dialogs start again");
        }
        let instance = first_instance + crash + 1;
        rig.until(
            &format!("the dialogs start again after a crash at {at}"),
            |rig| {
                colours.extend(dock_colour(rig));
                rig.state.hosts.component_instance("dialogs") == instance
                    && rig.state.hosts.component_connected("dialogs")
            },
        );
        let mut now = None;
        rig.until("the restarted dialogs own their name", |_| {
            now = owner(&session).filter(|(name, _)| *name != connection);
            now.is_some()
        });
        let (name, pid) = now.unwrap();
        assert_eq!(pid, host, "after the restart {} at {at}", crash + 1);
        connection = name;
        host_memory.restarted(crash + 1);
    }

    assert_eq!(
        rig.host_pid(HostKind::Shared),
        Some(host),
        "the host went on"
    );
    assert_eq!(
        rig.state.hosts.component_instance("dock"),
        1,
        "the dock was never restarted"
    );
    assert_eq!(
        colours,
        HashSet::from(["red", "blue"]),
        "the dock kept drawing while the dialogs started again"
    );
    assert!(!rig.state.hosts.component_gave_up("dialogs"));
    host_memory.assert_nothing_kept(rig);

    // the last instance shows a dialog again
    dialog_gone(rig, &bare);
    let asked = choose_file(&s.frontend, "OpenFile", 1000, &s.folder);
    dialog_up(rig, &bare, true);
    rig.state.scripted_key("Escape").unwrap();
    let (response, _) = answer(rig, &asked, "the restarted dialogs answer").expect("an answer");
    assert_eq!(response, 1);
}
