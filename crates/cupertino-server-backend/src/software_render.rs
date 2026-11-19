//! Rendering without a GPU: when no card offers a hardware renderer (a virtual machine without 3D, or a GPU whose
//! driver did not load), the server draws the desktop with the software one instead of not starting. It says so in its
//! log, in a line of the session's `session.log`, which `cupertino-diagnose` reports, and in the diagnostic event
//! `server.renderer.software`.

use std::path::{Path, PathBuf};

use anyhow::Result;
use cupertino_diagnostic_events::{self as diag, Severity, Value};

use crate::gpu_reset::{append, clean, local_now, state_dir};

/// What a `session.log` line of a software renderer starts with, after its time.
pub const LOG_MARK: &str = "only a software renderer is available:";

/// The card that renders and what opening it gave: `first`, the one chosen, unless it has only a software renderer and
/// one of `rest`, in order, opens with a hardware one. A failure to open `first` is the error; one of `rest` that does
/// not open is skipped.
pub fn choose<C, R>(
    first: C,
    rest: impl IntoIterator<Item = C>,
    mut open: impl FnMut(&C) -> Result<R>,
    software: impl Fn(&R) -> bool,
) -> Result<(C, R)> {
    let opened = open(&first)?;
    if !software(&opened) {
        return Ok((first, opened));
    }
    Ok(rest
        .into_iter()
        .find_map(|card| {
            let other = open(&card).ok()?;
            (!software(&other)).then_some((card, other))
        })
        .unwrap_or((first, opened)))
}

/// `gpu` renders with a software renderer: a warning in the server's log, the event `server.renderer.software` and a
/// line of `session.log`.
pub fn record(gpu: &str) {
    record_in(state_dir(), gpu);
}

/// [`record`], with `session.log` in `dir`.
fn record_in(dir: Option<PathBuf>, gpu: &str) {
    tracing::warn!(
        gpu,
        "only a software renderer is available: the CPU draws the desktop, slowly"
    );
    diag::event(
        Severity::Warn,
        "server.renderer.software",
        "only a software renderer is available",
    )
    .field("gpu", Value::Path(Path::new(gpu)))
    .emit();
    let Some(dir) = dir else { return };
    if let Err(e) = append(&dir.join("session.log"), &line(&local_now(), gpu)) {
        tracing::warn!(error = %e, "cannot record the software renderer in session.log");
    }
}

fn line(time: &str, gpu: &str) -> String {
    format!(
        "{time} {LOG_MARK} gpu={}; the CPU draws the desktop\n",
        clean(gpu)
    )
}
