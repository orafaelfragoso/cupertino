//! Inspect an asset pack.
//!
//! ```text
//! cupertino-assets check <dir>
//! ```
//!
//! `check` reads the pack in `<dir>` as the processes do, prints the problems it reports (dropped entries, files the
//! manifest names but lacks), then, per logical name, what provides it: the pack's source or the built-in fallback
//! (the embedded font, a placeholder icon, the procedural cursors and wallpaper). It exits 1 only when the manifest
//! cannot be read, is not valid JSON or has an unsupported version, never because assets are missing.
//!
//! `cupertino appearance assets check` replaces it: `check` prints a notice saying so on stderr first.

use cupertino_assets::{Assets, PackReport};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-assets check <dir>";

const DEPRECATED: &str = "cupertino-assets: use `cupertino appearance assets check`; \
                          this command will be removed in a later release";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [command, dir] = args.as_slice() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if command != "check" {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    eprintln!("{DEPRECATED}");
    match Assets::read_pack(Path::new(dir)) {
        Ok((assets, notes)) => {
            print!("{}", PackReport::new(&assets, &notes));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cupertino-assets: {e}");
            ExitCode::FAILURE
        }
    }
}
