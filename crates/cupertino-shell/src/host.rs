//! The host program (`shell-host.md#the-host-protocol`): its connection, its components and what follows each event
//! of the server, each end of a component and each pass of the watchdog.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use cupertino_kit_shell::bus::Message;
use cupertino_kit_shell::hosting::{
    Ended, Failure, Host, HostEvent, Kind, Liveness, Running, Runtime, Unavailable, EXIT_VERSION,
    LIVENESS_CHECK_INTERVAL,
};
use cupertino_kit_shell::{ComponentName, Entry, Error};

use crate::watchdog;

/// How long a component the host asked to stop may take to end before the host cuts it off (`Running::shut`): its
/// teardown is quick, so one still running then is stuck, and the watchdog takes it from there.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// The status the host exits with when its arguments are wrong.
pub const EXIT_USAGE: i32 = 2;

/// The status the host exits with when it cannot go on: its connection failed, or its render thread hangs.
pub const EXIT_FAILURE: i32 = 1;

/// How the host is started.
const USAGE: &str = "usage: cupertino-shell [--only COMPONENT]";

/// The component `--only` names, if any: such a host runs that component alone (D18), whatever else its list names.
/// A name outside the server's table is a usage error.
fn only(args: impl IntoIterator<Item = String>) -> Result<Option<ComponentName>, String> {
    let mut args = args.into_iter();
    let mut only = None;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--only" if only.is_none() => {
                let name = args.next().ok_or("--only needs a component")?;
                only = Some(
                    ComponentName::from_name(&name)
                        .ok_or_else(|| format!("{name:?} is not a component of the shell"))?,
                );
            }
            _ => return Err(format!("unknown argument {arg}")),
        }
    }
    Ok(only)
}

/// The id of the diagnostic event of a component that ended without being asked, or was lost to a hang.
const FAILED_EVENT: &str = "shell.component.failed";

/// Be the host of `components` with the arguments `args` (the program's, after its name), until the server says to
/// exit, a separate host's list becomes empty or loses its component, or the connection ends.
pub fn run(components: &'static [&'static Entry], args: impl IntoIterator<Item = String>) -> ! {
    let only = match only(args) {
        Ok(only) => only,
        Err(problem) => {
            eprintln!("cupertino-shell: {problem}\n{USAGE}");
            std::process::exit(EXIT_USAGE);
        }
    };
    let mut host = match Host::connect() {
        Ok(host) => host,
        Err(Error::Version { expected, found }) => {
            eprintln!(
                "cupertino-shell: the server offers version {found} of the host protocol, this host speaks {expected}"
            );
            std::process::exit(EXIT_VERSION);
        }
        Err(e) => {
            eprintln!("cupertino-shell: {e}");
            std::process::exit(EXIT_FAILURE);
        }
    };
    // the kind comes first, right after the host binds; what comes with it is handled once the runtime exists
    let mut kind = None;
    let mut early = Vec::new();
    while kind.is_none() {
        for event in wait(&mut host, None) {
            match event {
                HostEvent::Kind(k) => kind = Some(k),
                other => early.push(other),
            }
        }
    }
    let kind = kind.expect("the loop ends with the kind");
    let environment = match cupertino_kit::app::environment() {
        Ok(environment) => environment,
        Err(e) => {
            eprintln!("cupertino-shell: no toolkit environment: {e}");
            std::process::exit(EXIT_FAILURE);
        }
    };
    let runtime = match Runtime::for_host(&mut host, kind, environment) {
        Ok(runtime) => runtime,
        Err(e) => {
            eprintln!("cupertino-shell: {e}");
            std::process::exit(EXIT_FAILURE);
        }
    };
    let mut shell = Shell {
        host,
        runtime,
        kind,
        components,
        only,
        started: HashMap::new(),
        lost: Vec::new(),
        listed: Vec::new(),
        since: Instant::now(),
        next_check: Instant::now() + LIVENESS_CHECK_INTERVAL,
    };
    for event in early {
        shell.handle(event);
    }
    loop {
        let timeout = (!shell.started.is_empty())
            .then(|| shell.next_check.saturating_duration_since(Instant::now()));
        for event in wait(&mut shell.host, timeout) {
            shell.handle(event);
        }
        shell.reap();
        if !shell.started.is_empty() && Instant::now() >= shell.next_check {
            shell.next_check = Instant::now() + LIVENESS_CHECK_INTERVAL;
            shell.check();
        }
    }
}

