//! `kit::task`: work runs on the process's pools for the instance that asked, its result comes back to the UI thread as
//! a message, one instance holds at most its share of the I/O pool, an I/O task past its deadline is reported with its
//! instance, and a task's panic is the instance's.

mod support;

use cupertino_diagnostic_events::{self as diag, Record};
use cupertino_kit::core::{Application, Command, Window};
use cupertino_kit::task::{self, IO_SHARE};
use cupertino_kit::Instance;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

/// Where a task's work and its result ran.
#[derive(Clone, Debug)]
struct Ran {
    value: u64,
    /// The name of the thread the work ran on.
    worker: String,
    /// The instance the work ran for.
    instance: Option<String>,
    /// The thread the result was turned into a message on.
    done_on: ThreadId,
}

fn where_it_runs(value: u64) -> (u64, String, Option<String>) {
    (
        value,
        std::thread::current().name().unwrap_or_default().to_owned(),
        Instance::current().map(|i| i.name().to_owned()),
    )
}

fn ran((value, worker, instance): (u64, String, Option<String>)) -> Ran {
    Ran {
        value,
        worker,
        instance,
        done_on: std::thread::current().id(),
    }
}

struct Results;

impl Application for Results {
    type Model = Vec<Ran>;
    type Message = Ran;
    const RESIDENT: bool = true;

    fn init() -> (Vec<Ran>, Vec<Command<Ran>>) {
        let commands = vec![
            task::cpu(|| where_it_runs(42), ran),
            task::io(Duration::from_secs(10), || where_it_runs(7), ran),
        ];
        (Vec::new(), commands)
    }

    fn update(model: &mut Vec<Ran>, message: Ran) -> Vec<Command<Ran>> {
        model.push(message);
        Vec::new()
    }

    fn view(_: &Vec<Ran>) -> Vec<Window<Ran>> {
        Vec::new()
    }
}

#[test]
fn work_runs_on_the_pools_for_its_instance_and_its_result_comes_back_to_the_ui_thread_as_a_message()
{
    let _dock = Instance::new("dock").enter();
    let mut driver = support::driver::<Results>(support::text_system());
    support::run_until(&mut driver, |ran| ran.len() == 2);
    let mut ran = driver.runtime().model().clone();
    ran.sort_by_key(|r| r.value);
    let ui = std::thread::current().id();
    for (r, value, pool) in [(&ran[0], 7, "kit-io-"), (&ran[1], 42, "kit-cpu-")] {
        assert_eq!(r.value, value);
        assert!(r.worker.starts_with(pool), "{r:?}");
        assert_eq!(r.instance.as_deref(), Some("dock"), "{r:?}");
        assert_eq!(r.done_on, ui, "{r:?}");
    }
}

/// The I/O tasks of the instance `wide` running now, the most that ran at once, and the gate they wait at.
static RUNNING: AtomicUsize = AtomicUsize::new(0);
static MOST: AtomicUsize = AtomicUsize::new(0);
static GATE: (Mutex<(usize, bool)>, Condvar) = (Mutex::new((0, false)), Condvar::new());

/// Waits at the gate until it opens, counted as running meanwhile.
fn held() -> u64 {
    let now = RUNNING.fetch_add(1, Ordering::SeqCst) + 1;
    MOST.fetch_max(now, Ordering::SeqCst);
    let (gate, changed) = &GATE;
    let mut state = gate.lock().unwrap();
    state.0 += 1;
    changed.notify_all();
    while !state.1 {
        state = changed.wait(state).unwrap();
    }
    state.0 -= 1;
    drop(state);
    RUNNING.fetch_sub(1, Ordering::SeqCst);
    1
}

/// Asks for twice its share of I/O tasks that wait at the gate.
struct Wide;

impl Application for Wide {
    type Model = usize;
    type Message = u64;
    const RESIDENT: bool = true;

    fn init() -> (usize, Vec<Command<u64>>) {
        let commands = (0..IO_SHARE * 2)
            .map(|_| task::io(Duration::from_secs(60), held, |n| n))
            .collect();
        (0, commands)
    }

    fn update(model: &mut usize, _: u64) -> Vec<Command<u64>> {
        *model += 1;
        Vec::new()
    }

    fn view(_: &usize) -> Vec<Window<u64>> {
        Vec::new()
    }
}

/// Asks for one quick I/O task.
struct Quick;

impl Application for Quick {
    type Model = bool;
    type Message = ();
    const RESIDENT: bool = true;

    fn init() -> (bool, Vec<Command<()>>) {
        (
            false,
            vec![task::io(Duration::from_secs(60), || (), |()| ())],
        )
    }

