//! Runs the Editor example on the session's Wayland display: `cargo run -p cupertino-kit --example editor`.

#[path = "apps/editor.rs"]
mod editor;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cupertino_kit::app::run::<editor::Editor>() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("editor: {e}");
            ExitCode::FAILURE
        }
    }
}
