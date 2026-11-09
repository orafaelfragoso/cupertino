//! The `cupertino-settings` process: the settings window as a client of the server, on the first sidebar row or the
//! pane `--pane` names. It exits when the window is closed. See the README.

use cupertino_diagnostic_events as diag;
use std::process::ExitCode;

use cupertino_settings::{about_app, shell, sidebar, PaneId};

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
        (false, pane) => Ok(Launch::Pane(pane.unwrap_or(sidebar::rows(&[])[0]))),
    }
}

fn main() -> ExitCode {
    diag::init(diag::component!(), "settings", diag::Sink::UserStore);
    let pane = match options(std::env::args().skip(1)) {
        Ok(pane) => pane,
        Err(problem) => {
            eprintln!("cupertino-settings: {problem}\n{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    // Consume the launch grant before the bus or display starts threads, so children cannot inherit a spent token.
    let token = std::env::var("XDG_ACTIVATION_TOKEN");
    std::env::remove_var("XDG_ACTIVATION_TOKEN");
    let token = match token {
        Ok(token) => token,
        Err(std::env::VarError::NotPresent) => String::new(),
        Err(std::env::VarError::NotUnicode(_)) => {
            eprintln!("cupertino-settings: activation token is not valid UTF-8");
            return ExitCode::FAILURE;
        }
    };
    let result = match pane {
        Launch::About => about_app::run(token),
        Launch::Pane(pane) => shell::run(pane, token),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            diag::exit_error(&e);
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
