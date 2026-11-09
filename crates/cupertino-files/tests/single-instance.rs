//! One Files process per session, on a private bus: a second launch hands its request to the running Files and exits,
//! `Activate` and the progress action bring windows back when none is open, a call naming anything but a local folder
//! or the trash is refused, and a running Files that hangs fails the launch instead of letting a second one start.

#![cfg(feature = "app")]

mod common;

use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use common::{place, Run};
use cupertino_files::instance::{
    self, Launch, LaunchError, Request, Service, Target, BUS_NAME, INTERFACE, OBJECT_PATH,
};
use cupertino_files::kit::Observer;
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::{Key, WindowKey};
use cupertino_kit::platform::{WindowEvent, WindowId, WindowRequest, WindowStates};
use cupertino_kit::theme::{Appearance, ThemePreferences};
use cupertino_test_bus::{missing_tool, PrivateBus};
use zbus::zvariant::{OwnedValue, Value};

const FILES: &str = env!("CARGO_BIN_EXE_cupertino-files");

/// A gate a copy waits at once it copied part of a file, until the test opens it.
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn new() -> Arc<Gate> {
        Arc::new(Gate {
            open: Mutex::new(false),
            opened: Condvar::new(),
        })
    }

    fn observer(gate: &Arc<Gate>, reached: mpsc::Sender<()>) -> Observer {
        let gate = Arc::clone(gate);
        let reached = Mutex::new(Some(reached));
        Arc::new(move |progress| {
            if progress.bytes == 0 || progress.bytes == progress.total_bytes {
                return;
            }
            if let Some(reached) = reached.lock().unwrap().take() {
                let _ = reached.send(());
            }
            let open = gate.open.lock().unwrap();
            let _open = gate
                .opened
                .wait_timeout_while(open, Duration::from_secs(20), |open| !*open)
                .unwrap();
        })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// Four chunks of a copy, so it can wait between two of them.
const BIG: usize = 4 << 20;

fn bus() -> Option<PrivateBus> {
    let bus = PrivateBus::start();
    if bus.is_none() {
        missing_tool("dbus-daemon");
    }
    bus
}

/// Files serving the name on `bus`, with a window on `home/Documents` (`Notes`, `big.bin`) and what `configure` makes of
/// its start.
fn files(
    bus: &PrivateBus,
    configure: impl FnOnce(cupertino_files::kit::Start) -> cupertino_files::kit::Start,
) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::write(documents.join("big.bin"), vec![5u8; BIG]).unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let service = Service::serve(bus.builder()).expect("the name is free");
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = Run::start_configured(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        |start| configure(start.with_instance(Some(service))),
    );
    run.wait_until(|run| run.rows().len() == 2);
    run
}

/// The window with `key`, while it is open.
fn window(run: &Run, key: &str) -> Option<WindowId> {
    let key = WindowKey(Key::from(key.to_owned()));
    run.driver
        .runtime()
        .windows()
        .into_iter()
        .find_map(|(k, id)| (k == key).then_some(id))
}

fn title(run: &mut Run, window: WindowId) -> String {
    run.driver
        .window_system_mut()
        .spec(window)
        .unwrap()
        .title
        .clone()
}

/// The activations asked for `window`, with their tokens.
fn activations(run: &mut Run, window: WindowId) -> Vec<Option<String>> {
    run.driver
        .window_system_mut()
        .requests(window)
        .unwrap()
        .iter()
        .filter_map(|r| match r {
            WindowRequest::Activate(token) => Some(token.clone()),
            _ => None,
        })
        .collect()
}

/// Have the compositor configure `window`, so it draws its first frame.
fn present(run: &mut Run, window: WindowId) {
    run.driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    run.settle();
}

fn close(run: &mut Run, window: WindowId) {
    run.driver
        .window_system_mut()
        .enqueue_window(window, WindowEvent::CloseRequested)
        .unwrap();
    run.settle();
}

/// Run `work` on a thread of its own while Files keeps turning, and give what it returned. The thread wakes the
/// runtime once the work ended, so the turning waits for Files' own threads and that one alike; the thirty seconds only
/// guard against a hang.
fn while_turning<T: Send + 'static>(run: &mut Run, work: impl FnOnce() -> T + Send + 'static) -> T {
    let waker = run.waker();
    let work = std::thread::spawn(move || {
        let out = work();
        waker.wake();
        out
    });
    let deadline = Instant::now() + Duration::from_secs(30);
    while !work.is_finished() {
        assert!(run.wait_for_progress(deadline), "it never ended");
        run.settle();
    }
    work.join().unwrap()
}