/// What the server sent within `timeout` (`None`: until it sends something or a component ends); a host whose
/// connection ended has no server to run components for.
fn wait(host: &mut Host, timeout: Option<Duration>) -> Vec<HostEvent> {
    match host.wait(timeout.unwrap_or(Duration::MAX)) {
        Ok(events) => events,
        Err(e) => {
            eprintln!("cupertino-shell: {e}");
            std::process::exit(EXIT_FAILURE);
        }
    }
}

/// A component the host started.
struct Started {
    running: Running,
    started: Instant,
    /// What the host last reported of it; `Responsive` until it reports otherwise.
    reported: Liveness,
    /// The host asked it to stop, at that time; `again` asks for its connection once it ended.
    stopping: Option<Stopping>,
}

#[derive(Clone, Copy)]
struct Stopping {
    since: Instant,
    again: bool,
    /// Cut off after [`STOP_GRACE`].
    shut: bool,
}

/// The host, its runtime and its components.
struct Shell {
    host: Host,
    runtime: Runtime,
    kind: Kind,
    components: &'static [&'static Entry],
    /// The one component a host started with `--only` runs.
    only: Option<ComponentName>,
    started: HashMap<ComponentName, Started>,
    /// Components lost to a hang: their threads cannot be ended, so they are kept, cut off, until the process exits.
    lost: Vec<(ComponentName, Running)>,
    /// The server's last list.
    listed: Vec<ComponentName>,
    /// When the host started: the render thread's start-up grace.
    since: Instant,
    next_check: Instant,
}

impl Shell {
    fn handle(&mut self, event: HostEvent) {
        match event {
            HostEvent::Kind(_) => {}
            HostEvent::Components(list) => self.list(&list),
            HostEvent::Connection {
                component,
                connection,
            } => self.start(component, connection),
            HostEvent::Stopped { component } => {
                eprintln!(
                    "cupertino-shell: the server stopped {} after repeated failures",
                    component.as_str()
                );
            }
            HostEvent::ComponentStopped { component } => {
                self.runtime
                    .publish(Message::ComponentStopped { component });
            }
            HostEvent::RestartComponent { component } => self.stop(component, true),
            HostEvent::Restart => self.exit(),
        }
    }

