//! Live settings against a headless compositor: changes to `compositor.json`, `keyboard.conf` and `power.conf` apply while
//! the server runs, a wrong key keeps its last good value, a setting changed another way survives an edit of other keys,
//! the session interface's setters store and apply the appearance and the accessibility settings, and the machine's
//! accessibility defaults apply to an account without a file of its own, live, and never from a file root does not own. Tests skip (pass)
//! when no EGL device is available; the bus test also when `dbus-daemon` is missing.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use cupertino_defaults::accessibility::{
    AccessibilitySettings, DisplayOption, SystemAccessibility,
};
use cupertino_defaults::atomic::write_atomic;
use cupertino_defaults::power::Timer;
use cupertino_server::backend::headless;
use cupertino_server::config::{Config, SettingsFiles};
use cupertino_server::live::SettingsPaths;
use cupertino_server::quit::Refusal;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    _dir: tempfile::TempDir,
}

impl Harness {
    /// A headless compositor watching the three files in a fresh directory, which starts empty.
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("create test event loop");
        let display: Display<Cupertino> = Display::new().expect("create test display");
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("create test compositor");
        if let Err(e) = headless::init(&mut state, (800, 600), 1.0) {
            common::egl_unavailable(format!("{e:#}"));
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SettingsPaths {
            compositor: Some(dir.path().join("compositor.json")),
            system: Some(dir.path().join("system.json")),
            keyboard: Some(dir.path().join("keyboard.conf")),
            power: Some(dir.path().join("power.conf")),
            system_accessibility: Some(dir.path().join("system-accessibility.conf")),
            data_control: Some(dir.path().join("data-control.conf")),
        };
        state.start_settings_watch(SettingsFiles::default(), paths);
        Some(Harness {
            state,
            event_loop,
            _dir: dir,
        })
    }

    fn path(&self, name: &str) -> std::path::PathBuf {
        self._dir.path().join(name)
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Write `text` to the watched file `name` and wait until the server applied it.
    fn write(&mut self, name: &str, text: &str) {
        let before = self.state.live.reloads;
        write_atomic(&self.path(name), text.as_bytes()).unwrap();
        self.wait_until(&format!("{name} to be reloaded"), move |s| {
            s.live.reloads > before
        });
    }

    /// Steps for twice the watcher's quiet time: what a change would have done by then.
    fn settle(&mut self) {
        let end = Instant::now() + cupertino_defaults::watch::COALESCE * 6;
        while Instant::now() < end {
            self.step();
        }
    }
}

#[test]
fn compositor_json_changes_apply_and_a_bad_key_keeps_its_last_good_value() {
    let Some(mut h) = Harness::new() else { return };
    assert!(!h.state.config.is_dark());
    h.write(
        "compositor.json",
        r#"{"appearance":"dark","mouse_speed":2.0,"minimize_effect":"scale","corner_radius":9}"#,
    );
    assert!(h.state.config.is_dark(), "the appearance applied");
    assert_eq!(h.state.config.pointer.mouse_speed, 2.0);
    assert_eq!(
        h.state.fx.cfg.minimize,
        Some(cupertino_effects::MinimizeKind::Scale)
    );
    assert_eq!(h.state.config.window.corner_radius, 9.0);

    // a value that does not parse keeps the last good one; the other keys still apply
    h.write(
        "compositor.json",
        r#"{"appearance":"dark","mouse_speed":"fast","minimize_effect":"scale","corner_radius":7}"#,
    );
    assert_eq!(
        h.state.config.pointer.mouse_speed, 2.0,
        "kept, not defaulted"
    );
    assert_eq!(h.state.config.window.corner_radius, 7.0);

    // a toggle at run time survives an edit of another key; a removed key goes back to its default
    h.state.toggle_appearance();
    assert!(!h.state.config.is_dark());
    h.write(
        "compositor.json",
        r#"{"appearance":"dark","mouse_speed":2.0,"minimize_effect":"scale"}"#,
    );
    assert!(
        !h.state.config.is_dark(),
        "the appearance key did not change, so the toggle stays"
    );
    assert_eq!(
        h.state.config.window.corner_radius,
        Config::default().window.corner_radius,
        "removed: the default"
    );
}

#[test]
fn keyboard_and_power_changes_apply() {
    let Some(mut h) = Harness::new() else { return };
    h.write(
        "keyboard.conf",
        "repeat_delay = 250\nrepeat_interval = 30\ninput_sources = us, de\n",
    );
    assert_eq!(
        (
            h.state.config.keyboard.repeat.initial_delay_ms,
            h.state.config.keyboard.repeat.interval_ms
        ),
        (250, 30)
    );
    assert_eq!(
        h.state.overlays.inputs.group_of("de"),
        Some(1),
        "the input sources were rebuilt"
    );
    h.write(
        "keyboard.conf",
        "repeat_delay = soon\nrepeat_interval = 30\ninput_sources = us, de\n",
    );
    assert_eq!(
        h.state.config.keyboard.repeat.initial_delay_ms, 250,
        "a bad line keeps the last good value"
    );

    h.write(
        "power.conf",
        "display_sleep_ac = 45\ndim_before_display_sleep = off\n",
    );
    let settings = *h.state.power.policy().settings();
    assert_eq!(
        settings.ac.display_sleep,
        Timer::After(Duration::from_secs(45))
    );
    assert!(!settings.dim_before_display_sleep);
}

#[test]
fn the_appearance_setter_stores_the_key_and_applies_it_once() {
    let Some(mut h) = Harness::new() else { return };
    let path = h.path("compositor.json");
    std::fs::write(
        &path,
        "{\"keybindings\":{\"Super+t\":\"spawn-terminal\"},\"future_key\":1}\n",
    )
    .unwrap();
    h.settle();
    let reloads = h.state.live.reloads;
    assert_eq!(h.state.set_session_appearance("dark"), Ok(()));
    assert!(h.state.config.is_dark());
    let written: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        written,
        serde_json::json!({"appearance": "dark", "future_key": 1, "keybindings": {"Super+t": "spawn-terminal"}}),
        "only the key was added"
    );
    h.wait_until("the server's own write to be seen", move |s| {
        s.live.reloads > reloads
    });
    assert!(
        h.state.config.is_dark(),
        "seeing its own write changes nothing"
    );
    assert!(matches!(
        h.state.set_session_appearance("auto"),
        Err(Refusal::NotSupported(_))
    ));
    assert!(matches!(
        h.state.set_session_appearance("sepia"),
        Err(Refusal::InvalidArgs(_))
    ));
    assert!(h.state.config.is_dark(), "refused calls change nothing");
}

