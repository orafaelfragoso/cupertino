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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::anyhow;

    /// A card by name, opened with a software renderer when its name says so.
    fn open(card: &&str) -> Result<bool> {
        match *card {
            "broken" => Err(anyhow!("cannot open {card}")),
            card => Ok(card.starts_with("software")),
        }
    }

    fn chosen(first: &'static str, rest: &[&'static str]) -> Result<(&'static str, bool)> {
        choose(first, rest.iter().copied(), open, |&software| software)
    }

    #[test]
    fn a_hardware_renderer_on_the_chosen_card_renders_without_trying_another() {
        let mut tried = Vec::new();
        let (card, _) = choose(
            "hardware",
            ["other"],
            |card| {
                tried.push(*card);
                open(card)
            },
            |&software| software,
        )
        .unwrap();
        assert_eq!(card, "hardware");
        assert_eq!(tried, ["hardware"]);
    }

    #[test]
    fn a_software_renderer_gives_way_to_the_first_card_with_a_hardware_one() {
        assert_eq!(
            chosen("software", &["broken", "software-too", "hardware"]).unwrap(),
            ("hardware", false)
        );
    }

    #[test]
    fn with_only_software_renderers_the_chosen_card_renders_in_software() {
        assert_eq!(
            chosen("software", &["software-too", "broken"]).unwrap(),
            ("software", true)
        );
        assert_eq!(chosen("software", &[]).unwrap(), ("software", true));
    }

    #[test]
    fn a_chosen_card_that_does_not_open_is_the_error() {
        assert!(chosen("broken", &["hardware"]).is_err());
    }

    #[test]
    fn a_software_renderer_is_a_warning_event_beside_its_session_log_line() {
        crate::logging::test_events::memory();
        let dir = tempfile::tempdir().unwrap();
        record_in(Some(dir.path().to_owned()), "/dev/dri/card1");
        let log = std::fs::read_to_string(dir.path().join("session.log")).unwrap();
        assert!(
            log.ends_with(
                " only a software renderer is available: gpu=/dev/dri/card1; the CPU draws the desktop\n"
            ),
            "{log}"
        );
        assert_eq!(log.lines().count(), 1);
        let events = crate::logging::test_events::recorded("server.renderer.software");
        assert_eq!(events.len(), 1);
        assert_eq!(events[0].severity, Severity::Warn);
        assert_eq!(events[0].message, "only a software renderer is available");
        assert_eq!(events[0].fields.get("gpu").unwrap(), "/dev/dri/card1");
        assert!(events[0].redacted.is_empty());
    }

    #[test]
    fn a_software_renderer_is_one_line_of_the_session_log() {
        assert_eq!(
            line("2026-11-02 09:00:00", "/dev/dri/card1"),
            "2026-11-02 09:00:00 only a software renderer is available: gpu=/dev/dri/card1; the CPU draws the desktop\n"
        );
    }
}
