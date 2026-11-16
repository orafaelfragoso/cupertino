//! One Files process per session, on a private bus: a second launch hands its request to the running Files and exits,
//! `Activate` and the progress action bring windows back when none is open, a call naming anything but a local folder
//! or the trash is refused, and a running Files that hangs fails the launch instead of letting a second one start.

#![cfg(feature = "app")]

use crate::common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use common::{place, Run};
use cupertino_files::instance::{
    self, Launch, LaunchError, Request, Service, Target, BUS_NAME, FILE_MANAGER_INTERFACE,
    FILE_MANAGER_NAME, FILE_MANAGER_PATH, INTERFACE, OBJECT_PATH,
};
use cupertino_files::kit::{LookUp, Observer, LOOK_UP_DEADLINE, MAX_LOOK_UPS};
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
            gate.wait();
        })
    }

    /// Wait until the gate is opened, or 20 seconds passed, so a failing test ends.
    fn wait(&self) {
        let open = self.open.lock().unwrap();
        let _open = self
            .opened
            .wait_timeout_while(open, Duration::from_secs(20), |open| !*open)
            .unwrap();
    }

    /// A look-up of the items a call names that waits at the gate, as on a mount that does not answer: it tells
    /// `held` of each item it waits for, then wakes the runtime with the waker put in its [`Held`], and counts there
    /// each item it no longer waits for.
    fn look_up(gate: &Arc<Gate>, held: mpsc::Sender<PathBuf>) -> (LookUp, Arc<Held>) {
        let gate = Arc::clone(gate);
        let state = Arc::new(Held::default());
        let shared = Arc::clone(&state);
        let held = Mutex::new(held);
        let look: LookUp = Arc::new(move |path| {
            let _ = held.lock().unwrap().send(path.to_path_buf());
            if let Some(waker) = &*shared.waker.lock().unwrap() {
                waker.wake_by_ref();
            }
            gate.wait();
            shared.ended.fetch_add(1, Ordering::AcqRel);
            Ok(())
        });
        (look, state)
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

/// What [`Gate::look_up`] shares with its test: the runtime's waker, and how many items it stopped waiting for.
#[derive(Default)]
struct Held {
    waker: Mutex<Option<std::task::Waker>>,
    ended: AtomicUsize,
}

/// Turn Files until its look-up waits for `n` more items, which it tells `held` of, and give their paths.
fn until_held(run: &mut Run, held: &mpsc::Receiver<PathBuf>, n: usize) -> Vec<PathBuf> {
    let deadline = Instant::now() + HANG_GUARD;
    let mut paths: Vec<PathBuf> = Vec::new();
    loop {
        paths.extend(held.try_iter());
        if paths.len() >= n {
            return paths;
        }
        assert!(run.wait_for_progress(deadline), "the look-up never began");
        run.settle();
    }
}

/// Ask Files on `bus` to show `items` on a thread of its own, which sends the answer to `answered`.
fn ask_to_show(bus: &PrivateBus, items: &[PathBuf], answered: &mpsc::Sender<zbus::Result<()>>) {
    let call = file_manager(bus, "ShowItems", items.iter().map(|p| uri(p)).collect(), "");
    let answered = answered.clone();
    std::thread::spawn(move || {
        let _ = answered.send(call());
    });
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
    let mut run = files_noticed(bus, Notices::start(None), configure);
    run.wait_until(|run| run.rows().len() == 2);
    run
}

/// [`files`] posting its notices to `notices`, before its window is listed (none, when `configure` takes it away).
fn files_noticed(
    bus: &PrivateBus,
    notices: Notices,
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
    Run::start_configured(
        home,
        documents,
        places,
        notices,
        None,
        preferences,
        |start| configure(start.with_instance(Some(service))),
    )
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

/// How long a wait for an event may last before the test calls it a hang: far past what any event takes on a loaded
/// machine, and under the two minutes after which the runner kills a test.
const HANG_GUARD: Duration = Duration::from_secs(60);

/// Run `work` on a thread of its own while Files keeps turning, and give what it returned. The thread hands its result
/// over and then wakes the runtime, so the turning waits for Files' own threads and that one alike, and finds the result
/// after the wake that follows it; [`HANG_GUARD`] only ends a hang.
fn while_turning<T: Send + 'static>(run: &mut Run, work: impl FnOnce() -> T + Send + 'static) -> T {
    let waker = run.waker();
    let (done, result) = mpsc::channel();
    let work = std::thread::spawn(move || {
        // the result (or the work's panic) is sent before the wake: a thread that woke the runtime may not have ended
        let out = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work));
        let _ = done.send(out);
        waker.wake();
    });
    let deadline = Instant::now() + HANG_GUARD;
    loop {
        if let Ok(out) = result.try_recv() {
            work.join().unwrap();
            return out.unwrap_or_else(|panic| std::panic::resume_unwind(panic));
        }
        assert!(run.wait_for_progress(deadline), "it never ended");
        run.settle();
    }
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
fn activate_with_no_window_open_brings_the_hidden_progress_window_back_then_opens_one() {
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

    // so does Activate while the copy runs and the progress window is hidden
    close(&mut run, progress);
    assert!(run.windows().is_empty());
    let reopen = call(&bus, "Activate", (data("reopen-token"),));
    while_turning(&mut run, reopen).expect("Activate is taken");
    let progress = window(&run, "progress").expect("the progress window is back");
    present(&mut run, progress);
    assert_eq!(
        activations(&mut run, progress),
        [Some("reopen-token".to_owned())]
    );
    assert_eq!(run.windows().len(), 1, "no browser window comes with it");

    // Activate with the progress window shown and no browser window open opens one on the home folder
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

/// Call `method` of the file manager object on `bus` with these URIs and startup id, as another program's "Show in
/// folder" does.
fn file_manager(
    bus: &PrivateBus,
    method: &'static str,
    uris: Vec<String>,
    startup_id: &str,
) -> impl FnOnce() -> zbus::Result<()> + Send + 'static {
    let connection = bus.connect();
    let startup_id = startup_id.to_owned();
    move || {
        connection
            .call_method(
                Some(FILE_MANAGER_NAME),
                FILE_MANAGER_PATH,
                Some(FILE_MANAGER_INTERFACE),
                method,
                &(uris, startup_id),
            )
            .map(drop)
    }
}

