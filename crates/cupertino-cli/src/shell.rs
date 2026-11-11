//! `cupertino shell …`: the shell processes the running desktop's server supervises (the dock, the menu bar, the
//! launcher…), listed and restarted one at a time through the server's `org.cupertino.Session1` interface on the
//! session bus.

use crate::{Arg, Args, Command, Context, Failure, FailureKind, Group, Reply, Run, Stability};
use serde::Serialize;
use std::fmt::Write as _;
use std::path::Path;
use std::time::Duration;

pub(crate) const GROUP: Group = Group {
    name: "shell",
    summary: "The shell processes of the running desktop: list them and restart one",
    description: "Commands for the processes the desktop's server starts and keeps running in the session: the dock, \
                  the menu bar, the launcher, the notifications, the control panel and the others its configuration \
                  names, and the built-in ones. They talk to the server of the current session over the session bus, \
                  which allows them to the session's own user alone, outside any Flatpak or Snap.\n\n\
                  When the system configuration declares a development folder (see `cupertino dev-folder enable`), a \
                  process it applies to starts the folder's file of its program's name at every start, when that file \
                  is a regular file of the user or root that no one else can write. These commands say which program \
                  each process runs and where the folder's file for it goes.",
    commands: &[COMMANDS],
};

const NAME: Arg = Arg::positional(
    "name",
    "the name of the process, as `cupertino shell list` shows it",
);

const COMMANDS: &[Command] = &[
    Command {
        path: &["shell", "list"],
        summary: "Lists the processes the server supervises, with their state, process, program and whether it is \
                  the development folder's",
        description: "Asks the server of the current session for every process it supervises, in the order they \
                      start. Each has its name, its state (running, stopping, waiting to start again, or stopped \
                      after it ended successfully), and while it runs its pid, its start time (also as clock ticks \
                      after boot, field 22 of /proc/PID/stat, which tells a process from a later one with the same \
                      pid), the program that runs and whether that is the development folder's. installed is the \
                      program of the configuration, and dev_program the file of the development folder the process \
                      starts when it is there (null when the folder does not apply to it).\n\n\
                      Without a session bus the command fails with exit code 3 (no_session_bus), without a desktop \
                      server on it also with 3 (no_server), and with a server that has no such interface with exit code \
                      8 (not_supported). A caller the server refuses fails with exit code 4 (refused).",
        examples: &[
            ("cupertino shell list", "Show the shell processes of this session."),
            (
                "cupertino --format json shell list",
                "Report them as one JSON envelope, for a script.",
            ),
        ],
        stability: Stability::Preview,
        args: &[],
        run: Run::Once(list),
    },
    Command {
        path: &["shell", "restart"],
        summary: "Ends one supervised process and starts it again at once, the development folder looked at again",
        description: "Asks the server to end the process <name> (SIGTERM, then SIGKILL when it has not ended 5 \
                      seconds later) and to start it again at once, whatever its exit status: a process the \
                      development folder applies to starts the folder's file when one is there and passes the checks, \
                      else its installed program. The command returns once the new process runs, with its pid, start \
                      time, program and whether that is the development folder's. A process that was not running \
                      starts at once.\n\n\
                      The process loses what it held only in memory. An unknown name fails with exit code 2 \
                      (unknown_process); a process that ended and could not start again fails with exit code 6 \
                      (not_started), and the server tries again after its restart throttle. The other failures are \
                      those of `shell list`.",
        examples: &[
            (
                "cupertino shell restart dock",
                "Restart the dock, from the development folder when it holds a build of it.",
            ),
            (
                "cupertino --format json shell restart menubar",
                "Restart the menu bar and report the new process as JSON.",
            ),
        ],
        stability: Stability::Preview,
        args: &[NAME],
        run: Run::Once(restart),
    },
    Command {
        path: &["shell", "use-installed"],
        summary: "Removes the process's file from the development folder, then restarts it on its installed program",
        description: "Takes the development build of <name> out of use: removes the file the process would start \
                      from the development folder (as `shell list` shows it in dev_program), when it is there, then \
                      restarts the process as `shell restart` does, so it runs its installed program again. Nothing \
                      else in the folder changes.\n\n\
                      A process the development folder does not apply to fails with exit code 5 (no_dev_program) \
                      without a restart; a file that cannot be removed fails with exit code 4 (cannot_remove). The \
                      other failures are those of `shell restart`.",
        examples: &[(
            "cupertino shell use-installed dock",
            "Go back to the installed dock.",
        )],
        stability: Stability::Preview,
        args: &[NAME],
        run: Run::Once(use_installed),
    },
];

/// The server's bus name, object and interface.
const SERVER: &str = "org.cupertino.Session1";
const SERVER_PATH: &str = "/org/cupertino/Session1";

/// How long one call to the server may take: a restart waits for the process to end, at most its stop grace of 5
/// seconds, and then for it to start.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// A supervised process as the server sends it: name, state, pid, start ticks, start time in seconds since 1970,
/// program, from the development folder, installed program, development folder's file; 0 and empty for what is unknown.
type Record = (String, String, u32, u64, u64, String, bool, String, String);

