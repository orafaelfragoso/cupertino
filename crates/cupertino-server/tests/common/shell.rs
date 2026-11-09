//! Real shell binaries for the golden scenes: prepared before the tests, started with a cleared environment, and a private
//! session bus so no process sees the host's.
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cupertino_server::authorization::ShellRole;
use cupertino_server::config::ShellProcess;

const BINARIES: [&str; 3] = ["cupertino-dock", "cupertino-launcher", "cupertino-login"];

/// The shell binaries prepared once by `ci/build-test-fixtures.py` before the test runner starts.
pub fn bin_dir() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let mut directory = None;
        for name in BINARIES {
            let path = super::fixtures::artifact(name, "bin", name);
            let parent = path.parent().unwrap().to_path_buf();
            if let Some(expected) = &directory {
                assert_eq!(&parent, expected, "shell fixture directories differ");
            }
            directory = Some(parent);
        }
        directory.unwrap()
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

/// A private session bus without service activation: a child of the test that dies with it
/// ([`cupertino_test_bus::PrivateBus`]).
pub fn private_bus() -> cupertino_test_bus::PrivateBus {
    let config = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(config.path(), BUS_CONFIG).unwrap();
    // the daemon has read its configuration once it prints its address
    cupertino_test_bus::PrivateBus::start_with_config(config.path())
        .expect("goldens need dbus-daemon")
}

/// The time the shell processes show (`CUPERTINO_TEST_CLOCK`, debug builds): 2027-01-01 09:41 UTC, a Friday. Their
/// display clocks stand still at it, so a scene settles.
pub const TEST_CLOCK: &str = "1798796460";

/// The shell binary `binary` on the private bus with the built-in assets, no host application data, the fixed
/// [`TEST_CLOCK`], and `args`. The calling worker's `HOME`, `XDG_*`, `LANG` and `TZ` are already the isolated ones; `env`
/// passes the bus, the empty asset search path, the clock and the test PAM shim on: the shim starts every PAM
/// transaction from a private policy (the lock screen's, here) and refuses every connection to the journal and the real
/// buses ([`cupertino_test_pam`]).
pub fn process(binary: &str, bus_address: &str, args: &[&str]) -> ShellProcess {
    let mut command = vec![
        "env".to_string(),
        format!("DBUS_SESSION_BUS_ADDRESS={bus_address}"),
        "CUPERTINO_ASSET_SEARCH_PATH=".to_string(),
        format!("CUPERTINO_TEST_CLOCK={TEST_CLOCK}"),
    ];
    command.extend(cupertino_test_pam::shared().env_words());
    command.push(bin_dir().join(binary).display().to_string());
    command.extend(args.iter().map(|a| (*a).to_string()));
    ShellProcess {
        name: binary.into(),
        command,
        roles: match binary {
            "cupertino-dock" => vec![ShellRole::Session, ShellRole::Dock],
            "cupertino-login" => vec![ShellRole::Session, ShellRole::SessionLock],
            _ => vec![ShellRole::Session],
        },
    }
}
