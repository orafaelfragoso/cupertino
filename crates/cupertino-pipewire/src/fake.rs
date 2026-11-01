//! A stand-in `wpctl` for tests: a shell script that keeps the default sink and source in files and prints levels
//! with two decimals, as `wpctl get-volume` does.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

use crate::{parse_volume, Target, Volume};

const SCRIPT: &str = r#"#!/bin/sh
dir=$(dirname "$0")
if [ -e "$dir/down" ]; then echo "Could not connect to PipeWire" >&2; exit 1; fi
case "$2" in
@DEFAULT_AUDIO_SINK@) node="$dir/sink" ;;
@DEFAULT_AUDIO_SOURCE@) node="$dir/source" ;;
*) echo "Translate ID error: '$2' is not a valid ID" >&2; exit 1 ;;
esac
read -r level muted < "$node"
case "$1" in
get-volume) if [ "$muted" = 1 ]; then printf 'Volume: %.2f [MUTED]\n' "$level"; else printf 'Volume: %.2f\n' "$level"; fi ;;
set-volume) echo "$3 $muted" > "$node" ;;
set-mute) echo "$level $3" > "$node" ;;
*) exit 1 ;;
esac
"#;

/// The script and its state, removed on drop.
pub struct FakeWpctl {
    dir: tempfile::TempDir,
}

impl FakeWpctl {
    /// A sink and a source at these volumes.
    pub fn new(sink: Volume, source: Volume) -> Self {
        let dir = tempfile::tempdir().expect("a scratch directory");
        let program = dir.path().join("wpctl");
        fs::write(&program, SCRIPT).expect("the script is written");
        fs::set_permissions(&program, fs::Permissions::from_mode(0o755))
            .expect("the script is executable");
        let fake = Self { dir };
        fake.set(Target::DefaultSink, sink);
        fake.set(Target::DefaultSource, source);
        fake
    }

    /// The program to run instead of `wpctl`.
    pub fn program(&self) -> PathBuf {
        self.dir.path().join("wpctl")
    }

    /// What `wpctl get-volume` prints for `target` now.
    pub fn volume(&self, target: Target) -> Volume {
        let output = std::process::Command::new(self.program())
            .args(["get-volume", target.id()])
            .output()
            .expect("the script runs");
        parse_volume(&String::from_utf8_lossy(&output.stdout)).expect("the script prints a volume")
    }

    /// Change `target` behind the caller's back.
    pub fn set(&self, target: Target, volume: Volume) {
        fs::write(
            self.node(target),
            format!("{} {}\n", volume.level, u8::from(volume.muted)),
        )
        .expect("the state is written");
    }

    /// Make every call fail as when no PipeWire daemon runs, or answer again.
    pub fn set_running(&self, running: bool) {
        let down = self.dir.path().join("down");
        if running {
            let _ = fs::remove_file(down);
        } else {
            fs::write(down, "").expect("the marker is written");
        }
    }

    fn node(&self, target: Target) -> PathBuf {
        let name = match target {
            Target::DefaultSink => "sink",
            Target::DefaultSource => "source",
        };
        self.dir.path().join(name)
    }
}