fn uri(path: &Path) -> String {
    Target::Folder(path.to_path_buf()).uri()
}

/// Make `window` the one the helpers act on, and wait until its list shows `rows` items.
fn look_at(run: &mut Run, window: WindowId, rows: usize) {
    run.activate(window);
    run.wait_until(|run| run.rows().len() == rows);
}

#[test]
fn files_owns_the_file_manager_name_beside_its_own() {
    let Some(bus) = bus() else { return };
    let dbus = zbus::blocking::fdo::DBusProxy::new(&bus.connect()).unwrap();
    let owner_of = |name: &str| {
        dbus.get_name_owner(name.try_into().unwrap())
            .ok()
            .map(|o| o.to_string())
    };
    // another file manager holds the name: Files waits in line and gets it once that one lets go
    let other = bus.connect();
    zbus::blocking::fdo::DBusProxy::new(&other)
        .unwrap()
        .request_name(
            FILE_MANAGER_NAME.try_into().unwrap(),
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        )
        .unwrap();
    let service = Service::serve(bus.builder()).expect("its own name is free");
    let files = owner_of(BUS_NAME).expect("Files owns its name");
    assert_ne!(owner_of(FILE_MANAGER_NAME).as_ref(), Some(&files));
    drop(other);
    let deadline = Instant::now() + Duration::from_secs(10);
    while owner_of(FILE_MANAGER_NAME).as_ref() != Some(&files) {
        assert!(Instant::now() < deadline, "the name never came to Files");
        std::thread::yield_now();
    }
    drop(service);
    wait_until_unowned(&bus);
    let deadline = Instant::now() + Duration::from_secs(10);
    while owner_of(FILE_MANAGER_NAME).is_some() {
        assert!(
            Instant::now() < deadline,
            "Files kept the file manager name"
        );
        std::thread::yield_now();
    }
}

