//! The hosting runtime through its public interface against a stand-in server: a helper thread's panic fails its
//! component, a panic in a component's teardown after a panic of its loop is caught, a component's end wakes its host's
//! wait, a helper that ignores its stop gets its host restarted, a helper's result wakes the task that waits for
//! it, a component's children are reaped as they end, a host whose server connection closed hears an error, and no
//! program a host starts inherits its connection.

use crate::support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cupertino_kit::core::{Application, Command, Window, WindowKey};
use cupertino_kit_shell::hosting::{Ended, Failure, HostEvent, Kind, Runtime};
use cupertino_kit_shell::outputs::OutputId;
use cupertino_kit_shell::{holds, host, transaction};
use cupertino_kit_shell::{Component, ComponentName, Entry, Event, Program, Role, Surface};
use support::{
    component_connection, connect_host, eventually, wait_for, Seen, Server, HELD, INSTANCE,
    PATIENCE,
};

static SPAWNED: AtomicBool = AtomicBool::new(false);
/// The window manager's first transaction and the holds it heard of.
static STARTED: Mutex<Option<(transaction::Id, Vec<String>)>> = Mutex::new(None);

fn output() -> OutputId {
    serde_json::from_value(serde_json::json!(17)).unwrap()
}

macro_rules! component {
    ($ty:ident, $name:expr, $init:expr) => {
        struct $ty;

        impl Application for $ty {
            type Model = ();
            type Message = ();

            fn init() -> ((), Vec<Command<()>>) {
                ((), $init())
            }

            fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
                Vec::new()
            }

            fn view(_: &()) -> Vec<Window<()>> {
                Vec::new()
            }
        }

        impl Program for $ty {
            fn surface(_: &(), _: &WindowKey) -> Surface {
                Surface {
                    role: Role::Window,
                    persistent: None,
                    holes: Vec::new(),
                }
            }

            fn event(_: Event) -> Option<()> {
                None
            }
        }

        impl Component for $ty {
            const NAME: ComponentName = $name;
        }
    };
}

// its task is never run: the helper's panic fails the component all the same
component!(Panicking, ComponentName::OSD, || {
    let _task = host::thread("boom", |_| panic!("a helper of the osd"), |()| ());
    Vec::new()
});

component!(Spawning, ComponentName::DOCK, || {
    SPAWNED.store(
        host::spawn(std::process::Command::new("/bin/true")),
        Ordering::SeqCst,
    );
    Vec::new()
});

component!(Placing, ComponentName::WINDOWMANAGER, || {
    let part = transaction::create(output());
    *STARTED.lock().unwrap() = Some((part.id(), holds()));
    Vec::new()
});

/// What the stuck helper waits for instead of its stop.
static STUCK_GATE: Mutex<Option<Receiver<()>>> = Mutex::new(None);

component!(Stuck, ComponentName::LAUNCHER, || {
    let gate = STUCK_GATE.lock().unwrap().take();
    let _task = host::thread(
        "stuck",
        move |_| {
            if let Some(gate) = gate {
                let _ = gate.recv_timeout(PATIENCE);
            }
        },
        |()| (),
    );
    Vec::new()
});

/// A model whose drop panics: dropped while its thread unwinds, it would abort the process.
struct Brittle;

impl Drop for Brittle {
    fn drop(&mut self) {
        panic!("the model panics as it is dropped");
    }
}

/// Panics as it handles its first message, and its model panics again in the teardown.
struct Twice;

impl Application for Twice {
    type Model = Brittle;
    type Message = ();

    fn init() -> (Brittle, Vec<Command<()>>) {
        (Brittle, vec![Command::Perform(Box::pin(async {}))])
    }

    fn update(_: &mut Brittle, (): ()) -> Vec<Command<()>> {
        panic!("the control panel panics as it handles a message");
    }

    fn view(_: &Brittle) -> Vec<Window<()>> {
        Vec::new()
    }
}

impl Program for Twice {
    fn surface(_: &Brittle, _: &WindowKey) -> Surface {
        Surface {
            role: Role::Window,
            persistent: None,
            holes: Vec::new(),
        }
    }

