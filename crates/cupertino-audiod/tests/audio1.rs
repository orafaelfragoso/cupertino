//! Activate the packaged service on a private bus, with file-backed wpctl and pactl stand-ins.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use tempfile::TempDir;
use zbus::blocking::{connection::Builder, Connection, Proxy};

const NAME: &str = "org.cupertino.Audio1";
const PATH: &str = "/org/cupertino/Audio1";
const ACTIVATION: &str = include_str!("../data/dbus-1/services/org.cupertino.Audio1.service");
const STATUS: &str = "Audio\n ├─ Sinks:\n │  *   5. Speakers [vol: 0.50]\n │      9. Headphones [vol: 0.20]\n ├─ Sources:\n │  *   7. Microphone\n";

struct Bus {
    process: Child,
    dir: TempDir,
    connection: Connection,
}

impl Bus {
    fn new() -> Self {
        let dir = TempDir::new().unwrap();
        let (bin, data) = (dir.path().join("bin"), dir.path().join("data"));
        let services = data.join("dbus-1/services");
        fs::create_dir_all(&services).unwrap();
        fs::create_dir(&bin).unwrap();
        // The service file is the shipping one, with only the staging prefix changed.
        let installed = "/usr/bin/cupertino-audiod";
        assert!(ACTIVATION.contains(&format!("Exec={installed}\n")));
        fs::write(
            services.join(format!("{NAME}.service")),
            ACTIVATION.replace(installed, env!("CARGO_BIN_EXE_cupertino-audiod")),
        )
        .unwrap();
        // The Cargo-built stand-in reads each response beside its own executable.
        let test = std::env::current_exe().unwrap();
        let stand_in = test
            .ancestors()
            .map(|dir| dir.join("examples/audio-backend-stand-in"))
            .find(|path| path.is_file())
            .expect("Cargo built the backend example");
        fs::copy(stand_in, bin.join("wpctl")).unwrap();
        fs::copy(bin.join("wpctl"), bin.join("pactl")).unwrap();
        let daemon = std::env::split_paths(&std::env::var_os("PATH").unwrap())
            .map(|dir| dir.join("dbus-daemon"))
            .find(|p| p.is_file())
            .expect("dbus-daemon is installed");
        let mut process = Command::new(daemon)
            .args(["--session", "--nofork", "--print-address=1"])
            .env("XDG_DATA_HOME", &data)
            .env("XDG_DATA_DIRS", &data)
            .env("PATH", &bin)
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut address = String::new();
        BufReader::new(process.stdout.take().unwrap())
            .read_line(&mut address)
            .unwrap();
        let connection = Builder::address(address.trim())
            .unwrap()
            .method_timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        Self {
            process,
            dir,
            connection,
        }
    }

    fn path(&self, file: &str) -> PathBuf {
        self.dir.path().join("bin").join(file)
    }

    fn answer(&self, level: &str, balance: &str) {
        fs::write(
            self.path("behaviour"),
            format!(
            "get-volume\tstdout\t{level}\nstatus\tstdout\t{}\nget-sink-volume\tstdout\t{balance}\n",
            STATUS.replace('\n', "\\n")
        ),
        )
        .unwrap();
    }

    fn proxy(&self) -> Proxy<'_> {
        Proxy::new(&self.connection, NAME, PATH, NAME).unwrap()
    }
}

