//! File operations apart from the windows, run headless: a copy outlives the browser window that started it and the
//! progress window, Files exits only once it ended, Stop stops it, the progress window opens only after its delay, and
//! each failure, each end of an operation that lasted and each end nobody saw is one notification, whose click shows the
//! progress window again.

#![cfg(feature = "app")]

use crate::common;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use common::{place, Run};
use cupertino_files::kit::{Observer, Start, PROGRESS_WINDOW_DELAY};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::{Key, Role, WindowKey};
use cupertino_kit::platform::{Mods, WindowEvent, WindowId, WindowRequest, WindowStates};
use cupertino_kit::theme::{Appearance, ThemePreferences};
use cupertino_test_bus::{missing_tool, PrivateBus};
use zbus::zvariant::OwnedValue;

/// A gate the copy waits at once it copied part of a file, until the test opens it; `reached` hears when it waits.
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

/// The actions each notification offered, by its id.
type Offered = Arc<Mutex<Vec<(u32, Vec<String>)>>>;

/// What a fake notification service heard, as "summary / body", and the actions of each notification by its id.
struct Notifications {
    heard: Mutex<mpsc::Sender<String>>,
    actions: Offered,
}

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let mut all = self.actions.lock().unwrap();
        let id = all.len() as u32 + 1;
        all.push((id, actions));
        let _ = self
            .heard
            .lock()
            .unwrap()
            .send(format!("{summary} / {body}"));
        id
    }
}

/// A bus with a notification service, and what it hears.
struct Bus {
    bus: PrivateBus,
    service: zbus::blocking::Connection,
    heard: mpsc::Receiver<String>,
    actions: Offered,
}

impl Bus {
    /// The id of the last notification posted, which must offer a click.
    fn last_clickable(&self) -> u32 {
        let actions = self.actions.lock().unwrap();
        let (id, offered) = actions.last().expect("a notification was posted");
        assert_eq!(offered[0], "default", "a click on it does something");
        *id
    }

    /// A click on the notification `id`, as the service tells it: the activation token, then the action.
    fn click(&self, id: u32, token: &str) {
        for (member, body) in [("ActivationToken", token), ("ActionInvoked", "default")] {
            self.service
                .emit_signal(
                    None::<()>,
                    "/org/freedesktop/Notifications",
                    "org.freedesktop.Notifications",
                    member,
                    &(id, body),
                )
                .unwrap();
        }
    }
}

fn notifications() -> Option<Bus> {
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return None;
    };
    let (sent, heard) = mpsc::channel();
    let actions = Arc::default();
    let service = bus
        .blocking_builder()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            Notifications {
                heard: Mutex::new(sent),
                actions: Arc::clone(&actions),
            },
        )
        .unwrap()
        .build()
        .unwrap();
    Some(Bus {
        bus,
        service,
        heard,
        actions,
    })
}

/// Four chunks of a copy, so it can wait and stop between two of them.
const BIG: usize = 4 << 20;

/// A window on `home/Documents` (`Notes`, `big.bin`), posting to `notices`, with what `configure` makes of it.
fn window(notices: Notices, configure: impl FnOnce(Start) -> Start) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::write(documents.join("big.bin"), vec![5u8; BIG]).unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let preferences = ThemePreferences {
        appearance: Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    // a system that takes drops, for the drop of another application
    let host = common::Host {
        capabilities: cupertino_kit::platform::Capabilities {
            drag_and_drop: true,
            ..Default::default()
        },
        ..common::Host::default()
    };
    let mut run = Run::start_hosted(
        home,
        documents,
        places,
        notices,
        None,
        preferences,
        configure,
        host,
    );
    run.wait_until(|run| run.rows().len() == 2);
    run
}

/// Copy `big.bin` and paste it into `Notes`, in the window.
fn paste_into_notes(run: &mut Run) {
    run.click_row("big.bin");
    run.menu("copy");
    enter(run, "Notes");
    run.menu("paste");
}

/// Go into the folder `name` of the window, as Primary+Down on its row does.
fn enter(run: &mut Run, name: &str) {
    run.click_row(name);
    run.key_with(0xff54, None, Mods::PRIMARY);
    run.wait_until(|run| run.title() == name);
}