#[test]
fn without_a_settings_directory_the_appearance_setter_refuses_and_applies_nothing() {
    let Some(mut h) = Harness::new() else { return };
    h.state.live.paths.compositor = None;
    assert!(matches!(
        h.state.set_session_appearance("dark"),
        Err(Refusal::Failed(_))
    ));
    assert!(
        !h.state.config.is_dark(),
        "store, then apply: nothing was stored, nothing applies"
    );
}

#[test]
fn the_accessibility_setter_takes_the_file_keys_and_values() {
    let Some(mut h) = Harness::new() else { return };
    assert_eq!(
        h.state.set_session_accessibility("reduce_motion", "on"),
        Ok(())
    );
    assert!(h.state.display_options().reduce_motion);
    assert_eq!(
        h.state
            .set_session_accessibility("color_filter", "tint 0.5"),
        Ok(())
    );
    assert!(h.state.accessibility.settings.color_filter.is_some());
    assert!(matches!(
        h.state.set_session_accessibility("display_contrast", "2"),
        Err(Refusal::InvalidArgs(_))
    ));
    assert!(matches!(
        h.state.set_session_accessibility("zoom", "on"),
        Err(Refusal::InvalidArgs(_))
    ));
    let props = h.state.session_settings();
    assert_eq!(
        props.accessibility.get("reduce_motion").map(String::as_str),
        Some("on")
    );
    assert_eq!(
        props.accessibility.get("color_filter").map(String::as_str),
        Some("tint 0.5")
    );
    assert_eq!(props.appearance, "light");
}

