//! Logging: standard error for development runs, and on the TTY a file in the state directory that survives the session
//! (the console is overwritten by the compositor, and a crash leaves nothing else to read).

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::os::unix::fs::OpenOptionsExt;
use std::path::PathBuf;
use std::sync::Mutex;

use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

const DEFAULT_FILTER: &str =
    "info,smithay=warn,usvg=error,resvg=error,smithay::backend::egl::ffi=off";

/// The TTY log: `CUPERTINO_LOG_FILE`, else `$XDG_STATE_HOME/cupertino/server.log`, else
/// `~/.local/state/cupertino/server.log`.
pub fn log_path(var: impl Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(path) = var("CUPERTINO_LOG_FILE").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(path));
    }
    let state = var("XDG_STATE_HOME")
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            var("HOME")
                .filter(|p| !p.is_empty())
                .map(|home| PathBuf::from(home).join(".local/state"))
        })?;
    Some(state.join("cupertino/server.log"))
}

/// Creates the log file for this run, mode 0600. The previous run's log is kept as `<name>.1`.
pub fn open_log(path: &PathBuf) -> io::Result<File> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    if path.exists() {
        let mut previous = path.clone().into_os_string();
        previous.push(".1");
        fs::rename(path, previous)?;
    }
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
}

/// Installs the subscriber and a panic hook that writes the panic to the log before the default report.
/// With a file, everything goes to it and only warnings and errors also reach standard error.
pub fn init(file: Option<File>) {
    let filter =
        || EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    match file {
        Some(file) => {
            let file = tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(Mutex::new(file))
                .with_filter(filter());
            tracing_subscriber::registry()
                .with(file)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(io::stderr)
                        .with_filter(LevelFilter::WARN),
                )
                .init();
        }
        None => tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .with_writer(io::stderr)
                    .with_filter(filter()),
            )
            .init(),
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        previous(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn env(pairs: &'static [(&'static str, &'static str)]) -> impl Fn(&str) -> Option<OsString> {
        move |key| {
            pairs
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, v)| OsString::from(v))
        }
    }

    #[test]
    fn the_log_file_follows_the_override_then_the_state_directory_then_home() {
        let both = env(&[
            ("CUPERTINO_LOG_FILE", "/tmp/x.log"),
            ("XDG_STATE_HOME", "/s"),
        ]);
        assert_eq!(log_path(both), Some(PathBuf::from("/tmp/x.log")));
        assert_eq!(
            log_path(env(&[("XDG_STATE_HOME", "/s"), ("HOME", "/h")])),
            Some(PathBuf::from("/s/cupertino/server.log"))
        );
        assert_eq!(
            log_path(env(&[("HOME", "/h")])),
            Some(PathBuf::from("/h/.local/state/cupertino/server.log"))
        );
        assert_eq!(log_path(env(&[])), None);
    }

    #[test]
    fn a_new_run_keeps_the_previous_log_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state/server.log");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "first run").unwrap();
        let mut file = open_log(&path).unwrap();
        io::Write::write_all(&mut file, b"second run").unwrap();
        let read = |p: PathBuf| {
            let mut text = String::new();
            File::open(p).unwrap().read_to_string(&mut text).unwrap();
            text
        };
        assert_eq!(read(path.clone()), "second run");
        assert_eq!(read(dir.path().join("state/server.log.1")), "first run");
        let mode = fs::metadata(&path).unwrap().permissions();
        assert_eq!(
            std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
            0o600
        );
    }
}
