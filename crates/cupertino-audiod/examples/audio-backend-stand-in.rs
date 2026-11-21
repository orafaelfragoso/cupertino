//! A stand-in for `wpctl`, used by `tests/wpctl.rs`.
//!
//! It appends its arguments, joined by spaces, as one line to the `calls` file beside its own
//! executable, then follows the `behaviour` file there. Each line of `behaviour` is
//! `COMMAND<TAB>ACTION[<TAB>TEXT]`, where COMMAND is the first argument or `*` for any, and
//! ACTION is `stdout` or `stderr` (print TEXT, `\n` standing for a new line), `exit` (leave
//! with the code in TEXT), `abort` (end by a signal) or `hang` (stay silent until killed).
//! Lines apply in order; a missing file means: record and succeed.

use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

/// How long `hang` stays silent; the test stops the program long before.
const HANG: Duration = Duration::from_secs(30);

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let here: PathBuf = std::env::current_exe()
        .expect("own path")
        .parent()
        .expect("own directory")
        .to_path_buf();
    if let Ok(mut log) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(here.join("calls"))
    {
        let _ = writeln!(log, "{}", args.join(" "));
    }
    let behaviour = std::fs::read_to_string(here.join("behaviour")).unwrap_or_default();
    let first = args.first().map(String::as_str).unwrap_or("");
    for line in behaviour.lines() {
        let mut parts = line.splitn(3, '\t');
        let (command, action) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
        let text = parts.next().unwrap_or("").replace("\\n", "\n");
        if command != "*" && command != first {
            continue;
        }
        match action {
            "stdout" => {
                print!("{text}");
                let _ = std::io::stdout().flush();
            }
            "stderr" => eprint!("{text}"),
            "exit" => std::process::exit(text.trim().parse().unwrap_or(1)),
            "abort" => std::process::abort(),
            "hang" => std::thread::sleep(HANG),
            _ => {}
        }
    }
}