impl Drop for Bus {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

fn refused<T: std::fmt::Debug>(result: zbus::Result<T>, expected: &str) {
    let error = result.unwrap_err();
    assert!(
        matches!(&error, zbus::Error::MethodError(name, _, _)
        if name.as_str() == format!("org.freedesktop.DBus.Error.{expected}")),
        "{error}"
    );
}

#[test]
fn activated_service_lists_selects_reads_and_writes_the_current_devices() {
    let bus = Bus::new();
    let stereo = "Volume: front-left: 32768 / 50% / ..., front-right: 16384 / 25% / ...";
    bus.answer("Volume: 0.51 [MUTED]", stereo);
    let proxy = bus.proxy();
    type Rows = Vec<(u32, String, bool)>;
    assert_eq!(
        proxy.call::<_, _, Rows>("ListDevices", &"output").unwrap(),
        [
            (5, "Speakers".into(), true),
            (9, "Headphones".into(), false)
        ]
    );
    assert_eq!(
        proxy.call::<_, _, Rows>("ListDevices", &"input").unwrap(),
        [(7, "Microphone".into(), true)]
    );
    for (direction, id) in [("input", 7u32), ("output", 9)] {
        proxy
            .call::<_, _, ()>("SetDefault", &(direction, id))
            .unwrap();
        assert_eq!(
            proxy
                .call::<_, _, (f64, bool)>("GetVolume", &direction)
                .unwrap(),
            (0.51, true)
        );
    }
    // An external change is visible immediately. The command returns the backend's actual readback.
    bus.answer("Volume: 0.42", stereo);
    for direction in ["input", "output"] {
        assert_eq!(
            proxy
                .call::<_, _, (f64, bool)>("SetVolume", &(direction, 0.417, false))
                .unwrap(),
            (0.42, false)
        );
    }
    assert_eq!(proxy.call::<_, _, f64>("GetBalance", &()).unwrap(), -0.5);
    proxy.call::<_, _, ()>("SetBalance", &1.0f64).unwrap();
    let calls = fs::read_to_string(bus.path("calls")).unwrap();
    for command in [
        "set-default 7",
        "set-default 9",
        "set-volume @DEFAULT_AUDIO_SINK@ 0.4170",
        "set-volume @DEFAULT_AUDIO_SOURCE@ 0.4170",
        "set-sink-volume @DEFAULT_SINK@ 0% 50%",
    ] {
        assert!(calls.lines().any(|c| c == command), "{command}: {calls}");
    }
    bus.answer("Volume: 0.42 [MUTED]", stereo);
    refused(
        proxy.call::<_, _, (f64, bool)>("SetVolume", &("input", 0.5, false)),
        "NotSupported",
    );
    assert!(fs::read_to_string(bus.path("calls"))
        .unwrap()
        .contains("set-mute @DEFAULT_AUDIO_SOURCE@ 0"));
    assert!(matches!(
        bus.connection.request_name_with_flags(
            NAME,
            zbus::fdo::RequestNameFlags::ReplaceExisting | zbus::fdo::RequestNameFlags::DoNotQueue
        ),
        Err(zbus::Error::NameTaken) | Ok(zbus::fdo::RequestNameReply::Exists)
    ));
}

#[test]
fn concurrent_callers_cannot_interleave_the_steps_of_two_volume_changes() {
    let bus = Bus::new();
    bus.answer("Volume: 0.42", "");
    std::thread::scope(|scope| {
        for direction in ["input", "output"] {
            let proxy = bus.proxy();
            scope.spawn(move || {
                proxy
                    .call::<_, _, (f64, bool)>("SetVolume", &(direction, 0.417, false))
                    .unwrap()
            });
        }
    });
    let log = fs::read_to_string(bus.path("calls")).unwrap();
    let calls: Vec<_> = log.lines().collect();
    assert_eq!(calls.len(), 6, "{log}");
    for three in calls.as_chunks::<3>().0 {
        let target = three[0].strip_prefix("get-volume ").unwrap();
        assert_eq!(
            *three,
            [
                format!("get-volume {target}"),
                format!("set-volume {target} 0.4170"),
                format!("get-volume {target}")
            ],
            "{log}"
        );
    }
}

#[test]
fn invalid_arguments_stale_devices_and_unsupported_controls_have_distinct_bus_errors() {
    let bus = Bus::new();
    bus.answer("Volume: 0.51", "Volume: mono: 32768 / 50% / ...");
    let proxy = bus.proxy();
    for level in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
        refused(
            proxy.call::<_, _, (f64, bool)>("SetVolume", &("output", level, false)),
            "InvalidArgs",
        );
    }
    for balance in [f64::NEG_INFINITY, f64::NAN, -1.01, 1.01] {
        refused(
            proxy.call::<_, _, ()>("SetBalance", &balance),
            "InvalidArgs",
        );
    }
    refused(
        proxy.call::<_, _, Vec<(u32, String, bool)>>("ListDevices", &"sink"),
        "InvalidArgs",
    );
    refused(
        proxy.call::<_, _, (f64, bool)>("GetVolume", &"sink"),
        "InvalidArgs",
    );
    assert!(!bus.path("calls").exists());
    refused(
        proxy.call::<_, _, ()>("SetDefault", &("output", 7u32)),
        "Failed",
    );
    refused(
        proxy.call::<_, _, ()>("SetDefault", &("input", 5u32)),
        "Failed",
    );
    refused(proxy.call::<_, _, f64>("GetBalance", &()), "NotSupported");
    refused(
        proxy.call::<_, _, ()>("SetBalance", &0.0f64),
        "NotSupported",
    );
    let outputs: Vec<(u32, String, bool)> = proxy.call("ListDevices", &"output").unwrap();
    let removed = outputs[0].0;
    fs::write(
        bus.path("behaviour"),
        "status\tstdout\tAudio\\n ├─ Sinks:\\n\n",
    )
    .unwrap();
    refused(
        proxy.call::<_, _, ()>("SetDefault", &("output", removed)),
        "Failed",
    );
    fs::write(bus.path("behaviour"), "*\texit\t1\n").unwrap();
    refused(
        proxy.call::<_, _, Vec<(u32, String, bool)>>("ListDevices", &"input"),
        "Failed",
    );
    refused(
        proxy.call::<_, _, (f64, bool)>("GetVolume", &"input"),
        "Failed",
    );
    let calls = fs::read_to_string(bus.path("calls")).unwrap();
    assert!(!calls.contains("set-default"));
    assert!(!calls.contains("set-sink-volume"));
}
