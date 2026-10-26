//! Command line front end.
//!
//! ```text
//! cupertino-cursor [--procedural] [--output DIR] [--name NAME] [--sizes 24,32,48,64] [--base-size 24]
//!                  [--no-shadow] [--hyprcursor] [--inherits THEME] [--list-names]
//! ```
//!
//! The default output directory is the crate's `output/cursor-theme` (git-ignored).

use cupertino_cursor::names;
use cupertino_cursor::theme::{build_theme, BuildOptions, ShadowMode};
use std::path::PathBuf;
use std::process::ExitCode;

const DEFAULT_OUTPUT: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/output/cursor-theme");

const HELP: &str =
    "cupertino-cursor: build an XCursor theme from the built-in procedural cursor set

usage: cupertino-cursor [options]
  --procedural          accepted for scripts: the procedural cursors are the only source
  --output DIR          output directory (default: the crate's output/cursor-theme/, git-ignored)
  --name NAME           theme name (default: Cupertino)
  --sizes LIST          nominal sizes, comma separated (default: 24,32,48,64,96)
  --base-size N         nominal size that maps to 1 pixel per point (default: 24)
  --no-shadow           do not bake shadows
  --hyprcursor          also write a hyprcursor source theme next to the XCursor theme
  --inherits THEME      Inherits= entry of index.theme
  --list-names          print the cursor names and their aliases and exit
  -h, --help
";

fn parse(mut it: impl Iterator<Item = String>) -> Result<Option<(BuildOptions, bool)>, String> {
    let mut o = BuildOptions::new(DEFAULT_OUTPUT);
    let mut list = false;
    while let Some(arg) = it.next() {
        let mut value = |n: &str| it.next().ok_or_else(|| format!("{n} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--procedural" => {}
            "--output" => o.output = PathBuf::from(value("--output")?),
            "--name" => o.name = value("--name")?,
            "--sizes" => {
                o.sizes = value("--sizes")?
                    .split(',')
                    .map(|s| {
                        s.trim()
                            .parse::<u32>()
                            .map_err(|_| format!("bad size {s:?}"))
                    })
                    .collect::<Result<_, _>>()?;
            }
            "--base-size" => {
                o.base_size = value("--base-size")?
                    .parse()
                    .map_err(|_| "bad base size".to_string())?
            }
            "--no-shadow" => o.shadow = ShadowMode::Never,
            "--hyprcursor" => o.hyprcursor = true,
            "--inherits" => o.inherits = Some(value("--inherits")?),
            "--list-names" => list = true,
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
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
        for m in names::NAMES {
            println!("{:<14} {}", m.primary, m.aliases.join(" "));
        }
        return ExitCode::SUCCESS;
    }
    match build_theme(&o) {
        Ok(r) => {
            println!(
                "wrote {} files and {} symlinks to {}",
                r.files.len(),
                r.symlinks.len(),
                r.theme_dir.display()
            );
            if let Some(h) = r.hypr_dir {
                println!("wrote hyprcursor source theme to {}", h.display());
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
    fn the_default_output_is_the_git_ignored_crate_directory() {
        let (o, _) = p(&[]).unwrap().unwrap();
        assert!(
            o.output.ends_with("output/cursor-theme"),
            "{}",
            o.output.display()
        );
        assert_eq!(o.sizes, DEFAULT_SIZES.to_vec());
    }

    #[test]
    fn options_are_read() {
        let (o, list) = p(&[
            "--procedural",
            "--sizes",
            "24, 48",
            "--no-shadow",
            "--hyprcursor",
            "--name",
            "Mine",
            "--list-names",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(o.sizes, vec![24, 48]);
        assert!(o.hyprcursor && o.shadow == ShadowMode::Never && list);
        assert_eq!(o.name, "Mine");
    }

    #[test]
    fn errors() {
        assert!(p(&["--sizes", "a"]).is_err());
        assert!(p(&["--bogus"]).is_err());
        assert!(
            p(&["--source", "x"]).is_err(),
            "there is no other source than the procedural set"
        );
        assert!(p(&["--output"]).is_err());
        assert!(p(&["-h"]).unwrap().is_none());
    }
}
