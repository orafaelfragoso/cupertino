//! The app a control request comes from. The requester is the process on the other end of the controller's local
//! socket (an agent's bridge), named by its process id and start time so that a reused id is another process. Its app is
//! found up its parent chain: the first ancestor that is a connected display client decides. An application client whose
//! windows name one desktop entry makes that entry the app, so a terminal holds the permission for the tools run inside
//! it. Any other answer (no such ancestor, a shell process, the X11 bridge, no entry or several) makes the app the
//! executable of the requester's parent, the program that started the bridge.
//!
//! The walk reads the process table through a closure and the display clients through another, so it runs without
//! `/proc` or a display; the server supplies both.

/// How many processes up from the requester the walk looks for a display client. Agents run a few shells and
/// launchers deep under their window; a longer chain belongs to no app that holds a window.
pub const MAX_ANCESTRY: usize = 32;

/// What the process table says of one process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProcessInfo {
    /// The parent process id; zero for a process without a parent in this namespace.
    pub parent: u32,
    /// When the process started, in clock ticks since boot: a reused process id has a later start.
    pub start: u64,
}

/// A connected display client at one process.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ClientKind {
    /// A shell process or the compositor's own connection: never an app of its own.
    Shell,
    /// The bridge that shows X11 programs: its windows belong to many programs.
    Xwayland,
    /// An application, with the desktop entry ids its windows' app ids name (repeats allowed).
    App(Vec<String>),
}

/// The app found for a requester.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Responsible {
    /// The desktop entry id of the application window the requester runs under.
    Desktop(String),
    /// The executable of the requester's parent, this process and start time; the caller reads its path and checks the
    /// start again, so a parent that exited meanwhile names nothing.
    Executable { pid: u32, start: u64 },
}

/// Why a requester has no app; such a request is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Unidentified {
    /// The requester's process is gone.
    Gone,
    /// The process id now names a process that started at another time.
    Reused,
    /// The requester has no parent that is still the one that started it.
    NoParent,
}

/// The app of the requester `pid` that started at `start`: [`Responsible::Desktop`] when its nearest ancestor that is a
/// display client (`client`, given a process id and start) is an application whose windows name exactly one desktop
/// entry, else the executable of its parent. `process` reads the process table; the walk stops after [`MAX_ANCESTRY`]
/// processes, at a process without a parent, and where a parent started after its child (its id was reused).
pub fn responsible(
    pid: u32,
    start: u64,
    process: impl Fn(u32) -> Option<ProcessInfo>,
    client: impl Fn(u32, u64) -> Option<ClientKind>,
) -> Result<Responsible, Unidentified> {
    let requester = process(pid).ok_or(Unidentified::Gone)?;
    if requester.start != start {
        return Err(Unidentified::Reused);
    }
    let parent = parent_of(requester, &process).ok_or(Unidentified::NoParent)?;
    let fallback = Responsible::Executable {
        pid: requester.parent,
        start: parent.start,
    };
    let (mut pid, mut info) = (pid, requester);
    for _ in 0..MAX_ANCESTRY {
        if let Some(kind) = client(pid, info.start) {
            return Ok(match kind {
                ClientKind::App(entries) => {
                    one_entry(entries).map_or(fallback, Responsible::Desktop)
                }
                ClientKind::Shell | ClientKind::Xwayland => fallback,
            });
        }
        let Some(next) = parent_of(info, &process) else {
            break;
        };
        (pid, info) = (info.parent, next);
    }
    Ok(fallback)
}

/// The parent of a process described by `info`, when it exists and started no later than its child.
fn parent_of(
    info: ProcessInfo,
    process: &impl Fn(u32) -> Option<ProcessInfo>,
) -> Option<ProcessInfo> {
    if info.parent == 0 {
        return None;
    }
    process(info.parent).filter(|parent| parent.start <= info.start)
}

/// The entry when all of `entries` are one.
fn one_entry(entries: Vec<String>) -> Option<String> {
    let mut entries = entries.into_iter();
    let first = entries.next()?;
    entries.all(|e| e == first).then_some(first)
}
