//! Command line front end.
//!
//! ```text
//! cupertino-cursor [--source DIR | --procedural] [--metadata FILE] [--output DIR] [--name NAME]
//!                  [--sizes 24,32,48,64] [--base-size 24] [--hotspot unscaled|scaled]
//!                  [--no-shadow] [--no-default-shadow] [--no-flip-shadow-y] [--no-fill] [--default-delay-ms N]
//!                  [--hyprcursor] [--inherits THEME] [--list-names]
//! ```
//!
//! The default output directory is of the repository
//! (git-ignored). Themes built from `--source` are for local use only.

use cupertino_cursor::meta::HotspotSource;
use cupertino_cursor::names;
use cupertino_cursor::theme::{build_theme, BuildOptions, ShadowMode, Source};
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_OUTPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/output/cursor-theme");

const HELP: &str = "cupertino-cursor: build an XCursor theme from vector PDFs or the procedural cursor set

usage: cupertino-cursor [options]
  --source DIR          directory with <cursor>/cursor.pdf (+ cursor.meta): a vector cursor set
  --procedural          use only the built-in procedural cursors (default when --source is absent)
  --metadata FILE       metadata JSON (hotspots, shadows, animation frames) for a --source set
  --output DIR          output directory (default: the crate's output/cursor-theme/, git-ignored)
  --name NAME           theme name (default: Cupertino)
  --sizes LIST          nominal sizes, comma separated (default: 24,32,48,64)
  --base-size N         nominal size that maps to 1 pixel per point (default: 24)
  --hotspot WHICH       unscaled (default) or scaled hotspot keys of the metadata
  --no-shadow           do not bake shadows
  --no-default-shadow   no shadow for cursors without shadow metadata
  --no-flip-shadow-y    treat the metadata's y shadow offset as pointing down
  --no-fill             do not complete a PDF theme with procedural cursors
  --default-delay-ms N  frame delay for animations without one in the metadata (default: 100)
  --hyprcursor          also write a hyprcursor source theme next to the XCursor theme
  --inherits THEME      Inherits= entry of index.theme
  --pdftocairo PATH     poppler's pdftocairo (default: from PATH)
  --pdfinfo PATH        poppler's pdfinfo (default: from PATH)
  --list-names          print the name mapping and exit
  -h, --help

Themes built from --source are built from third-party artwork: local use only, never redistribute.
";

fn parse(mut it: impl Iterator<Item = String>) -> Result<Option<(BuildOptions, bool)>, String> {
    let mut o = BuildOptions::new(DEFAULT_OUTPUT);
    let mut source: Option<PathBuf> = None;
    let mut procedural = false;
    let mut list = false;
    while let Some(arg) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--source" => source = Some(PathBuf::from(value("--source")?)),
            "--procedural" => procedural = true,
            "--metadata" => o.metadata = Some(PathBuf::from(value("--metadata")?)),
            "--output" => o.output = PathBuf::from(value("--output")?),
            "--name" => o.name = value("--name")?,
            "--sizes" => {
                o.sizes = value("--sizes")?.split(',').map(|s| s.trim().parse::<u32>().map_err(|_| format!("bad size {s:?}"))).collect::<Result<_, _>>()?;
            }
            "--base-size" => o.base_size = value("--base-size")?.parse().map_err(|_| "bad base size".to_string())?,
            "--hotspot" => {
                o.hotspot_source = match value("--hotspot")?.as_str() {
                    "unscaled" => HotspotSource::Unscaled,
                    "scaled" => HotspotSource::Scaled,
                    other => return Err(format!("unknown hotspot source {other:?}")),
                }
            }
            "--no-shadow" => o.shadow = ShadowMode::Never,
            "--no-default-shadow" => o.default_shadow = false,
            "--no-flip-shadow-y" => o.flip_shadow_y = false,
            "--no-fill" => o.fill_missing = false,
            "--default-delay-ms" => o.default_delay_ms = value("--default-delay-ms")?.parse().map_err(|_| "bad delay".to_string())?,
            "--hyprcursor" => o.hyprcursor = true,
            "--inherits" => o.inherits = Some(value("--inherits")?),
            "--pdftocairo" => o.poppler.pdftocairo = PathBuf::from(value("--pdftocairo")?),
            "--pdfinfo" => o.poppler.pdfinfo = PathBuf::from(value("--pdfinfo")?),
            "--list-names" => list = true,
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }
    if source.is_some() && procedural {
        return Err("--source and --procedural exclude each other".into());
    }
    if let Some(dir) = source {
        o.source = Source::Pdf(dir);
    }
    Ok(Some((o, list)))
}

fn main() -> ExitCode {
    let (o, list) = match parse(std::env::args().skip(1)) {
        Ok(Some(x)) => x,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if list {
        for m in names::SOURCE_MAP.iter().chain(names::PROCEDURAL_ONLY) {
            println!("{:<26} -> {:<14} {}", m.reference, m.primary, m.aliases.join(" "));
        }
        println!("\ninstalled under their own name only: {}", names::UNMAPPED_SOURCE.join(" "));
        return ExitCode::SUCCESS;
    }
    match build_theme(&o) {
        Ok(r) => {
            for w in &r.warnings {
                eprintln!("warning: {w}");
            }
            println!("wrote {} files and {} symlinks to {}", r.files.len(), r.symlinks.len(), r.theme_dir.display());
            if let Some(h) = r.hypr_dir {
                println!("wrote hyprcursor source theme to {}", h.display());
            }
            if matches!(o.source, Source::Pdf(_)) {
                println!("note: built from third-party artwork — for local use only");
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_cursor::theme::DEFAULT_SIZES;

    fn p(a: &[&str]) -> Result<Option<(BuildOptions, bool)>, String> {
        parse(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn default_output_is_the_git_ignored_reference_directory() {
        let (o, _) = p(&[]).unwrap().unwrap();
        assert!(o.output.ends_with("output/cursor-theme"), "{}", o.output.display());
        assert_eq!(o.source, Source::Procedural);
        assert_eq!(o.sizes, DEFAULT_SIZES.to_vec());
    }

    #[test]
    fn source_selects_pdf_mode_and_default_metadata() {
        let (o, _) = p(&["--source", "/some/dir", "--sizes", "24, 48", "--hotspot", "scaled", "--no-shadow", "--hyprcursor"]).unwrap().unwrap();
        assert_eq!(o.source, Source::Pdf("/some/dir".into()));
        assert_eq!(o.sizes, vec![24, 48]);
        assert_eq!(o.hotspot_source, HotspotSource::Scaled);
        assert!(o.hyprcursor && o.shadow == ShadowMode::Never);
        assert!(o.metadata.is_none());
    }

    #[test]
    fn errors() {
        assert!(p(&["--sizes", "a"]).is_err());
        assert!(p(&["--bogus"]).is_err());
        assert!(p(&["--source", "x", "--procedural"]).is_err());
        assert!(p(&["--hotspot", "weird"]).is_err());
        assert!(p(&["--output"]).is_err());
        assert!(p(&["-h"]).unwrap().is_none());
    }
}
