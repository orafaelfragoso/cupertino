//! The process behind a connection is read off the compositor's loop and remembered; the loop waits for a reading or a
//! check until a deadline, and what does not answer in time names no process and allows nothing. A window with the
//! portal's prompt app id stays kept from desktop control unless its process is known to differ from the portal's.

use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_peer::ProcessHandle;
use cupertino_server::global_shortcuts::portal_prompt_verdict;
use cupertino_server::process::{read_facts, Alive, Lookups, ProcessFacts, ProcessLookup};

/// How long a test lets the loop wait for a lookup that does not answer.
const WAIT: Duration = Duration::from_millis(50);
/// Bounds what a loaded builder takes to start a thread or a process and see it end.
const SETTLE: Duration = Duration::from_secs(10);

/// Field 22 of `/proc/<pid>/stat`, counted from the last `)`.
fn start_of(pid: u32) -> u64 {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();
    let after_name = &stat[stat.rfind(')').unwrap() + 1..];
    after_name
        .split_whitespace()
        .nth(19)
        .unwrap()
        .parse()
        .unwrap()
}

fn until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + SETTLE;
    while !done() {
        assert!(Instant::now() < deadline, "{what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A sleeping child that is killed and reaped on drop so a failing assertion never leaks it.
struct Sleeper(std::process::Child);

impl Sleeper {
    fn id(&self) -> u32 {
        self.0.id()
    }

    fn kill_and_wait(mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for Sleeper {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A sleeping child that holds `env`, waited until the kernel has published its environment block.
fn sleeper(env: &[(&str, &str)]) -> Sleeper {
    let mut command = Command::new("sleep");
    command
        .arg("30")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_remove("XDG_ACTIVATION_TOKEN");
    for (name, value) in env {
        command.env(name, value);
    }
    let child = Sleeper(command.spawn().unwrap());
    let pid = child.id();
    until("the child's environment is published", || {
        std::fs::read(format!("/proc/{pid}/environ")).is_ok_and(|bytes| !bytes.is_empty())
    });
    child
}

#[test]
fn a_lookup_that_does_not_answer_in_time_answers_nothing_and_stays_counted_until_it_ends() {
    let lookups = Lookups::new(2);
    let (release, released) = mpsc::channel::<()>();
    let started = Instant::now();
    let answer = lookups.run(None, WAIT, move || {
        let _ = released.recv();
        7
    });
    assert_eq!(answer, None, "a stalled lookup answers nothing");
    assert!(
        started.elapsed() >= WAIT,
        "the loop waited for the deadline"
    );
    assert!(started.elapsed() < SETTLE, "and no longer");
    assert_eq!(lookups.alive(), 1, "the stalled thread stays counted");
    drop(release);
    until("the thread ends once its read ends", || {
        lookups.alive() == 0
    });
    assert_eq!(
        lookups.run(None, SETTLE, || 7),
        Some(7),
        "an answer in time"
    );
}

#[test]
fn at_a_limit_no_lookup_starts_and_the_loop_does_not_wait() {
    let lookups = Lookups::new(1);
    let (release, released) = mpsc::channel::<()>();
    assert!(lookups.spawn(None, move || {
        let _ = released.recv();
    }));
    let ran = Arc::new(AtomicBool::new(false));
    let flag = ran.clone();
    let started = Instant::now();
    let answer = lookups.run(None, SETTLE, move || flag.store(true, Ordering::SeqCst));
    assert_eq!(answer, None, "the limit refuses the lookup");
    assert!(started.elapsed() < SETTLE / 2, "without waiting for it");
    drop(release);
    until("the stalled thread ends", || lookups.alive() == 0);
    assert!(!ran.load(Ordering::SeqCst), "the refused lookup never ran");

    // a connection's own limit holds whatever the server's
    let lookups = Lookups::new(8);
    let owner = Alive::default();
    let (release, released) = mpsc::channel::<()>();
    assert!(lookups.spawn(Some((&owner, 1)), move || {
        let _ = released.recv();
    }));
    assert_eq!(lookups.run(Some((&owner, 1)), SETTLE, || 1), None);
    assert_eq!(
        lookups.run(Some((&Alive::default(), 1)), SETTLE, || 2),
        Some(2),
        "another connection still looks up"
    );
    drop(release);
    until("the owner's thread ends", || {
        owner.load(Ordering::SeqCst) == 0 && lookups.alive() == 0
    });
}

#[test]
fn a_connection_process_is_read_once_with_its_start_program_and_launch_token() {
    let child = sleeper(&[("XDG_ACTIVATION_TOKEN", "token-1")]);
    let pid = child.id();
    let handle = ProcessHandle::open(pid).unwrap();
    let lookup = Arc::new(ProcessLookup::default());
    assert!(lookup.start(
        &Lookups::new(4),
        (&Alive::default(), 2),
        move || read_facts(&handle)
    ));
    let facts = lookup.wait_until(Instant::now() + SETTLE).expect("read");
    assert_eq!(facts.process, (pid, start_of(pid)));
    assert_eq!(facts.launch_token.as_deref(), Some("token-1"));
    let program = std::fs::read_link(format!("/proc/{pid}/exe")).unwrap();
    assert_eq!(facts.program.as_deref(), Some(program.as_path()));
    assert_eq!(lookup.known(), Some(facts), "remembered");
    child.kill_and_wait();

    // the test's own program is the user's: it names no trusted executable
    if !rustix::process::getuid().is_root() {
        let own = ProcessHandle::open(std::process::id()).unwrap();
        let facts = read_facts(&own).expect("own process");
        assert_eq!(facts.trusted_executable, None);
        assert_eq!(facts.launch_token, None);
    }
}

#[test]
fn a_process_that_exited_names_nothing() {
    let child = sleeper(&[]);
    let handle = ProcessHandle::open(child.id()).unwrap();
    child.kill_and_wait();
    assert_eq!(read_facts(&handle), None);
}

#[test]
fn a_reading_that_stalls_is_waited_for_once_then_answers_at_once_until_it_ends() {
    let lookup = Arc::new(ProcessLookup::default());
    let (release, released) = mpsc::channel::<()>();
    let facts = ProcessFacts {
        process: (1, 2),
        program: None,
        trusted_executable: None,
        launch_token: None,
    };
    let read = facts.clone();
    assert!(
        lookup.start(&Lookups::new(4), (&Alive::default(), 2), move || {
            let _ = released.recv();
            Some(read)
        })
    );
    let started = Instant::now();
    assert_eq!(lookup.wait_until(started + WAIT), None, "not read in time");
    assert!(started.elapsed() >= WAIT);
    let again = Instant::now();
    assert_eq!(
        lookup.wait_until(again + SETTLE),
        None,
        "not waited for again"
    );
    assert!(again.elapsed() < SETTLE / 2, "answered at once");
    drop(release);
    until("the reading ends", || lookup.known().is_some());
    assert_eq!(lookup.wait_until(Instant::now()).as_deref(), Some(&facts));
}

#[test]
fn a_reading_with_no_thread_left_is_not_started_and_names_nothing_until_a_later_start() {
    let lookup = Arc::new(ProcessLookup::default());
    let owner = Alive::default();
    let (release, released) = mpsc::channel::<()>();
    let lookups = Lookups::new(4);
    assert!(lookups.spawn(Some((&owner, 1)), move || {
        let _ = released.recv();
    }));
    assert!(!lookup.start(&lookups, (&owner, 1), || None));
    assert!(lookup.is_unstarted());
    assert_eq!(lookup.wait_until(Instant::now() + SETTLE), None);
    drop(release);
    until("the owner's thread ends", || {
        owner.load(Ordering::SeqCst) == 0
    });
    let facts = ProcessFacts {
        process: (3, 4),
        program: None,
        trusted_executable: None,
        launch_token: None,
    };
    let read = facts.clone();
    assert!(lookup.start(&lookups, (&owner, 1), move || Some(read)));
    assert_eq!(
        lookup.wait_until(Instant::now() + SETTLE).as_deref(),
        Some(&facts)
    );
}

#[test]
fn a_prompt_window_is_ordinary_only_when_its_process_is_known_to_differ_from_the_portal() {
    let window = Some((10, 100));
    let portal = Some((20, 200));
    let rows = [
        ("the portal's own window", window, vec![window], true),
        ("another process's window", window, vec![portal], false),
        (
            "the window's process is not known",
            None,
            vec![portal],
            true,
        ),
        (
            "the portal's process is not known",
            window,
            vec![None],
            true,
        ),
        (
            "one portal connection not known",
            window,
            vec![portal, None],
            true,
        ),
        (
            "a reused id with another start",
            Some((20, 300)),
            vec![portal],
            false,
        ),
        ("no portal connection to compare", window, vec![], true),
    ];
    for (row, window, portals, kept) in rows {
        assert_eq!(portal_prompt_verdict(window, portals), kept, "{row}");
    }
}
