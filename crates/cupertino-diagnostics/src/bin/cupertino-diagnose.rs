use std::process::ExitCode;

use cupertino_diagnostics::{Area, Sources};

const USAGE: &str =
    "usage: cupertino-diagnose [--json] [--area gpu|session|portals|services|crash|logs|events]...
Checks this machine and session for what Cupertino needs. Exits 0 once the checks ran, whatever they found.";

/// Printed on stderr before a text report: `cupertino diagnose` replaces this command. The `--json` output stays as it
/// is until the command is removed.
const DEPRECATED: &str =
    "cupertino-diagnose: use `cupertino diagnose`; this command will be removed in a later release";

fn main() -> ExitCode {
    let mut json = false;
    let mut areas = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--json" => json = true,
            "--area" => match args.next().as_deref().and_then(Area::from_name) {
                Some(area) => areas.push(area),
                None => return usage(),
            },
            "-h" | "--help" => {
                println!("{USAGE}");
                return ExitCode::SUCCESS;
            }
            _ => return usage(),
        }
    }
    let report = cupertino_diagnostics::run(&Sources::system(), &areas);
    if json {
        let text = serde_json::to_string_pretty(&report).expect("a report is strings and numbers");
        println!("{text}");
    } else {
        eprintln!("{DEPRECATED}");
        print!("{report}");
    }
    ExitCode::SUCCESS
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}