    fn event(_: Event) -> Option<()> {
        None
    }
}

impl Component for Twice {
    const NAME: ComponentName = ComponentName::CONTROLCENTER;
}

static TWICE: Entry = Entry::of::<Twice>();
static PLACING: Entry = Entry::of::<Placing>();
static STUCK: Entry = Entry::of::<Stuck>();
static PANICKING: Entry = Entry::of::<Panicking>();
static SPAWNING: Entry = Entry::of::<Spawning>();

/// Runs a task to its end on this thread.
fn finish<M>(command: Command<M>) -> M {
    match command {
        Command::Perform(task) => async_io::block_on(task),
        _ => panic!("a helper's command is a task"),
    }
}

/// The process ids of this process's children that ended and were not reaped.
fn zombies() -> Vec<u32> {
    let me = std::process::id().to_string();
    std::fs::read_dir("/proc")
        .unwrap()
        .filter_map(|entry| {
            let pid: u32 = entry.ok()?.file_name().to_str()?.parse().ok()?;
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // pid (comm) state ppid ...: the command may hold spaces and parentheses
            let rest = &stat[stat.rfind(')')? + 2..];
            let mut fields = rest.split(' ');
            let state = fields.next()?;
            let ppid = fields.next()?;
            (state == "Z" && ppid == me).then_some(pid)
        })
        .collect()
}

