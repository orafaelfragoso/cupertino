//! Kit instances: the parts of a process that each run Kit on a UI thread of their own, as the components of a shell
//! host do. What differs between instances is theirs, never the process's: a thread records its diagnostic events in
//! the scope of the instance it runs for, and the work it hands to [`task`](crate::task) runs for that instance too.

use cupertino_diagnostic_events as diag;
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::process::{Child, Command};
use std::sync::atomic::AtomicUsize;
use std::sync::{Arc, Mutex, PoisonError};

thread_local! {
    /// The instance the calling thread runs for, while one is entered.
    static CURRENT: RefCell<Option<Instance>> = const { RefCell::new(None) };
}

/// One Kit instance of the process. Clones are the same instance.
#[derive(Clone)]
pub struct Instance(Arc<Shared>);

struct Shared {
    name: Box<str>,
    /// Where its diagnostic events go; `None` records them as the process's.
    diag: Option<diag::Scope>,
    /// Its tasks running on the I/O pool now, changed only under the pool's lock.
    io_running: AtomicUsize,
    /// The programs it started and has not reaped yet.
    children: Mutex<Vec<Started>>,
}

impl fmt::Debug for Instance {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("Instance").field(&self.0.name).finish()
    }
}

impl PartialEq for Instance {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl Eq for Instance {}

impl Instance {
    /// An instance named `area` (`dock`, `menubar`), whose diagnostic events are its own: its panic is
    /// `<area>.panic` and its exit with an error `<area>.exit_error`.
    pub fn new(area: &'static str) -> Instance {
        Instance(Arc::new(Shared {
            name: area.into(),
            diag: Some(diag::Scope::new(area)),
            io_running: AtomicUsize::new(0),
            children: Mutex::new(Vec::new()),
        }))
    }

    /// The instance of a process that runs one: named by the program, its diagnostic events the process's.
    pub(crate) fn process() -> Instance {
        Instance(Arc::new(Shared {
            name: diag::profile::program().into(),
            diag: None,
            io_running: AtomicUsize::new(0),
            children: Mutex::new(Vec::new()),
        }))
    }

    /// The instance the calling thread runs for, if it entered one.
    pub fn current() -> Option<Instance> {
        CURRENT
            .try_with(|c| c.try_borrow().ok().and_then(|c| c.clone()))
            .ok()
            .flatten()
    }

    /// The calling thread runs for this instance until the guard is dropped, which restores what it ran for before.
    pub fn enter(&self) -> Entered {
        let diag = self.0.diag.as_ref().map(diag::Scope::enter);
        let previous = CURRENT.with(|c| c.replace(Some(self.clone())));
        Entered {
            previous,
            _diag: diag,
            _thread: PhantomData,
        }
    }

    pub fn name(&self) -> &str {
        &self.0.name
    }

    /// Its tasks running on the I/O pool now.
    pub(crate) fn io_running(&self) -> &AtomicUsize {
        &self.0.io_running
    }

    /// Reaps the programs it started that have ended, each reported as [`spawn`] says.
    pub(crate) fn reap(&self) {
        let mut children = self
            .0
            .children
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        children.retain_mut(|started| match started.child.try_wait() {
            Ok(None) => true,
            Ok(Some(status)) => {
                if !status.success() {
                    eprintln!(
                        "{}: {} exited with {status}",
                        started.process, started.program
                    );
                }
                false
            }
            Err(e) => {
                eprintln!("{}: {}: {e}", started.process, started.program);
                false
            }
        });
    }
}

/// A program an instance started, until it is reaped.
struct Started {
    /// The name of the process that started it, which its problems are reported under.
    process: Box<str>,
    program: String,
    child: Child,
}

/// Runs `command` detached from the calling thread's loop ([`cupertino_apps::launch::spawn`]) and hands it to the
/// instance the thread runs for, whose UI loop reaps it on its first turn after it ends; no thread waits for it. False
/// when it did not start. Problems, an unsuccessful exit included, are reported on standard error prefixed with
/// `process`, the caller's name. On a thread that runs for no instance the program is still started and is not reaped,
/// which is said on standard error.
pub fn spawn(process: &str, command: Command) -> bool {
    let program = command.get_program().to_string_lossy().into_owned();
    let Some(child) = cupertino_apps::launch::spawn(process, command) else {
        return false;
    };
    match Instance::current() {
        Some(instance) => instance
            .0
            .children
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Started {
                process: process.into(),
                program,
                child,
            }),
        None => {
            eprintln!("{process}: {program} is not reaped: no Kit instance runs on this thread")
        }
    }
    true
}

/// A thread's time running for an [`Instance`]; dropping it, on the thread that entered, restores what it ran for
/// before.
pub struct Entered {
    previous: Option<Instance>,
    _diag: Option<diag::Entered>,
    /// Bound to the thread whose instance it restores.
    _thread: PhantomData<*const ()>,
}

impl Drop for Entered {
    fn drop(&mut self) {
        let previous = self.previous.take();
        CURRENT.with(|c| *c.borrow_mut() = previous);
    }
}