#[test]
fn the_system_file_applies_live_under_the_users_file_and_a_removed_user_key_falls_back_to_it() {
    let Some(mut h) = Harness::new() else { return };
    let radius = |h: &Harness| h.state.config.window.corner_radius;
    let default = Config::default().window.corner_radius;

    // the system file alone
    h.write("system.json", r#"{"corner_radius":14}"#);
    assert_eq!(radius(&h), 14.0, "a change of the system file applies");

    // the user's file over it
    h.write("compositor.json", r#"{"corner_radius":20}"#);
    assert_eq!(radius(&h), 20.0);
    h.write("system.json", r#"{"corner_radius":16}"#);
    assert_eq!(
        radius(&h),
        20.0,
        "the user's value stays over a changed system value"
    );

    // the key leaves the user's file: the system file's value, not the default
    h.write("compositor.json", r#"{}"#);
    assert_eq!(radius(&h), 16.0);

    // and leaves the system file too
    h.write("system.json", r#"{}"#);
    assert_eq!(radius(&h), default);

    // the layers combine key by key
    h.write("system.json", r#"{"corner_radius":12,"mouse_speed":1.5}"#);
    h.write("compositor.json", r#"{"mouse_speed":2.5}"#);
    assert_eq!(
        (radius(&h), h.state.config.pointer.mouse_speed),
        (12.0, 2.5)
    );
    h.write("compositor.json", r#"{"corner_radius":8}"#);
    assert_eq!((radius(&h), h.state.config.pointer.mouse_speed), (8.0, 1.5));
}

/// The machine's defaults with Reduce Motion `on` or off.
fn machine(on: bool) -> AccessibilitySettings {
    SystemAccessibility::from_text(&format!(
        "reduce_motion = {}",
        if on { "on" } else { "off" }
    ))
    .0
    .settings
}

#[test]
fn the_machines_accessibility_is_an_accounts_without_its_own_file_and_follows_it() {
    let Some(mut h) = Harness::new() else { return };
    let own = h.path("accessibility.conf");
    h.state.use_accessibility_store(own.clone()).unwrap();
    h.state.set_system_accessibility(machine(true));
    assert!(h.state.display_options().reduce_motion);
    h.state.set_system_accessibility(machine(false));
    assert!(
        !h.state.display_options().reduce_motion,
        "a change applies at once"
    );
    assert!(!own.exists(), "nothing is written for the account");
}

#[test]
fn an_account_with_its_own_file_keeps_its_settings() {
    let Some(mut h) = Harness::new() else { return };
    let own = h.path("accessibility.conf");
    std::fs::write(&own, "reduce_motion = off\n").unwrap();
    h.state.set_system_accessibility(machine(true));
    h.state.use_accessibility_store(own).unwrap();
    assert!(!h.state.display_options().reduce_motion);
    h.state.set_system_accessibility(machine(true));
    assert!(!h.state.display_options().reduce_motion);
}

#[test]
fn a_person_who_changed_one_key_keeps_following_the_machine_for_the_rest() {
    let Some(mut h) = Harness::new() else { return };
    let own = h.path("accessibility.conf");
    h.state.set_system_accessibility(machine(true));
    h.state.use_accessibility_store(own.clone()).unwrap();
    h.state
        .set_display_option(DisplayOption::ReduceTransparency, true);
    assert_eq!(
        std::fs::read_to_string(&own).unwrap(),
        "reduce_transparency = on\n",
        "only the changed key is written"
    );
    h.state.set_system_accessibility(machine(false));
    let options = h.state.display_options();
    assert!(
        options.reduce_transparency && !options.reduce_motion,
        "{options:?}"
    );
}

#[test]
fn a_key_the_person_set_back_to_the_machines_value_stays_theirs() {
    let Some(mut h) = Harness::new() else { return };
    let own = h.path("accessibility.conf");
    h.state.set_system_accessibility(machine(true));
    h.state.use_accessibility_store(own.clone()).unwrap();
    h.state
        .set_display_option(DisplayOption::ReduceMotion, false);
    h.state
        .set_display_option(DisplayOption::ReduceMotion, true);
    h.state.set_system_accessibility(machine(false));
    assert!(h.state.display_options().reduce_motion);
    assert_eq!(
        std::fs::read_to_string(&own).unwrap(),
        "reduce_motion = on\n"
    );
}

#[test]
fn a_file_with_every_key_keeps_them_all_over_the_machines() {
    let Some(mut h) = Harness::new() else { return };
    let own = h.path("accessibility.conf");
    std::fs::write(&own, AccessibilitySettings::default().to_text()).unwrap();
    h.state.set_system_accessibility(machine(true));
    h.state.use_accessibility_store(own).unwrap();
    assert!(!h.state.display_options().reduce_motion);
}

#[test]
fn a_machine_file_that_someone_other_than_root_could_have_written_is_ignored() {
    let Some(mut h) = Harness::new() else { return };
    h.state
        .use_accessibility_store(h.path("accessibility.conf"))
        .unwrap();
    // the test's own file, not root's: what an account could forge
    h.write("system-accessibility.conf", "reduce_motion = on\n");
    assert!(!h.state.display_options().reduce_motion);
}

#[test]
fn the_setters_are_served_on_the_session_bus() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    let (unavailable, first_read) = std::sync::mpsc::channel();
    let address = bus.address.clone();
    let client = std::thread::spawn(move || {
        let conn = zbus::blocking::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .unwrap();
        // Startup probes must reach the bus: a failed initial GetAll is cached permanently.
        let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'_>>::new(&conn)
            .destination("org.cupertino.Session1")
            .unwrap()
            .path("/org/cupertino/Session1")
            .unwrap()
            .interface("org.cupertino.Session1")
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .unwrap();
        // Make the startup race deterministic: the first read precedes name ownership.
        let error = proxy.get_property::<String>("Appearance").unwrap_err();
        assert!(
            matches!(&error, zbus::Error::FDO(error)
                if matches!(error.as_ref(), zbus::fdo::Error::ServiceUnknown(_))),
            "{error}"
        );
        unavailable.send(()).unwrap();
        // the name is taken once the service thread is up
        let end = Instant::now() + Duration::from_secs(10);
        while proxy.get_property::<String>("Appearance").is_err() {
            assert!(Instant::now() < end, "the session interface did not appear");
            std::thread::sleep(Duration::from_millis(20));
        }
        let before: String = proxy.get_property("Appearance").unwrap();
        proxy.call_method("SetAppearance", &("dark",)).unwrap();
        let auto = proxy
            .call_method("SetAppearance", &("auto",))
            .unwrap_err()
            .to_string();
        proxy
            .call_method("SetAccessibility", &("reduce_transparency", "on"))
            .unwrap();
        let bad = proxy
            .call_method("SetAccessibility", &("zoom", "on"))
            .unwrap_err()
            .to_string();
        let end = Instant::now() + Duration::from_secs(10);
        let (mut appearance, mut accessibility) = (
            String::new(),
            std::collections::HashMap::<String, String>::new(),
        );
        while Instant::now() < end
            && (appearance != "dark"
                || accessibility.get("reduce_transparency").map(String::as_str) != Some("on"))
        {
            appearance = proxy.get_property("Appearance").unwrap();
            accessibility = proxy.get_property("Accessibility").unwrap();
            std::thread::sleep(Duration::from_millis(20));
        }
        (before, auto, bad, appearance, accessibility)
    });
    first_read.recv_timeout(Duration::from_secs(10)).unwrap();
    cupertino_server::quit::start_session_service_on(&mut h.state, Some(bus.address.clone()));
    while !client.is_finished() {
        h.step();
    }
    let (before, auto, bad, appearance, accessibility) = client.join().unwrap();
    assert_eq!(before, "light");
    assert!(auto.contains("NotSupported"), "{auto}");
    assert!(bad.contains("InvalidArgs"), "{bad}");
    assert_eq!(appearance, "dark", "the property follows the setter");
    assert_eq!(
        accessibility.get("reduce_transparency").map(String::as_str),
        Some("on")
    );
    assert!(h.state.display_options().reduce_transparency);
    assert!(
        Path::new(&h.path("compositor.json")).exists(),
        "the appearance was stored"
    );
}

/// What Settings' Screen reader switch calls, over a private bus: `SetAccessibility("screen_reader", ...)` starts the
/// session's reader and stops it, and the `Accessibility` property says which.
#[test]
fn the_screen_reader_switch_starts_and_stops_the_reader_over_the_session_bus() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    h.state
        .use_accessibility_store(h.path("accessibility.conf"))
        .unwrap();
    let reader = h.path("fake-reader");
    std::fs::write(&reader, "#!/bin/sh\nexec /usr/bin/sleep 300\n").unwrap();
    std::fs::set_permissions(&reader, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    h.state.session_reader.program = Some(reader);
    let address = bus.address.clone();
    let (said, heard) = std::sync::mpsc::channel::<(String, String)>();
    let (done, next) = std::sync::mpsc::channel::<()>();
    let client = std::thread::spawn(move || {
        let conn = zbus::blocking::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .unwrap();
        let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy<'_>>::new(&conn)
            .destination("org.cupertino.Session1")
            .unwrap()
            .path("/org/cupertino/Session1")
            .unwrap()
            .interface("org.cupertino.Session1")
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .unwrap();
        let end = Instant::now() + Duration::from_secs(10);
        while proxy
            .get_property::<std::collections::HashMap<String, String>>("Accessibility")
            .is_err()
        {
            assert!(Instant::now() < end, "the session interface did not appear");
            std::thread::sleep(Duration::from_millis(20));
        }
        for value in ["on", "off"] {
            proxy
                .call_method("SetAccessibility", &("screen_reader", value))
                .unwrap();
            let held: std::collections::HashMap<String, String> =
                proxy.get_property("Accessibility").unwrap();
            said.send((value.to_owned(), held["screen_reader"].clone()))
                .unwrap();
            next.recv().unwrap();
        }
    });
    cupertino_server::quit::start_session_service_on(&mut h.state, Some(bus.address.clone()));
    let mut seen = Vec::new();
    let mut pid = None;
    while seen.len() < 2 {
        h.step();
        if let Ok((value, held)) = heard.try_recv() {
            let running = h.state.session_reader.pid();
            if value == "on" {
                pid = running;
            }
            seen.push((value, held, running.is_some()));
            done.send(()).unwrap();
        }
    }
    client.join().unwrap();
    assert_eq!(
        seen,
        [
            ("on".to_owned(), "on".to_owned(), true),
            ("off".to_owned(), "off".to_owned(), false)
        ]
    );
    let pid = pid.unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while Path::new(&format!("/proc/{pid}")).exists() && Instant::now() < end {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !Path::new(&format!("/proc/{pid}")).exists(),
        "the reader ended"
    );
}

/// SEC-04: a process of the same user that is neither the session agent nor the settings application cannot change the
/// session, whatever it calls; the refusal has its own error name, and the properties stay readable.
#[test]
fn the_session_interface_refuses_a_host_process_that_is_not_the_agent() {
    if let Err(error) = std::process::Command::new("dbus-send")
        .arg("--version")
        .output()
    {
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::NotFound,
            "run dbus-send: {error}"
        );
        cupertino_test_bus::missing_tool("dbus-send");
        return;
    }
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    cupertino_server::quit::start_session_service_on(&mut h.state, Some(bus.address.clone()));
    let address = bus.address.clone();
    let client = std::thread::spawn(move || {
        let call = |member: &str, arguments: &[&str]| {
            std::process::Command::new("dbus-send")
                .arg(format!("--bus={address}"))
                .args(["--print-reply", "--dest=org.cupertino.Session1"])
                .arg("/org/cupertino/Session1")
                .arg(format!("org.cupertino.Session1.{member}"))
                .args(arguments)
                .output()
                .unwrap()
        };
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            let probe = call("QuitNextApplication", &[]);
            let stderr = String::from_utf8_lossy(&probe.stderr).to_string();
            if !stderr.contains("ServiceUnknown") && !stderr.contains("NameHasNoOwner") {
                break;
            }
            assert!(Instant::now() < end, "the session interface did not appear");
            std::thread::sleep(Duration::from_millis(20));
        }
        [
            call("QuitNextApplication", &[]),
            call("ForceQuitApplication", &["string:org.example.App"]),
            call("SetAppearance", &["string:dark"]),
            call("SetAccessibility", &["string:reduce_motion", "string:on"]),
        ]
        .map(|out| {
            (
                out.status.success(),
                String::from_utf8_lossy(&out.stderr).to_string(),
            )
        })
    });
    while !client.is_finished() {
        h.step();
    }
    for (succeeded, stderr) in client.join().unwrap() {
        assert!(
            !succeeded,
            "a host process that is no agent changed the session"
        );
        assert!(
            stderr.contains("org.cupertino.Session1.Error.UntrustedCaller"),
            "{stderr}"
        );
    }
    assert_eq!(
        h.state.session_settings().appearance,
        "light",
        "the appearance was not set"
    );
}
