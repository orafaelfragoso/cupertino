//! Kit instances: the parts of a process that each run Kit on a UI thread of their own, as the components of a shell
//! host do. What differs between instances is theirs, never the process's: a thread records its diagnostic events in
//! the scope of the instance it runs for.

use cupertino_diagnostic_events as diag;
use std::cell::RefCell;
use std::fmt;
use std::marker::PhantomData;
use std::sync::Arc;

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
        }))
    }

    /// The instance of a process that runs one: named by the program, its diagnostic events the process's.
    pub(crate) fn process() -> Instance {
        Instance(Arc::new(Shared {
            name: diag::profile::program().into(),
            diag: None,
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
