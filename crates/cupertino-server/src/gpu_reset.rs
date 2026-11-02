//! Recovering from a GPU reset: the rendering context is lost, the backend makes a new renderer, every GPU object of the
//! server is made again from it and every client buffer is imported again, so the clients stay connected. Each reset is
//! a line in the session's `session.log`, which `cupertino-diagnose` reports. Too many resets in a row end the session as
//! a lost context did before, so a GPU that keeps resetting cannot hang it.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use smithay::backend::renderer::gles::{GlesRenderer, GraphicsResetStatus};

use crate::render::scene::RenderResources;
use crate::state::Cupertino;

/// What a `session.log` line of a reset starts with, after its time.
pub const LOG_MARK: &str = "the GPU was reset:";

/// More resets than this within [`WINDOW`] end the session.
const LIMIT: usize = 3;
const WINDOW: Duration = Duration::from_secs(60);

/// The resets of the last [`WINDOW`].
#[derive(Debug, Default)]
pub struct Resets(Vec<Instant>);

impl Resets {
    /// Counts a reset at `now`; whether recovery may be tried, or the session should end.
    pub fn allow(&mut self, now: Instant) -> bool {
        self.0
            .retain(|&at| now.saturating_duration_since(at) < WINDOW);
        self.0.push(now);
        self.0.len() <= LIMIT
    }
}

/// Whether the GPU was reset since the last check, and why, as `session.log` says it; `None` with no reset or a driver
/// that cannot tell.
pub fn status(renderer: &mut GlesRenderer) -> Option<&'static str> {
    match renderer.graphics_reset_status() {
        Ok(GraphicsResetStatus::Guilty) => Some("the server's rendering caused it"),
        Ok(GraphicsResetStatus::Innocent) => Some("another program caused it"),
        Ok(GraphicsResetStatus::Unknown | GraphicsResetStatus::Other(_)) => {
            Some("the cause is unknown")
        }
        Ok(GraphicsResetStatus::NoError | GraphicsResetStatus::Unsupported) | Err(_) => None,
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
        Ok(())
    }
}

/// A reset of `gpu` for `reason`, and how recovering from it went, as a line of `session.log` in the session's state
/// directory; nothing without one.
pub fn record(gpu: &str, reason: &str, recovery: &Result<(), String>) {
    let outcome = match recovery {
        Ok(()) => "renderer remade, the clients stayed connected".to_owned(),
        Err(e) => format!("renderer not remade ({e}), the session ends"),
    };
    tracing::warn!(gpu, reason, outcome, "the GPU was reset");
    let Some(dir) = state_dir() else { return };
    if let Err(e) = append(
        &dir.join("session.log"),
        &line(&local_now(), gpu, reason, &outcome),
    ) {
        tracing::warn!(error = %e, "cannot record the GPU reset in session.log");
    }
}

fn line(time: &str, gpu: &str, reason: &str, outcome: &str) -> String {
    let clean = |s: &str| {
        s.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect::<String>()
    };
    format!(
        "{time} {LOG_MARK} gpu={} reason={}; {}\n",
        clean(gpu),
        clean(reason),
        clean(outcome)
    )
}

fn append(path: &Path, line: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    file.write_all(line.as_bytes())
}

/// `$XDG_STATE_HOME/cupertino`, else `~/.local/state/cupertino`, where `cupertino-session` keeps `session.log`.
fn state_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/state")))?;
    Some(base.join("cupertino"))
}

/// The local time as `cupertino-session` writes it, `YYYY-MM-DD HH:MM:SS`; empty when the C library cannot tell.
fn local_now() -> String {
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
        let mut resets = Resets::default();
        for s in 0..3 {
            assert!(resets.allow(start + Duration::from_secs(s)));
        }
        assert!(!resets.allow(start + Duration::from_secs(3)));
        assert!(resets.allow(start + Duration::from_secs(70)));
    }

    #[test]
    fn a_reset_is_one_line_of_the_session_log() {
        assert_eq!(
            line("2026-11-01 22:40:00", "/dev/dri/card1", "another program\ncaused it", "renderer remade"),
            "2026-11-01 22:40:00 the GPU was reset: gpu=/dev/dri/card1 reason=another program caused it; renderer remade\n"
        );
    }
}