    fn update(model: &mut bool, (): ()) -> Vec<Command<()>> {
        *model = true;
        Vec::new()
    }

    fn view(_: &bool) -> Vec<Window<()>> {
        Vec::new()
    }
}

#[test]
fn one_instance_runs_at_most_its_share_of_the_io_pool_and_another_instance_still_gets_its_turn() {
    let text = support::text_system();
    let mut wide = {
        let _wide = Instance::new("wide").enter();
        support::driver::<Wide>(text.clone())
    };
    wide.run_until_idle(std::num::NonZeroU32::new(1000).unwrap())
        .unwrap();
    {
        let (gate, changed) = &GATE;
        let state = gate.lock().unwrap();
        let (state, waited) = changed
            .wait_timeout_while(state, support::PATIENCE, |s| s.0 < IO_SHARE)
            .unwrap();
        assert!(!waited.timed_out(), "{} started", state.0);
    }
    let mut quick = {
        let _quick = Instance::new("quick").enter();
        support::driver::<Quick>(text)
    };
    support::run_until(&mut quick, |done| *done);
    assert_eq!(RUNNING.load(Ordering::SeqCst), IO_SHARE);
    {
        let (gate, changed) = &GATE;
        gate.lock().unwrap().1 = true;
        changed.notify_all();
    }
    support::run_until(&mut wide, |finished| *finished == IO_SHARE * 2);
    assert_eq!(MOST.load(Ordering::SeqCst), IO_SHARE);
}

/// Asks for an I/O task that takes longer than it says.
struct Late;

impl Application for Late {
    type Model = bool;
    type Message = ();
    const RESIDENT: bool = true;

    fn init() -> (bool, Vec<Command<()>>) {
        let slow = || std::thread::sleep(Duration::from_millis(400));
        (
            false,
            vec![task::io(Duration::from_millis(20), slow, |()| ())],
        )
    }

    fn update(model: &mut bool, (): ()) -> Vec<Command<()>> {
        *model = true;
        Vec::new()
    }

    fn view(_: &bool) -> Vec<Window<()>> {
        Vec::new()
    }
}

fn recorded() -> Vec<Record> {
    diag::recent()
        .iter()
        .map(|line| Record::parse(line.trim_end()).unwrap())
        .collect()
}

#[test]
fn an_io_task_past_its_deadline_is_reported_once_with_its_instance_and_still_finishes() {
    diag::init(diag::component!(), "kit_test", diag::Sink::Memory);
    let _menubar = Instance::new("menubar").enter();
    let mut driver = support::driver::<Late>(support::text_system());
    support::run_until(&mut driver, |done| *done);
    let overdue: Vec<Record> = recorded()
        .into_iter()
        .filter(|r| r.id == task::OVERDUE)
        .collect();
    assert_eq!(overdue.len(), 1, "{overdue:?}");
    let field = |k: &str| overdue[0].fields.get(k).cloned();
    assert_eq!(
        field("instance").and_then(|v| v.as_str().map(str::to_owned)),
        Some("menubar".to_owned())
    );
    assert_eq!(field("deadline").and_then(|v| v.as_u64()), Some(20));
    let site = field("site").and_then(|v| v.as_str().map(str::to_owned));
    assert!(
        site.as_deref().is_some_and(|s| s.starts_with("task.rs:")),
        "{site:?}"
    );
}

/// Asks for a CPU task that panics.
struct Broken;

impl Application for Broken {
    type Model = ();
    type Message = ();
    const RESIDENT: bool = true;

    fn init() -> ((), Vec<Command<()>>) {
        ((), vec![task::cpu(|| panic!("the task broke"), |()| ())])
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        Vec::new()
    }
}

#[test]
fn a_task_that_panics_is_recorded_as_its_instance_panic_and_goes_on_in_the_ui_thread_waiting_for_it(
) {
    diag::init(diag::component!(), "kit_test", diag::Sink::Memory);
    let _osd = Instance::new("osd").enter();
    let mut driver = support::driver::<Broken>(support::text_system());
    let ui = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        support::run_until(&mut driver, |()| false)
    }));
    let payload = ui.unwrap_err();
    assert_eq!(payload.downcast_ref::<&str>(), Some(&"the task broke"));
    let panics: Vec<Record> = recorded()
        .into_iter()
        .filter(|r| r.id.ends_with(".panic"))
        .collect();
    assert_eq!(panics.len(), 1, "{panics:?}");
    assert_eq!(panics[0].id, "osd.panic");
    let thread = panics[0].fields.get("thread").and_then(|v| v.as_str());
    assert!(
        thread.is_some_and(|t| t.starts_with("kit-cpu-")),
        "{thread:?}"
    );
}
