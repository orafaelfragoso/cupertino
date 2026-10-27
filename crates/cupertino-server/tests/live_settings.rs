//! Live settings against a headless compositor: changes to `compositor.json`, `keyboard.conf` and `power.conf` apply while
//! the server runs, a wrong key keeps its last good value, a setting changed another way survives an edit of other keys,
//! and the session interface's setters store and apply the appearance and the accessibility settings. Tests skip (pass)
//! when no EGL device is available; the bus test also when `dbus-daemon` is missing.

use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use cupertino_defaults::atomic::write_atomic;
use cupertino_defaults::power::Timer;
use cupertino_server::backend::headless;
use cupertino_server::config::{Config, SettingsFiles};
use cupertino_server::live::SettingsPaths;
use cupertino_server::quit::Refusal;
use cupertino_server::Cupertino;
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
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .ok()?;
        if let Err(e) = headless::init(&mut state, (800, 600), 1.0) {
            eprintln!("skipping: no EGL device ({e:#})");
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let paths = SettingsPaths {
            compositor: Some(dir.path().join("compositor.json")),
            keyboard: Some(dir.path().join("keyboard.conf")),
            power: Some(dir.path().join("power.conf")),
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
        let end = Instant::now() + Duration::from_secs(10);
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

/// A private session bus (`dbus-daemon --session`), killed on drop.
struct PrivateBus {
    address: String,
    pid: String,
}

impl PrivateBus {
    /// `None` when `dbus-daemon` is not installed.
    fn start() -> Option<PrivateBus> {
        let out = Command::new("dbus-daemon")
            .args(["--session", "--print-address=1", "--print-pid=1", "--fork"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        assert!(out.status.success(), "dbus-daemon failed: {}", out.status);
        let text = String::from_utf8(out.stdout).unwrap();
        let mut lines = text.lines().map(str::trim).filter(|l| !l.is_empty());
        Some(PrivateBus {
            address: lines.next().unwrap().to_string(),
            pid: lines.next().unwrap().to_string(),
        })
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

#[test]
fn the_setters_are_served_on_the_session_bus() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let Some(mut h) = Harness::new() else { return };
    // the only test of this binary that reads the variable
    std::env::set_var("DBUS_SESSION_BUS_ADDRESS", &bus.address);
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
    cupertino_server::quit::start_session_service(&mut h.state);
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
