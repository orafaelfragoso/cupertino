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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A process table: pid, parent, start.
    fn table(rows: &[(u32, u32, u64)]) -> impl Fn(u32) -> Option<ProcessInfo> {
        let rows: HashMap<u32, ProcessInfo> = rows
            .iter()
            .map(|&(pid, parent, start)| (pid, ProcessInfo { parent, start }))
            .collect();
        move |pid| rows.get(&pid).copied()
    }

    /// The machine of the module docs: compositor 1 → dock 10 → terminal 20 → shell 30 → agent 40 → bridge 50.
    const CHAIN: [(u32, u32, u64); 6] = [
        (1, 0, 1),
        (10, 1, 5),
        (20, 10, 7),
        (30, 20, 8),
        (40, 30, 9),
        (50, 40, 12),
    ];

    fn clients(kinds: Vec<(u32, u64, ClientKind)>) -> impl Fn(u32, u64) -> Option<ClientKind> {
        move |pid, start| {
            kinds
                .iter()
                .find(|(p, s, _)| *p == pid && *s == start)
                .map(|(_, _, k)| k.clone())
        }
    }

    fn terminal() -> ClientKind {
        ClientKind::App(vec![
            "org.example.Terminal".into(),
            "org.example.Terminal".into(),
        ])
    }

    #[test]
    fn the_window_the_bridge_runs_under_is_its_app() {
        let found = responsible(
            50,
            12,
            table(&CHAIN),
            clients(vec![(10, 5, ClientKind::Shell), (20, 7, terminal())]),
        );
        assert_eq!(
            found,
            Ok(Responsible::Desktop("org.example.Terminal".into()))
        );
    }

    #[test]
    fn the_requester_must_still_be_the_process_that_connected() {
        let found = responsible(50, 11, table(&CHAIN), clients(vec![(20, 7, terminal())]));
        assert_eq!(found, Err(Unidentified::Reused));
        let found = responsible(51, 12, table(&CHAIN), clients(vec![(20, 7, terminal())]));
        assert_eq!(found, Err(Unidentified::Gone));
    }

    #[test]
    fn a_requester_without_its_parent_has_no_app() {
        // the agent exited and its id went to a later process
        let mut rows = CHAIN;
        rows[4] = (40, 30, 13);
        let found = responsible(50, 12, table(&rows), clients(vec![(20, 7, terminal())]));
        assert_eq!(found, Err(Unidentified::NoParent));
        let found = responsible(1, 1, table(&CHAIN), clients(Vec::new()));
        assert_eq!(found, Err(Unidentified::NoParent));
    }

    #[test]
    fn a_client_whose_id_was_reused_or_whose_parent_started_later_does_not_count() {
        // the terminal's display client is another process with the same id
        let found = responsible(50, 12, table(&CHAIN), clients(vec![(20, 6, terminal())]));
        assert_eq!(found, Ok(Responsible::Executable { pid: 40, start: 9 }));
        // the shell's id was reused by a process that started after the agent: the chain above it is not followed
        let mut rows = CHAIN;
        rows[3] = (30, 20, 10);
        let found = responsible(50, 12, table(&rows), clients(vec![(20, 7, terminal())]));
        assert_eq!(found, Ok(Responsible::Executable { pid: 40, start: 9 }));
    }

    #[test]
    fn shell_processes_and_the_x11_bridge_make_the_parent_executable_the_app() {
        for kind in [ClientKind::Shell, ClientKind::Xwayland] {
            let found = responsible(
                50,
                12,
                table(&CHAIN),
                clients(vec![(30, 8, kind), (20, 7, terminal())]),
            );
            assert_eq!(
                found,
                Ok(Responsible::Executable { pid: 40, start: 9 }),
                "the nearest client decides"
            );
        }
    }

    #[test]
    fn an_app_without_one_entry_is_named_by_the_parent_executable() {
        for entries in [
            Vec::new(),
            vec!["org.example.A".into(), "org.example.B".into()],
        ] {
            let found = responsible(
                50,
                12,
                table(&CHAIN),
                clients(vec![(20, 7, ClientKind::App(entries))]),
            );
            assert_eq!(found, Ok(Responsible::Executable { pid: 40, start: 9 }));
        }
        let found = responsible(50, 12, table(&CHAIN), clients(Vec::new()));
        assert_eq!(
            found,
            Ok(Responsible::Executable { pid: 40, start: 9 }),
            "no client at all"
        );
    }

    #[test]
    fn the_requester_itself_can_be_the_app() {
        let found = responsible(50, 12, table(&CHAIN), clients(vec![(50, 12, terminal())]));
        assert_eq!(
            found,
            Ok(Responsible::Desktop("org.example.Terminal".into()))
        );
    }

    #[test]
    fn the_walk_is_bounded() {
        // a chain longer than the bound, with the only client at its top
        let depth = MAX_ANCESTRY as u32 + 4;
        let rows: Vec<(u32, u32, u64)> = (1..=depth).map(|pid| (pid, pid - 1, 100)).collect();
        let found = responsible(
            depth,
            100,
            table(&rows),
            clients(vec![(1, 100, terminal())]),
        );
        assert_eq!(
            found,
            Ok(Responsible::Executable {
                pid: depth - 1,
                start: 100
            })
        );
        let near = MAX_ANCESTRY as u32;
        let found = responsible(near, 100, table(&rows), clients(vec![(1, 100, terminal())]));
        assert_eq!(
            found,
            Ok(Responsible::Desktop("org.example.Terminal".into())),
            "within the bound"
        );
    }
}