/// `cupertino-files` started with `args` in `dir`, on `bus` alone (no display, no other session), as a launch from
/// the dock or a terminal is.
fn launch(bus: &PrivateBus, home: &Path, dir: &Path, args: &[&str]) -> Command {
    let mut command = Command::new(FILES);
    command
        .args(args)
        .current_dir(dir)
        .env_clear()
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("HOME", home)
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    command
}

/// Run `command` to its end while Files keeps turning: whether it succeeded, and what it said on stderr.
fn run_launch(run: &mut Run, mut command: Command) -> (bool, String) {
    let child = command.spawn().unwrap();
    let output = while_turning(run, move || child.wait_with_output().unwrap());
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stderr).into_owned(),
    )
}

/// Call `method` of the application object on `bus` with `body`, as another program of the session does.
fn call<B>(
    bus: &PrivateBus,
    method: &'static str,
    body: B,
) -> impl FnOnce() -> zbus::Result<()> + Send + 'static
where
    B: serde::Serialize + zbus::zvariant::DynamicType + Send + 'static,
{
    let connection = bus.connect();
    move || {
        connection
            .call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, &body)
            .map(drop)
    }
}

fn data(token: &str) -> HashMap<String, OwnedValue> {
    instance::platform_data(Some(token))
}

fn owner(bus: &PrivateBus) -> Option<String> {
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus.connect()).unwrap();
    dbus.get_name_owner(BUS_NAME.try_into().unwrap())
        .ok()
        .map(|o| o.to_string())
}

/// Waits until nobody owns the name, as the bus announces it; the ten seconds only guard against a hang.
fn wait_until_unowned(bus: &PrivateBus) {
    let connection = bus.connect();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&connection).unwrap();
    // listening before looking, so a change between the two is heard
    let changes = dbus
        .receive_name_owner_changed_with_args(&[(0, BUS_NAME)])
        .unwrap();
    if owner(bus).is_none() {
        return;
    }
    let (unowned, heard) = mpsc::channel();
    std::thread::spawn(move || {
        for change in changes {
            if change.args().is_ok_and(|args| args.new_owner().is_none()) {
                let _ = unowned.send(());
                return;
            }
        }
    });
    heard
        .recv_timeout(Duration::from_secs(10))
        .expect("the name is given up");
}

#[test]
fn a_second_launch_without_a_folder_brings_the_first_window_forward_and_exits() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let first = run.window;
    let home = run.home();
    let (ok, said) = run_launch(&mut run, launch(&bus, &home, &home, &[]));
    assert!(ok, "the launch hands over and succeeds: {said}");
    assert_eq!(
        run.windows(),
        [first],
        "no second window, no second process"
    );
    assert_eq!(
        activations(&mut run, first),
        [None],
        "the window comes forward"
    );
    assert!(owner(&bus).is_some(), "the first Files still serves");

    // the name's owner runs another program (this test), so it gets the launch but not its token
    let mut with_token = launch(&bus, &home, &home, &[]);
    with_token.env("XDG_ACTIVATION_TOKEN", "launch-token");
    let (ok, said) = run_launch(&mut run, with_token);
    assert!(ok, "the launch still hands over: {said}");
    assert!(said.contains("without its activation token"), "{said}");
    assert_eq!(activations(&mut run, first), [None, None]);
}

#[test]
fn a_second_launch_with_a_folder_opens_it_in_the_first() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let home = run.home();
    // a relative folder, made absolute where it was launched
    let (ok, said) = run_launch(
        &mut run,
        launch(&bus, &home, &home.join("Documents"), &["Notes"]),
    );
    assert!(ok, "the launch hands over and succeeds: {said}");
    assert_eq!(run.windows().len(), 2);
    let opened = window(&run, "browser/1").expect("a window on the folder");
    assert_eq!(title(&mut run, opened), "Notes");

    // a file URI and the trash, as the desktop entry's %U gives them
    let notes = home.join("Documents/Notes");
    let uri = Target::Folder(notes).uri();
    let (ok, said) = run_launch(&mut run, launch(&bus, &home, &home, &[&uri]));
    assert!(ok, "{said}");
    let (ok, said) = run_launch(&mut run, launch(&bus, &home, &home, &["trash:///"]));
    assert!(ok, "{said}");
    let trash = window(&run, "browser/3").expect("a window on the trash");
    present(&mut run, trash);
    assert_eq!(run.windows().len(), 4);
    assert!(
        home.join("Trash/files").is_dir() && home.join("Trash/info").is_dir(),
        "the trash is made when it is opened"
    );
    assert_eq!(activations(&mut run, trash), [None]);
}

