//! Runs the Virtual List example on the session's Wayland display: `cargo run -p cupertino-kit --example virtual-list`.

#[path = "apps/virtual-list.rs"]
mod virtual_list;

use std::process::ExitCode;

fn main() -> ExitCode {
    match cupertino_kit::app::run::<virtual_list::VirtualList<50_000>>() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("virtual-list: {e}");
            ExitCode::FAILURE
        }
    }
}
