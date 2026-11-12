//! The `cupertino-powerd` process: serves `org.freedesktop.UPower` and `org.cupertino.Power1`
//! on the system bus and keeps them current from kernel uevents and the lid switch. See the
//! README for how to run it.

use std::convert::Infallible;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::{Duration, Instant};

use cupertino_devices::Monitor;
use cupertino_diagnostic_events as diag;
use cupertino_powerd::auth::{AllowAll, Authorizer, LogindAuthorizer};
use cupertino_powerd::daemon::{Bus, Config, Daemon};
use cupertino_powerd::error::{start_failed, Error};
use cupertino_powerd::events::concerns_power;
use cupertino_powerd::lid::LidSwitch;
use cupertino_powerd::sysfs::Lid;
use rustix::event::{poll, PollFd, PollFlags};
use rustix::io::Errno;
use rustix::time::Timespec;

const USAGE: &str =
    "usage: cupertino-powerd [--session-bus [--no-auth]] [--sysfs-root PATH] [--history PATH]";

/// How often sysfs is read again when no uevent arrives. approximate: battery drivers do not
/// send an event for every change of charge or rate, so the figures would go stale; UPower
/// polls for the same reason. 30 s is our placeholder.
const POLL_INTERVAL_SECONDS: u64 = 30;

struct Options {
    session_bus: bool,
    no_auth: bool,
    sysfs_root: PathBuf,
    /// Where the charge history is kept.
    history_path: PathBuf,
}

impl Options {
    /// The first argument that is unknown, or a combination that is refused.
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self {
            session_bus: false,
            no_auth: false,
            sysfs_root: PathBuf::from("/sys"),
            history_path: PathBuf::from("/var/lib/cupertino/power-history"),
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--session-bus" => options.session_bus = true,
                "--no-auth" => options.no_auth = true,
                "--sysfs-root" => {
                    options.sysfs_root = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| "--sysfs-root needs a path".to_owned())?
                }
                "--history" => {
                    options.history_path = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or_else(|| "--history needs a path".to_owned())?
                }
                other => return Err(format!("unknown argument {other}")),
            }
        }
        // Skipping authorisation on the system bus would let any user set the brightness.
        if options.no_auth && !options.session_bus {
            return Err("--no-auth is only allowed with --session-bus".to_owned());
        }
        Ok(options)
    }
}

fn main() -> ExitCode {
    // first: the panic hook and the journal are in place for everything below
    diag::init(diag::component!(), "power", diag::Sink::Journal);
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("cupertino-powerd: {message}\n{USAGE}");
            diag::exit_error(&message);
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = run(&options);
    eprintln!("cupertino-powerd: {e}");
    diag::exit_error(&e);
    ExitCode::FAILURE
}

/// `result`, with its error recorded as `power.backend.start_failed` at `step`.
fn starting<T>(step: &'static str, result: Result<T, Error>) -> Result<T, Error> {
    result.inspect_err(|error| start_failed(step, error))
}

fn run(options: &Options) -> Result<Infallible, Error> {
    let mut lid_switch = starting("lid", LidSwitch::find(&options.sysfs_root))?;
    let lid = match &lid_switch {
        Some(switch) => Lid {
            present: true,
            closed: starting("lid", switch.closed())?,
        },
        None => Lid {
            present: false,
            closed: false,
        },
    };
    let authorizer: Box<dyn Authorizer> = if options.no_auth {
        Box::new(AllowAll)
    } else {
        Box::new(LogindAuthorizer::new(starting(
            "logind",
            cupertino_seat::connect().map_err(Error::Seat),
        )?))
    };
    // Listening before the first read, so no change between the two is missed.
    let monitor = starting("uevent", Monitor::new().map_err(Error::Devices))?;
    let bus = if options.session_bus {
        Bus::Session
    } else {
        Bus::System
    };
    let daemon = Daemon::start(Config {
        bus,
        sysfs_root: options.sysfs_root.clone(),
        lid,
        authorizer,
        history_path: Some(options.history_path.clone()),
    })?;

    // the interval runs from the last read, so uevents that do not concern power do not put the read off
    let mut next_read = Instant::now() + Duration::from_secs(POLL_INTERVAL_SECONDS);
    loop {
        // the warning rules may need a reading before the poll interval is over
        let to_read = next_read.saturating_duration_since(Instant::now());
        let wait = daemon.next_deadline().map_or(to_read, |d| d.min(to_read));
        let wake_at = Instant::now() + wait;
        let timeout = Timespec {
            tv_sec: wait.as_secs() as i64,
            tv_nsec: i64::from(wait.subsec_nanos()),
        };
        let mut fds = vec![PollFd::new(&monitor, PollFlags::IN)];
        if let Some(switch) = &lid_switch {
            fds.push(PollFd::new(switch, PollFlags::IN));
        }
        match poll(&mut fds, Some(&timeout)) {
            Ok(_) => {}
            Err(Errno::INTR) => continue,
            Err(errno) => return Err(Error::Poll(errno)),
        }
        let uevents_ready = fds[0].revents().contains(PollFlags::IN);
        let lid_ready = fds
            .get(1)
            .is_some_and(|fd| fd.revents().contains(PollFlags::IN));
        let mut refresh = Instant::now() >= wake_at;
        if uevents_ready {
            let drained = monitor.drain().map_err(Error::Devices)?;
            for error in &drained.errors {
                eprintln!("cupertino-powerd: malformed uevent: {error}");
            }
            refresh |= drained.overflowed || drained.events.iter().any(concerns_power);
        }
        if let (true, Some(switch)) = (lid_ready, lid_switch.as_mut()) {
            switch.drain()?;
            report(daemon.set_lid(Lid {
                present: true,
                closed: switch.closed()?,
            }));
        }
        if refresh {
            report(daemon.refresh());
            next_read = Instant::now() + Duration::from_secs(POLL_INTERVAL_SECONDS);
        }
    }
}

/// A failed refresh keeps the previous state and is retried at the next event or tick; it is
/// reported, not hidden.
fn report(result: Result<(), Error>) {
    if let Err(e) = result {
        eprintln!("cupertino-powerd: {e}");
    }
}
