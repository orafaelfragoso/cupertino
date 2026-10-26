//! Command line front end: generate the theme files, optionally install them.
//!
//! ```text
//! cupertino-gtk-theme [--output DIR] [--accent NAME] [--size regular|small|large]
//!                     [--high-contrast] [--name NAME] [--map-sans-serif] [--spec-dir DIR]
//!                     [--install [--force] [--dry-run] [--home DIR]] [--list-accents]
//! ```
//!
//! Files go to `--output` (default `prototypes/tools/cupertino-gtk-theme/output/`). Nothing is written
//! below the home directory unless `--install` is given.

use cupertino_gtk_theme::generate::{generate, write_files, GeneratorOptions};
use cupertino_gtk_theme::install::{install, Action, InstallOptions, Targets};
use cupertino_gtk_theme::{Accent, ControlSizeName};
use cupertino_theme::Theme;
use std::path::PathBuf;
use std::process::ExitCode;

const HELP: &str = "cupertino-gtk-theme: generate GTK 3/4, libadwaita, Qt and fontconfig files from the design tokens

usage: cupertino-gtk-theme [options]
  --output DIR        output directory (default: the crate's output/ directory)
  --accent NAME       blue (default), purple, pink, red, orange, yellow, green, graphite
  --size SIZE         control size: regular (default), small, large
  --high-contrast     use the high-contrast catalogs
  --name NAME         theme name (default: Cupertino)
  --map-sans-serif    also map the generic sans-serif family to the UI font
  --spec-dir DIR      specification directory (default: $CUPERTINO_SPEC_DIR or the repository's)
  --install           also copy the files into the user's XDG directories
  --force             with --install: replace differing files (the old ones are kept as .bak)
  --dry-run           with --install: only print what would happen
  --home DIR          with --install: use DIR instead of $HOME
  --list-accents      print the accent names and exit
  -h, --help          this text
";

struct Args {
    output: PathBuf,
    opts: GeneratorOptions,
    spec_dir: Option<PathBuf>,
    install: bool,
    install_opts: InstallOptions,
    home: Option<PathBuf>,
    list_accents: bool,
}

fn parse(mut it: impl Iterator<Item = String>) -> Result<Option<Args>, String> {
    let mut a = Args {
        output: PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/output")),
        opts: GeneratorOptions::default(),
        spec_dir: None,
        install: false,
        install_opts: InstallOptions::default(),
        home: None,
        list_accents: false,
    };
    while let Some(arg) = it.next() {
        let mut value = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "-h" | "--help" => return Ok(None),
            "--output" => a.output = PathBuf::from(value("--output")?),
            "--accent" => a.opts.accent = value("--accent")?.parse::<Accent>()?,
            "--size" => a.opts.size = value("--size")?.parse::<ControlSizeName>()?,
            "--name" => a.opts.name = value("--name")?,
            "--spec-dir" => a.spec_dir = Some(PathBuf::from(value("--spec-dir")?)),
            "--home" => a.home = Some(PathBuf::from(value("--home")?)),
            "--high-contrast" => a.opts.high_contrast = true,
            "--map-sans-serif" => a.opts.map_sans_serif = true,
            "--install" => a.install = true,
            "--force" => a.install_opts.force = true,
            "--dry-run" => a.install_opts.dry_run = true,
            "--list-accents" => a.list_accents = true,
            other => return Err(format!("unknown argument {other:?} (see --help)")),
        }
    }
    if (a.install_opts.force || a.install_opts.dry_run || a.home.is_some()) && !a.install {
        return Err("--force, --dry-run and --home only make sense with --install".into());
    }
    if a.opts.name.is_empty()
        || !a
            .opts
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("--name may only contain letters, digits, '-' and '_'".into());
    }
    Ok(Some(a))
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(Some(a)) => a,
        Ok(None) => {
            print!("{HELP}");
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("error: {e}");
            return ExitCode::from(2);
        }
    };
    if args.list_accents {
        for a in Accent::ALL {
            println!("{}", a.name());
        }
        return ExitCode::SUCCESS;
    }
    let theme = match &args.spec_dir {
        Some(d) => Theme::from_dir(d),
        None => Theme::load(),
    };
    let theme = match theme {
        Ok(t) => t,
        Err(e) => {
            eprintln!("error: cannot load the specification files: {e}");
            return ExitCode::FAILURE;
        }
    };
    let files = generate(&theme, &args.opts);
    if let Err(e) = write_files(&files, &args.output) {
        eprintln!("error: cannot write to {}: {e}", args.output.display());
        return ExitCode::FAILURE;
    }
    println!("wrote {} files to {}", files.len(), args.output.display());
    if args.install {
        let Some(targets) = Targets::from_env(args.home.as_deref()) else {
            eprintln!("error: no home directory (set HOME or pass --home)");
            return ExitCode::FAILURE;
        };
        match install(&files, &targets, args.install_opts) {
            Ok(actions) => {
                for a in actions {
                    match a {
                        Action::Written(p) => println!(
                            "{} {}",
                            if args.install_opts.dry_run {
                                "would write"
                            } else {
                                "installed"
                            },
                            p.display()
                        ),
                        Action::Unchanged(p) => println!("unchanged {}", p.display()),
                        Action::SkippedExisting(p) => {
                            println!("kept existing {} (use --force to replace)", p.display())
                        }
                        Action::BackedUpAndWritten { path, backup } => println!(
                            "{} {} (old file: {})",
                            if args.install_opts.dry_run {
                                "would replace"
                            } else {
                                "replaced"
                            },
                            path.display(),
                            backup.display()
                        ),
                    }
                }
            }
            Err(e) => {
                eprintln!("error: install failed: {e}");
                return ExitCode::FAILURE;
            }
        }
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &[&str]) -> Result<Option<Args>, String> {
        parse(s.iter().map(|x| x.to_string()))
    }

    #[test]
    fn defaults_write_to_the_crate_output_directory() {
        let a = args(&[]).unwrap().unwrap();
        assert!(a.output.ends_with("cupertino-gtk-theme/output"));
        assert!(!a.install);
        assert_eq!(a.opts.accent, Accent::Blue);
    }

    #[test]
    fn options_parse() {
        let a = args(&[
            "--output",
            "/tmp/x",
            "--accent",
            "green",
            "--size",
            "small",
            "--high-contrast",
            "--install",
            "--dry-run",
            "--home",
            "/h",
        ])
        .unwrap()
        .unwrap();
        assert_eq!(a.opts.accent, Accent::Green);
        assert_eq!(a.opts.size, ControlSizeName::Small);
        assert!(a.opts.high_contrast && a.install && a.install_opts.dry_run);
        assert_eq!(a.home.as_deref(), Some(std::path::Path::new("/h")));
    }

    #[test]
    fn invalid_input_is_reported() {
        assert!(args(&["--accent", "teal"]).is_err());
        assert!(args(&["--bogus"]).is_err());
        assert!(args(&["--output"]).is_err());
        assert!(args(&["--force"]).is_err(), "--force without --install");
        assert!(args(&["--name", "../evil"]).is_err());
        assert!(args(&["--help"]).unwrap().is_none());
    }
}
