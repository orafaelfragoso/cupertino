//! The `cupertino-netd` process: serves `org.cupertino.Network1` and a subset of
//! `org.freedesktop.NetworkManager` on the system bus and keeps the machine's network
//! configured from kernel events, DHCP and the Wi-Fi supplicant. See the README for how to run
//! it.

use std::convert::Infallible;
use std::os::fd::{AsFd, BorrowedFd};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

use cupertino_diagnostic_events as diag;
use cupertino_netd::auth::{AllowAll, Authorizer, LogindAuthorizer};
use cupertino_netd::daemon::{Bus, Config, Daemon};
use cupertino_netd::dns;
use cupertino_netd::engine::Engine;
use cupertino_netd::error::{start_failed, Error, Result};
use cupertino_netd::host::SystemHost;
use cupertino_netd::iwd::IwdSupplicant;
use cupertino_netd::model::DeviceKind;
use cupertino_netd::snapshot::Snapshot;
use cupertino_netd::supplicant::Supplicant;
use cupertino_netlink::nl80211::{is_unsupported, Event as WirelessEvent, Nl80211Monitor};
use cupertino_netlink::rfkill::DEVICE as RFKILL_DEVICE;
use cupertino_netlink::rtnetlink::RouteMonitor;
use rustix::event::{poll, PollFd, PollFlags};
use rustix::io::Errno;
use rustix::time::Timespec;

const USAGE: &str = "usage: cupertino-netd [--session-bus [--no-auth]] [--resolv-conf PATH]";

struct Options {
    session_bus: bool,
    no_auth: bool,
    resolv_conf: PathBuf,
}

impl Options {
    /// The first argument that is unknown, or a combination that is refused.
    fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Self {
            session_bus: false,
            no_auth: false,
            resolv_conf: PathBuf::from(dns::DEFAULT_PATH),
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--session-bus" => options.session_bus = true,
                "--no-auth" => options.no_auth = true,
                "--resolv-conf" => {
                    options.resolv_conf = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| "--resolv-conf needs a path".to_owned())?
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        // Skipping authorisation on the system bus would let any user switch the radio and join networks.
        if options.no_auth && !options.session_bus {
            return Err("--no-auth is only allowed with --session-bus".to_owned());
        }
        Ok(options)
    }
}

fn main() -> ExitCode {
    // first: the panic hook and the journal are in place for everything below
    diag::init(diag::component!(), "network", diag::Sink::Journal);
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("cupertino-netd: {message}\n{USAGE}");
            diag::exit_error(&message);
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = run(&options);
    eprintln!("cupertino-netd: {e}");
    diag::exit_error(&e);
    ExitCode::FAILURE
}

/// `result`, with its error recorded as `network.backend.start_failed` at `step`.
fn starting<T>(step: &'static str, result: Result<T>) -> Result<T> {
    result.inspect_err(|error| start_failed(step, error))
}

/// What a descriptor in the poll set is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Source {
    Routes,
    Wireless,
    Rfkill,
    Supplicant,
    Requests,
    Dhcp(u32),
}

/// Failures of one step are reported and the loop goes on: the state the model holds is the
/// truth, and the next event or timer retries.
fn report(errors: Vec<Error>) {
    for error in errors {
        eprintln!("cupertino-netd: {error}");
    }
}

