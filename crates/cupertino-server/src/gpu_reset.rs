//! Recovering from a GPU reset: the rendering context is lost, the backend makes a new renderer, every GPU object of the
//! server is made again from it and every client buffer is imported again, so the clients stay connected. Each reset is
//! a line in the session's `session.log`, which `cupertino-diagnose` reports, and a `server.gpu.reset` diagnostic event.
//! Too many resets in a row end the session as a lost context did before, so a GPU that keeps resetting cannot hang it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use cupertino_diagnostic_events::{self as diag, Severity, Value};
use smithay::backend::renderer::gles::{GlesRenderer, GraphicsResetStatus};

use crate::render::scene::RenderResources;
use crate::state::Cupertino;

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

impl Cupertino {
    /// Makes every GPU object of the scene again with `renderer`, the new one: the shaders and passes. The window
    /// snapshots and closing ghosts lived in the lost context and are dropped; a window closing later fades out from a
    /// capture made with the new renderer. Each output's next frame is full.
    pub fn rebuild_render(&mut self, renderer: &mut GlesRenderer) -> anyhow::Result<()> {
        self.fx.drop_snapshots();
        self.render = None;
        self.render = Some(RenderResources::new(renderer)?);
        for output in &mut self.outputs {
            output.tracker = smithay::backend::renderer::damage::OutputDamageTracker::from_output(
                &output.output,
            );
            output.scheduler.reset();
        }
        self.request_redraw();
        // clients drawing on a GPU of their own may have lost their buffers' pixels too: they draw whole again
        self.sessions.buffers_lost();
        Ok(())
    }
}

/// A reset of `gpu` for `reason`, and how recovering from it went: a warning, the event `server.gpu.reset` (`warn` when
/// the renderer was made again, `error` when the session ends) and a line of `session.log` in the session's state
/// directory, none without one.
pub fn record(gpu: &str, reason: &str, recovery: &Result<(), String>) {
    record_in(state_dir(), gpu, reason, recovery);
}

/// [`record`], with `session.log` in `dir`.
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
pub(crate) fn clean(s: &str) -> String {
    s.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect()
}

pub(crate) fn append(path: &Path, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

/// `$XDG_STATE_HOME/cupertino`, else `~/.local/state/cupertino`, where `cupertino-session` keeps `session.log`.
pub(crate) fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("cupertino"))
}

/// The local time as `cupertino-session` writes it, `YYYY-MM-DD HH:MM:SS`; empty when the C library cannot tell.
pub(crate) fn local_now() -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fourth_reset_within_a_minute_ends_the_session() {
        let start = Instant::now();
        let reset = Loss::Reset("the cause is unknown".to_owned());
        let mut resets = Resets::default();
        for s in 0..3 {
            assert!(resets.allow(&reset, start + Duration::from_secs(s)));
        }
        assert!(!resets.allow(&reset, start + Duration::from_secs(3)));
        assert!(resets.allow(&reset, start + Duration::from_secs(70)));
    }

    #[test]
    fn purges_of_video_memory_never_end_the_session_nor_count_as_resets() {
        let start = Instant::now();
        let reset = Loss::Reset("the cause is unknown".to_owned());
        let mut resets = Resets::default();
        for s in 0..10 {
            assert!(resets.allow(&Loss::Purge, start + Duration::from_secs(s)));
        }
        for s in 10..13 {
            assert!(resets.allow(&reset, start + Duration::from_secs(s)));
        }
        assert!(!resets.allow(&reset, start + Duration::from_secs(13)));
    }

    #[test]
    fn a_purge_of_video_memory_is_named_as_its_cause() {
        assert_eq!(loss(GraphicsResetStatus::Other(0x92BB)), Some(Loss::Purge));
        assert_eq!(Loss::Purge.reason(), "the driver purged video memory");
        assert_eq!(
            loss(GraphicsResetStatus::Other(0x1234)),
            Some(Loss::Reset("the cause is unknown".to_owned()))
        );
        assert_eq!(loss(GraphicsResetStatus::NoError), None);
    }

    /// The lines of `session.log` in `dir`, each without its time.
    fn logged(dir: &Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("session.log"))
            .unwrap()
            .lines()
            .map(|l| l[l.find(LOG_MARK).unwrap()..].to_owned())
            .collect()
    }

    #[test]
    fn a_reset_the_renderer_survived_is_a_warning_event_beside_its_session_log_line() {
        crate::logging::test_events::memory();
        let dir = tempfile::tempdir().unwrap();
        record_in(
            Some(dir.path().to_owned()),
            "/dev/dri/card1",
            "another program caused it",
            &Ok(()),
        );
        assert_eq!(
            logged(dir.path()),
            ["the GPU was reset: gpu=/dev/dri/card1 reason=another program caused it; renderer remade, the clients stayed connected"]
        );
        let events = crate::logging::test_events::recorded("server.gpu.reset");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.severity, Severity::Warn);
        assert_eq!(event.message, "the GPU was reset");
        assert_eq!(event.fields.get("gpu").unwrap(), "/dev/dri/card1");
        assert_eq!(
            event.fields.get("reason").unwrap(),
            "another program caused it"
        );
        assert_eq!(
            event.fields.get("outcome").unwrap(),
            "renderer remade, the clients stayed connected"
        );
        assert_eq!(event.fields.get("remade").unwrap(), true);
        assert!(event.redacted.is_empty(), "{:?}", event.redacted);
    }

    #[test]
    fn a_reset_that_ends_the_session_is_an_error_event_and_its_private_paths_are_hashed() {
        crate::logging::test_events::memory();
        let dir = tempfile::tempdir().unwrap();
        let home = std::env::var("HOME").unwrap_or_else(|_| "/home/someone".into());
        let error = format!("cannot read {home}/private/notes.txt");
        record_in(
            Some(dir.path().to_owned()),
            "/dev/dri/card0",
            "the server's rendering caused it",
            &Err(error.clone()),
        );
        assert_eq!(
            logged(dir.path()),
            [format!("the GPU was reset: gpu=/dev/dri/card0 reason=the server's rendering caused it; renderer not remade ({error}), the session ends")]
        );
        let events = crate::logging::test_events::recorded("server.gpu.reset");
        assert_eq!(events.len(), 1);
        let event = &events[0];
        assert_eq!(event.severity, Severity::Error);
        assert_eq!(event.fields.get("remade").unwrap(), false);
        let outcome = event.fields.get("outcome").unwrap().as_str().unwrap();
        assert!(outcome.starts_with("renderer not remade ("), "{outcome}");
        assert!(outcome.ends_with("), the session ends"), "{outcome}");
        assert!(!outcome.contains("private/notes.txt"), "{outcome}");
        assert!(
            event.redacted.contains(&"outcome".to_owned()),
            "{:?}",
            event.redacted
        );
    }

    #[test]
    fn a_reset_is_one_line_of_the_session_log() {
        assert_eq!(
            line("2026-11-01 22:40:00", "/dev/dri/card1", "another program\ncaused it", "renderer remade"),
            "2026-11-01 22:40:00 the GPU was reset: gpu=/dev/dri/card1 reason=another program caused it; renderer remade\n"
        );
    }
}
