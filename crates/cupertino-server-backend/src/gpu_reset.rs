//! Recovering from a GPU reset: reset status classification, bounded reset rate tracking, and
//! `session.log` / diagnostic event recording.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use smithay::backend::renderer::gles::{GlesRenderer, GraphicsResetStatus};

/// What a `session.log` line of a reset starts with, after its time.
pub const LOG_MARK: &str = "the GPU was reset:";

/// More resets than this within [`WINDOW`] end the session.
const LIMIT: usize = 3;
const WINDOW: Duration = Duration::from_secs(60);

/// What lost the rendering context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Loss {
    /// A reset of the GPU, for the reason `session.log` gives.
    Reset(String),
    /// The driver purged video memory, as NVIDIA's does across a suspend, a VT switch or a mode set: routine, so it is
    /// never counted against [`LIMIT`].
    Purge,
}

impl Loss {
    /// Why the context was lost, as `session.log` says it.
    pub fn reason(&self) -> &str {
        match self {
            Loss::Reset(reason) => reason,
            Loss::Purge => "the driver purged video memory",
        }
    }
}

/// The resets of the last [`WINDOW`].
#[derive(Debug, Default)]
pub struct Resets(Vec<Instant>);

impl Resets {
    /// Counts `loss` at `now`; whether recovery may be tried, or the session should end. A purge always may.
    pub fn allow(&mut self, loss: &Loss, now: Instant) -> bool {
        if *loss == Loss::Purge {
            return true;
        }
        self.0
            .retain(|&at| now.saturating_duration_since(at) < WINDOW);
        self.0.push(now);
        self.0.len() <= LIMIT
    }
}

/// Whether the context was lost since the last check, and why; `None` with no loss or a driver that cannot tell.
pub fn status(renderer: &mut GlesRenderer) -> Option<Loss> {
    renderer.graphics_reset_status().ok().and_then(loss)
}

/// `GL_PURGED_CONTEXT_RESET_NV`: the NVIDIA driver lost the video memory.
const PURGED_CONTEXT_RESET_NV: u32 = 0x92BB;

fn loss(status: GraphicsResetStatus) -> Option<Loss> {
    let reset = |reason: &str| Some(Loss::Reset(reason.to_owned()));
    match status {
        GraphicsResetStatus::Guilty => reset("the server's rendering caused it"),
        GraphicsResetStatus::Innocent => reset("another program caused it"),
        GraphicsResetStatus::Other(PURGED_CONTEXT_RESET_NV) => Some(Loss::Purge),
        GraphicsResetStatus::Unknown | GraphicsResetStatus::Other(_) => {
            reset("the cause is unknown")
        }
        GraphicsResetStatus::NoError | GraphicsResetStatus::Unsupported => None,
    }
}

/// A reset of `gpu` for `reason`, and how recovering from it went: a warning, the event `server.gpu.reset`
/// and a line of `session.log` in the session's state directory, none without one.
pub fn record(gpu: &str, reason: &str, recovery: &Result<(), String>) {
    record_in(state_dir(), gpu, reason, recovery);
}

fn record_in(dir: Option<PathBuf>, gpu: &str, reason: &str, recovery: &Result<(), String>) {
    let outcome = match recovery {
        Ok(()) => "renderer remade, the clients stayed connected".to_owned(),
        Err(e) => format!("renderer not remade ({e}), the session ends"),
    };
    tracing::warn!(gpu, reason, outcome, "the GPU was reset");
    let remade = recovery.is_ok();
    let event = if remade {
        diag::event(Severity::Warn, "server.gpu.reset", "the GPU was reset")
    } else {
        diag::event(Severity::Error, "server.gpu.reset", "the GPU was reset")
    };
    event
        .field("gpu", Value::Path(Path::new(gpu)))
        .field("reason", Value::Public(reason))
        .field("outcome", Value::Public(&outcome))
        .field("remade", Value::Bool(remade))
        .emit();
    let Some(dir) = dir else { return };
    if let Err(e) = append(
        &dir.join("session.log"),
        &line(&local_now(), gpu, reason, &outcome),
    ) {
        tracing::warn!(error = %e, "cannot record the GPU reset in session.log");
    }
}

fn line(time: &str, gpu: &str, reason: &str, outcome: &str) -> String {
    format!(
        "{time} {LOG_MARK} gpu={} reason={}; {}\n",
        clean(gpu),
        clean(reason),
        clean(outcome)
    )
}

/// `s` with its control characters as spaces, so it stays one line of `session.log`.
pub fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub fn append(path: &Path, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

/// `$XDG_STATE_HOME/cupertino`, else `~/.local/state/cupertino`, where the session keeps `session.log`.
pub fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("cupertino"))
}

/// The local time as `session.log` has it, `YYYY-MM-DD HH:MM:SS`; empty when the C library cannot tell.
pub fn local_now() -> String {
    // SAFETY: `time` with a null pointer only returns the time; `localtime_r` writes the zeroed `tm` it is given.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    if unsafe { libc::localtime_r(&now, &mut tm) }.is_null() {
        return String::new();
    }
    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}
