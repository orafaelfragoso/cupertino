//! The screenshot program against the real server, headless (`crates/cupertino-screenshot`).
//! - A capture of the screen, of a selection spanned with the keys and of a window chosen with the keys writes a PNG of
//!   what it took to the Pictures folder that `user-dirs.dirs` names.
//! - The clipboard shortcut offers the capture as `image/png`: the server keeps a copy of that type.
//! - As the portal's service, on a session bus of the test's own: the colour of the pixel clicked on the colour
//!   picker's overlay reaches the portal; the source chosen in the sharing chooser reaches the portal; a caller other
//!   than the portal is refused, and Escape cancels the portal's pick.
//!
//! A capture runs the program in this process, on a connection with the capture role, as the server starts it for a
//! shortcut. For the service the server starts this test binary twice over: as the portal, hard-linked as
//! `cupertino-portal` under an install root of the test's own ([`as_portal`]), which asks through
//! `cupertino_portal::screenshot_tool` what [`TASK`] says; and as the screenshot program with `--serve`
//! ([`screenshot_program`]).

// shared with `shell-host.rs`, which uses what this test does not
#[allow(dead_code)]
#[path = "shell-host/app.rs"]
mod app;
#[allow(dead_code)]
#[path = "shell-host/rig.rs"]
mod rig;

/// The rig names a host program; no test here starts a shell host.
mod host {
    pub const MARK: &str = "cupertino-screenshot-tests-host";
}

use std::io::{BufRead, BufReader};
use std::os::fd::IntoRawFd;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command as Process, Stdio};
use std::sync::{mpsc, Once};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_kit::render::ImageResource;
use cupertino_portal::screen_cast::{Ask, Source};
use cupertino_portal::screenshot_tool::{self, Answer};
use cupertino_screenshot::control;
use cupertino_screenshot::shot::{self, Options, PNG};
use cupertino_server::config::BuiltIn;
use cupertino_server::quit::start_session_service_on;
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

use rig::{Area, Rig, OUTPUT_SIZE};

/// The settings of a capture: to the Pictures folder, without the floating thumbnail, so the file is written at once.
const SETTINGS: &str = "location = pictures\nshow-thumbnail = false\n";

/// The Pictures folder under the test's home, as `user-dirs.dirs` names it: not the default, so the folder shows that
/// the program read the file.
const PICTURES: &str = "Shots";

/// What the portal stand-in asks (`pick` or `share`), and the file it writes the answer to.
const TASK: &str = "CUPERTINO_SCREENSHOT_TEST_TASK";
const ANSWER: &str = "CUPERTINO_SCREENSHOT_TEST_ANSWER";

/// The argument that makes this test binary the screenshot program: a filter after `--` that names no test.
const SERVE_MARK: &str = "cupertino-screenshot-tests-program";

/// The file name the portal stand-in is linked as, which the supervisor starts it by.
const PORTAL_FILE: &str = "cupertino-portal";

/// The request handle the portal stand-in asks with.
const HANDLE: &str = "/org/freedesktop/portal/desktop/request/1_1/screenshot_test";

/// The left button's evdev code.
const BUTTON_LEFT: u32 = 0x110;

/// How long the test lets an overlay or a window be on screen before it answers: the program's arming delay
/// (`color_picker::ARMING`) and room for the frame that starts it to be presented.
const ARMED: Duration = Duration::from_millis(2_500);