#[test]
fn a_launch_hands_its_activation_token_to_the_window_it_opens() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let notes = run.home().join("Documents/Notes");
    let address = bus.address.clone();
    let launch = while_turning(&mut run, move || {
        let connect = move || zbus::connection::Builder::address(address.as_str());
        let request = Request::Open(vec![Target::Folder(notes)]);
        instance::serve_or_forward(
            connect,
            &request,
            Some("launch-token"),
            Duration::from_secs(5),
        )
        .map(|launch| matches!(launch, Launch::Forwarded))
        .map_err(|e| e.to_string())
    });
    assert_eq!(launch, Ok(true), "handed over");
    let opened = window(&run, "browser/1").expect("a window on the folder");
    assert!(
        activations(&mut run, opened).is_empty(),
        "not before it is shown"
    );
    present(&mut run, opened);
    assert_eq!(
        activations(&mut run, opened),
        [Some("launch-token".to_owned())]
    );
}

#[test]
fn activate_with_no_window_open_opens_one_and_show_progress_brings_the_progress_window_back() {
    let Some(bus) = bus() else { return };
    let gate = Gate::new();
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let mut run = files(&bus, |start| {
        start
            .with_transfer_observer(observer)
            .with_progress_delay(Duration::ZERO)
    });
    // a copy that runs until the test lets it go keeps Files running with no window open
    run.click_row("big.bin");
    run.menu("copy");
    run.click_row("Notes");
    run.key_with(0xff54, None, cupertino_kit::platform::Mods::PRIMARY);
    run.wait_until(|run| run.title() == "Notes");
    run.menu("paste");
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    let deadline = Instant::now() + Duration::from_secs(20);
    while window(&run, "progress").is_none() {
        assert!(
            run.wait_for_progress(deadline),
            "the progress window never opened"
        );
        run.settle();
    }
    let progress = window(&run, "progress").unwrap();
    close(&mut run, progress);
    let browser = run.window;
    close(&mut run, browser);
    assert!(run.windows().is_empty(), "nothing of Files is on screen");
    assert!(
        !run.driver.runtime().exited(),
        "the copy keeps Files running"
    );

    // the progress action brings the hidden progress window back, raised with the caller's token
    let show = call(
        &bus,
        "ActivateAction",
        (
            "show-progress",
            Vec::<Value<'static>>::new(),
            data("progress-token"),
        ),
    );
    while_turning(&mut run, show).expect("show-progress is taken");
    let progress = window(&run, "progress").expect("the progress window is back");
    present(&mut run, progress);
    assert_eq!(
        activations(&mut run, progress),
        [Some("progress-token".to_owned())]
    );
    assert_eq!(run.windows().len(), 1, "no browser window comes with it");

    // Activate with no browser window open opens one on the home folder
    let activate = call(&bus, "Activate", (data("window-token"),));
    while_turning(&mut run, activate).expect("Activate is taken");
    let opened = window(&run, "browser/1").expect("a browser window opens");
    present(&mut run, opened);
    let home_name = run
        .home()
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(title(&mut run, opened), home_name);
    assert_eq!(
        activations(&mut run, opened),
        [Some("window-token".to_owned())]
    );

    // Activate with a window open brings it forward instead of opening another
    let again = call(&bus, "Activate", (HashMap::<String, OwnedValue>::new(),));
    while_turning(&mut run, again).expect("Activate is taken");
    assert!(window(&run, "browser/2").is_none());
    assert_eq!(
        activations(&mut run, opened),
        [Some("window-token".to_owned()), None]
    );
    gate.release();
}

