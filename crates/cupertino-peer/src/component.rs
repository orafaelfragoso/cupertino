//! Resolving a session bus caller to the server, a shell host component or a program the server started itself (D20).
//!
//! Callers are resolved without comparing process parent/child relations:
//! 1. The unique name that owns `org.cupertino.Session1` is [`ComponentCaller::Server`].
//! 2. A caller whose process `(pid, start_ticks)` matches one of `org.cupertino.Session1.ShellHosts` is asked of
//!    `org.cupertino.Session1.ComponentOf(unique_name)`: a non-empty answer is [`ComponentCaller::Component`], empty is
//!    [`ComponentCaller::Other`].
//! 3. Any other caller with a process is asked of `org.cupertino.Session1.ProgramOf(unique_name)`, which the server
//!    answers from the pidfds it holds of the programs it started: a non-empty answer is [`ComponentCaller::Program`],
//!    empty is [`ComponentCaller::Other`].
//! 4. An anonymous caller, or one whose answer fails, is [`ComponentCaller::Other`].

use std::io::Read;
use std::path::Path;

use zbus::names::{BusName, UniqueName};

use crate::{identify, Caller, ProcessHandle};

/// Well-known bus name owned by the compositor for the life of the session.
pub const SESSION_BUS_NAME: &str = "org.cupertino.Session1";
/// Object path of the session interface.
pub const SESSION_OBJECT_PATH: &str = "/org/cupertino/Session1";
/// Interface name of the session interface.
pub const SESSION_INTERFACE: &str = "org.cupertino.Session1";

/// Maximum bytes read from `/proc/<pid>/stat` when extracting a process start time.
const STAT_LIMIT: u64 = 4096;

/// Who a session-bus caller is with respect to the shell host and compositor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ComponentCaller {
    /// The display server (owner of `org.cupertino.Session1`).
    Server,
    /// An accepted shell component, named by its component table name (e.g. `"menubar"`, `"agent"`).
    Component(String),
    /// A program the server started itself, by the server's name for it (`"portal"`, `"login"`, `"desktop-control"`,
    /// `"screenshot"`, ...), as `ProgramOf` answers: a service that only the portal may ask checks for
    /// `Program("portal")`.
    Program(String),
    /// Neither the server nor an accepted component of a running shell host.
    Other,
}

/// Read field 22 (process start time in clock ticks since boot) of `/proc/<pid>/stat`, guarded by `process` not having
/// exited so a reused PID cannot match a live host.
pub fn process_start_ticks(process: &ProcessHandle) -> Option<u64> {
    let ticks = read_start_ticks(Path::new("/proc"), process.pid)?;
    (!process.has_exited()).then_some(ticks)
}

/// Parse field 22 (start time in ticks) of `<proc_root>/<pid>/stat`.
fn read_start_ticks(proc_root: &Path, pid: u32) -> Option<u64> {
    let mut stat = String::new();
    std::fs::File::open(proc_root.join(pid.to_string()).join("stat"))
        .ok()?
        .take(STAT_LIMIT)
        .read_to_string(&mut stat)
        .ok()?;
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

/// Pure decision step of D20 caller resolution, given the Session1 owner, the caller's unique name and
/// `(pid, start_ticks)` (`None` for a caller with no process), the running `ShellHosts` list, and the answers of
/// `ComponentOf` and `ProgramOf`, each asked only when its step is reached (`None` when the call failed).
pub fn classify_caller(
    session_owner: Option<&str>,
    sender: &str,
    caller_process: Option<(u32, u64)>,
    shell_hosts: &[(u32, u64)],
    component_of: impl FnOnce() -> Option<String>,
    program_of: impl FnOnce() -> Option<String>,
) -> ComponentCaller {
    if session_owner == Some(sender) {
        return ComponentCaller::Server;
    }
    let Some(proc_id) = caller_process else {
        return ComponentCaller::Other;
    };
    if shell_hosts.contains(&proc_id) {
        named(component_of(), ComponentCaller::Component)
    } else {
        named(program_of(), ComponentCaller::Program)
    }
}

/// `make` of a non-empty answer, [`ComponentCaller::Other`] of an empty or failed one.
fn named(answer: Option<String>, make: fn(String) -> ComponentCaller) -> ComponentCaller {
    match answer {
        Some(name) if !name.is_empty() => make(name),
        _ => ComponentCaller::Other,
    }
}

/// Resolve `sender` on `connection` using `org.cupertino.Session1` at `/org/cupertino/Session1`.
pub async fn resolve_component(
    connection: &zbus::Connection,
    sender: &UniqueName<'_>,
) -> ComponentCaller {
    resolve_component_at(
        connection,
        SESSION_BUS_NAME,
        SESSION_OBJECT_PATH,
        SESSION_INTERFACE,
        sender,
    )
    .await
}

/// Resolve `sender` against a session service at `(bus_name, object_path, interface)`.
pub async fn resolve_component_at(
    connection: &zbus::Connection,
    bus_name: &str,
    object_path: &str,
    interface: &str,
    sender: &UniqueName<'_>,
) -> ComponentCaller {
    let caller = identify(connection, sender).await;
    resolve_identified_at(
        connection,
        bus_name,
        object_path,
        interface,
        sender,
        &caller,
    )
    .await
}

/// Resolve `sender`, whose [`Caller`] credentials were already fetched (for example via [`crate::lookup::Lookup`]),
/// against `(bus_name, object_path, interface)`.
pub async fn resolve_identified_at(
    connection: &zbus::Connection,
    bus_name: &str,
    object_path: &str,
    interface: &str,
    sender: &UniqueName<'_>,
    caller: &Caller,
) -> ComponentCaller {
    let Ok(dbus) = zbus::fdo::DBusProxy::new(connection).await else {
        return ComponentCaller::Other;
    };
    let Ok(target_name) = BusName::try_from(bus_name) else {
        return ComponentCaller::Other;
    };
    let owner = dbus.get_name_owner(target_name.clone()).await.ok();
    if owner.as_deref().map(|o| o.as_str()) == Some(sender.as_str()) {
        return ComponentCaller::Server;
    }

    let process = match caller {
        Caller::Process(id) => &id.process,
        Caller::Unclassified(p) => p,
        Caller::Anonymous => return ComponentCaller::Other,
    };
    // a process whose start time cannot be read is no host's
    let start = process_start_ticks(process);

    let Ok(proxy) = zbus::Proxy::new(connection, target_name, object_path, interface).await else {
        return ComponentCaller::Other;
    };
    let Ok(hosts): zbus::Result<Vec<(u32, u64)>> = proxy.call("ShellHosts", &()).await else {
        return ComponentCaller::Other;
    };
    if process.has_exited() {
        return ComponentCaller::Other;
    }
    let host = start.is_some_and(|start| hosts.contains(&(process.pid, start)));
    let method = if host { "ComponentOf" } else { "ProgramOf" };
    let answer = proxy
        .call::<_, _, String>(method, &(sender.as_str(),))
        .await
        .ok();
    if host {
        named(answer, ComponentCaller::Component)
    } else {
        named(answer, ComponentCaller::Program)
    }
}
