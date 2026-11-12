//! The session bus worker against a private bus (`dbus-daemon --session`): the notification centre and
//! launcher toggles (its search and its application browser), the overlay's media keys, the lock request and the power key's press and release reach their processes. Skipped when `dbus-daemon` is not installed.
#![cfg(feature = "dbus")]

use crate::common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_server::dbus::{DbusHandle, LoginTrust, Request, Toggle};
use cupertino_server::input::OsdKey;
use cupertino_test_bus::PrivateBus;
use zbus::{block_on, connection};

/// Wait until `cond` holds, checking it as the bus work goes on; panics after [`common::HANG_GUARD`].
async fn until(cond: impl Fn() -> bool) {
    let end = Instant::now() + common::HANG_GUARD;
    while !cond() {
        assert!(Instant::now() < end, "timed out");
        async_io::Timer::after(Duration::from_millis(5)).await;
    }
}

/// Wait until the worker is done with every call queued so far: made (and answered), refused, or failed, with what
/// it does about that (the fallback, the notice) done too.
async fn worker_done(handle: &DbusHandle) {
    let (queued, _) = handle.shell_calls();
    until(|| handle.shell_calls().1 >= queued).await;
}

/// The shell processes' side of the interfaces: count the toggles.
struct FakeCentre(Arc<Mutex<u32>>);

#[zbus::interface(name = "org.cupertino.NotificationCenter1")]
impl FakeCentre {
    fn toggle_centre(&self) {
        *self.0.lock().unwrap() += 1;
    }
}

/// Counts the search toggles and the application browser's apart.
struct FakeLauncher(Arc<Mutex<u32>>, Arc<Mutex<u32>>);

#[zbus::interface(name = "org.cupertino.Launcher1")]
impl FakeLauncher {
    fn toggle(&self) {
        *self.0.lock().unwrap() += 1;
    }

