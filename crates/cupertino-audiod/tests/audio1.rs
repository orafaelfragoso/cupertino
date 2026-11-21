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

/// What `wpctl status` prints on a machine with a webcam: video nodes, streams and endpoints around the audio ones.
const BUSY_STATUS: &str = "\
PipeWire 'pipewire-0' [1.4.2, user@host, cookie:123]
 └─ Clients:
        33. WirePlumber                         [1.4.2, user@host, pid:900]

Audio
 ├─ Devices:
 │      45. Built-in Audio                      [alsa]
 │
 ├─ Sinks:
 │      46. Built-in Audio Analog Stereo        [vol: 0.40]
 │  *   52. HDMI / DisplayPort 1                   [vol: 1.00]
 │
 ├─ Sink endpoints:
 │
 ├─ Sources:
 │  *   47. Built-in Audio Analog Stereo        [vol: 1.00]
 │      48. Webcam Microphone
 │
 └─ Streams:
        60. Firefox
             61. output_FL       > Built-in Audio:playback_FL

Video
 ├─ Devices:
 │      41. Webcam                              [v4l2]
 │
 ├─ Sinks:
 │
 ├─ Sources:
 │      42. Webcam: Webcam (V4L2)
";

type Rows = Vec<(u32, String, bool)>;

fn rows(list: &[(u32, &str, bool)]) -> Rows {
    list.iter()
        .map(|(id, n, d)| (*id, n.to_string(), *d))
        .collect()
}

impl Bus {
    /// Replace the stand-in's behaviour: each `(command, text)` prints `text` for `command`.
    fn prints(&self, lines: &[(&str, &str)]) {
        let text: String = lines
            .iter()
            .map(|(command, out)| format!("{command}\tstdout\t{}\n", out.replace('\n', "\\n")))
            .collect();
        fs::write(self.path("behaviour"), text).unwrap();
    }
}

#[test]
fn device_lists_come_from_the_audio_section_in_order_with_the_default_marked() {
    let bus = Bus::new();
    let proxy = bus.proxy();
    for (name, status, outputs, inputs) in [
        (
            "video nodes, streams and endpoints left out, the bracket group cut off",
            BUSY_STATUS,
            rows(&[
                (46, "Built-in Audio Analog Stereo", false),
                (52, "HDMI / DisplayPort 1", true),
            ]),
            rows(&[
                (47, "Built-in Audio Analog Stereo", true),
                (48, "Webcam Microphone", false),
            ]),
        ),
        ("empty output", "", rows(&[]), rows(&[])),
        (
            "empty sections",
            "Audio\n ├─ Sinks:\n │\n └─ Streams:\n",
            rows(&[]),
            rows(&[]),
        ),
        (
            "video only",
            "Video\n ├─ Sinks:\n │      42. Camera\n ├─ Sources:\n │      43. Camera 2\n",
            rows(&[]),
            rows(&[]),
        ),
        (
            "lines that are not nodes skipped",
            "Audio\n ├─ Sinks:\n │      not-a-number. Name\n │      7. Kept\n │      8.NoSpace\n",
            rows(&[(7, "Kept", false)]),
            rows(&[]),
        ),
        (
            "brackets inside a name kept",
            "Audio\n ├─ Sinks:\n │      9. Name [with] brackets inside\n",
            rows(&[(9, "Name [with] brackets inside", false)]),
            rows(&[]),
        ),
    ] {
        bus.prints(&[("status", status)]);
        assert_eq!(
            proxy.call::<_, _, Rows>("ListDevices", &"output").unwrap(),
            outputs,
            "{name}"
        );
        assert_eq!(
            proxy.call::<_, _, Rows>("ListDevices", &"input").unwrap(),
            inputs,
            "{name}"
        );
    }
}

#[test]
fn a_volume_line_is_a_level_in_zero_to_one_and_a_mute_state() {
    let bus = Bus::new();
    let proxy = bus.proxy();
    for (text, level, muted) in [
        ("Volume: 0.42\n", 0.42, false),
        ("Volume: 0.30 [MUTED]\n", 0.30, true),
        ("Volume: 1.50", 1.0, false),
        ("Volume: -0.20", 0.0, false),
    ] {
        bus.prints(&[("get-volume", text)]);
        assert_eq!(
            proxy
                .call::<_, _, (f64, bool)>("GetVolume", &"input")
                .unwrap(),
            (level, muted),
            "{text:?}"
        );
    }
}

