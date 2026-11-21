//! The host program of these tests: what `cupertino-shell` does, built from `cupertino_kit_shell::hosting`
//! (`docs/architecture/shell-host.md#the-host-program's-side`). The supervisor starts this test binary with
//! [`MARK`] among its arguments, a separate host with `--only NAME` after them; [`run`] then connects on
//! `WAYLAND_SOCKET`, asks for a connection for every component of its list it was built with and answers
//! `unavailable` for the others, starts each on its connection, checks their loops and the render thread every
//! [`LIVENESS_CHECK_INTERVAL`] and reports what it sees, starts a component again when the server asks, and stops
//! its components and exits at the server's `restart`, or as a separate host whose list became empty.
//!
//! What the host does goes to the report ([`report`]) as lines starting with `host`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_kit_shell::hosting::{
    Ended, Failure, Host, HostEvent, Kind, Liveness, Running, Runtime, Unavailable, BUSY_THRESHOLD,
    EXIT_VERSION, LIVENESS_CHECK_INTERVAL, LOST_THRESHOLD, STARTUP_GRACE, UNRESPONSIVE_THRESHOLD,
};
use cupertino_kit_shell::{ComponentName, Entry, Error};

use super::components::{report, report_to, ENTRIES};

/// The argument that makes this test binary the host: a filter after `--` that names no test.
pub const MARK: &str = "kit-shell-host";

/// How long the host waits for a component it stopped to end before it cuts it off.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// The report file the arguments after [`MARK`] name, when this process is the host.
pub fn report_file() -> Option<PathBuf> {
    let args: Vec<String> = std::env::args().collect();
    let at = args.iter().position(|a| a == MARK)?;
    args.get(at + 1).map(PathBuf::from)
}

/// A component the host started, and how its liveness was last reported.
struct Started {
    running: Running,
    started: Instant,
    reported: Liveness,
    /// The host asked it to stop, at that time; the server asked for it again when `again`.
    stopping: Option<(Instant, bool)>,
}

/// Be the host, reporting to `file`, until the server says to exit or closes the connection.
pub fn run(file: PathBuf) -> ! {
    report_to(file);
    let mut host = match Host::connect() {
        Ok(host) => host,
        Err(Error::Version { .. }) => std::process::exit(EXIT_VERSION),
        Err(e) => {
            report(format!("host failed {e}"));
            std::process::exit(1);
        }
    };
    let mut kind = None;
    let mut pending = Vec::new();
    while kind.is_none() {
        for event in wait(&mut host, LIVENESS_CHECK_INTERVAL) {
            match event {
                HostEvent::Kind(k) => kind = Some(k),
                other => pending.push(other),
            }
        }
    }
    let kind = kind.expect("the kind came first");
    let environment = cupertino_kit::app::environment().expect("the toolkit's environment");
    let runtime = Runtime::for_host(&mut host, kind, environment).expect("the host's runtime");
    report(format!("host {kind:?} {}", std::process::id()));
    let mut host = Hosted {
        host,
        runtime,
        kind,
        started: HashMap::new(),
        next_check: Instant::now() + LIVENESS_CHECK_INTERVAL,
        render_busy: false,
    };
    for event in pending {
        host.handle(event);
    }
    loop {
        let timeout = host.next_check.saturating_duration_since(Instant::now());
        for event in wait(&mut host.host, timeout) {
            host.handle(event);
        }
        if Instant::now() >= host.next_check {
            host.next_check = Instant::now() + LIVENESS_CHECK_INTERVAL;
            host.check();
        }
    }
}

/// What the server sent within `timeout`; a host whose connection ended has no server to run components for.
fn wait(host: &mut Host, timeout: Duration) -> Vec<HostEvent> {
    match host.wait(Some(timeout)) {
        Ok(events) => events,
        Err(e) => {
            report(format!("host connection ended {e}"));
            std::process::exit(0);
        }
    }
}

/// The host and the components it runs.
struct Hosted {
    host: Host,
    runtime: Runtime,
    kind: Kind,
    started: HashMap<ComponentName, Started>,
    next_check: Instant,
    /// Whether the last pass saw the render thread busy: it is reported on its own, as the loop of a component that
    /// waits on it is then busy too, from about the same time.
    render_busy: bool,
}

