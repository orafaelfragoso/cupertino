//! The `cupertino` command; `cupertino help` lists its subcommands.

use std::io::Write;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = cupertino_cli::run(&args, &cupertino_cli::request_id());
    // A closed pipe (`cupertino help | head`) is not an error of the command.
    let _ = std::io::stdout().write_all(out.stdout.as_bytes());
    let _ = std::io::stderr().write_all(out.stderr.as_bytes());
    ExitCode::from(out.code)
}
