//! The default PipeWire sink and source through WirePlumber's `wpctl`, so a level read or set here is the level
//! `wpctl get-volume` reports. See the README.

use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::process::{Command, Output};

#[cfg(any(test, feature = "fake"))]
pub mod fake;

/// The program run when no other is given.
pub const WPCTL: &str = "wpctl";

/// Which node a call changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    /// The output applications play to.
    DefaultSink,
    /// The input applications record from.
    DefaultSource,
}

impl Target {
    fn id(self) -> &'static str {
        match self {
            Target::DefaultSink => "@DEFAULT_AUDIO_SINK@",
            Target::DefaultSource => "@DEFAULT_AUDIO_SOURCE@",
        }
    }
}

/// A node's volume.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Volume {
    /// Level, 0 to 1. A node amplified above 100% reads as 1.
    pub level: f64,
    /// Whether the node is muted.
    pub muted: bool,
}

/// Why a call failed.
#[derive(Debug)]
pub enum Error {
    /// The program could not be started.
    Spawn {
        /// The program.
        program: PathBuf,
        /// Why.
        source: std::io::Error,
    },
    /// The program ran and failed, for example because no PipeWire daemon or no default node answers.
    Failed {
        /// The exit status, `None` when a signal ended it.
        code: Option<i32>,
        /// What it wrote to its standard error, trimmed.
        stderr: String,
    },
    /// `get-volume` printed something other than `Volume: <level>`, optionally followed by `[MUTED]`.
    Unparsed(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Spawn { program, source } => {
                write!(f, "cannot run {}: {source}", program.display())
            }
            Self::Failed {
                code: Some(code),
                stderr,
            } => write!(f, "wpctl exited with {code}: {stderr}"),
            Self::Failed { code: None, stderr } => write!(f, "wpctl was killed: {stderr}"),
            Self::Unparsed(line) => write!(f, "unexpected wpctl output {line:?}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Spawn { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Result of this crate's calls.
pub type Result<T> = std::result::Result<T, Error>;

/// The `wpctl` program. Every call starts it once and waits for it.
#[derive(Clone, Debug)]
pub struct Wpctl {
    program: PathBuf,
}

impl Default for Wpctl {
    fn default() -> Self {
        Self::new(PathBuf::from(WPCTL))
    }
}

impl Wpctl {
    /// `program` is looked up on `PATH` when it has no directory.
    pub fn new(program: PathBuf) -> Self {
        Self { program }
    }

    /// The volume of `target` now.
    pub fn volume(&self, target: Target) -> Result<Volume> {
        let output = self.run(["get-volume", target.id()])?;
        parse_volume(&String::from_utf8_lossy(&output.stdout))
    }

    /// Set the level of `target`, clamped to 0 to 1, without touching its mute state.
    pub fn set_level(&self, target: Target, level: f64) -> Result<()> {
        let level = if level.is_nan() {
            0.0
        } else {
            level.clamp(0.0, 1.0)
        };
        self.run(["set-volume", target.id(), &format!("{level:.4}")])
            .map(drop)
    }

    /// Mute or unmute `target`.
    pub fn set_muted(&self, target: Target, muted: bool) -> Result<()> {
        self.run(["set-mute", target.id(), if muted { "1" } else { "0" }])
            .map(drop)
    }

    fn run<const N: usize>(&self, args: [&str; N]) -> Result<Output> {
        let output = Command::new(&self.program)
            .args(args.map(OsString::from))
            .output()
            .map_err(|source| Error::Spawn {
                program: self.program.clone(),
                source,
            })?;
        if !output.status.success() {
            return Err(Error::Failed {
                code: output.status.code(),
                stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
            });
        }
        Ok(output)
    }
}

/// The volume in `wpctl get-volume` output: `Volume: 0.40`, or `Volume: 0.40 [MUTED]`.
pub fn parse_volume(output: &str) -> Result<Volume> {
    let unparsed = || Error::Unparsed(output.trim().to_string());
    let rest = output.trim().strip_prefix("Volume:").ok_or_else(unparsed)?;
    let mut words = rest.split_whitespace();
    let level: f64 = words
        .next()
        .and_then(|w| w.parse().ok())
        .ok_or_else(unparsed)?;
    let muted = match words.next() {
        None => false,
        Some("[MUTED]") => true,
        Some(_) => return Err(unparsed()),
    };
    if !level.is_finite() || words.next().is_some() {
        return Err(unparsed());
    }
    Ok(Volume {
        level: level.clamp(0.0, 1.0),
        muted,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeWpctl;

    const QUIET: Volume = Volume {
        level: 0.25,
        muted: false,
    };

    #[test]
    fn a_level_set_is_the_level_wpctl_reports_and_keeps_the_mute_state() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let wpctl = Wpctl::new(fake.program());
        wpctl.set_muted(Target::DefaultSink, true).unwrap();
        wpctl.set_level(Target::DefaultSink, 0.5625).unwrap();
        let sink = Volume {
            level: 0.56,
            muted: true,
        };
        assert_eq!(wpctl.volume(Target::DefaultSink).unwrap(), sink);
        assert_eq!(fake.volume(Target::DefaultSink), sink);
        assert_eq!(
            fake.volume(Target::DefaultSource),
            QUIET,
            "the source is untouched"
        );
    }

    #[test]
    fn a_level_outside_the_range_is_clamped() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let wpctl = Wpctl::new(fake.program());
        wpctl.set_level(Target::DefaultSource, 1.7).unwrap();
        assert_eq!(fake.volume(Target::DefaultSource).level, 1.0);
        wpctl.set_level(Target::DefaultSource, -0.2).unwrap();
        assert_eq!(fake.volume(Target::DefaultSource).level, 0.0);
    }

    #[test]
    fn without_a_pipewire_daemon_every_call_fails_with_wpctl_s_message() {
        let fake = FakeWpctl::new(QUIET, QUIET);
        let wpctl = Wpctl::new(fake.program());
        fake.set_running(false);
        match wpctl.volume(Target::DefaultSink) {
            Err(Error::Failed { code, stderr }) => {
                assert_eq!(code, Some(1));
                assert_eq!(stderr, "Could not connect to PipeWire");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
        assert!(wpctl.set_muted(Target::DefaultSink, true).is_err());
    }

    #[test]
    fn a_missing_program_fails_to_start() {
        let dir = tempfile::tempdir().unwrap();
        let wpctl = Wpctl::new(dir.path().join("wpctl"));
        assert!(matches!(
            wpctl.volume(Target::DefaultSink),
            Err(Error::Spawn { .. })
        ));
    }

    #[test]
    fn get_volume_output_gives_the_level_and_the_mute_state() {
        assert_eq!(
            parse_volume("Volume: 0.40\n").unwrap(),
            Volume {
                level: 0.4,
                muted: false
            }
        );
        assert_eq!(
            parse_volume("Volume: 0.00 [MUTED]\n").unwrap(),
            Volume {
                level: 0.0,
                muted: true
            }
        );
    }

    #[test]
    fn an_amplified_node_reads_as_full() {
        assert_eq!(parse_volume("Volume: 1.50").unwrap().level, 1.0);
    }

    #[test]
    fn other_output_is_refused() {
        for output in [
            "",
            "Translate ID error: ...",
            "Volume: loud",
            "Volume: 0.40 [LOUD]",
            "Volume: NaN",
            "Volume: 0.40 [MUTED] extra",
        ] {
            assert!(
                matches!(parse_volume(output), Err(Error::Unparsed(_))),
                "{output:?}"
            );
        }
    }
}