fn notes(run: &Run) -> PathBuf {
    run.home().join("Documents/Notes")
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The progress window, while it is shown.
fn progress(run: &Run) -> Option<WindowId> {
    let key = WindowKey(Key::from("progress"));
    run.driver
        .runtime()
        .windows()
        .into_iter()
        .find_map(|(k, id)| (k == key).then_some(id))
}

/// The names of the accessible nodes with `role` in `window`.
fn named_in(run: &Run, window: WindowId, role: Role) -> Vec<String> {
    let snapshot = run.driver.runtime().snapshot(window).unwrap();
    snapshot
        .accessibility
        .iter()
        .filter(|(_, node)| node.role == role)
        .filter_map(|(_, node)| node.name.clone())
        .collect()
}

/// Have the compositor configure `window`, so it draws a frame.
fn present(run: &mut Run, window: WindowId) {
    run.driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    run.settle();
}

/// Ask `window` to close, as its title bar's close button does.
fn close(run: &mut Run, window: WindowId) {
    run.driver
        .window_system_mut()
        .enqueue_window(window, WindowEvent::CloseRequested)
        .unwrap();
    run.settle();
}

/// Settle until `done` holds, with or without windows, waiting for a thread to wake the runtime between two looks;
/// the twenty seconds only guard against a hang.
fn wait(run: &mut Run, mut done: impl FnMut(&Run) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(run) {
        assert!(run.wait_for_progress(deadline), "it never happened");
        run.settle();
    }
}

fn exited(run: &Run) -> bool {
    run.driver.runtime().exited()
}

#[test]
fn a_copy_outlives_its_window_and_the_progress_window_and_files_exits_only_once_it_ended() {
    let Some(bus) = notifications() else {
        return;
    };
    let gate = Gate::new();
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let notices = Notices::start(Some(bus.bus.connect()));
    let mut run = window(notices.clone(), |start| {
        start
            .with_transfer_observer(observer)
            .with_progress_delay(Duration::ZERO)
    });
    paste_into_notes(&mut run);
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    wait(&mut run, |run| progress(run).is_some());
    let shown = progress(&run).unwrap();
    assert_eq!(
        named_in(&run, shown, Role::ProgressIndicator),
        ["Copying “big.bin” to “Notes”"]
    );
    let title = run
        .driver
        .window_system_mut()
        .spec(shown)
        .unwrap()
        .title
        .clone();
    assert_eq!(title, "Copy", "named by the kind of operation it lists");
    assert!(named_in(&run, shown, Role::Button).contains(&"Stop".to_owned()));

    // the browser window goes: the copy and the progress window stay, and so does Files
    let browser = run.window;
    close(&mut run, browser);
    assert_eq!(run.windows(), [shown]);
    assert!(!exited(&run), "Files stays while the copy runs");

    // the progress window's Close only hides it
    close(&mut run, shown);
    assert!(run.windows().is_empty());
    assert!(!exited(&run), "Files stays while the copy runs");
    notices.flush(Duration::from_secs(5));
    assert!(bus.heard.try_recv().is_err(), "nothing is said yet");

    // the copy goes on to its end, once, and Files exits after saying so
    gate.release();
    wait(&mut run, exited);
    let copied = std::fs::read(notes(&run).join("big.bin")).unwrap();
    assert_eq!(copied.len(), BIG);
    assert_eq!(names(&notes(&run)), ["big.bin"], "copied once");
    assert_eq!(
        bus.heard.recv_timeout(Duration::from_secs(5)).unwrap(),
        "Copied “big.bin” to “Notes” / "
    );
    assert!(
        bus.heard.try_recv().is_err(),
        "one notification for the copy"
    );
}

#[test]
fn stop_in_the_progress_window_stops_the_copy_and_leaves_nothing_half_written() {
    let Some(bus) = notifications() else {
        return;
    };
    let gate = Gate::new();
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let notices = Notices::start(Some(bus.bus.connect()));
    let mut run = window(notices.clone(), |start| {
        start
            .with_transfer_observer(observer)
            .with_progress_delay(Duration::ZERO)
    });
    paste_into_notes(&mut run);
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    wait(&mut run, |run| progress(run).is_some());
    let shown = progress(&run).unwrap();
    present(&mut run, shown);
    run.driver.look(shown, "files-progress-window").unwrap();
    run.window = shown;
    run.click_named(Role::Button, "Stop");
    gate.release();
    wait(&mut run, |run| progress(run).is_none());
    assert!(
        names(&notes(&run)).is_empty(),
        "a stopped copy leaves nothing half written"
    );
    assert_eq!(
        std::fs::read(run.home().join("Documents/big.bin"))
            .unwrap()
            .len(),
        BIG
    );
    assert!(!exited(&run), "the browser window is still open");
    notices.flush(Duration::from_secs(5));
    assert!(
        bus.heard.try_recv().is_err(),
        "a stop the person asked for is no news"
    );
}

#[test]
fn the_progress_window_opens_by_itself_only_after_its_delay_and_on_request() {
    let Some(bus) = notifications() else {
        return;
    };
    let gate = Gate::new();
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let notices = Notices::start(Some(bus.bus.connect()));
    let mut run = window(notices.clone(), |start| {
        start.with_transfer_observer(observer)
    });
    paste_into_notes(&mut run);
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    run.settle();
    let delay_ms = PROGRESS_WINDOW_DELAY.as_millis() as u64;
    run.frames(delay_ms - 20, 20);
    assert!(progress(&run).is_none(), "not before the delay");
    // the window's own row shows the copy at once
    assert_eq!(
        run.named(Role::ProgressIndicator),
        ["Copying “big.bin” to “Notes”"]
    );
    run.frames(20, 20);
    wait(&mut run, |run| progress(run).is_some());

    // hidden, it comes back from the menu bar and from a click on the window's row
    let shown = progress(&run).unwrap();
    close(&mut run, shown);
    assert!(progress(&run).is_none());
    assert!(!run.disabled().contains(&common::action("show-progress")));
    run.menu("show-progress");
    let shown = progress(&run).expect("Show Progress shows it");
    close(&mut run, shown);
    run.click_named(Role::ProgressIndicator, "Copying “big.bin” to “Notes”");
    assert!(progress(&run).is_some(), "a click on the row shows it");

    gate.release();
    wait(&mut run, |run| progress(run).is_none());
    assert_eq!(names(&notes(&run)), ["big.bin"], "copied once");
    assert!(run.disabled().contains(&common::action("show-progress")));
    // it ran past the delay: its end is one notification, though its window is open
    notices.flush(Duration::from_secs(5));
    assert_eq!(
        bus.heard.try_recv().unwrap(),
        "Copied “big.bin” to “Notes” / "
    );
    assert!(
        bus.heard.try_recv().is_err(),
        "one notification for the copy"
    );
}

#[test]
fn a_quick_operation_never_shows_the_progress_window_nor_a_notification() {
    let Some(bus) = notifications() else {
        return;
    };
    let notices = Notices::start(Some(bus.bus.connect()));
    let mut run = window(notices.clone(), |start| start);
    std::fs::write(run.home().join("Documents/a.txt"), "a").unwrap();
    run.wait_until(|run| run.rows().len() == 3);
    run.click_row("a.txt");
    run.menu("duplicate");
    run.wait_until(|run| run.rows().contains(&"a copy.txt".to_owned()));
    run.frames(PROGRESS_WINDOW_DELAY.as_millis() as u64 + 200, 20);
    assert!(progress(&run).is_none());
    assert_eq!(run.windows().len(), 1);
    notices.flush(Duration::from_secs(5));
    assert!(bus.heard.try_recv().is_err(), "a quick success is no news");
}

#[test]
fn a_name_taken_gives_the_arrival_a_free_one_and_a_failure_opens_the_progress_window() {
    let Some(bus) = notifications() else {
        return;
    };
    // every operation passes while the valve is open; it is closed to hold one until its window is gone
    let valve = Gate::new();
    valve.release();
    let held = Arc::clone(&valve);
    let observer: Observer = Arc::new(move |_| {
        let open = held.open.lock().unwrap();
        let _open = held
            .opened
            .wait_timeout_while(open, Duration::from_secs(20), |open| !*open)
            .unwrap();
    });
    let notices = Notices::start(Some(bus.bus.connect()));
    let mut run = window(notices.clone(), |start| {
        start.with_transfer_observer(observer)
    });
    let documents = run.home().join("Documents");
    std::fs::write(documents.join("Notes/a.txt"), "old").unwrap();
    std::fs::write(documents.join("a.txt"), "new").unwrap();
    std::fs::write(documents.join("secret.txt"), "s").unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(
        documents.join("secret.txt"),
        std::fs::Permissions::from_mode(0o000),
    )
    .unwrap();
    if std::fs::read(documents.join("secret.txt")).is_ok() {
        // a user who reads every file (root) has no failure to see
        return;
    }
    run.wait_until(|run| run.rows().len() == 4);

    // a conflict: the folder has the name, so the copy takes a free one and the old file stays
    run.click_row("a.txt");
    run.menu("copy");
    enter(&mut run, "Notes");
    run.menu("paste");
    run.wait_until(|run| run.rows().len() == 2);
    assert_eq!(names(&documents.join("Notes")), ["a copy.txt", "a.txt"]);
    assert_eq!(
        std::fs::read(documents.join("Notes/a.txt")).unwrap(),
        b"old"
    );
    assert!(progress(&run).is_none());

    // a failure: the progress window opens with what failed and why, and one notification says so too
    run.menu("go-back");
    run.wait_until(|run| run.title() == "Documents");
    run.click_row("secret.txt");
    run.menu("duplicate");
    wait(&mut run, |run| progress(run).is_some());
    let shown = progress(&run).unwrap();
    let texts = named_in(&run, shown, Role::StaticText);
    assert!(
        texts.contains(&"Could not duplicate “secret.txt”".to_owned()),
        "{texts:?}"
    );
    assert!(
        texts.contains(&"Permission denied.".to_owned()),
        "{texts:?}"
    );
    present(&mut run, shown);
    run.driver.look(shown, "files-progress-failure").unwrap();
    notices.flush(Duration::from_secs(5));
    assert_eq!(
        bus.heard.try_recv().unwrap(),
        "Could not duplicate “secret.txt” / Permission denied."
    );
    assert!(bus.heard.try_recv().is_err(), "one notification for it");

    // its Close forgets the failure; a click on the notification lists it again, raised with the click's token
    close(&mut run, shown);
    assert!(progress(&run).is_none());
    bus.click(bus.last_clickable(), "click-token");
    wait(&mut run, |run| progress(run).is_some());
    let shown = progress(&run).unwrap();
    present(&mut run, shown);
    let texts = named_in(&run, shown, Role::StaticText);
    assert!(
        texts.contains(&"Could not duplicate “secret.txt”".to_owned()),
        "{texts:?}"
    );
    assert!(run
        .driver
        .window_system_mut()
        .requests(shown)
        .unwrap()
        .contains(&WindowRequest::Activate(Some("click-token".into()))));
    assert_eq!(run.windows().len(), 2, "no other window opens");

    // with no browser window a failure is posted too, and opens the progress window again
    close(&mut run, shown);
    assert!(progress(&run).is_none());
    // still selected
    run.menu("copy");
    run.menu("go-enclosing-folder");
    run.wait_until(|run| run.title() != "Documents");
    let browser = run.window;
    *valve.open.lock().unwrap() = false;
    run.menu("paste");
    close(&mut run, browser);
    valve.release();
    wait(&mut run, |run| progress(run).is_some());
    assert!(!exited(&run), "the failure is still shown");
    let summary = bus.heard.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(
        summary.starts_with("Could not copy “secret.txt” to “"),
        "{summary}"
    );
    assert!(summary.ends_with(" / Permission denied."), "{summary}");
    let shown = progress(&run).unwrap();
    close(&mut run, shown);
    assert!(exited(&run), "nothing is left to show");
}

#[test]
fn closing_the_last_window_with_nothing_running_exits() {
    let mut run = window(Notices::start(None), |start| start);
    let browser = run.window;
    close(&mut run, browser);
    assert!(exited(&run));
}

#[test]
fn a_drop_from_another_application_is_copied_beside_the_window_and_its_source_told_once_it_ended() {
    use cupertino_kit::platform::{ClipboardData, DropAction};
    let gate = Gate::new();
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let mut run = window(Notices::start(None), |start| {
        start.with_transfer_observer(observer)
    });
    let elsewhere = run.home().join("Elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("big.bin"), vec![7u8; BIG]).unwrap();
    let list = vec![ClipboardData {
        mime: "text/uri-list".into(),
        bytes: format!("file://{}\r\n", elsewhere.join("big.bin").display()).into_bytes(),
    }];
    let at = run.row_bounds("Notes").center();
    let window = run.window;
    let both = [DropAction::Copy, DropAction::Move].into();
    let system = run.driver.window_system_mut();
    // Option copies
    let start = cupertino_kit::geometry::Point::new(1.0, 1.0);
    let token = system
        .drag_enter(window, start, list, both, Mods::NONE)
        .unwrap();
    system.drag_move(window, at, Mods::ALT).unwrap();
    run.settle();
    assert!(run.driver.window_system_mut().drag_drop(window).unwrap());
    run.settle();
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    run.settle();
    let finished = |run: &Run| {
        let system = run.driver.runtime().window_system();
        system.incoming_drag(token).unwrap().finished
    };
    assert_eq!(finished(&run), None, "the source waits for the copy");
    assert_eq!(
        run.named(Role::ProgressIndicator),
        ["Copying “big.bin” to “Notes”"],
        "the window answers while it copies"
    );
    gate.release();
    wait(&mut run, |run| finished(run).is_some());
    assert_eq!(finished(&run), Some(Some(DropAction::Copy)));
    assert_eq!(names(&notes(&run)), ["big.bin"]);
    assert_eq!(names(&elsewhere), ["big.bin"], "a copy leaves the original");
}

#[test]
fn new_folder_with_selection_moves_the_items_beside_the_window_then_edits_the_folders_name() {
    let mut run = window(Notices::start(None), |start| start);
    let documents = run.home().join("Documents");
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    run.wait_until(|run| run.rows().len() == 3);
    run.click_row("a.txt");
    run.click_row_with("big.bin", Mods::SHIFT);
    run.menu("new-folder-with-selection");
    run.wait_until(|run| {
        let runtime = run.driver.runtime();
        runtime
            .find(run.window, &Key::from("name-field/0"))
            .is_some()
    });
    assert_eq!(
        names(&documents.join("untitled folder")),
        ["a.txt", "big.bin"]
    );
}

#[test]
fn a_drop_of_the_windows_own_items_held_while_it_copies_does_not_make_other_drags_its_own() {
    use cupertino_kit::geometry::Point;
    use cupertino_kit::platform::{
        ClipboardData, DragProgress, DropAction, MouseButton, PlatformEvent, PointerEvent,
        Timestamp,
    };
    let gate = Gate::new();
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let mut run = window(Notices::start(None), |start| {
        start.with_transfer_observer(observer)
    });
    run.menu("view-icons");
    let window = run.window;
    let cells = run.icon_cells();
    let cell = |name: &str| cells.iter().find(|(n, _, _)| n == name).unwrap().1;
    let (big, folder) = (cell("big.bin"), cell("Notes"));
    // below the last row of icons: the folder shown, which holds big.bin
    let low = cells
        .iter()
        .map(|(_, r, _)| r.origin.y + r.size.height)
        .fold(0.0, f64::max);
    let background = Point::new(folder.center().x, low + 30.0);

    // big.bin is dragged out of the window and dropped on Notes with Option held: a copy, held open while it runs
    let pointer = |run: &mut Run, event| run.send(WindowEvent::Pointer(event, Timestamp(0)));
    pointer(
        &mut run,
        PointerEvent::Motion {
            position: big.center(),
        },
    );
    pointer(
        &mut run,
        PointerEvent::Button {
            position: big.center(),
            button: MouseButton::Left,
            pressed: true,
            serial: 1,
            mods: Mods::NONE,
        },
    );
    let moved = Point::new(big.center().x + 12.0, big.center().y + 6.0);
    pointer(&mut run, PointerEvent::Motion { position: moved });
    let started = run
        .driver
        .runtime()
        .window_system()
        .drags()
        .last()
        .cloned()
        .expect("a drag");
    let both: std::collections::BTreeSet<DropAction> = [DropAction::Copy, DropAction::Move].into();
    let system = run.driver.window_system_mut();
    let own = system
        .drag_enter(
            window,
            big.center(),
            started.request.data.clone(),
            both.clone(),
            Mods::NONE,
        )
        .unwrap();
    system
        .drag_move(window, folder.center(), Mods::ALT)
        .unwrap();
    run.settle();
    assert!(run.driver.window_system_mut().drag_drop(window).unwrap());
    // the source hears the drop was made, as the compositor tells it
    run.driver
        .window_system_mut()
        .enqueue(PlatformEvent::Drag {
            token: started.token,
            progress: DragProgress::Dropped,
        })
        .unwrap();
    run.settle();
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    run.settle();
    let finished = |run: &Run, token| {
        let system = run.driver.runtime().window_system();
        system.incoming_drag(token).unwrap().finished
    };
    assert_eq!(finished(&run, own), None, "held while it copies");

    // meanwhile another application drags a file onto the folder shown: it is no item of the window's, so it may go
    let elsewhere = run.home().join("Elsewhere");
    std::fs::create_dir(&elsewhere).unwrap();
    std::fs::write(elsewhere.join("x.txt"), "x").unwrap();
    let list = vec![ClipboardData {
        mime: "text/uri-list".into(),
        bytes: format!("file://{}\r\n", elsewhere.join("x.txt").display()).into_bytes(),
    }];
    let system = run.driver.window_system_mut();
    let other = system
        .drag_enter(window, Point::new(1.0, 1.0), list, both, Mods::NONE)
        .unwrap();
    system.drag_move(window, background, Mods::NONE).unwrap();
    run.settle();
    let chosen = run
        .driver
        .runtime()
        .window_system()
        .incoming_drag(other)
        .unwrap()
        .chosen;
    assert_eq!(chosen, Some(DropAction::Move), "within the device");

    gate.release();
    wait(&mut run, |run| finished(run, own).is_some());
    assert_eq!(finished(&run, own), Some(Some(DropAction::Copy)));
    assert_eq!(names(&notes(&run)), ["big.bin"]);
}
