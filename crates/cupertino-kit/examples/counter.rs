//! Runs the Counter example on the session's Wayland display: `cargo run -p cupertino-kit --example counter`.

#[path = "apps/counter.rs"]
mod counter;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cupertino_kit::app::run::<counter::Counter>() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("counter: {e}");
            ExitCode::FAILURE
        }
    }
}
