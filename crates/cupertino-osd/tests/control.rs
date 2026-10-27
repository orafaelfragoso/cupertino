//! The control interface against a private session bus (`dbus-daemon --session`) with in-memory backends; skipped when
//! the daemon is not available.
#![cfg(feature = "dbus")]

use cupertino_osd::control::{serve, BUS_NAME, OBJECT_PATH};
use cupertino_osd::{Command, Controller, Kind, MemoryBacklight, MemoryVolume};
use std::process::{Command as Process, Stdio};
use std::sync::mpsc;
use std::time::Duration;
use zbus::{block_on, connection, Connection};

const INTERFACE: &str = "org.cupertino.Osd1";

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Process::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        let text = String::from_utf8(out.stdout)
            .ok()
            .filter(|_| out.status.success())?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next()?.to_string(),
            pid: lines.next()?.to_string(),
        })
    }

    fn builder(&self) -> connection::Builder<'static> {
        connection::Builder::address(self.address.as_str()).expect("address")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        Process::new("kill")
            .arg(&self.pid)
            .status()
            .expect("kill the private bus");
    }
}

/// What the overlay shows after each command, as `(kind, level)`.
type Shown = Option<(Kind, f64)>;

/// Serve the interface over fake backends (volume 0.5, display 0.5, keyboard 0.5). Every command that arrives is
/// applied at its sequence number as the time, and what the overlay shows then is reported.
fn serve_fakes(bus: &PrivateBus) -> (Connection, mpsc::Receiver<Shown>) {
    let (commands_tx, commands) = mpsc::channel::<Command>();
    let (shown_tx, shown) = mpsc::channel();
    let commands_tx = std::sync::Mutex::new(commands_tx);
    let connection = serve(bus.builder(), move |command| {
        commands_tx.lock().unwrap().send(command).unwrap()
    })
    .expect("claims the name");
    std::thread::spawn(move || {
        let mut controller = Controller::new(
            Box::new(MemoryVolume::new(0.5, false)),
            Box::new(MemoryBacklight::new(0.5)),
            Box::new(MemoryBacklight::new(0.5)),
        );
        for (now, command) in commands.into_iter().enumerate() {
            controller
                .apply(now as f64 * 0.1, command)
                .expect("the fake backends do not fail");
            shown_tx
                .send(
                    controller
                        .view(now as f64 * 0.1)
                        .map(|view| (view.overlay.kind(), view.overlay.level())),
                )
                .unwrap();
        }
    });
    (connection, shown)
}

/// Call `method` of the interface with `body`.
fn call(
    caller: &Connection,
    method: &str,
    body: &(impl serde::Serialize + zbus::zvariant::DynamicType),
) -> zbus::Result<()> {
    block_on(caller.call_method(Some(BUS_NAME), OBJECT_PATH, Some(INTERFACE), method, body))
        .map(|_| ())
}

fn next(shown: &mpsc::Receiver<Shown>) -> Shown {
    shown
        .recv_timeout(Duration::from_secs(5))
        .expect("the command was applied")
}

#[test]
fn volume_step_changes_the_fake_level_by_one_step_and_shows_the_volume_overlay() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (_osd, shown) = serve_fakes(&bus);
    let caller: Connection = block_on(bus.builder().build()).expect("connect");

    call(&caller, "VolumeStep", &(1, false)).expect("VolumeStep");
    assert_eq!(next(&shown), Some((Kind::Volume, 0.5625)));
    // the level is kept between calls: a second press adds another step, a fine press a sixty-fourth
    call(&caller, "VolumeStep", &(1, false)).expect("VolumeStep");
    assert_eq!(next(&shown), Some((Kind::Volume, 0.625)));
    call(&caller, "VolumeStep", &(-1, true)).expect("VolumeStep");
    assert_eq!(next(&shown), Some((Kind::Volume, 0.609375)));
}

#[test]
fn the_other_methods_reach_their_effects() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (_osd, shown) = serve_fakes(&bus);
    let caller: Connection = block_on(bus.builder().build()).expect("connect");

    call(&caller, "ToggleMute", &()).expect("ToggleMute");
    assert_eq!(next(&shown), Some((Kind::Mute, 0.0)));
    call(&caller, "BrightnessStep", &(-1, false)).expect("BrightnessStep");
    assert_eq!(next(&shown), Some((Kind::Brightness, 0.4375)));
    call(&caller, "KeyboardBrightnessStep", &(1, false)).expect("KeyboardBrightnessStep");
    assert_eq!(next(&shown), Some((Kind::KeyboardBrightness, 0.5625)));
    call(&caller, "Show", &("brightness", 0.25)).expect("Show");
    assert_eq!(next(&shown), Some((Kind::Brightness, 0.25)));
}

#[test]
fn invalid_arguments_are_rejected_before_any_effect() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (_osd, shown) = serve_fakes(&bus);
    let caller: Connection = block_on(bus.builder().build()).expect("connect");

    assert!(matches!(
        call(&caller, "VolumeStep", &(0, false)),
        Err(zbus::Error::MethodError(..))
    ));
    assert!(matches!(
        call(&caller, "Show", &("volume", 1.5)),
        Err(zbus::Error::MethodError(..))
    ));
    assert!(matches!(
        call(&caller, "Show", &("eject", 0.5)),
        Err(zbus::Error::MethodError(..))
    ));
    assert_eq!(shown.try_recv(), Err(mpsc::TryRecvError::Empty));
}

#[test]
fn a_second_overlay_cannot_take_the_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (_osd, _shown) = serve_fakes(&bus);
    let second = serve(bus.builder(), |_| {});
    assert!(
        matches!(second, Err(zbus::Error::NameTaken)),
        "{:?}",
        second.err()
    );
}

/// SEC-04: a process of the same user that is neither the server nor this process cannot change a hardware level; each
/// refusal has its own error name. `Show` only draws and stays open.
#[test]
fn a_host_process_that_is_not_the_server_cannot_step_a_level() {
    if Process::new("dbus-send").arg("--version").output().is_err() {
        eprintln!("skipping: dbus-send is not available");
        return;
    }
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let (commands_tx, commands_rx) = mpsc::channel();
    let _osd = serve(bus.builder(), move |command| {
        commands_tx.send(command).unwrap()
    })
    .expect("claims the name");
    let send = |member: &str, arguments: &[&str]| {
        Process::new("dbus-send")
            .arg(format!("--bus={}", bus.address))
            .args(["--print-reply", "--dest=org.cupertino.Osd1", OBJECT_PATH])
            .arg(format!("{INTERFACE}.{member}"))
            .args(arguments)
            .output()
            .expect("dbus-send")
    };

    for (member, arguments) in [
        ("VolumeStep", vec!["int32:1", "boolean:false"]),
        ("ToggleMute", vec![]),
        ("BrightnessStep", vec!["int32:-1", "boolean:false"]),
        ("KeyboardBrightnessStep", vec!["int32:1", "boolean:true"]),
    ] {
        let out = send(member, &arguments);
        let stderr = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{member} was accepted");
        assert!(
            stderr.contains("org.cupertino.Error.UntrustedCaller"),
            "{member}: {stderr}"
        );
    }
    assert!(
        commands_rx.try_recv().is_err(),
        "a refused call reached the controller"
    );

    let shown = send("Show", &["string:volume", "double:0.5"]);
    assert!(shown.status.success(), "Show stays open");
    assert!(matches!(
        commands_rx.recv_timeout(Duration::from_secs(5)),
        Ok(Command::Show(_))
    ));
}