    fn toggle_applications(&self) {
        *self.1.lock().unwrap() += 1;
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
    block_on(async {
        let (centre_toggles, launcher_toggles) = (Arc::new(Mutex::new(0)), Arc::new(Mutex::new(0)));
        let browser_toggles = Arc::new(Mutex::new(0));
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
                FakeLauncher(launcher_toggles.clone(), browser_toggles.clone()),
            )
            .expect("object")
            .build()
            .await
            .expect("connect");
        let handle = DbusHandle::start_on(Some(bus.address.clone()), || {}).expect("worker thread");
        handle.set_login(LoginTrust::Agent(std::process::id()));
        handle.send(Request::Toggle(Toggle::Launcher));
        until(|| *launcher_toggles.lock().unwrap() > 0).await;
        assert_eq!(
            (
                *centre_toggles.lock().unwrap(),
                *launcher_toggles.lock().unwrap()
            ),
            (0, 1)
        );
        handle.send(Request::Toggle(Toggle::NotificationCentre));
        until(|| *centre_toggles.lock().unwrap() > 0).await;
        assert_eq!(
            (
                *centre_toggles.lock().unwrap(),
                *launcher_toggles.lock().unwrap()
            ),
            (1, 1)
        );
        // the app-grid action: the launcher's application browser
        handle.send(Request::Toggle(Toggle::Applications));
        until(|| *browser_toggles.lock().unwrap() > 0).await;
        assert_eq!(
            (
                *launcher_toggles.lock().unwrap(),
                *browser_toggles.lock().unwrap()
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
        until(|| osd_calls.lock().unwrap().len() == 2).await;
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
        until(|| *lock_calls.lock().unwrap() > 0).await;
        assert_eq!(*lock_calls.lock().unwrap(), 1);

        handle.send(Request::PowerKey { pressed: true });
        handle.send(Request::PowerKey { pressed: false });
        until(|| power_keys.lock().unwrap().len() >= 2).await;
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
    block_on(async {
        let fallbacks = Arc::new(Mutex::new(0u32));
        let counted = fallbacks.clone();
        let handle = DbusHandle::start_on(Some(bus.address.clone()), move || {
            *counted.lock().unwrap() += 1
        })
        .expect("worker thread");
        // a supervised login process that has not taken its name yet is not a session without one
        handle.set_login(LoginTrust::Agent(std::process::id()));
        assert!(handle.send(Request::LogOut));
        worker_done(&handle).await;
        assert_eq!(
            *fallbacks.lock().unwrap(),
            0,
            "a supervised login process may still be starting"
        );

        handle.set_login(LoginTrust::Nobody);
        assert!(handle.send(Request::LogOut));
        until(|| *fallbacks.lock().unwrap() > 0).await;
        assert_eq!(
            *fallbacks.lock().unwrap(),
            1,
            "nobody owns org.cupertino.Login1 and no login process is supervised"
        );
        handle.set_login(LoginTrust::Agent(std::process::id()));

        // other requests to a missing process are only logged
        assert!(handle.send(Request::Toggle(Toggle::Launcher)));
        worker_done(&handle).await;
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
        until(|| *asked.lock().unwrap() > 0).await;
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

/// Send a log out request while `trust` is set and report whether the login process got it and whether the fallback ran.
fn log_out_with(bus: &PrivateBus, trust: LoginTrust) -> (u32, u32) {
    block_on(async {
        let (asked, fallbacks) = (Arc::new(Mutex::new(0u32)), Arc::new(Mutex::new(0u32)));
        let _login = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.cupertino.Login1")
            .expect("name")
            .serve_at("/org/cupertino/Login1", FakeLoginPanel(asked.clone()))
            .expect("object")
            .build()
            .await
            .expect("connect");
        let counted = fallbacks.clone();
        let handle = DbusHandle::start_on(Some(bus.address.clone()), move || {
            *counted.lock().unwrap() += 1
        })
        .expect("worker thread");
        handle.set_login(trust);
        assert!(handle.send(Request::LogOut));
        // the login process's answer and the fallback both come before the worker is done with the call
        worker_done(&handle).await;
        let counts = (*asked.lock().unwrap(), *fallbacks.lock().unwrap());
        counts
    })
}

#[test]
fn only_the_supervised_login_process_owns_the_login_name() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep");
    let other_pid = other.id();
    let results = (
        log_out_with(&bus, LoginTrust::Agent(std::process::id())),
        log_out_with(&bus, LoginTrust::Any),
        log_out_with(&bus, LoginTrust::Agent(other_pid)),
        log_out_with(&bus, LoginTrust::Nobody),
    );
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(
        results,
        ((1, 0), (1, 0), (0, 0), (0, 0)),
        "delivered to the supervised process or in a dev shell; an owner that is not the login process gets nothing and does not end the session"
    );
}

/// The notification service's side: record the banners.
struct FakeNotifications(Arc<Mutex<Vec<(String, String)>>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl FakeNotifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        self.0.lock().unwrap().push((summary, body));
        1
    }
}

/// Send a log out request while `trust` is set, with the login service up or not, and report the banners shown.
fn log_out_banners(bus: &PrivateBus, trust: LoginTrust, login_up: bool) -> Vec<(String, String)> {
    block_on(async {
        let shown = Arc::new(Mutex::new(Vec::new()));
        let _notifications = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.freedesktop.Notifications")
            .expect("name")
            .serve_at(
                "/org/freedesktop/Notifications",
                FakeNotifications(shown.clone()),
            )
            .expect("object")
            .build()
            .await
            .expect("connect");
        let _login = if login_up {
            Some(
                connection::Builder::address(bus.address.as_str())
                    .expect("address")
                    .name("org.cupertino.Login1")
                    .expect("name")
                    .serve_at(
                        "/org/cupertino/Login1",
                        FakeLoginPanel(Arc::new(Mutex::new(0))),
                    )
                    .expect("object")
                    .build()
                    .await
                    .expect("connect"),
            )
        } else {
            None
        };
        let handle = DbusHandle::start_on(Some(bus.address.clone()), || ()).expect("worker thread");
        handle.set_login(trust);
        assert!(handle.send(Request::LogOut));
        // a notice the worker posts is on the bus before it is done with the call; the service has it once it answers a
        // call made after that
        worker_done(&handle).await;
        let probe = connection::Builder::address(bus.address.as_str())
            .expect("address")
            .build()
            .await
            .expect("connect");
        probe
            .call_method(
                Some("org.freedesktop.Notifications"),
                "/org/freedesktop/Notifications",
                Some("org.freedesktop.DBus.Peer"),
                "Ping",
                &(),
            )
            .await
            .expect("the notification service answers");
        let banners = shown.lock().unwrap().clone();
        banners
    })
}

#[test]
fn a_log_out_that_reaches_nobody_shows_a_banner_and_one_that_is_delivered_or_ends_the_server_does_not(
) {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let mut other = std::process::Command::new("sleep")
        .arg("30")
        .spawn()
        .expect("sleep");
    let other_pid = other.id();
    let banner = |dropped: cupertino_server::dbus::LogOutDropped| {
        let (summary, body) = dropped.text();
        vec![(summary.to_string(), body.to_string())]
    };
    let squatted = log_out_banners(&bus, LoginTrust::Agent(other_pid), true);
    let not_ready = log_out_banners(&bus, LoginTrust::Agent(std::process::id()), false);
    let delivered = log_out_banners(&bus, LoginTrust::Agent(std::process::id()), true);
    let fallback = log_out_banners(&bus, LoginTrust::Nobody, false);
    let _ = other.kill();
    let _ = other.wait();
    assert_eq!(
        squatted,
        banner(cupertino_server::dbus::LogOutDropped::OwnerNotTheLoginProcess)
    );
    assert_eq!(
        not_ready,
        banner(cupertino_server::dbus::LogOutDropped::LoginProcessNotReady)
    );
    assert!(delivered.is_empty(), "{delivered:?}");
    assert!(fallback.is_empty(), "{fallback:?}");
}

#[test]
fn the_notice_does_not_wait_for_the_notification_service() {
    struct Silent;
    #[zbus::interface(name = "org.freedesktop.Notifications")]
    impl Silent {
        #[allow(clippy::too_many_arguments)]
        async fn notify(
            &self,
            _app_name: String,
            _replaces_id: u32,
            _app_icon: String,
            _summary: String,
            _body: String,
            _actions: Vec<String>,
            _hints: std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
            _expire_timeout: i32,
        ) -> u32 {
            std::future::pending().await
        }
    }
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    zbus::block_on(async {
        let _service = zbus::connection::Builder::address(bus.address.as_str())
            .expect("address")
            .name("org.freedesktop.Notifications")
            .expect("name")
            .serve_at("/org/freedesktop/Notifications", Silent)
            .expect("object")
            .build()
            .await
            .expect("connect");
        let conn = zbus::connection::Builder::address(bus.address.as_str())
            .expect("address")
            .build()
            .await
            .expect("connect");
        let (done, posted) = async_channel::bounded(1);
        let task_conn = conn.clone();
        conn.executor()
            .spawn(
                async move {
                    cupertino_server::dbus::post_notice(
                        &task_conn,
                        cupertino_server::dbus::LogOutDropped::LoginProcessNotReady,
                    )
                    .await;
                    let _ = done.send(()).await;
                },
                "notice",
            )
            .detach();
        let mut waited = 0;
        while posted.try_recv().is_err() {
            assert!(
                waited < 100,
                "posting the notice waited for a service that never answers"
            );
            async_io::Timer::after(std::time::Duration::from_millis(20)).await;
            waited += 1;
        }
    });
}
