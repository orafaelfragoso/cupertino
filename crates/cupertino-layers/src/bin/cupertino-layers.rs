//! Reads `cargo metadata` for the workspace and reports every dependency that breaks the layer rules.
//!
//! ```text
//! cupertino-layers            (run from anywhere inside the workspace)
//! cupertino-layers --graph    DOT graph of the layers on stdout
//! ```
//!
//! `cupertino dev layers` runs the same check through [`cupertino_layers::check_workspace`].

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let graph = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => false,
        ["--graph"] => true,
        _ => {
            eprintln!("usage: cupertino-layers [--graph]");
            return ExitCode::from(2);
        }
    };

    let here = std::env::current_dir().expect("the working directory");
    let findings = cupertino_layers::check_workspace(&here).unwrap_or_else(|e| panic!("{e}"));
    // printing the graph is how the snapshot is made again, so an out-of-date one does not fail that run
    let problems: Vec<_> = if graph {
        findings.problems_printing_the_graph().collect()
    } else {
        findings.all_problems().collect()
    };
    for p in &problems {
        eprintln!("{p}");
    }
    if graph {
        print!("{}", findings.dot());
    } else {
        println!("{}", findings.summary());
    }
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
