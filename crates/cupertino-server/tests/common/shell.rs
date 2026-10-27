//! Real shell binaries for the golden scenes: built once with cargo, started with a cleared environment, and a private
//! session bus so no process sees the host's.
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use cupertino_server::authorization::ShellRole;
use cupertino_server::config::ShellProcess;

/// Set by the parent test process for its isolated worker, whose cleared environment cannot run cargo.
pub const BIN_DIR_ENV: &str = "CUPERTINO_GOLDEN_BIN_DIR";

const BINARIES: [&str; 2] = ["cupertino-dock", "cupertino-launcher"];

/// Build the `app` feature of the shell crates once (a no-op when current) and return the directory of the binaries.
pub fn bin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(dir) = std::env::var_os(BIN_DIR_ENV) {
            return PathBuf::from(dir);
        }
        // <target>/<profile>/deps/<this test> -> <target>/<profile>
        let profile_dir = std::env::current_exe()
            .unwrap()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .to_path_buf();
        let mut cargo =
            Command::new(std::env::var_os("CARGO").expect("CARGO is set by `cargo test`"));
        cargo.args(["build", "-p", "cupertino-dock", "-p", "cupertino-launcher"]);
        cargo.args(["--features", "cupertino-dock/app,cupertino-launcher/app"]);
        cargo.args([
            "--target-dir",
            profile_dir.parent().unwrap().to_str().unwrap(),
        ]);
        if profile_dir.file_name().unwrap() == "release" {
            cargo.arg("--release");
        }
        let status = cargo.status().unwrap();
        assert!(
            status.success(),
            "building the shell process binaries: {status}"
        );
        for name in BINARIES {
            assert!(profile_dir.join(name).is_file(), "missing binary {name}");
        }
        profile_dir
    })
}

/// No `servicedirs`: a shell process can never activate a host service on the private bus.
const BUS_CONFIG: &str = r#"<!DOCTYPE busconfig PUBLIC "-//freedesktop//DTD D-Bus Bus Configuration 1.0//EN"
 "http://www.freedesktop.org/standards/dbus/1.0/busconfig.dtd">
<busconfig>
  <type>session</type>
  <listen>unix:tmpdir=/tmp</listen>
  <auth>EXTERNAL</auth>
  <policy context="default">
    <allow send_destination="*" eavesdrop="true"/>
    <allow eavesdrop="true"/>
    <allow own="*"/>
  </policy>
</busconfig>
"#;

/// A private session bus without service activation, killed on drop.
pub struct PrivateBus {
    pub address: String,
    pid: String,
}

impl PrivateBus {
    pub fn start() -> PrivateBus {
        let config = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(config.path(), BUS_CONFIG).unwrap();
        let out = Command::new("dbus-daemon")
            .arg(format!("--config-file={}", config.path().display()))
            .args(["--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .expect("goldens need dbus-daemon");
        assert!(out.status.success(), "dbus-daemon failed: {}", out.status);
        let text = String::from_utf8(out.stdout).unwrap();
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        PrivateBus {
            address: lines.next().unwrap().to_string(),
            pid: lines.next().unwrap().to_string(),
        }
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        assert!(
            Command::new("kill")
                .arg(&self.pid)
                .status()
                .unwrap()
                .success(),
            "killing the private bus"
        );
    }
}

/// The shell binary `binary` on the private bus with the built-in assets, no host application data, and `args`. The
/// calling worker's `HOME`, `XDG_*`, `LANG` and `TZ` are already the isolated ones; `env` passes the bus and the empty
/// asset search path on.
pub fn process(binary: &str, bus_address: &str, args: &[&str]) -> ShellProcess {
    let mut command = vec![
        "env".to_string(),
        format!("DBUS_SESSION_BUS_ADDRESS={bus_address}"),
        "CUPERTINO_ASSET_SEARCH_PATH=".to_string(),
        bin_dir().join(binary).display().to_string(),
    ];
    command.extend(args.iter().map(|a| (*a).to_string()));
    ShellProcess {
        name: binary.into(),
        command,
        roles: match binary {
            "cupertino-dock" => vec![ShellRole::Session, ShellRole::Dock],
            _ => vec![ShellRole::Session],
        },
    }
}