/// A supervised process as the command reports it.
#[derive(Serialize)]
struct Process {
    name: String,
    state: String,
    pid: Option<u32>,
    /// Seconds since 1970.
    started: Option<u64>,
    /// Clock ticks after boot, field 22 of `/proc/PID/stat`.
    start_ticks: Option<u64>,
    program: Option<String>,
    from_dev_folder: bool,
    installed: String,
    dev_program: Option<String>,
}

impl From<Record> for Process {
    fn from(r: Record) -> Self {
        let text = |s: String| Some(s).filter(|s| !s.is_empty());
        Self {
            name: r.0,
            state: r.1,
            pid: Some(r.2).filter(|p| *p != 0),
            start_ticks: Some(r.3).filter(|t| *t != 0),
            started: Some(r.4).filter(|t| *t != 0),
            program: text(r.5),
            from_dev_folder: r.6,
            installed: r.7,
            dev_program: text(r.8),
        }
    }
}

impl Process {
    /// `development folder` or `installed`, while it runs.
    fn source(&self) -> &'static str {
        match (&self.program, self.from_dev_folder) {
            (None, _) => "-",
            (Some(_), true) => "development folder",
            (Some(_), false) => "installed",
        }
    }

    /// `pid 1234, started 2026-11-11 10:00:00 UTC, /usr/bin/cupertino-dock (installed)`
    fn running(&self) -> String {
        let mut line = String::new();
        if let Some(pid) = self.pid {
            let _ = write!(line, "pid {pid}");
        }
        if let Some(started) = self.started {
            let _ = write!(line, ", started {}", utc(started));
        }
        if let Some(program) = &self.program {
            let _ = write!(line, ", {program} ({})", self.source());
        }
        line
    }
}

/// `2026-11-11 10:00:00 UTC`
fn utc(seconds: u64) -> String {
    let days = seconds / 86_400;
    let time = seconds % 86_400;
    format!(
        "{} {:02}:{:02}:{:02} UTC",
        crate::manual::civil_date(days as i64),
        time / 3600,
        time / 60 % 60,
        time % 60
    )
}

/// The address of the session bus: `DBUS_SESSION_BUS_ADDRESS`, else the `bus` socket in `XDG_RUNTIME_DIR`.
fn bus_address(cx: &Context) -> Result<String, Failure> {
    let set = |name: &str| cx.var(name).filter(|v| !v.is_empty());
    if let Some(address) = set("DBUS_SESSION_BUS_ADDRESS") {
        return address.into_string().map_err(|_| {
            Failure::new(
                FailureKind::Unavailable,
                "no_session_bus",
                "DBUS_SESSION_BUS_ADDRESS is not UTF-8",
            )
        });
    }
    if let Some(socket) = set("XDG_RUNTIME_DIR").map(|dir| Path::new(&dir).join("bus")) {
        if socket.exists() {
            return Ok(format!("unix:path={}", socket.display()));
        }
    }
    Err(Failure::new(
        FailureKind::Unavailable,
        "no_session_bus",
        "no session bus: neither DBUS_SESSION_BUS_ADDRESS nor a bus socket in XDG_RUNTIME_DIR; run the command in the \
         desktop session",
    ))
}

/// Calls `method` of the server with `body` and reads its answer as `T`.
fn call<B, T>(cx: &Context, method: &str, body: &B) -> Result<T, Failure>
where
    B: serde::Serialize + zbus::zvariant::DynamicType,
    T: for<'d> serde::Deserialize<'d> + zbus::zvariant::Type,
{
    let address = bus_address(cx)?;
    let unavailable = |e: zbus::Error| {
        Failure::new(
            FailureKind::Unavailable,
            "no_session_bus",
            format!("cannot connect to the session bus at {address}: {e}"),
        )
    };
    let connection = zbus::blocking::connection::Builder::address(address.as_str())
        .and_then(|b| b.method_timeout(CALL_TIMEOUT).build())
        .map_err(unavailable)?;
    let reply = connection
        .call_method(Some(SERVER), SERVER_PATH, Some(SERVER), method, body)
        .map_err(|e| refusal(method, e))?;
    reply.body().deserialize().map_err(|e| {
        Failure::new(
            FailureKind::Unsupported,
            "not_supported",
            format!("the server answered {method} with something this command does not read: {e}"),
        )
    })
}

/// The error name under which the server's bus library sends a standard error it wraps, whose own name starts the
/// message.
const WRAPPED_ERROR: &str = "org.freedesktop.zbus.Error";

