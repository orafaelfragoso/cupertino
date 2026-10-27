//! The session bus worker against a private bus (`dbus-daemon --session`): the notification centre and
//! launcher toggles, the overlay's media keys, the lock request and the power key's press and release reach their processes. Skipped when `dbus-daemon` is not installed.
#![cfg(feature = "dbus")]

use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cupertino_server::dbus::{DbusHandle, Request, Toggle};
use cupertino_server::input::OsdKey;
use zbus::{block_on, connection};

struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !out.status.success() {
            return None;
        }
        let text = String::from_utf8(out.stdout).ok()?;
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next()?.to_string(),
            pid: lines.next()?.to_string(),
        })
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = Command::new("kill").arg(&self.pid).status();
    }
}

/// The worker finds the bus through the process environment: the tests that start one take turns.
static SESSION_BUS_ENV: Mutex<()> = Mutex::new(());

/// The shell processes' side of the interfaces: count the toggles.
struct FakeCentre(Arc<Mutex<u32>>);

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeCentre {
    fn toggle_centre(&self) {
        *self.0.lock().unwrap() += 1;
    }
}

struct FakeLauncher(Arc<Mutex<u32>>);

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl FakeLauncher {
    fn toggle(&self) {
        *self.0.lock().unwrap() += 1;
    }
}

/// The login process's side: count the lock requests and record the power key states.
struct FakeLogin(Arc<Mutex<u32>>, Arc<Mutex<Vec<bool>>>);

#[zbus::interface(name = "org.cupertino.Login1")]
impl FakeLogin {
    fn lock(&self) {
        *self.0.lock().unwrap() += 1;
    }

    fn power_key(&self, pressed: bool) {
        self.1.lock().unwrap().push(pressed);
    }
}

/// The overlay process's side: record the step calls with their arguments.
struct FakeOsd(Arc<Mutex<Vec<(String, i32, bool)>>>);

#[zbus::interface(name = "org.cupertino.Osd1")]
impl FakeOsd {
    fn volume_step(&self, direction: i32, fine: bool) {
        self.0
            .lock()
            .unwrap()
            .push(("VolumeStep".into(), direction, fine));
    }

    fn toggle_mute(&self) {
        self.0.lock().unwrap().push(("ToggleMute".into(), 0, false));
    }
}

#[test]
fn each_toggle_calls_its_own_process() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let _env = SESSION_BUS_ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
    block_on(async {
        let (centre_toggles, launcher_toggles) = (Arc::new(Mutex::new(0)), Arc::new(Mutex::new(0)));
        let _centre = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.cupertino.NotificationCenter1")
            .expect("name")
            .serve_at(
                "/org/cupertino/NotificationCenter1",
                FakeCentre(centre_toggles.clone()),
            )
            .expect("object")
            .build()
            .await
            .expect("connect");
        let _launcher = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.cupertino.Launcher1")
            .expect("name")
            .serve_at(
                "/org/cupertino/Launcher1",
                FakeLauncher(launcher_toggles.clone()),
            )
            .expect("object")
            .build()
            .await
            .expect("connect");
        let handle = DbusHandle::start().expect("worker thread");
        handle.send(Request::Toggle(Toggle::Launcher));
        for _ in 0..200 {
            if *launcher_toggles.lock().unwrap() > 0 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        assert_eq!(
            (
                *centre_toggles.lock().unwrap(),
                *launcher_toggles.lock().unwrap()
            ),
            (0, 1)
        );
        handle.send(Request::Toggle(Toggle::NotificationCentre));
        for _ in 0..200 {
            if *centre_toggles.lock().unwrap() > 0 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        assert_eq!(
            (
                *centre_toggles.lock().unwrap(),
                *launcher_toggles.lock().unwrap()
            ),
            (1, 1)
        );

        let osd_calls = Arc::new(Mutex::new(Vec::new()));
        let _osd = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.cupertino.Osd1")
            .expect("name")
            .serve_at("/org/cupertino/Osd1", FakeOsd(osd_calls.clone()))
            .expect("object")
            .build()
            .await
            .expect("connect");
        handle.send(Request::Osd {
            key: OsdKey::VolumeDown,
            fine: true,
        });
        handle.send(Request::Osd {
            key: OsdKey::Mute,
            fine: false,
        });
        for _ in 0..200 {
            if osd_calls.lock().unwrap().len() == 2 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        let mut calls = osd_calls.lock().unwrap().clone();
        calls.sort();
        assert_eq!(
            calls,
            vec![
                ("ToggleMute".to_string(), 0, false),
                ("VolumeStep".to_string(), -1, true)
            ]
        );

        let lock_calls = Arc::new(Mutex::new(0));
        let power_keys = Arc::new(Mutex::new(Vec::new()));
        let _login = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.cupertino.Login1")
            .expect("name")
            .serve_at(
                "/org/cupertino/Login1",
                FakeLogin(lock_calls.clone(), power_keys.clone()),
            )
            .expect("object")
            .build()
            .await
            .expect("connect");
        handle.send(Request::Lock);
        for _ in 0..200 {
            if *lock_calls.lock().unwrap() > 0 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        assert_eq!(*lock_calls.lock().unwrap(), 1);

        handle.send(Request::PowerKey { pressed: true });
        handle.send(Request::PowerKey { pressed: false });
        for _ in 0..200 {
            if power_keys.lock().unwrap().len() >= 2 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        assert_eq!(
            (
                *lock_calls.lock().unwrap(),
                power_keys.lock().unwrap().clone()
            ),
            (1, vec![true, false])
        );
    });
}

/// The login process's side for the log out request: counts the calls.
struct FakeLoginPanel(Arc<Mutex<u32>>);

#[zbus::interface(name = "org.cupertino.Login1")]
impl FakeLoginPanel {
    fn request_log_out(&self) {
        *self.0.lock().unwrap() += 1;
    }
}

#[test]
fn a_log_out_request_without_a_login_process_runs_the_fallback_and_with_one_does_not() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let _env = SESSION_BUS_ENV.lock().unwrap_or_else(|e| e.into_inner());
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
    block_on(async {
        let fallbacks = Arc::new(Mutex::new(0u32));
        let counted = fallbacks.clone();
        let handle =
            DbusHandle::start_with(move || *counted.lock().unwrap() += 1).expect("worker thread");
        assert!(handle.send(Request::LogOut));
        for _ in 0..200 {
            if *fallbacks.lock().unwrap() > 0 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        assert_eq!(
            *fallbacks.lock().unwrap(),
            1,
            "nobody owns org.cupertino.Login1"
        );

        // other requests to a missing process are only logged
        assert!(handle.send(Request::Toggle(Toggle::Launcher)));
        async_io::Timer::after(Duration::from_millis(300)).await;
        assert_eq!(*fallbacks.lock().unwrap(), 1);

        let asked = Arc::new(Mutex::new(0u32));
        let _login = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.cupertino.Login1")
            .expect("name")
            .serve_at("/org/cupertino/Login1", FakeLoginPanel(asked.clone()))
            .expect("object")
            .build()
            .await
            .expect("connect");
        assert!(handle.send(Request::LogOut));
        for _ in 0..200 {
            if *asked.lock().unwrap() > 0 {
                break;
            }
            async_io::Timer::after(Duration::from_millis(20)).await;
        }
        assert_eq!(
            *asked.lock().unwrap(),
            1,
            "the login process got the request"
        );
        assert_eq!(
            *fallbacks.lock().unwrap(),
            1,
            "and the fallback did not run again"
        );
    });
}