#[test]
fn a_call_naming_anything_but_a_local_folder_or_the_trash_is_refused() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let notes = run.home().join("Documents/Notes");
    let good = Target::Folder(notes).uri();
    for uris in [
        vec!["http://example.com/".to_owned()],
        vec!["file://server/share".to_owned()],
        vec!["/home/user".to_owned()],
        vec!["trash:///secret.txt".to_owned()],
        vec![good.clone(), "sftp://host/home".to_owned()],
        vec![],
    ] {
        let open = call(
            &bus,
            "Open",
            (uris.clone(), "", HashMap::<String, OwnedValue>::new()),
        );
        let refused = while_turning(&mut run, open);
        match refused {
            Err(zbus::Error::MethodError(name, _, _)) => {
                assert_eq!(
                    name.as_str(),
                    "org.freedesktop.DBus.Error.InvalidArgs",
                    "{uris:?}"
                )
            }
            Err(zbus::Error::FDO(e)) => {
                assert!(
                    matches!(*e, zbus::fdo::Error::InvalidArgs(_)),
                    "{uris:?}: {e}"
                )
            }
            other => panic!("{uris:?} is refused, not {other:?}"),
        }
    }
    let unknown = call(
        &bus,
        "ActivateAction",
        (
            "quit",
            Vec::<Value<'static>>::new(),
            HashMap::<String, OwnedValue>::new(),
        ),
    );
    assert!(
        while_turning(&mut run, unknown).is_err(),
        "an unknown action"
    );
    assert_eq!(run.windows().len(), 1, "nothing opened");

    let open = call(
        &bus,
        "Open",
        (vec![good], "", HashMap::<String, OwnedValue>::new()),
    );
    while_turning(&mut run, open).expect("a local folder opens");
    assert_eq!(run.windows().len(), 2);
}

/// A program that owns the name and never answers a call; it says on `called`, if it has one, when a call came.
struct Hung {
    called: Option<Mutex<mpsc::Sender<()>>>,
}

impl Hung {
    fn hang(&self) -> impl std::future::Future<Output = ()> {
        if let Some(called) = &self.called {
            let _ = called.lock().unwrap().send(());
        }
        std::future::pending::<()>()
    }
}

#[zbus::interface(name = "org.freedesktop.Application")]
impl Hung {
    async fn activate(&self, _platform_data: HashMap<String, OwnedValue>) {
        self.hang().await
    }

    async fn open(
        &self,
        _uris: Vec<String>,
        _hint: String,
        _platform_data: HashMap<String, OwnedValue>,
    ) {
        self.hang().await
    }
}

fn hung(bus: &PrivateBus) -> zbus::blocking::Connection {
    hung_telling(bus, None)
}

/// [`hung`], saying on `called` when a call came.
fn hung_telling(bus: &PrivateBus, called: Option<mpsc::Sender<()>>) -> zbus::blocking::Connection {
    bus.blocking_builder()
        .name(BUS_NAME)
        .unwrap()
        .serve_at(
            OBJECT_PATH,
            Hung {
                called: called.map(Mutex::new),
            },
        )
        .unwrap()
        .build()
        .unwrap()
}

fn connect(bus: &PrivateBus) -> impl Fn() -> zbus::Result<zbus::connection::Builder<'static>> {
    let address = bus.address.clone();
    move || zbus::connection::Builder::address(address.as_str())
}

#[test]
fn an_owner_that_hangs_fails_the_launch_after_the_timeout() {
    let Some(bus) = bus() else { return };
    let _hung = hung(&bus);
    let before = owner(&bus);
    let timeout = Duration::from_millis(400);
    let started = Instant::now();
    let launch = instance::serve_or_forward(connect(&bus), &Request::Activate, None, timeout);
    let waited = started.elapsed();
    match launch {
        Err(LaunchError::Refused(why)) => assert!(why.contains("did not answer"), "{why}"),
        Err(e) => panic!("refused for the timeout, not {e}"),
        Ok(_) => panic!("a hung owner neither takes the launch nor gives the name up"),
    }
    assert!(
        waited >= timeout && waited < Duration::from_secs(3),
        "{waited:?}"
    );
    assert_eq!(
        owner(&bus),
        before,
        "the name stays with its owner: no second Files"
    );

    // the program itself waits its own timeout, then exits with a message and starts nothing
    let home = tempfile::tempdir().unwrap();
    let output = launch_alone(&bus, home.path(), &[]);
    assert!(!output.status.success());
    let said = String::from_utf8_lossy(&output.stderr);
    assert!(said.contains("did not answer"), "{said}");
    assert_eq!(owner(&bus), before);
}

/// `cupertino-files` started with `args` on `bus` with no Files serving it, to its end.
fn launch_alone(bus: &PrivateBus, home: &Path, args: &[&str]) -> std::process::Output {
    launch(bus, home, home, args).output().unwrap()
}

