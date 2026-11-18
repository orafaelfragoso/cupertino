//! The session bus activates the mixer on the first Audio1 call.

use std::path::Path;

use cupertino_alsa::{Master, OpenMode, DEVICE_DIR};
use cupertino_pipewire::{Sink, Target, Volume, Wpctl};
use zbus::fdo::{Error, Result};

/// Well-known session bus name.
const NAME: &str = "org.cupertino.Audio1";
/// Object implementing the Audio1 contract.
const PATH: &str = "/org/cupertino/Audio1";

fn failed(e: impl std::fmt::Display) -> Error {
    Error::Failed(e.to_string())
}

fn target(direction: &str) -> Result<Target> {
    match direction {
        "input" => Ok(Target::DefaultSource),
        "output" => Ok(Target::DefaultSink),
        _ => Err(Error::InvalidArgs("Expected input or output.".into())),
    }
}

#[derive(Default)]
struct Audio {
    pw: Wpctl,
    master: Option<Master>,
}

impl Audio {
    fn alsa<T>(&mut self, action: impl FnOnce(&Master) -> Result<T>) -> Result<T> {
        if self.master.is_none() {
            self.master = Some(
                Master::open_default(Path::new(DEVICE_DIR), OpenMode::ReadWrite).map_err(failed)?,
            );
        }
        let result = action(self.master.as_ref().expect("opened above"));
        if matches!(result, Err(Error::Failed(_))) {
            self.master = None;
        }
        result
    }

    fn devices(&mut self, target: Target) -> Result<Vec<Sink>> {
        let list = match target {
            Target::DefaultSink => self.pw.sinks(),
            Target::DefaultSource => self.pw.sources(),
        };
        match list {
            Ok(rows) => Ok(rows),
            // ALSA playback has a single master rather than a selectable device list.
            Err(_) if target == Target::DefaultSink => self.alsa(|_| Ok(Vec::new())),
            Err(e) => Err(failed(e)),
        }
    }

    fn stereo(&mut self) -> Result<(f64, f64)> {
        if self.pw.volume(Target::DefaultSink).is_err() {
            return self.alsa(|_| {
                Err(Error::NotSupported(
                    "The playback master does not support balance.".into(),
                ))
            });
        }
        match self.pw.balance() {
            Ok((balance, level))
                if (-1.0..=1.0).contains(&balance) && (0.0..=1.0).contains(&level) =>
            {
                Ok((balance, level))
            }
            Ok(_) => Err(Error::Failed("Invalid output channel levels.".into())),
            Err(cupertino_pipewire::Error::Unparsed(text))
                if text.starts_with("Volume:")
                    && text.lines().next().is_some_and(|line| {
                        line.split_whitespace().any(|s| {
                            s.strip_suffix('%')
                                .is_some_and(|p| p.parse::<u32>().is_ok())
                        })
                    }) =>
            {
                Err(Error::NotSupported("The output is not stereo.".into()))
            }
            Err(e) => Err(failed(e)),
        }
    }
}

// Dispatch in wire order. In particular no other request can run between validating a default,
// setting its level and mute, and reading back the level the backend actually reached.
#[zbus::interface(name = "org.cupertino.Audio1", spawn = false)]
impl Audio {
    fn list_devices(&mut self, direction: &str) -> Result<Vec<(u32, String, bool)>> {
        Ok(self
            .devices(target(direction)?)?
            .into_iter()
            .map(|s| (s.id, s.name, s.default))
            .collect())
    }

    fn set_default(&mut self, direction: &str, id: u32) -> Result<()> {
        let target = target(direction)?;
        if !self.devices(target)?.iter().any(|s| s.id == id) {
            return Err(Error::Failed(
                "The selected device is no longer available.".into(),
            ));
        }
        match target {
            Target::DefaultSink => self.pw.set_default_sink(id),
            Target::DefaultSource => self.pw.set_default_source(id),
        }
        .map_err(failed)
    }

    #[zbus(out_args("level", "muted"))]
    fn get_volume(&mut self, direction: &str) -> Result<(f64, bool)> {
        let target = target(direction)?;
        match self.pw.volume(target) {
            Ok(v) => Ok((v.level, v.muted)),
            Err(_) if target == Target::DefaultSink => self.alsa(|m| {
                let level = m.level().map_err(failed)?;
                Ok((level, m.muted().map_err(failed)?.unwrap_or(level == 0.0)))
            }),
            Err(e) => Err(failed(e)),
        }
    }

    #[zbus(out_args("actual_level", "actual_muted"))]
    fn set_volume(&mut self, direction: &str, level: f64, muted: bool) -> Result<(f64, bool)> {
        let target = target(direction)?;
        if !(0.0..=1.0).contains(&level) {
            return Err(Error::InvalidArgs(
                "Expected a finite level between 0 and 1.".into(),
            ));
        }
        match self.pw.volume(target) {
            Ok(_) => {
                self.pw.set_level(target, level).map_err(failed)?;
                let mut state = self.pw.volume(target).map_err(failed)?;
                if state.muted != muted {
                    self.pw.set_muted(target, muted).map_err(failed)?;
                    state = self.pw.volume(target).map_err(failed)?;
                }
                let Volume {
                    level,
                    muted: actual,
                } = state;
                if actual != muted {
                    return Err(Error::NotSupported(
                        "The device cannot set the requested mute state.".into(),
                    ));
                }
                Ok((level, actual))
            }
            Err(_) if target == Target::DefaultSink => self.alsa(|m| {
                let switch = m.muted().map_err(failed)?;
                if switch.is_none() && muted != (level == 0.0) {
                    return Err(Error::NotSupported(
                        "The playback master has no mute switch.".into(),
                    ));
                }
                let actual = m.set_level(level).map_err(failed)?;
                if switch.is_some_and(|v| v != muted) {
                    m.set_muted(muted).map_err(failed)?;
                }
                let actual_mute = m.muted().map_err(failed)?.unwrap_or(actual == 0.0);
                if actual_mute != muted {
                    return Err(Error::NotSupported(
                        "The master cannot represent the requested mute state.".into(),
                    ));
                }
                Ok((actual, actual_mute))
            }),
            Err(e) => Err(failed(e)),
        }
    }

    fn get_balance(&mut self) -> Result<f64> {
        self.stereo().map(|(balance, _)| balance)
    }

    fn set_balance(&mut self, balance: f64) -> Result<()> {
        if !(-1.0..=1.0).contains(&balance) {
            return Err(Error::InvalidArgs(
                "Expected a finite balance between -1 and 1.".into(),
            ));
        }
        let (_, level) = self.stereo()?;
        self.pw.set_balance(balance, level).map_err(failed)
    }
}

fn main() -> zbus::Result<()> {
    let conn = zbus::blocking::connection::Builder::session()?
        .serve_at(PATH, Audio::default())?
        .name(NAME)?
        .allow_name_replacements(false)
        .replace_existing_names(false)
        .build()?;
    conn.closed();
    Ok(())
}
