//! The hosting runtime through its public interface against a stand-in server: a helper thread's panic fails its
//! component, a helper's result wakes the task that waits for it, a component's children are reaped as they end, and
//! a host whose server connection closed hears an error.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use cupertino_kit::core::{Application, Command, Window, WindowKey};
use cupertino_kit_shell::hosting::{Ended, Failure, HostEvent, Kind, Runtime};
use cupertino_kit_shell::outputs::OutputId;
use cupertino_kit_shell::{holds, host, transaction};
use cupertino_kit_shell::{Component, ComponentName, Entry, Event, Program, Role, Surface};
use support::{
    component_connection, connect_host, eventually, wait_for, Server, HELD, INSTANCE, PATIENCE,
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

static PLACING: Entry = Entry::of::<Placing>();
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
fn a_host_hears_an_error_when_its_server_connection_closes() {
    let (server, host_end) = Server::start(Kind::Shared, &[]);
    let mut host = connect_host(host_end);
    wait_for(&mut host, |event| matches!(event, HostEvent::Components(_)));
    server.close_host();
    let deadline = Instant::now() + PATIENCE;
    loop {
        match host.wait(Duration::from_millis(50)) {
            Err(_) => break,
            Ok(_) => assert!(
                Instant::now() < deadline,
                "the host waits on a closed connection"
            ),
        }
    }
    // and again: the connection stays closed, never an empty success
    assert!(host.wait(Duration::from_millis(50)).is_err());
}