#[test]
fn show_items_selects_the_items_of_one_folder_in_the_window_that_shows_it() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let first = run.window;
    present(&mut run, first);
    let documents = run.home().join("Documents");
    let show = file_manager(
        &bus,
        "ShowItems",
        vec![
            uri(&documents.join("big.bin")),
            uri(&documents.join("Notes")),
        ],
        "show-token",
    );
    while_turning(&mut run, show).expect("ShowItems is taken");
    assert_eq!(run.windows().len(), 1, "the window on the folder is used");
    assert_eq!(run.selected_rows(), ["big.bin", "Notes"]);
    assert_eq!(
        activations(&mut run, first),
        [Some("show-token".to_owned())],
        "raised with the startup id"
    );

    // Files has no information window: the items' properties select them too, here with no startup id
    let properties = file_manager(
        &bus,
        "ShowItemProperties",
        vec![uri(&documents.join("Notes"))],
        "",
    );
    while_turning(&mut run, properties).expect("ShowItemProperties is taken");
    assert_eq!(run.selected_rows(), ["Notes"]);
    assert_eq!(
        activations(&mut run, first),
        [Some("show-token".to_owned()), None]
    );

    // the folder spelled otherwise is the same folder: its window is used again
    let spelled = format!("{}//./Notes/../big.bin", documents.display());
    let again = file_manager(&bus, "ShowItems", vec![spelled], "");
    while_turning(&mut run, again).expect("ShowItems is taken");
    assert_eq!(run.windows().len(), 1, "no second window on the folder");
    assert_eq!(run.selected_rows(), ["big.bin"]);
}

#[test]
fn show_items_opens_a_window_a_folder_with_its_items_selected() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let home = run.home();
    let notes = home.join("Documents/Notes");
    let pictures = home.join("Pictures");
    std::fs::create_dir_all(&pictures).unwrap();
    for name in ["a.png", "b.png", "c.png"] {
        std::fs::write(pictures.join(name), b"x").unwrap();
    }
    std::fs::write(notes.join("todo.txt"), b"x").unwrap();
    // an absolute path is taken as a URI is
    let show = file_manager(
        &bus,
        "ShowItems",
        vec![
            uri(&pictures.join("c.png")),
            notes.join("todo.txt").display().to_string(),
            uri(&pictures.join("a.png")),
        ],
        "show-token",
    );
    while_turning(&mut run, show).expect("ShowItems is taken");
    assert_eq!(run.windows().len(), 3, "one new window a folder");
    let on_pictures = window(&run, "browser/1").expect("a window on Pictures");
    let on_notes = window(&run, "browser/2").expect("a window on Notes");
    assert_eq!(title(&mut run, on_pictures), "Pictures");
    assert_eq!(title(&mut run, on_notes), "Notes");
    look_at(&mut run, on_pictures, 3);
    run.wait_until(|run| run.selected_rows() == ["a.png", "c.png"]);
    look_at(&mut run, on_notes, 1);
    run.wait_until(|run| run.selected_rows() == ["todo.txt"]);
    assert!(
        activations(&mut run, on_pictures).is_empty(),
        "one window is raised"
    );
    assert_eq!(
        activations(&mut run, on_notes),
        [Some("show-token".to_owned())],
        "the last, once it is shown"
    );
}

#[test]
fn show_folders_opens_each_folder() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let notes = run.home().join("Documents/Notes");
    let show = file_manager(&bus, "ShowFolders", vec![uri(&notes)], "folder-token");
    while_turning(&mut run, show).expect("ShowFolders is taken");
    let opened = window(&run, "browser/1").expect("a window on the folder");
    assert_eq!(title(&mut run, opened), "Notes");
    present(&mut run, opened);
    assert_eq!(
        activations(&mut run, opened),
        [Some("folder-token".to_owned())]
    );
}

/// The `..` after a symlinked folder leads to its target's parent, as opening the path does: the item is shown there.
#[test]
fn show_items_follows_a_symlinked_folder_s_parent_as_the_disk_does() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let home = run.home();
    let pictures = home.join("Pictures");
    std::fs::create_dir_all(pictures.join("trips")).unwrap();
    std::fs::write(pictures.join("a.png"), b"x").unwrap();
    std::os::unix::fs::symlink(pictures.join("trips"), home.join("Documents/trips")).unwrap();
    let through = format!("{}/Documents/trips/../a.png", home.display());
    let show = file_manager(&bus, "ShowItems", vec![through], "");
    while_turning(&mut run, show).expect("ShowItems is taken");
    let opened = window(&run, "browser/1").expect("a window on the link's target's parent");
    assert_eq!(title(&mut run, opened), "Pictures");
    look_at(&mut run, opened, 2);
    run.wait_until(|run| run.selected_rows() == ["a.png"]);

    // a folder spelled so is opened where the disk finds it, too
    let folder = format!("{}/Documents/trips/..", home.display());
    let show = file_manager(&bus, "ShowFolders", vec![folder], "");
    while_turning(&mut run, show).expect("ShowFolders is taken");
    let opened = window(&run, "browser/2").expect("a window on the folder");
    assert_eq!(title(&mut run, opened), "Pictures");
}