/// What a failed call means for the command.
fn refusal(method: &str, e: zbus::Error) -> Failure {
    let zbus::Error::MethodError(name, message, _) = &e else {
        return Failure::new(
            FailureKind::Unavailable,
            "no_server",
            format!("calling {method} on the desktop's server failed: {e}"),
        );
    };
    let message = message.clone().unwrap_or_default();
    // a standard error the server's bus library wraps arrives under its own name, the standard one first in the text
    let (name, message) = match message.split_once(": ") {
        Some((inner, rest)) if name.as_str() == WRAPPED_ERROR && inner.starts_with("org.") => {
            (inner.to_owned(), rest.to_owned())
        }
        _ => (name.to_string(), message),
    };
    match name.as_str() {
        "org.freedesktop.DBus.Error.ServiceUnknown" | "org.freedesktop.DBus.Error.NameHasNoOwner" => {
            Failure::new(
                FailureKind::Unavailable,
                "no_server",
                format!("no desktop server on the session bus ({SERVER}): is a Cupertino session running?"),
            )
        }
        "org.freedesktop.DBus.Error.UnknownMethod"
        | "org.freedesktop.DBus.Error.UnknownInterface"
        | "org.freedesktop.DBus.Error.UnknownObject" => Failure::new(
            FailureKind::Unsupported,
            "not_supported",
            format!(
                "the running server has no {method}: it is older than this command; it takes the shell commands \
                 from the next session that starts a newer server"
            ),
        ),
        "org.freedesktop.DBus.Error.InvalidArgs" => {
            Failure::usage("unknown_process", format!("{message}; see `cupertino shell list`"))
        }
        "org.freedesktop.DBus.Error.NoReply" | "org.freedesktop.DBus.Error.Timeout" => Failure::new(
            FailureKind::Temporary,
            "no_answer",
            format!("the server did not answer {method} in time"),
        ),
        error if error.starts_with("org.cupertino.Session1.Error.") => Failure::new(
            FailureKind::Denied,
            "refused",
            format!(
                "the server refuses this process ({message}): only an unsandboxed process of the session's own user \
                 may manage its shell processes"
            ),
        ),
        _ => Failure::new(
            FailureKind::Temporary,
            "not_started",
            format!("{message} ({name})"),
        ),
    }
}

fn processes(cx: &Context) -> Result<Vec<Process>, Failure> {
    let records: Vec<Record> = call(cx, "ListShellProcesses", &())?;
    Ok(records.into_iter().map(Process::from).collect())
}

fn restart_process(cx: &Context, name: &str) -> Result<Process, Failure> {
    let record: Record = call(cx, "RestartShellProcess", &(name,))?;
    Ok(Process::from(record))
}

#[derive(Serialize)]
struct Listed {
    processes: Vec<Process>,
}

fn list(_: &Args, cx: &Context) -> Result<Reply, Failure> {
    let processes = processes(cx)?;
    let width = processes
        .iter()
        .map(|p| p.name.len())
        .max()
        .unwrap_or(4)
        .max(4);
    let mut human = format!(
        "{:width$}  {:8}  {:>7}  {:18}  PROGRAM\n",
        "NAME", "STATE", "PID", "SOURCE"
    );
    for p in &processes {
        let pid = p.pid.map_or_else(|| "-".to_owned(), |pid| pid.to_string());
        let program = p.program.as_deref().unwrap_or(&p.installed);
        let _ = writeln!(
            human,
            "{:width$}  {:8}  {pid:>7}  {:18}  {program}",
            p.name,
            p.state,
            p.source()
        );
    }
    Ok(Reply::new(&Listed { processes }, human))
}

fn restart(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let name = args.value("name").expect("<name> is a positional argument");
    let process = restart_process(cx, name)?;
    let human = format!("{name} restarted: {}\n", process.running());
    Ok(Reply::new(&process, human))
}

#[derive(Serialize)]
struct UsedInstalled {
    /// The file taken out of the development folder; `None` when there was none.
    removed: Option<String>,
    process: Process,
}

fn use_installed(args: &Args, cx: &Context) -> Result<Reply, Failure> {
    let name = args.value("name").expect("<name> is a positional argument");
    let listed = processes(cx)?;
    let process = listed.iter().find(|p| p.name == name).ok_or_else(|| {
        Failure::usage(
            "unknown_process",
            format!("no shell process is named {name:?}; see `cupertino shell list`"),
        )
    })?;
    let file = process.dev_program.clone().ok_or_else(|| {
        Failure::new(
            FailureKind::Validation,
            "no_dev_program",
            format!(
                "the development folder does not apply to {name}: it always runs {}",
                process.installed
            ),
        )
    })?;
    let removed = match std::fs::remove_file(&file) {
        Ok(()) => Some(file),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => {
            return Err(Failure::new(
                FailureKind::Denied,
                "cannot_remove",
                format!("cannot remove {file}: {e}"),
            ))
        }
    };
    let process = restart_process(cx, name)?;
    let mut human = match &removed {
        Some(file) => format!("removed {file}\n"),
        None => format!("the development folder holds no {name} program\n"),
    };
    let _ = writeln!(human, "{name} restarted: {}", process.running());
    Ok(Reply::new(&UsedInstalled { removed, process }, human))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_is_written_as_utc() {
        assert_eq!(utc(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(utc(1_794_398_405), "2026-11-11 12:00:05 UTC");
    }

    #[test]
    fn what_the_server_does_not_know_is_null() {
        let p = Process::from((
            "osd".into(),
            "waiting".into(),
            0,
            0,
            0,
            String::new(),
            false,
            "/usr/bin/cupertino-osd".into(),
            String::new(),
        ));
        assert_eq!(
            (p.pid, p.started, p.program, p.dev_program),
            (None, None, None, None)
        );
    }
}