fn run(options: &Options) -> Result<Infallible> {
    let authorizer: Box<dyn Authorizer> = if options.no_auth {
        Box::new(AllowAll)
    } else {
        Box::new(LogindAuthorizer::new(starting(
            "logind",
            cupertino_seat::connect().map_err(Error::Seat),
        )?))
    };
    // Listening before the first read, so no change between the two is missed.
    let route_monitor = starting("rtnetlink", RouteMonitor::open().map_err(Error::from))?;
    let wireless_monitor = match Nl80211Monitor::open() {
        Ok(monitor) => Some(monitor),
        Err(error) if is_unsupported(&error) => None,
        Err(error) => return starting("nl80211", Err(Error::Netlink(error))),
    };
    let host = starting(
        "host",
        SystemHost::open(Path::new(RFKILL_DEVICE), options.session_bus),
    )?;
    let bus = if options.session_bus {
        Bus::Session
    } else {
        Bus::System
    };
    let (daemon, requests) = Daemon::connect(Config { bus, authorizer })?;
    let supplicant = starting(
        "supplicant",
        IwdSupplicant::new(daemon.connection().clone()),
    )?;
    let mut engine = Engine::new(host, supplicant, options.resolv_conf.clone());

    let origin = Instant::now();
    let now = || origin.elapsed();
    if let Some(rfkill) = engine.host().rfkill() {
        let events = starting("rfkill", rfkill.read_events().map_err(Error::from))?;
        engine.on_rfkill(now(), &events);
    }
    engine.resync(now());
    let mut published = Snapshot::default();
    publish(&daemon, &engine.snapshot(), &mut published);
    report(engine.take_errors());
    daemon.claim_names()?;

    loop {
        let (ready, timed_out) = wait(
            &engine,
            &route_monitor,
            wireless_monitor.as_ref(),
            &requests,
            origin,
        )?;
        let time = now();
        if ready.contains(&Source::Routes) {
            let drained = route_monitor.drain()?;
            for error in &drained.errors {
                eprintln!("cupertino-netd: malformed rtnetlink message: {error}");
            }
            engine.on_route_events(time, drained.events);
            if drained.overflowed || drained.truncated > 0 {
                engine.resync(time);
            }
        }
        if let (true, Some(monitor)) =
            (ready.contains(&Source::Wireless), wireless_monitor.as_ref())
        {
            let drained = monitor.drain()?;
            for error in &drained.errors {
                eprintln!("cupertino-netd: malformed nl80211 message: {error}");
            }
            let mut events = drained.events;
            if drained.overflowed || drained.truncated > 0 {
                // events were lost: read every Wi-Fi device's list again
                events.extend(
                    engine
                        .model()
                        .devices()
                        .filter(|device| device.kind == DeviceKind::Wifi)
                        .map(|device| WirelessEvent::ScanResultsReady {
                            ifindex: device.ifindex,
                        }),
                );
            }
            engine.on_wireless_events(time, events);
        }
        if let (true, Some(rfkill)) = (ready.contains(&Source::Rfkill), engine.host().rfkill()) {
            let events = rfkill.read_events()?;
            engine.on_rfkill(time, &events);
        }
        if ready.contains(&Source::Supplicant) {
            engine.on_supplicant(time);
        }
        for source in &ready {
            if let Source::Dhcp(ifindex) = source {
                engine.on_dhcp_readable(time, *ifindex);
            }
        }
        if timed_out
            || engine
                .next_deadline()
                .is_some_and(|deadline| deadline <= time)
        {
            engine.on_timer(time);
        }
        if ready.contains(&Source::Requests) {
            for request in requests.drain()? {
                let result = engine.command(time, request.command.clone());
                request.reply(result);
            }
        }
        publish(&daemon, &engine.snapshot(), &mut published);
        report(engine.take_errors());
    }
}

/// Publishes the snapshot when it differs from the last one. A failure to signal is reported; the
/// objects serve the new state regardless.
fn publish(daemon: &Daemon, snapshot: &Snapshot, published: &mut Snapshot) {
    if snapshot == published {
        return;
    }
    if let Err(error) = daemon.publish(snapshot.clone()) {
        eprintln!("cupertino-netd: {error}");
    }
    *published = snapshot.clone();
}

/// Waits for a descriptor or the next DHCP timer (`origin` is the zero of the engine's times). Returns the sources that are ready and whether
/// the wait timed out.
fn wait(
    engine: &Engine<SystemHost, IwdSupplicant>,
    routes: &RouteMonitor,
    wireless: Option<&Nl80211Monitor>,
    requests: &cupertino_netd::daemon::Requests,
    origin: Instant,
) -> Result<(Vec<Source>, bool)> {
    let mut sources: Vec<(Source, BorrowedFd<'_>)> = vec![
        (Source::Routes, routes.as_fd()),
        (Source::Requests, requests.as_fd()),
    ];
    if let Some(wireless) = wireless {
        sources.push((Source::Wireless, wireless.as_fd()));
    }
    if let Some(rfkill) = engine.host().rfkill() {
        sources.push((Source::Rfkill, rfkill.as_fd()));
    }
    if let Some(fd) = engine.supplicant().as_fd() {
        sources.push((Source::Supplicant, fd));
    }
    sources.extend(
        engine
            .dhcp_fds()
            .into_iter()
            .map(|(ifindex, fd)| (Source::Dhcp(ifindex), fd)),
    );

    let timeout = engine.next_deadline().map(|deadline| {
        let remaining = deadline.saturating_sub(origin.elapsed());
        Timespec {
            tv_sec: remaining.as_secs() as i64,
            tv_nsec: i64::from(remaining.subsec_nanos()),
        }
    });
    let mut fds: Vec<PollFd<'_>> = sources
        .iter()
        .map(|(_, fd)| PollFd::new(fd, PollFlags::IN))
        .collect();
    loop {
        match poll(&mut fds, timeout.as_ref()) {
            Ok(_) => break,
            Err(Errno::INTR) => continue,
            Err(errno) => {
                return Err(Error::Syscall {
                    call: "poll",
                    errno,
                })
            }
        }
    }
    let ready: Vec<Source> = sources
        .iter()
        .zip(&fds)
        .filter(|(_, fd)| !fd.revents().is_empty())
        .map(|((source, _), _)| *source)
        .collect();
    Ok((ready.clone(), ready.is_empty()))
}