/// The window goes on answering while the items of a `ShowItems` call are looked up on a mount that does not answer;
/// the call is answered, and its item selected, once the look-up ends.
#[test]
fn the_window_answers_while_the_items_of_a_call_are_looked_up() {
    let Some(bus) = bus() else { return };
    let gate = Gate::new();
    let (to_held, held) = mpsc::channel();
    let (look, state) = Gate::look_up(&gate, to_held);
    let mut run = files(&bus, |start| start.with_look_up(look));
    *state.waker.lock().unwrap() = Some(run.waker());
    let first = run.window;
    present(&mut run, first);
    let big = run.home().join("Documents/big.bin");
    let (answered, answer) = mpsc::channel();
    ask_to_show(&bus, std::slice::from_ref(&big), &answered);
    assert_eq!(until_held(&mut run, &held, 1), [big]);

    // the look-up waits at the gate, and the window still takes a click and shows it
    run.click_row("Notes");
    assert_eq!(run.selected_rows(), ["Notes"]);
    assert_eq!(
        state.ended.load(Ordering::Acquire),
        0,
        "the look-up still waits"
    );
    assert!(answer.try_recv().is_err(), "the caller waits for it");

    gate.release();
    let answer = while_turning(&mut run, move || {
        answer.recv_timeout(HANG_GUARD).expect("an answer")
    });
    answer.expect("ShowItems is taken");
    run.wait_until(|run| run.selected_rows() == ["big.bin"]);
}

/// A look-up held by a mount that does not answer is answered at its deadline with what was found by then (here,
/// nothing: each item is a notice), and gives its place back, though its thread still waits: a call past the places
/// the held ones took is still looked up, and Files still exits once nothing is left open.
#[test]
fn a_look_up_a_mount_holds_is_answered_at_its_deadline_and_gives_its_place_back() {
    let Some(bus) = bus() else { return };
    let gate = Gate::new();
    let (to_held, held) = mpsc::channel();
    let (look, state) = Gate::look_up(&gate, to_held);
    let mut run = files(&bus, |start| start.with_look_up(look));
    *state.waker.lock().unwrap() = Some(run.waker());
    let first = run.window;
    let hung = run.home().join("hung");
    let deadline = LOOK_UP_DEADLINE.as_millis() as u64;
    let (answered, answers) = mpsc::channel();
    let answer = || {
        answers
            .recv_timeout(Duration::from_secs(10))
            .expect("the caller is answered at the deadline")
    };

    // every place is taken by a call whose item does not answer
    for i in 0..MAX_LOOK_UPS {
        ask_to_show(&bus, &[hung.join(format!("{i}.txt"))], &answered);
    }
    until_held(&mut run, &held, MAX_LOOK_UPS);
    assert!(answers.try_recv().is_err(), "no answer before the deadline");
    run.frames(deadline - 500, 500);
    assert!(answers.try_recv().is_err(), "no answer before the deadline");
    run.frames(500, 500);
    for _ in 0..MAX_LOOK_UPS {
        answer().expect("answered with what was found");
    }
    assert_eq!(run.windows(), [first], "nothing found, nothing shown");

    // the threads still wait, and the places are free again
    ask_to_show(&bus, &[hung.join("next.txt")], &answered);
    assert_eq!(until_held(&mut run, &held, 1), [hung.join("next.txt")]);
    close(&mut run, first);
    assert!(
        !run.driver.runtime().exited(),
        "it waits for the call's look-up"
    );
    run.frames(deadline, 500);
    answer().expect("answered with what was found");
    assert!(
        run.driver.runtime().exited(),
        "it exits once the call is answered"
    );
    assert_eq!(state.ended.load(Ordering::Acquire), 0, "the threads wait");
    gate.release();
}

/// A notification server that hands over the summary of each notification.
struct Summaries(Mutex<mpsc::Sender<String>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Summaries {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        _body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let _ = self.0.lock().unwrap().send(summary);
        1
    }
}