#[test]
fn output_that_is_not_a_volume_line_fails_the_call_and_quotes_the_text() {
    let bus = Bus::new();
    let proxy = bus.proxy();
    for text in [
        "",
        "Volume:",
        "Volume: loud",
        "volume: 0.5",
        "Volume: 0.5 [MUTE]",
        "Volume: 0.5 [MUTED] extra",
        "Volume: NaN",
        "Volume: inf",
        "Volume: 0.5 0.5",
        "0.5",
    ] {
        bus.prints(&[("get-volume", text)]);
        let error = proxy
            .call::<_, _, (f64, bool)>("GetVolume", &"input")
            .unwrap_err();
        let zbus::Error::MethodError(name, message, _) = &error else {
            panic!("{text:?}: {error}")
        };
        assert_eq!(
            name.as_str(),
            "org.freedesktop.DBus.Error.Failed",
            "{text:?}"
        );
        assert_eq!(
            message.as_deref(),
            Some(format!("unexpected wpctl output {:?}", text.trim()).as_str()),
            "{text:?}"
        );
    }
}

#[test]
fn two_channels_give_a_balance_toward_the_louder_one() {
    let bus = Bus::new();
    let proxy = bus.proxy();
    for (text, balance) in [
        ("Volume: front-left: 32768 /  50% / -18.06 dB,   front-right: 32768 /  50% / -18.06 dB\n", 0.0),
        ("Volume: front-left: 1 / 100% / 0 dB, front-right: 1 / 50% / 0 dB", -0.5),
        ("Volume: front-left: 1 / 20% / 0 dB, front-right: 1 / 80% / 0 dB", 0.75),
        ("Volume: front-left: 1 / 120% / 0 dB, front-right: 1 / 120% / 0 dB", 0.0),
        ("Volume: front-left: 0 / 0% / -inf dB, front-right: 0 / 0% / -inf dB", 0.0),
        ("Volume: front-left: 1 / 50% / 0 dB, front-right: 1 / 50% / 0 dB\nBalance: 0.00\n", 0.0),
    ] {
        bus.prints(&[("get-volume", "Volume: 0.5"), ("get-sink-volume", text)]);
        let got = proxy.call::<_, _, f64>("GetBalance", &()).unwrap();
        assert!((got - balance).abs() < 1e-9, "{text:?}: {got}");
    }
    // The louder channel's level (80%) is kept: centring gives 80% on both sides.
    bus.prints(&[
        ("get-volume", "Volume: 0.5"),
        (
            "get-sink-volume",
            "Volume: front-left: 1 / 20% / 0 dB, front-right: 1 / 80% / 0 dB",
        ),
    ]);
    proxy.call::<_, _, ()>("SetBalance", &0.0f64).unwrap();
    let calls = fs::read_to_string(bus.path("calls")).unwrap();
    assert!(
        calls
            .lines()
            .any(|c| c == "set-sink-volume @DEFAULT_SINK@ 80% 80%"),
        "{calls}"
    );
}

#[test]
fn an_output_that_is_not_two_channels_has_no_balance() {
    let bus = Bus::new();
    let proxy = bus.proxy();
    for (text, expected) in [
        ("Volume: mono: 65536 / 100% / 0.00 dB", "NotSupported"),
        (
            "Volume: a: 1 / 10% / 0 dB, b: 1 / 20% / 0 dB, c: 1 / 30% / 0 dB",
            "NotSupported",
        ),
        ("", "Failed"),
        ("Volume: nothing", "Failed"),
    ] {
        bus.prints(&[("get-volume", "Volume: 0.5"), ("get-sink-volume", text)]);
        refused(proxy.call::<_, _, f64>("GetBalance", &()), expected);
    }
}

#[test]
fn a_backend_that_fails_is_reported_by_how_it_failed() {
    let bus = Bus::new();
    let proxy = bus.proxy();
    for (name, behaviour, message) in [
        (
            "exit",
            "*\tstderr\t  no default node  \\n\n*\texit\t3\n",
            "wpctl exited with 3: no default node",
        ),
        ("signal", "*\tabort\n", "wpctl was killed: "),
        (
            "timeout",
            "*\thang\n",
            "wpctl was killed: no answer within 1000 ms",
        ),
    ] {
        fs::write(bus.path("behaviour"), behaviour).unwrap();
        let started = std::time::Instant::now();
        let error = proxy
            .call::<_, _, (f64, bool)>("GetVolume", &"input")
            .unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(4), "{name}");
        let zbus::Error::MethodError(kind, text, _) = &error else {
            panic!("{name}: {error}")
        };
        assert_eq!(kind.as_str(), "org.freedesktop.DBus.Error.Failed", "{name}");
        assert_eq!(text.as_deref(), Some(message), "{name}");
    }
    // A program that cannot start.
    fs::remove_file(bus.path("wpctl")).unwrap();
    let error = proxy
        .call::<_, _, (f64, bool)>("GetVolume", &"input")
        .unwrap_err();
    assert!(
        matches!(&error, zbus::Error::MethodError(kind, Some(text), _)
            if kind.as_str() == "org.freedesktop.DBus.Error.Failed" && text.starts_with("cannot run wpctl")),
        "{error}"
    );
}
