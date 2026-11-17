//! Repository tasks. `cargo xtask ci` works out what a change touches and runs the checks the chosen profile asks for
//! on it; `cargo xtask vendored` runs only its check of the vendored trees' patch series, and `cargo xtask remaining`
//! only its check that the layers check's remaining-work list did not grow.

mod config;
mod gate;
mod glob;
mod licences;
mod plan;
mod remaining;
mod run;
mod vendored;

use std::process::ExitCode;

use config::Scope;

const USAGE: &str = "\
usage: cargo xtask ci [--scope changed|affected|all] [--base <git-ref>] [--profile agent|integration|pr|full]
                      [--format text|json] [--dry-run]
       cargo xtask vendored
       cargo xtask remaining [<git-ref>]

Plans the checks a change needs from .config/validation.toml and runs them.
  --scope    changed: the packages whose files changed; affected: those and every package depending on them;
             all: the whole workspace. Defaults to the profile's scope.
  --base     the ref the change is measured against (default origin/main), from its merge base with HEAD,
             with staged, unstaged and untracked files included.
  --profile  which checks run (default agent).
  --format   text (a summary) or json (the report); the report is also written to target/xtask/ci-latest.json.
  --dry-run  print the plan and its commands and run nothing.
`vendored` reverses each vendored tree's patches in memory and compares the result with its UPSTREAM.sha256; `ci`
runs it in the profiles with the gate and whenever a vendored tree changed. The profiles with the layers check also
check that crates/cupertino-layers/remaining.txt holds no line its version at the merge base does not; `remaining`
runs only that check, against the merge base of the ref (default origin/main) with HEAD.
Exit status: 0 every check passed, 1 a check failed, 2 the plan could not be made.";

/// Options of `cargo xtask ci`.
pub struct Options {
    pub scope: Option<Scope>,
    pub base: String,
    pub profile: String,
    pub json: bool,
    pub dry_run: bool,
}

fn parse(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        scope: None,
        base: "origin/main".to_owned(),
        profile: "agent".to_owned(),
        json: false,
        dry_run: false,
    };
    let mut args = args.iter();
    match args.next().map(String::as_str) {
        Some("ci") => {}
        Some(other) => {
            return Err(format!(
                "unknown command `{other}`; the commands are `ci`, `vendored` and `remaining`"
            ))
        }
        None => return Err("a command is required: `ci`, `vendored` or `remaining`".to_owned()),
    }
    while let Some(arg) = args.next() {
        let (flag, inline) = match arg.split_once('=') {
            Some((flag, value)) if flag.starts_with("--") => (flag, Some(value.to_owned())),
            _ => (arg.as_str(), None),
        };
        let mut value = || {
            inline
                .clone()
                .or_else(|| args.next().cloned())
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag {
            "--scope" => {
                let text = value()?;
                options.scope =
                    Some(Scope::parse(&text).ok_or_else(|| format!("unknown scope `{text}`"))?);
            }
            "--base" => options.base = value()?,
            "--profile" => {
                let text = value()?;
                if !matches!(text.as_str(), "agent" | "integration" | "pr" | "full") {
                    return Err(format!("unknown profile `{text}`"));
                }
                options.profile = text;
            }
            "--format" => {
                options.json = match value()?.as_str() {
                    "text" => false,
                    "json" => true,
                    other => return Err(format!("unknown format `{other}`")),
                }
            }
            "--dry-run" if inline.is_none() => options.dry_run = true,
            _ => return Err(format!("unknown option `{arg}`")),
        }
    }
    Ok(options)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.first().map(String::as_str) == Some("vendored") {
        if args.len() > 1 {
            eprintln!("xtask: `vendored` takes no options\n\n{USAGE}");
            return ExitCode::from(2);
        }
        let root = match plan::repository_root() {
            Ok(root) => root,
            Err(error) => {
                eprintln!("xtask: {error}");
                return ExitCode::from(2);
            }
        };
        return if run::run_vendored(&root) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    if args.first().map(String::as_str) == Some("remaining") {
        if args.len() > 2 {
            eprintln!("xtask: `remaining` takes at most a git ref\n\n{USAGE}");
            return ExitCode::from(2);
        }
        let base = args.get(1).map_or("origin/main", String::as_str);
        let merge_base = plan::repository_root().and_then(|root| {
            let merge_base = plan::output(&root, "git", &["merge-base", base, "HEAD"])?;
            Ok((root, merge_base.trim().to_owned()))
        });
        let (root, merge_base) = match merge_base {
            Ok(found) => found,
            Err(error) => {
                eprintln!("xtask: {error}");
                return ExitCode::from(2);
            }
        };
        return if run::run_remaining(&root, &merge_base) {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }
    let options = match parse(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("xtask: {error}\n\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    ExitCode::from(run::ci(&options))
}