#[test]
fn an_owner_that_leaves_while_it_is_asked_lets_the_launch_serve() {
    let Some(bus) = bus() else { return };
    let (called, calls) = mpsc::channel();
    let hung = hung_telling(&bus, Some(called));
    let leaves = std::thread::spawn(move || {
        // once the launch's call is waiting on it
        calls
            .recv_timeout(Duration::from_secs(10))
            .expect("the launch calls the owner");
        // as the process exits: the socket closes, whatever call it was answering
        hung.close().unwrap();
    });
    let launch = instance::serve_or_forward(
        connect(&bus),
        &Request::Activate,
        None,
        Duration::from_secs(10),
    );
    leaves.join().unwrap();
    let Ok(Launch::Serve(service)) = launch else {
        panic!("the launch serves once the owner left");
    };
    assert!(owner(&bus).is_some());
    drop(service);
    wait_until_unowned(&bus);
}

#[test]
fn files_gives_its_name_up_as_it_exits_and_the_next_launch_serves() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    assert!(owner(&bus).is_some());
    let browser = run.window;
    close(&mut run, browser);
    assert!(run.driver.runtime().exited());
    wait_until_unowned(&bus);
    let launch = instance::serve_or_forward(
        connect(&bus),
        &Request::Activate,
        None,
        Duration::from_secs(1),
    );
    assert!(matches!(launch, Ok(Launch::Serve(_))));
}

#[test]
fn a_launch_racing_the_exit_starts_a_new_files_instead_of_handing_over() {
    let Some(bus) = bus() else { return };
    // the service of a Files that exits while a launch waits: its waiting call is refused and the name freed
    let service = Service::serve(bus.builder()).unwrap();
    let address = bus.address.clone();
    let launch = std::thread::spawn(move || {
        let connect = move || zbus::connection::Builder::address(address.as_str());
        instance::serve_or_forward(connect, &Request::Activate, None, Duration::from_secs(10))
            .map(|launch| matches!(launch, Launch::Serve(_)))
            .map_err(|e| e.to_string())
    });
    // the service's wake, heard on a thread of its own so the wait has a guard
    let woken = service.woken().clone();
    let (wake, wakes) = mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(()) = woken.recv_blocking() {
            if wake.send(()).is_err() {
                break;
            }
        }
    });
    let waiting = loop {
        let calls = service.take();
        if !calls.is_empty() {
            break calls;
        }
        wakes
            .recv_timeout(Duration::from_secs(10))
            .expect("the launch calls");
    };
    // Files quits without answering
    drop(waiting);
    drop(service);
    assert_eq!(
        launch.join().unwrap(),
        Ok(true),
        "the launch serves in its place"
    );
}

/// The D-Bus error name a call failed with.
fn error_name(e: &zbus::Error) -> String {
    match e {
        zbus::Error::MethodError(name, _, _) => name.to_string(),
        zbus::Error::FDO(e) => zbus::DBusError::name(&**e).to_string(),
        other => format!("{other:?}"),
    }
}

#[test]
fn calls_past_the_ones_waiting_are_refused_at_once() {
    let Some(bus) = bus() else { return };
    // the most calls that wait for the window's thread
    const WAITING: usize = 16;
    const CALLERS: usize = WAITING + 8;
    let service = Service::serve(bus.builder()).unwrap();
    let (results, answered) = mpsc::channel();
    for _ in 0..CALLERS {
        let activate = call(&bus, "Activate", (HashMap::<String, OwnedValue>::new(),));
        let results = results.clone();
        std::thread::spawn(move || {
            let _ = results.send(activate());
        });
    }
    // the window's thread takes nothing yet: the calls past the waiting ones are answered while those still wait
    for _ in WAITING..CALLERS {
        let refused = answered
            .recv_timeout(Duration::from_secs(20))
            .expect("a call past the waiting ones is answered");
        let e = refused.expect_err("only the waiting calls are taken");
        assert_eq!(error_name(&e), "org.freedesktop.DBus.Error.LimitsExceeded");
    }
    let calls = service.take();
    assert_eq!(calls.len(), WAITING);
    calls.into_iter().for_each(instance::Call::done);
    for _ in 0..WAITING {
        answered
            .recv_timeout(Duration::from_secs(20))
            .expect("a taken call is answered")
            .expect("and succeeds");
    }
}

#[test]
fn the_first_window_is_raised_with_the_token_files_was_started_with() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| {
        start.with_activation_token(Some("startup-token".into()))
    });
    let first = run.window;
    assert_eq!(
        activations(&mut run, first),
        [Some("startup-token".to_owned())],
        "once it is shown, and once only"
    );
}
