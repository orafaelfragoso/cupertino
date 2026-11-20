//! The host program of these tests: `cupertino-shell`'s own (`cupertino_shell::run`) with the test components of
//! [`super::components`]. The supervisor starts this test binary with [`MARK`] among its arguments, then the report
//! file, then what it passes the host (`--only NAME` for a separate host).

use std::path::PathBuf;

use super::components::{report_to, ENTRIES};

/// The argument that makes this test binary the host: a filter after `--` that names no test.
pub const MARK: &str = "cupertino-shell-host";

/// The report file and the host's own arguments that follow [`MARK`], when this process is the host.
pub fn arguments() -> Option<(PathBuf, Vec<String>)> {
    let args: Vec<String> = std::env::args().collect();
    let at = args.iter().position(|a| a == MARK)?;
    let report = PathBuf::from(args.get(at + 1)?);
    Some((report, args[at + 2..].to_vec()))
}

/// Be the host, its components reporting to `report`.
pub fn run(report: PathBuf, args: Vec<String>) -> ! {
    report_to(report);
    cupertino_shell::run(&ENTRIES, args)
}
