//! Runs the Forms example on the session's Wayland display: `cargo run -p cupertino-kit --example forms`.

#[path = "apps/forms.rs"]
mod forms;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cupertino_kit::app::run::<forms::Forms>() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("forms: {e}");
            ExitCode::FAILURE
        }
    }
}
