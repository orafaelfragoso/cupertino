//! Reads `cargo metadata` for the workspace and reports every problem of the layer rules.
//!
//! ```text
//! cupertino-layers                     (run from anywhere inside the workspace)
//! cupertino-layers --graph             DOT graph of the layers on stdout
//! cupertino-layers --write-clippy [CRATE…]
//!                                      write the clippy.toml of these shell and apps crates, or of all of them
//! ```
//!
//! `cupertino dev layers` runs the same check through [`cupertino_layers::check_workspace`].

use std::process::ExitCode;

enum Mode {
    Check,
    Graph,
    WriteClippy(Vec<String>),
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mode = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => Mode::Check,
        ["--graph"] => Mode::Graph,
        ["--write-clippy", ref crates @ ..] => {
            Mode::WriteClippy(crates.iter().map(|c| (*c).to_owned()).collect())
        }
        _ => {
            eprintln!("usage: cupertino-layers [--graph | --write-clippy [CRATE...]]");
            return ExitCode::from(2);
        }
    };

    let here = std::env::current_dir().expect("the working directory");
    let findings = cupertino_layers::check_workspace(&here).unwrap_or_else(|e| panic!("{e}"));
    let problems: Vec<_> = match mode {
        Mode::Check => findings.all_problems().collect(),
        // printing the graph is how the snapshot is made again, so an out-of-date one does not fail that run
        Mode::Graph => findings.problems_printing_the_graph().collect(),
        Mode::WriteClippy(ref crates) => {
            let crates: Vec<&str> = crates.iter().map(String::as_str).collect();
            match findings.write_clippy(&crates) {
                Ok(written) => {
                    for path in written {
                        println!("{}", path.display());
                    }
                }
                Err(e) => {
                    eprintln!("cannot write a clippy.toml: {e}");
                    return ExitCode::FAILURE;
                }
            }
            Vec::new()
        }
    };
    for p in &problems {
        eprintln!("{p}");
    }
    match mode {
        Mode::Check => println!("{}", findings.summary()),
        Mode::Graph => print!("{}", findings.dot()),
        Mode::WriteClippy(_) => {}
    }
    if problems.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
