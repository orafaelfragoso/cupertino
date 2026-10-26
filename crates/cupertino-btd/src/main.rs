//! The `cupertino-btd` process: serves `org.cupertino.Bluetooth1` on the system bus and keeps it
//! current from the kernel's management socket. See the README for how to run it.

use std::convert::Infallible;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;

use cupertino_btd::auth::{AllowAll, Authorizer, LogindAuthorizer};
use cupertino_btd::daemon::{Bus, Config, Daemon};
use cupertino_btd::error::Error;
use cupertino_btd::transport::KernelTransport;

const USAGE: &str = "usage: cupertino-btd [--session-bus [--no-auth]] [--keys-dir PATH] [--read-only]";

/// Where the pairing keys live unless `--keys-dir` says otherwise.
const DEFAULT_KEYS_DIR: &str = "/var/lib/cupertino/bluetooth";

struct Options {
    session_bus: bool,
    no_auth: bool,
    read_only: bool,
    keys_dir: PathBuf,
}

impl Options {
    /// The first argument that is unknown, or a combination that is refused.
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, String> {
        let mut options = Self { session_bus: false, no_auth: false, read_only: false, keys_dir: PathBuf::from(DEFAULT_KEYS_DIR) };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--session-bus" => options.session_bus = true,
                "--no-auth" => options.no_auth = true,
                "--read-only" => options.read_only = true,
                "--keys-dir" => options.keys_dir = args.next().map(PathBuf::from).ok_or_else(|| "--keys-dir needs a path".to_owned())?,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        // Skipping authorisation on the system bus would let any user pair devices.
        if options.no_auth && !options.session_bus {
            return Err("--no-auth is only allowed with --session-bus".to_owned());
        }
        Ok(options)
    }
}

fn main() -> ExitCode {
    let options = match Options::parse(std::env::args().skip(1)) {
        Ok(options) => options,
        Err(message) => {
            eprintln!("cupertino-btd: {message}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // `run` only returns on failure
    let Err(e) = run(options);
    eprintln!("cupertino-btd: {e}");
    ExitCode::FAILURE
}

fn run(options: Options) -> Result<Infallible, Error> {
    let authorizer: Box<dyn Authorizer> = if options.no_auth {
        Box::new(AllowAll)
    } else {
        Box::new(LogindAuthorizer::new(cupertino_seat::connect().map_err(Error::Seat)?))
    };
    let transport = Arc::new(KernelTransport::open()?);
    let bus = if options.session_bus { Bus::Session } else { Bus::System };
    let daemon = Daemon::start(Config { bus, keys_dir: options.keys_dir, read_only: options.read_only, authorizer, transport })?;
    daemon.run()
}