#[test]
fn a_helper_panic_fails_its_component_and_a_child_is_reaped_when_it_ends() {
    let (server, host_end) = Server::start(Kind::Shared, &["osd", "dock", "windowmanager"]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();

    let connection = component_connection(&mut host, "osd");
    let mut osd = runtime.start(&PANICKING, connection).unwrap();
    let deadline = Instant::now() + PATIENCE;
    let ended = loop {
        if let Some(ended) = osd.ended() {
            break ended;
        }
        assert!(Instant::now() < deadline, "the osd did not end");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(ended, Ended::Failed(Failure::Panic));

    let connection = component_connection(&mut host, "dock");
    let mut dock = runtime.start(&SPAWNING, connection).unwrap();
    eventually("the dock spawns its child", || {
        SPAWNED.load(Ordering::SeqCst)
    });
    // nothing spawns after it: the child is reaped because it ended, not at the next spawn
    eventually("the child is reaped", || zombies().is_empty());
    dock.stop();
    eventually("the dock stops", || dock.ended() == Some(Ended::Stopped));

    // a transaction is named by its creator, the instance the server numbered and the creator's serial
    let connection = component_connection(&mut host, "windowmanager");
    let mut window_manager = runtime.start(&PLACING, connection).unwrap();
    eventually("the window manager starts", || {
        STARTED.lock().unwrap().is_some()
    });
    let (id, held) = STARTED.lock().unwrap().clone().unwrap();
    assert_eq!(
        (id.component, id.instance, id.serial),
        (ComponentName::WINDOWMANAGER, INSTANCE, 1)
    );
    assert_eq!(held, vec![HELD.to_owned()]);
    window_manager.stop();
    eventually("the window manager stops", || {
        window_manager.ended().is_some()
    });

    drop(server);
}

#[test]
fn a_panic_in_a_components_teardown_after_its_loop_panicked_is_caught() {
    let (server, host_end) = Server::start(Kind::Shared, &["controlcenter"]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
    let connection = component_connection(&mut host, "controlcenter");
    let mut panel = runtime.start(&TWICE, connection).unwrap();
    // the model outlives the unwind of the loop, so its drop panics outside it and the process goes on
    eventually("the control panel ends", || panel.ended().is_some());
    assert_eq!(panel.ended(), Some(Ended::Failed(Failure::Panic)));
    drop(server);
}

#[test]
fn a_components_end_wakes_its_hosts_wait() {
    let (server, host_end) = Server::start(Kind::Shared, &["osd"]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
    let connection = component_connection(&mut host, "osd");
    let asked = Instant::now();
    let mut osd = runtime.start(&PANICKING, connection).unwrap();
    // the server says nothing more: only the component's end ends a wait before its time
    let ended = loop {
        host.wait(Some(PATIENCE))
            .expect("the host's connection stays up");
        if let Some(ended) = osd.ended() {
            break ended;
        }
        assert!(asked.elapsed() < PATIENCE, "the osd did not end");
    };
    assert_eq!(ended, Ended::Failed(Failure::Panic));
    assert!(
        asked.elapsed() < PATIENCE / 2,
        "the wait returned as the component ended, after {:?}",
        asked.elapsed()
    );
    drop(server);
}

#[test]
fn a_helper_that_ignores_its_stop_gets_its_host_restarted() {
    let (server, host_end) = Server::start(Kind::Shared, &["launcher"]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    let environment = cupertino_kit::app::environment().expect("toolkit environment");
    let runtime = Runtime::for_host(&mut host, Kind::Shared, environment).unwrap();
    let (release, gate) = mpsc::channel::<()>();
    *STUCK_GATE.lock().unwrap() = Some(gate);
    let connection = component_connection(&mut host, "launcher");
    let mut launcher = runtime.start(&STUCK, connection).unwrap();
    launcher.stop();
    let asked = Instant::now();
    eventually("the launcher ends", || launcher.ended().is_some());
    assert!(
        asked.elapsed() >= host::HELPER_JOIN_TIMEOUT,
        "the host waited for the helper"
    );
    server.expect(|seen| *seen == Seen::RestartRequested);
    release.send(()).unwrap();
    drop(server);
}

#[test]
fn a_transaction_made_outside_a_component_names_no_component() {
    let part = transaction::create(output());
    assert_ne!(part.id().component, ComponentName::OVERVIEW);
    assert_eq!(part.id().instance, 0);
    part.submit().cancel();
}

#[test]
fn a_helper_result_wakes_its_task_and_a_helper_panic_unwinds_it() {
    assert_eq!(finish(host::thread("seven", |_| 7, |n| n * 6)), 42);
    let stopped = finish(host::thread(
        "waits",
        |stop: &host::Stop| stop.wait(Duration::from_millis(10)),
        |stopped| stopped,
    ));
    assert!(!stopped);
    let unwound = std::panic::catch_unwind(|| {
        finish(host::thread("boom", |_| -> u32 { panic!("helper") }, |n| n))
    });
    let panic = unwound.expect_err("the task unwinds with the helper's panic");
    assert_eq!(panic.downcast_ref::<&str>(), Some(&"helper"));
}

#[test]
fn no_program_a_host_starts_inherits_its_connection() {
    use std::os::fd::{AsRawFd, BorrowedFd};

    let (_server, host_end) = Server::start(Kind::Shared, &[]);
    // the server hands the connection over with close-on-exec cleared, so the fd survives its exec
    rustix::io::fcntl_setfd(&host_end, rustix::io::FdFlags::empty()).unwrap();
    let fd = host_end.as_raw_fd();
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    // Safety: `host` owns the fd and lives to the end of the test.
    let flags = rustix::io::fcntl_getfd(unsafe { BorrowedFd::borrow_raw(fd) }).unwrap();
    assert!(flags.contains(rustix::io::FdFlags::CLOEXEC));
    let inherited = std::process::Command::new("test")
        .arg("-e")
        .arg(format!("/proc/self/fd/{fd}"))
        .status()
        .expect("test runs");
    assert!(
        !inherited.success(),
        "a child of the host holds its host-role connection"
    );
}

#[test]
fn a_host_hears_an_error_when_its_server_connection_closes() {
    let (server, host_end) = Server::start(Kind::Shared, &[]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    server.close_host();
    let deadline = Instant::now() + PATIENCE;
    loop {
        match host.wait(Some(Duration::from_millis(50))) {
            Err(_) => break,
            Ok(_) => assert!(
                Instant::now() < deadline,
                "the host waits on a closed connection"
            ),
        }
    }
    // and again: the connection stays closed, never an empty success
    assert!(host.wait(Some(Duration::from_millis(50))).is_err());
}
