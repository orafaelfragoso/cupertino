use anyhow::Result;
use clap::Parser;
use cupertino_server::cli::{Cli, Mode};
use cupertino_server::logging;

fn main() -> Result<()> {
    let cli = Cli::parse();
    let on_tty = !cli.check_hardware && matches!(cli.mode(), Ok(Mode::Udev));
    let opened = on_tty
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
    if let Some((path, e)) = failure {
        tracing::warn!("cannot write the log file {}: {e}", path.display());
    }
    let result = cupertino_server::cli::run(cli);
    if let (true, Err(e)) = (to_file, &result) {
        tracing::error!("the server stopped: {e:#}");
    }
    result
}
