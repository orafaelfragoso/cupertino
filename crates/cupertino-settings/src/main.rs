//! The `cupertino-settings` process: the settings window as a client of the server, on the first sidebar row or the
//! pane `--pane` names. It exits when the window is closed. See the README.

mod about_app;
mod app;

use std::process::ExitCode;

use cupertino_settings::{sidebar, PaneId};

const USAGE: &str = "usage: cupertino-settings [--pane PANE-ID | --about]";

#[derive(Debug, PartialEq, Eq)]
enum Launch {
    Pane(PaneId),
    About,
}

/// The pane the window opens on: `--pane PANE-ID`, else the first sidebar row (approximate, §4.1).
fn options(mut args: impl Iterator<Item = String>) -> Result<Launch, String> {
    let mut pane = None;
    let mut about = false;
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--about" => about = true,
            "--pane" => {
                let id = args.next().ok_or("--pane needs a pane id")?;
                pane = Some(PaneId::from_key(&id).ok_or_else(|| format!("unknown pane {id}"))?);
            }
            other => return Err(format!("unexpected argument {other}")),
        }
    }
    match (about, pane) {
        (true, Some(_)) => Err("--about and --pane cannot be combined".into()),
        (true, None) => Ok(Launch::About),
        (false, pane) => Ok(Launch::Pane(pane.unwrap_or(sidebar::rows()[0]))),
    }
}

fn main() -> ExitCode {
    let pane = match options(std::env::args().skip(1)) {
        Ok(pane) => pane,
        Err(problem) => {
            eprintln!("cupertino-settings: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let result = match pane {
        Launch::About => about_app::run(),
        Launch::Pane(pane) => app::run(pane),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("cupertino-settings: {e}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(a: &[&str]) -> Result<Launch, String> {
        options(a.iter().map(|s| s.to_string()))
    }

    #[test]
    fn the_window_opens_on_the_named_pane_or_the_first_row() {
        assert_eq!(args(&[]), Ok(Launch::Pane(PaneId::Wifi)));
        assert_eq!(
            args(&["--pane", "keyboard"]),
            Ok(Launch::Pane(PaneId::Keyboard))
        );
        assert_eq!(args(&["--about"]), Ok(Launch::About));
        assert!(args(&["--about", "--pane", "keyboard"]).is_err());
        assert_eq!(
            args(&["--pane", "general"]),
            Err("unknown pane general".to_string())
        );
        assert_eq!(args(&["--pane"]), Err("--pane needs a pane id".to_string()));
        assert!(args(&["keyboard"]).is_err());
    }
}