impl Hosted {
    fn handle(&mut self, event: HostEvent) {
        match event {
            HostEvent::Kind(_) => {}
            HostEvent::Components(list) => self.list(&list),
            HostEvent::Connection {
                component,
                connection,
            } => {
                let Some(entry) = entry(component) else {
                    return;
                };
                match self.runtime.start(entry, connection) {
                    Ok(running) => {
                        report(format!("host started {}", component.as_str()));
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
                        report(format!(
                            "host version {} {expected} {found}",
                            component.as_str()
                        ));
                        self.host
                            .unavailable(component.as_str(), Unavailable::Version);
                    }
                    Err(e) => {
                        report(format!("host refused {} {e}", component.as_str()));
                        self.host.failed(component, Failure::Error);
                    }
                }
            }
            HostEvent::Stopped { component } => {
                report(format!("host gave up {}", component.as_str()));
            }
            HostEvent::ComponentStopped { component } => {
                self.runtime
                    .publish(cupertino_kit_shell::bus::Message::ComponentStopped { component });
            }
            HostEvent::RestartComponent { component } => {
                report(format!("host restarts {}", component.as_str()));
                self.stop(component, true);
            }
            HostEvent::Restart => {
                report("host restart");
                self.exit();
            }
        }
    }

    /// The list the server sent: a connection for every component of it the host has, `unavailable` for the rest,
    /// and the components that left it stop.
    fn list(&mut self, list: &[String]) {
        let names: Vec<ComponentName> = list
            .iter()
            .filter_map(|name| ComponentName::from_name(name))
            .collect();
        let gone: Vec<ComponentName> = self
            .started
            .keys()
            .copied()
            .filter(|c| !names.contains(c))
            .collect();
        for component in gone {
            self.stop(component, false);
            self.host.release(component.as_str());
        }
        for name in list {
            match ComponentName::from_name(name).and_then(entry) {
                Some(entry) if !self.started.contains_key(&entry.name()) => {
                    self.host.connect_component(name);
                }
                Some(_) => {}
                None => self.host.unavailable(name, Unavailable::Absent),
            }
        }
        if self.kind == Kind::Separate && list.is_empty() {
            report("host list empty");
            self.exit();
        }
    }

    /// Ask `component` to stop; `again` starts it once it ended.
    fn stop(&mut self, component: ComponentName, again: bool) {
        if let Some(started) = self.started.get_mut(&component) {
            started.running.stop();
            started.stopping = Some((Instant::now(), again));
        } else if again {
            self.host.connect_component(component.as_str());
        }
    }

    /// Stop every component, wait for them, and exit.
    fn exit(&mut self) -> ! {
        for started in self.started.values() {
            started.running.stop();
        }
        let deadline = Instant::now() + STOP_GRACE;
        for (component, mut started) in self.started.drain() {
            while started.running.ended().is_none() {
                if Instant::now() >= deadline {
                    started.running.shut();
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            report(format!("host ended {}", component.as_str()));
        }
        std::process::exit(0);
    }

    /// The watchdog's pass: each component's loop and the render thread, reported to the server as their state
    /// changes; how the components that ended did.
    fn check(&mut self) {
        let render = self.runtime.render_busy_for();
        let render_busy = render.is_some_and(|busy| busy >= BUSY_THRESHOLD);
        if render_busy != self.render_busy {
            self.render_busy = render_busy;
            report(if render_busy {
                "host render busy"
            } else {
                "host render idle"
            });
        }
        let mut ended = Vec::new();
        for (&component, started) in &mut self.started {
            if let Some((since, again)) = started.stopping {
                match started.running.ended() {
                    Some(how) => ended.push((component, how, true, again)),
                    None if since.elapsed() >= STOP_GRACE => started.running.shut(),
                    None => {}
                }
                continue;
            }
            if let Some(how) = started.running.ended() {
                ended.push((component, how, false, false));
                continue;
            }
            let own = started.running.busy_for();
            let (busy, cause) = match (own, render) {
                (Some(own), Some(render)) if render > own => (Some(render), "render"),
                (Some(own), _) => (Some(own), "loop"),
                (None, Some(render)) => (Some(render), "render"),
                (None, None) => (None, ""),
            };
            let busy = busy.unwrap_or_default();
            let in_grace = started.started.elapsed() < STARTUP_GRACE;
            let state = if busy >= LOST_THRESHOLD {
                Liveness::Lost
            } else if busy >= UNRESPONSIVE_THRESHOLD && !in_grace {
                Liveness::Unresponsive
            } else if busy >= BUSY_THRESHOLD {
                Liveness::Busy
            } else {
                Liveness::Responsive
            };
            if state != started.reported {
                started.reported = state;
                report(format!(
                    "host liveness {} {state:?} {cause}",
                    component.as_str()
                ));
                self.host.report(component, state);
            }
        }
        for (component, how, asked, again) in ended {
            self.started.remove(&component);
            report(format!("host ended {} {how:?}", component.as_str()));
            match how {
                Ended::Stopped if asked => {}
                Ended::Stopped => self.host.failed(component, Failure::Exit),
                Ended::Failed(cause) => self.host.failed(component, cause),
            }
            // a failure is followed by `connect`, which the server answers once the restart time passed
            if again || !asked {
                self.host.connect_component(component.as_str());
            }
        }
    }
}

/// The entry this host was built with for `component`.
fn entry(component: ComponentName) -> Option<&'static Entry> {
    ENTRIES.iter().copied().find(|e| e.name() == component)
}