/// A home of the test's own, whose `user-dirs.dirs` names [`PICTURES`], with the capture [`SETTINGS`]; its Pictures
/// folder.
fn home(name: &str) -> PathBuf {
    let home = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("screenshot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&home);
    let config = home.join(".config");
    std::fs::create_dir_all(&config).expect("the home's settings folder");
    std::fs::write(
        config.join("user-dirs.dirs"),
        format!("XDG_PICTURES_DIR=\"$HOME/{PICTURES}\"\n"),
    )
    .expect("user-dirs.dirs");
    let settings = config.join("screenshot.conf");
    std::fs::write(&settings, SETTINGS).expect("the capture settings");
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", &config);
    std::env::set_var("CUPERTINO_SCREENSHOT_CONFIG", &settings);
    home.join(PICTURES)
}

/// Run the capture `args` on a connection with the capture role, in this process.
fn capture(rig: &mut Rig, args: &[&str]) -> JoinHandle<Result<(), String>> {
    let options = Options::parse(args.iter().map(|a| a.to_string())).expect("the arguments");
    let stream = rig
        .state
        .privileged_client()
        .expect("a privileged connection");
    std::env::set_var("WAYLAND_SOCKET", stream.into_raw_fd().to_string());
    std::thread::spawn(move || shot::run(options))
}

/// Turn the server until the program ends, and its outcome.
fn ended(rig: &mut Rig, program: JoinHandle<Result<(), String>>) {
    rig.until("the program ends", |_| program.is_finished());
    program
        .join()
        .expect("the program does not panic")
        .expect("the capture is delivered");
}

/// The PNG files in `dir`.
fn written(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|entries| entries.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    files.retain(|f| f.extension().is_some_and(|e| e == "png"));
    files.sort();
    files
}

/// The one PNG written to `dir`, decoded.
fn only_png(dir: &Path) -> ImageResource {
    let files = written(dir);
    assert_eq!(files.len(), 1, "one file in {}: {files:?}", dir.display());
    let name = files[0].file_name().unwrap().to_string_lossy().into_owned();
    assert!(name.starts_with("Screenshot "), "{name}");
    let bytes = std::fs::read(&files[0]).expect("the PNG");
    cupertino_kit::image::decode(&bytes, u32::MAX).expect("a PNG")
}

/// How many pixels of `image` show what `shown` shows at `area`: the same colour within a step of rounding.
fn matching(image: &ImageResource, shown: &image::RgbaImage, area: Area) -> usize {
    let mut n = 0;
    for y in 0..image.height.min(area.h as u32) {
        for x in 0..image.width.min(area.w as u32) {
            let i = (y * image.stride + x * 4) as usize;
            let taken = &image.pixels[i..i + 3];
            let [r, g, b, _] = shown.get_pixel(area.x as u32 + x, area.y as u32 + y).0;
            if taken.iter().zip([r, g, b]).all(|(t, s)| t.abs_diff(s) <= 2) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn a_capture_of_the_screen_writes_it_to_the_pictures_folder() {
    as_portal();
    let pictures = home("screen");
    let mut rig = Rig::new("screenshot-screen", &[], &[]);
    let shown = rig.image();
    let program = capture(&mut rig, &["--mode", "screen"]);
    ended(&mut rig, program);
    let image = only_png(&pictures);
    assert_eq!((image.width as i32, image.height as i32), OUTPUT_SIZE);
    let all = Area::output();
    assert!(
        matching(&image, &shown, all) >= all.covered(),
        "the file shows the screen"
    );
}

#[test]
fn a_selection_spanned_with_the_keys_is_written_as_it_was_chosen() {
    as_portal();
    let pictures = home("selection");
    let mut rig = Rig::new("screenshot-selection", &[], &[]);
    let shown = rig.image();
    let program = capture(&mut rig, &["--mode", "selection"]);
    rig.until("the overlay is shown", |rig| {
        rig.state.screenshot_overlay_shown()
    });
    // an arrow spans half the output in its middle; Return captures it
    rig.state.scripted_key("Right").unwrap();
    rig.state.scripted_key("Return").unwrap();
    ended(&mut rig, program);
    let image = only_png(&pictures);
    let (w, h) = (OUTPUT_SIZE.0 / 2, OUTPUT_SIZE.1 / 2);
    assert_eq!((image.width as i32, image.height as i32), (w, h));
    let middle = Area::new(w / 2, h / 2, w, h);
    assert!(
        matching(&image, &shown, middle) >= middle.covered(),
        "the file shows the middle of the screen, without the overlay"
    );
}

#[test]
fn a_window_chosen_with_the_keys_is_written_alone() {
    as_portal();
    let pictures = home("window");
    let mut rig = Rig::new("screenshot-window", &[], &[]);
    let client = rig.client();
    app::spawn(client, "org.cupertino.test.captured");
    rig.until("the window is mapped", |rig| {
        rig.state.windows.iter().any(|w| w.mapped)
    });
    let id = rig.state.windows.iter().find(|w| w.mapped).unwrap().id;
    let frame = rig.state.wm.window(id).unwrap().geometry();
    rig.until("the window is drawn", |rig| {
        rig.shows(
            Area::output(),
            rig::magenta,
            Area::new(0, 0, app::WINDOW_SIZE.0, app::WINDOW_SIZE.1).covered(),
        )
    });
    let program = capture(&mut rig, &["--mode", "selection"]);
    rig.until("the overlay is shown", |rig| {
        rig.state.screenshot_overlay_shown()
    });
    // Space picks windows; an arrow chooses the front one; Return captures it
    rig.state.scripted_key("space").unwrap();
    rig.state.scripted_key("Down").unwrap();
    rig.state.scripted_key("Return").unwrap();
    ended(&mut rig, program);
    let image = only_png(&pictures);
    assert_eq!(
        (image.width as f64, image.height as f64),
        (frame.w, frame.h),
        "the file is the window's frame"
    );
    let content = Area::new(0, 0, image.width as i32, image.height as i32);
    let magenta = (0..image.height)
        .flat_map(|y| (0..image.width).map(move |x| (x, y)))
        .filter(|(x, y)| {
            let i = (y * image.stride + x * 4) as usize;
            rig::magenta([image.pixels[i], image.pixels[i + 1], image.pixels[i + 2]])
        })
        .count();
    let window = Area::new(0, 0, app::WINDOW_SIZE.0, app::WINDOW_SIZE.1);
    assert!(
        magenta >= window.covered() && content.meets(window),
        "the file shows the window's content: {magenta} magenta pixels"
    );
}

#[test]
fn the_clipboard_shortcut_offers_the_capture_as_png() {
    as_portal();
    let pictures = home("clipboard");
    let mut rig = Rig::new("screenshot-clipboard", &[], &[]);
    // the program serves the clipboard until another selection replaces it, so it runs on until the test ends
    let _program = capture(&mut rig, &["--mode", "screen", "--clipboard"]);
    rig.until("the server keeps the offered PNG", |rig| {
        rig.state.clipboard.is_complete() && rig.state.clipboard.captured_types().contains(&PNG)
    });
    assert!(
        written(&pictures).is_empty(),
        "nothing is written to a file"
    );
}

/// A program with nothing to do: it holds its connection until the server closes it.
struct Plain;

impl Dispatch<wl_registry::WlRegistry, ()> for Plain {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// Hold the connection of `WAYLAND_SOCKET` until the server closes it.
fn hold_connection() {
    let conn = Connection::connect_to_env().expect("started with a WAYLAND_SOCKET");
    let mut queue = conn.new_event_queue::<Plain>();
    let _registry = conn.display().get_registry(&queue.handle(), ());
    while queue.blocking_dispatch(&mut Plain).is_ok() {}
}

/// The screen-sharing request the portal stand-in makes: a display and two windows, nothing to remember.
fn share_ask() -> Ask {
    Ask {
        app_id: "org.example.Viewer".into(),
        app_name: Some("Viewer".into()),
        app_icon: None,
        parent_window: String::new(),
        sources: vec![
            Source::Display {
                output: 1,
                name: "headless-1".into(),
                description: "The test's output".into(),
            },
            Source::Window {
                identifier: "w1".into(),
                title: "Notes".into(),
                app_id: "org.example.Notes".into(),
            },
            Source::Window {
                identifier: "w2".into(),
                title: "Terminal".into(),
                app_id: String::new(),
            },
        ],
        offer_remember: false,
    }
}

/// Ask the screenshot program for `task` as the portal does, and the answer as a line.
fn ask_as_portal(task: &str) -> String {
    zbus::block_on(async {
        let conn = match zbus::Connection::session().await {
            Ok(conn) => conn,
            Err(e) => return format!("error {e}"),
        };
        match task {
            "pick" => match screenshot_tool::pick_color(&conn, HANDLE).await {
                Ok(Answer::Done([r, g, b])) => format!("colour {r} {g} {b}"),
                Ok(Answer::Cancelled) => "cancelled".into(),
                Ok(Answer::Failed) => "failed".into(),
                Err(e) => format!("error {e}"),
            },
            "share" => match screenshot_tool::select_sources(&conn, HANDLE, &share_ask()).await {
                Ok(Answer::Done((index, remember))) => format!("chosen {index} {remember}"),
                Ok(Answer::Cancelled) => "cancelled".into(),
                Ok(Answer::Failed) => "failed".into(),
                Err(e) => format!("error {e}"),
            },
            other => format!("error no task {other:?}"),
        }
    })
}

/// In the portal stand-in, be the portal: one test asks the screenshot program what [`TASK`] says and writes the answer
/// to [`ANSWER`] while it holds the portal's connection; the others wait, and the process ends with the connection.
fn as_portal() {
    let exe = std::env::current_exe().expect("the test binary");
    if exe.file_name().is_some_and(|name| name == PORTAL_FILE) {
        static HELD: Once = Once::new();
        HELD.call_once(|| {
            let task = std::env::var(TASK).unwrap_or_default();
            let answer = PathBuf::from(std::env::var_os(ANSWER).expect("the answer's file"));
            std::thread::spawn(move || {
                let line = ask_as_portal(&task);
                // whole or not at all, for the test that waits for it
                let part = answer.with_extension("part");
                std::fs::write(&part, line).expect("the answer");
                std::fs::rename(&part, &answer).expect("the answer");
            });
            hold_connection();
        });
        std::process::exit(0);
    }
}

/// The screenshot program, when the server started this binary with [`SERVE_MARK`] (and `--serve`); a test run does
/// nothing here.
#[test]
fn screenshot_program() {
    as_portal();
    if std::env::args().any(|arg| arg == SERVE_MARK) {
        let served = cupertino_screenshot::serve::serve();
        if let Err(e) = &served {
            eprintln!("cupertino-screenshot: {e}");
        }
        std::process::exit(i32::from(served.is_err()));
    }
}

/// This test binary, running [`screenshot_program`] alone.
fn screenshot_command() -> Vec<String> {
    let exe = std::env::current_exe().unwrap().display().to_string();
    [
        exe.as_str(),
        "--exact",
        "screenshot_program",
        "--nocapture",
        "--",
        SERVE_MARK,
    ]
    .map(String::from)
    .to_vec()
}

/// An install root of the test's own whose `bin/cupertino-portal` is this binary, removed on drop.
struct Root(PathBuf);

impl Root {
    fn new(name: &str) -> Root {
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("screenshot-{name}-root-{}", std::process::id()));
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

/// A session bus of the test's own that starts no service, whose address the server's programs inherit.
struct Bus(Child, String);

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
    fn start(name: &str) -> Bus {
        let config = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("screenshot-{name}-{}-bus.conf", std::process::id()));
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
        Bus(daemon, address)
    }

    /// Whether `name` has an owner on the bus.
    fn owned(&self, name: &str) -> bool {
        let conn = zbus::blocking::Connection::session().expect("the test's bus");
        let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(&conn) else {
            return false;
        };
        dbus.name_has_owner(name.try_into().unwrap())
            .unwrap_or(false)
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// The server with its session service on a bus of the test's own, the screenshot command this binary, and the portal
/// stand-in started to ask for `task`; the file its answer comes in.
struct Service {
    rig: Rig,
    answer: PathBuf,
    bus: Bus,
    _root: Root,
}

impl Service {
    fn new(name: &str, task: &str) -> Service {
        let pictures = home(name);
        let answer = pictures.with_file_name("answer");
        std::env::set_var(TASK, task);
        std::env::set_var(ANSWER, &answer);
        let bus = Bus::start(name);
        let root = Root::new(name);
        let mut rig = Rig::new(&format!("screenshot-{name}"), &[], &[]);
        rig.state.config.screenshot_command = screenshot_command();
        start_session_service_on(&mut rig.state, Some(bus.1.clone()));
        rig.until("the server serves its session service", |_| {
            bus.owned("org.cupertino.Session1")
        });
        rig.state
            .supervisor
            .add_built_ins([(BuiltIn::Portal, Some(root.portal()))], Instant::now());
        rig.state.supervise_shell(Instant::now());
        Service {
            rig,
            answer,
            bus,
            _root: root,
        }
    }

    /// Turn the server for `time`.
    fn wait(&mut self, time: Duration) {
        let until = Instant::now() + time;
        self.rig
            .until("the time passes", |_| Instant::now() >= until);
    }

    /// The portal stand-in's answer, once it came.
    fn answer(&mut self) -> String {
        let answer = self.answer.clone();
        self.rig
            .until("the portal has its answer", |_| answer.exists());
        std::fs::read_to_string(&answer).expect("the answer")
    }
}

#[test]
fn the_colour_clicked_on_the_picker_reaches_the_portal() {
    as_portal();
    let mut service = Service::new("pick", "pick");
    let at = (OUTPUT_SIZE.0 / 3, OUTPUT_SIZE.1 / 3);
    let [r, g, b, _] = service.rig.image().get_pixel(at.0 as u32, at.1 as u32).0;
    service.rig.until("the colour picker is shown", |rig| {
        rig.state.screenshot_overlay_shown()
    });
    service.wait(ARMED);
    service
        .rig
        .state
        .scripted_click((f64::from(at.0), f64::from(at.1)), BUTTON_LEFT);
    let answer = service.answer();
    let picked: Vec<f64> = answer
        .strip_prefix("colour ")
        .unwrap_or_else(|| panic!("a colour: {answer}"))
        .split(' ')
        .map(|c| c.parse().expect("a channel"))
        .collect();
    let shown = [r, g, b].map(|c| f64::from(c) / 255.0);
    assert!(
        picked
            .iter()
            .zip(shown)
            .all(|(p, s)| (p - s).abs() <= 1.0 / 255.0),
        "the portal gets the colour of the pixel clicked: {picked:?}, shown {shown:?}"
    );
    service.rig.until("the picker is gone", |rig| {
        !rig.state.screenshot_overlay_shown()
    });
}

#[test]
fn the_source_chosen_in_the_sharing_chooser_reaches_the_portal() {
    as_portal();
    let mut service = Service::new("share", "share");
    service.rig.until("the chooser is shown", |rig| {
        rig.state.windows.iter().any(|w| w.mapped)
    });
    service.wait(ARMED);
    // the focus starts on Cancel; the list is before it; End chooses its last row, and Return shares it
    for key in ["Shift+Tab", "End", "Return"] {
        service.rig.state.scripted_key(key).unwrap();
    }
    assert_eq!(service.answer(), "chosen 2 false");
    service.rig.until("the chooser is gone", |rig| {
        !rig.state.windows.iter().any(|w| w.mapped)
    });
}

/// Call `method` on `org.cupertino.Screenshot1` from the test's own connection, on a thread of its own while the
/// server turns: the program asks the server who calls; the D-Bus error's name.
fn call_as_stranger(service: &mut Service, method: &'static str) -> Option<String> {
    let (sent, answered) = mpsc::channel();
    std::thread::spawn(move || {
        let conn = zbus::blocking::Connection::session().expect("the test's bus");
        let reply = match method {
            "PickColor" => conn.call_method(
                Some(control::BUS_NAME),
                control::OBJECT_PATH,
                Some(control::BUS_NAME),
                method,
                &(HANDLE,),
            ),
            _ => conn.call_method(
                Some(control::BUS_NAME),
                control::OBJECT_PATH,
                Some(control::BUS_NAME),
                method,
                &(
                    HANDLE,
                    "Viewer",
                    "",
                    "",
                    false,
                    vec![("display", "headless-1", "")],
                ),
            ),
        };
        let _ = sent.send(reply.err().and_then(|e| match e {
            zbus::Error::MethodError(name, ..) => Some(name.to_string()),
            _ => None,
        }));
    });
    let mut error = None;
    service
        .rig
        .until("the call is answered", |_| match answered.try_recv() {
            Ok(e) => {
                error = Some(e);
                true
            }
            Err(_) => false,
        });
    error.flatten()
}

#[test]
fn a_caller_other_than_the_portal_is_refused_and_escape_cancels() {
    as_portal();
    let mut service = Service::new("refused", "pick");
    service.rig.until("the colour picker is shown", |rig| {
        rig.state.screenshot_overlay_shown()
    });
    assert!(service.bus.owned(control::BUS_NAME));
    let denied = Some("org.freedesktop.DBus.Error.AccessDenied".to_owned());
    assert_eq!(call_as_stranger(&mut service, "PickColor"), denied);
    assert_eq!(call_as_stranger(&mut service, "SelectSources"), denied);
    service.rig.state.scripted_key("Escape").unwrap();
    assert_eq!(service.answer(), "cancelled");
    service.rig.until("the picker is gone", |rig| {
        !rig.state.screenshot_overlay_shown()
    });
}