    /// The entry this host was built with for `component`, when it runs it: a host started with `--only` runs that
    /// component alone.
    fn entry(&self, component: ComponentName) -> Option<&'static Entry> {
        if self.only.is_some_and(|only| only != component) {
            return None;
        }
        self.components
            .iter()
            .copied()
            .find(|entry| entry.name() == component)
    }

    /// The list the server sent: a connection for every listed component the host has, `unavailable` for the rest,
    /// and the components that left it stop and lose their object.
    fn list(&mut self, list: &[String]) {
        let names: Vec<ComponentName> = list
            .iter()
            .filter_map(|name| ComponentName::from_name(name))
            .collect();
        let gone: Vec<ComponentName> = self
            .listed
            .iter()
            .copied()
            .filter(|c| !names.contains(c))
            .collect();
        for component in gone {
            self.stop(component, false);
            self.host.release(component.as_str());
        }
        for name in list {
            match ComponentName::from_name(name).and_then(|c| self.entry(c)) {
                Some(entry) if !self.listed.contains(&entry.name()) => {
                    self.host.connect_component(name);
                }
                Some(_) => {}
                None => self.host.unavailable(name, Unavailable::Absent),
            }
        }
        self.listed = names;
        if self.kind == Kind::Separate && list.is_empty() {
            self.exit();
        }
    }

    /// Starts `component` on the connection the server handed over.
    fn start(
        &mut self,
        component: ComponentName,
        connection: cupertino_kit_shell::hosting::Connection,
    ) {
        let Some(entry) = self.entry(component) else {
            return;
        };
        if !self.listed.contains(&component) || self.started.contains_key(&component) {
            // a connection for a component the list no longer holds, or that runs: it closes as it drops
            return;
        }
        match self.runtime.start(entry, connection) {
            Ok(running) => {
                self.started.insert(
                    component,
                    Started {
                        running,
                        started: Instant::now(),
                        reported: Liveness::Responsive,
                        stopping: None,
                    },
                );
            }
            Err(Error::Version { expected, found }) => {
                eprintln!(
                    "cupertino-shell: {} binds version {expected} of an interface the server offers at {found}",
                    component.as_str()
                );
                self.host
                    .unavailable(component.as_str(), Unavailable::Version);
            }
            Err(e) => {
                eprintln!("cupertino-shell: {} did not start: {e}", component.as_str());
                self.host.failed(component, Failure::Error);
                self.host.connect_component(component.as_str());
            }
        }
    }

    /// Asks `component` to stop; with `again` the host asks for its connection once it ended.
    fn stop(&mut self, component: ComponentName, again: bool) {
        match self.started.get_mut(&component) {
            Some(started) => {
                started.running.stop();
                started.stopping = Some(Stopping {
                    since: Instant::now(),
                    again,
                    shut: false,
                });
            }
            None if again && self.listed.contains(&component) => {
                self.host.connect_component(component.as_str());
            }
            None => {}
        }
    }

    /// The components that ended since the last look: a failure is reported and followed by `connect`, which the
    /// server answers once the component's restart time passed; one that stopped as asked starts again when asked.
    fn reap(&mut self) {
        let mut ended = Vec::new();
        for (&component, started) in &mut self.started {
            if let Some(how) = started.running.ended() {
                ended.push((component, how, started.stopping));
            }
        }
        for (component, how, stopping) in ended {
            let Some(started) = self.started.remove(&component) else {
                continue;
            };
            let failure = match (how, stopping) {
                (Ended::Stopped, Some(_)) => None,
                (Ended::Stopped, None) => Some(Failure::Exit),
                (Ended::Failed(cause), _) => Some(cause),
            };
            if let Some(cause) = failure {
                // its sockets close with it, so the server and the bus clean up before the next instance
                started.running.shut();
                report_failure(component, &format!("{cause:?}"));
                self.host.failed(component, cause);
            }
            let again = stopping.is_none_or(|s| s.again);
            if again && self.listed.contains(&component) {
                self.host.connect_component(component.as_str());
            }
        }
    }

    /// The watchdog's pass: each component's loop, reported to the server as its state changes; a stop that took
    /// too long; the render thread.
    fn check(&mut self) {
        if watchdog::render_hung(self.runtime.render_busy_for(), self.since.elapsed()) {
            eprintln!("cupertino-shell: the render thread hangs; the host exits");
            std::process::exit(EXIT_FAILURE);
        }
        let mut lost = Vec::new();
        for (&component, started) in &mut self.started {
            if let Some(stopping) = &mut started.stopping {
                if !stopping.shut && stopping.since.elapsed() >= STOP_GRACE {
                    stopping.shut = true;
                    started.running.shut();
                }
            }
            let state = watchdog::liveness(started.running.busy_for(), started.started.elapsed());
            if state == started.reported {
                continue;
            }
            started.reported = state;
            if state == Liveness::Lost {
                lost.push(component);
            } else {
                self.host.report(component, state);
            }
        }
        for component in lost {
            self.lose(component);
        }
    }

    /// `component` is lost to a hang: its sockets are shut down before the server hears it, so it reaches nothing
    /// more, and its thread is kept until the process exits. A shared host asks to be started again, which the server
    /// grants at its quiet moment; a separate host exits at once.
    fn lose(&mut self, component: ComponentName) {
        let Some(started) = self.started.remove(&component) else {
            return;
        };
        started.running.shut();
        report_failure(component, "Lost");
        self.host.report(component, Liveness::Lost);
        self.lost.push((component, started.running));
        match self.kind {
            Kind::Shared => self.host.request_restart(),
            Kind::Separate => std::process::exit(0),
        }
    }

    /// Stops every component, waits for them at most [`STOP_GRACE`], cuts off those still running, and exits.
    fn exit(&mut self) -> ! {
        for started in self.started.values() {
            started.running.stop();
        }
        let deadline = Instant::now() + STOP_GRACE;
        loop {
            self.started
                .retain(|_, started| started.running.ended().is_none());
            let left = deadline.saturating_duration_since(Instant::now());
            if self.started.is_empty() || left.is_zero() {
                break;
            }
            // a component's end ends the wait
            let _ = self.host.wait(left);
        }
        for started in self.started.values() {
            started.running.shut();
        }
        std::process::exit(0);
    }
}

/// Tells the event store that `component` failed, and how.
fn report_failure(component: ComponentName, how: &str) {
    eprintln!("cupertino-shell: {} failed: {how}", component.as_str());
    diag::event(
        Severity::Error,
        FAILED_EVENT,
        "a shell component failed and is started again",
    )
    .field("component", Value::Public(component.as_str()))
    .field("how", Value::Public(how))
    .emit();
}
