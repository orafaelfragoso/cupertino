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

use cupertino_assets::{
    Assets, FontRole, IconName, IconSource, CURSOR_NAMES, RASTER_NAMES, SYMBOL_NAMES,
};
use std::path::Path;
use std::process::ExitCode;

const USAGE: &str = "usage: cupertino-assets check <dir>";

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
    match Assets::read_pack(Path::new(dir)) {
        Ok((assets, notes)) => {
            print!("{}", report(&assets, &notes));
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("cupertino-assets: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The report of `check` for `assets` (one pack) and the problems reading it gave.
fn report(assets: &Assets, notes: &[String]) -> String {
    let mut out = String::new();
    let mut line = |s: String| {
        out.push_str(&s);
        out.push('\n');
    };
    for (root, name, credits) in assets.credits() {
        line(format!(
            "pack {} ({})",
            root.display(),
            name.unwrap_or("unnamed")
        ));
        line(format!(
            "  credits: {}",
            credits.map_or("none".to_string(), |c| c.display().to_string())
        ));
    }
    line(format!("problems: {}", notes.len()));
    for n in notes {
        line(format!("  {n}"));
    }
    let mut fallbacks = Vec::new();
    line("fonts:".to_string());
    for role in FontRole::ALL {
        match assets.font_family(role) {
            Some(family) => line(format!("  {}: {family}", role.key())),
            None if role == FontRole::Ui => {
                line(format!(
                    "  ui: built-in ({})",
                    cupertino_assets::BUILTIN_UI_FAMILY
                ));
                fallbacks.push("font ui".to_string());
            }
            None => line(format!("  {}: not set (optional)", role.key())),
        }
    }
    line("icons:".to_string());
    for name in RASTER_NAMES.iter().chain(&SYMBOL_NAMES) {
        let icon = IconName::new(name).expect("the logical names are valid");
        let provider = match assets.icon_sources(&icon, 32).into_iter().next() {
            Some((_, IconSource::Raster(r))) => format!("raster {}", r.path.display()),
            Some((_, IconSource::Svg(p))) => format!("svg {}", p.display()),
            Some((_, IconSource::Symbol(p))) => format!("symbol {}", p.display()),
            Some((_, IconSource::Theme { names, theme })) => format!(
                "host theme {} ({})",
                theme.as_deref().unwrap_or("of the user"),
                names.join(", ")
            ),
            None => {
                fallbacks.push(format!("icon {name}"));
                "placeholder (built-in)".to_string()
            }
        };
        line(format!("  {name}: {provider}"));
    }
    line("cursors:".to_string());
    match assets.cursor_theme() {
        Some(theme) => {
            let dir = theme.search_dir.join(&theme.name).join("cursors");
            line(format!(
                "  theme {} in {}",
                theme.name,
                theme.search_dir.display()
            ));
            for name in CURSOR_NAMES {
                if !dir.join(name).exists() {
                    fallbacks.push(format!("cursor {name}"));
                    line(format!("  {name}: the theme's `default`"));
                }
            }
        }
        None => {
            fallbacks.push("cursor theme".to_string());
            line("  procedural (built-in)".to_string());
        }
    }
    line("wallpaper:".to_string());
    for (label, dark) in [("light", false), ("dark", true)] {
        match assets.wallpaper_path(dark) {
            Some(p) => line(format!("  {label}: {}", p.display())),
            None => {
                fallbacks.push(format!("wallpaper {label}"));
                line(format!("  {label}: procedural (built-in)"));
            }
        }
    }
    line(format!("falls back: {}", fallbacks.len()));
    for f in fallbacks {
        line(format!("  {f}"));
    }
    out
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
        let r = report(&assets, &notes);
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
