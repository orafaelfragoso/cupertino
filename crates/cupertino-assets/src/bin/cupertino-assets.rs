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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_pack_reports_every_name_as_falling_back() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("manifest.json"),
            r#"{"version": 2, "name": "empty", "surplus": true}"#,
        )
        .unwrap();
        let (assets, notes) = Assets::read_pack(d.path()).unwrap();
        let r = PackReport::new(&assets, &notes).to_string();
        assert!(
            r.contains("problems: 1") && r.contains("unknown key \"surplus\""),
            "{r}"
        );
        assert!(
            r.contains("ui: built-in (Inter Variable)")
                && r.contains("symbol.wifi: host theme of the user (network-wireless-symbolic")
                && r.contains("procedural (built-in)"),
            "{r}"
        );
        // the font, the cursor theme and the two wallpapers; every icon name is served by the host theme map
        let expected = 1 + 1 + 2;
        assert!(r.contains(&format!("falls back: {expected}")), "{r}");
    }
}
