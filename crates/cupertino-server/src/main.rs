use anyhow::Result;
use clap::Parser;
use cupertino_diagnostic_events::{self as diag, Value};
use cupertino_server::cli::{Cli, Mode};
use cupertino_server::logging;

fn main() -> Result<()> {
    // before the logging: its panic hook chains to this one, and its fatal signal handler writes the lines made here
    diag::init(diag::component!(), "server", diag::Sink::UserStore);
    let cli = Cli::parse();
    let mode = cli.mode();
    // known before the backend starts, so the line a fatal signal writes carries it too
    if let Ok(mode) = &mode {
        let backend = match mode {
            Mode::Udev => "udev",
            Mode::Winit => "winit",
            Mode::Headless => "headless",
        };
        diag::set_context("backend", Value::Public(backend));
    }
    let on_tty = !cli.check_hardware && matches!(mode, Ok(Mode::Udev));
    // a log file named on purpose is written in every mode, as offscreen crash tests need
    let named_log = std::env::var_os(logging::LOG_FILE_ENV).is_some_and(|p| !p.is_empty());
    let opened = (on_tty || named_log)
        .then(|| logging::log_path(|key| std::env::var_os(key)))
        .flatten()
        .map(|path| (logging::open_log(&path), path));
    let (file, failure) = match opened {
        Some((Ok(file), path)) => {
            eprintln!("cupertino-server: logging to {}", path.display());
            (Some(file), None)
        }
        Some((Err(e), path)) => (None, Some((path, e))),
        None => (None, None),
    };
    let to_file = file.is_some();
    logging::init(file);
    logging::record_session_end(|key| std::env::var_os(key));
    // a server started from this one's clients writes neither this log nor this session's end
    std::env::remove_var(logging::LOG_FILE_ENV);
    std::env::remove_var(logging::SESSION_END_ENV);
    if let Some((path, e)) = failure {
        tracing::warn!("cannot write the log file {}: {e}", path.display());
    }
    cupertino_server::file_limit::raise();
    let result = cupertino_server::cli::run(cli);
    match &result {
        Ok(()) => logging::server_ended(0, None),
        Err(e) => {
            if to_file {
                tracing::error!("the server stopped: {e:#}");
            }
            logging::session_ends(&format!("{e:#}"), 1);
        }
    }
    result
}