#[test]
fn an_item_that_does_not_exist_is_a_notice_and_the_others_are_still_shown() {
    let Some(bus) = bus() else { return };
    let (sent, summaries) = mpsc::channel();
    let _server = bus
        .blocking_builder()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            Summaries(Mutex::new(sent)),
        )
        .unwrap()
        .build()
        .unwrap();
    let mut run = files_noticed(&bus, Notices::start(Some(bus.connect())), |start| start);
    run.wait_until(|run| run.rows().len() == 2);
    let documents = run.home().join("Documents");
    let notice = || {
        summaries
            .recv_timeout(Duration::from_secs(10))
            .expect("a notice")
    };
    let show = file_manager(
        &bus,
        "ShowItems",
        vec![
            uri(&documents.join("gone.txt")),
            uri(&documents.join("big.bin")),
        ],
        "",
    );
    while_turning(&mut run, show).expect("the call still succeeds");
    assert_eq!(run.selected_rows(), ["big.bin"]);
    let said = notice();
    assert!(said.contains("gone.txt"), "{said}");

    // nothing the call names is there: a notice, and no window
    let home = run.home();
    let gone = file_manager(
        &bus,
        "ShowItems",
        vec![uri(&home.join("nowhere/x.txt"))],
        "",
    );
    while_turning(&mut run, gone).expect("taken");
    assert_eq!(run.windows().len(), 1);
    let said = notice();
    assert!(said.contains("x.txt"), "{said}");
    let folder = file_manager(&bus, "ShowFolders", vec![uri(&home.join("nowhere"))], "");
    while_turning(&mut run, folder).expect("taken");
    assert_eq!(run.windows().len(), 1);
    let said = notice();
    assert!(said.contains("nowhere"), "{said}");
}

#[test]
fn a_file_manager_call_naming_no_local_item_is_refused() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    for uris in [
        vec!["sftp://host/x".to_owned()],
        vec!["relative.txt".to_owned()],
    ] {
        let refused = file_manager(&bus, "ShowItems", uris, "");
        let e = while_turning(&mut run, refused).expect_err("refused");
        assert_eq!(error_name(&e), "org.freedesktop.DBus.Error.InvalidArgs");
    }
    let empty = file_manager(&bus, "ShowItems", Vec::new(), "");
    while_turning(&mut run, empty).expect("no item is nothing to do");
    assert_eq!(run.windows().len(), 1);
}

#[test]
fn a_files_started_for_a_call_opens_only_what_the_call_asks_for() {
    let Some(bus) = bus() else { return };
    let mut run = files_noticed(&bus, Notices::start(None), |start| start.without_window());
    assert!(run.windows().is_empty(), "no window of its own");
    let documents = run.home().join("Documents");
    let show = file_manager(
        &bus,
        "ShowItems",
        vec![uri(&documents.join("big.bin"))],
        "activation-token",
    );
    while_turning(&mut run, show).expect("ShowItems is taken");
    let opened = window(&run, "browser/0").expect("a window on the folder");
    look_at(&mut run, opened, 2);
    run.wait_until(|run| run.selected_rows() == ["big.bin"]);
    assert_eq!(
        activations(&mut run, opened),
        [Some("activation-token".to_owned())]
    );
    close(&mut run, opened);
    assert!(
        run.driver.runtime().exited(),
        "it exits with its last window"
    );
}

#[test]
fn a_files_started_for_a_call_that_never_comes_exits_after_the_wait() {
    let Some(bus) = bus() else { return };
    let mut run = files_noticed(&bus, Notices::start(None), |start| start.without_window());
    let wait = cupertino_files::kit::CALL_WAIT.as_millis() as u64;
    run.frames(wait - 1000, 1000);
    assert!(!run.driver.runtime().exited(), "it waits for the call");
    run.frames(1000, 1000);
    assert!(run.driver.runtime().exited());
}

#[test]
fn the_program_the_bus_starts_leaves_the_calls_to_a_running_files() {
    let Some(bus) = bus() else { return };
    let mut run = files(&bus, |start| start);
    let home = run.home();
    let (ok, said) = run_launch(
        &mut run,
        launch(&bus, &home, &home, &["--gapplication-service"]),
    );
    assert!(ok, "it exits at once: {said}");
    assert_eq!(run.windows().len(), 1, "and asks for nothing");
    let (ok, said) = run_launch(
        &mut run,
        launch(&bus, &home, &home, &["--gapplication-service", "Documents"]),
    );
    assert!(!ok && said.contains("takes no folder"), "{said}");
}
