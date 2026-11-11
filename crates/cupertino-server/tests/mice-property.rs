//! The `Mice` property of `org.cupertino.Session1` against a headless compositor and a private session bus: the
//! connected mice (not the touchpads) with the resolution in use and whether it was stored, and the change signal when
//! `mouse_dpi` changes in `compositor.json`. Skipped when no EGL device or no `dbus-daemon` is available.

mod common;

use std::time::{Duration, Instant};

use cupertino_defaults::atomic::write_atomic;
use cupertino_server::backend::headless;
use cupertino_server::config::{Config, SettingsFiles};
use cupertino_server::events::{MouseIdentity, PointerDevice};
use cupertino_server::live::SettingsPaths;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

type Mice = Vec<(String, String, u32, bool)>;

fn mouse(key: &str, name: &str, hwdb_dpi: Option<&str>, cfg: &Config) -> PointerDevice {
    PointerDevice::with_identity(
        false,
        &cfg.pointer,
        MouseIdentity {
            key: Some(key.into()),
            name: name.into(),
            hwdb_dpi: hwdb_dpi.map(str::to_owned),
        },
    )
}

fn used(mut device: PointerDevice) -> PointerDevice {
    device.note_use();
    device
}

#[test]
fn the_mice_property_lists_the_mice_with_their_resolution_and_follows_the_config() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: no dbus-daemon");
        return;
    };
    let mut event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let mut config = Config::default();
    config.pointer.mouse_dpi = [("1234:5678:Mouse B".to_string(), 3200)].into();
    let Ok(mut state) = Cupertino::new(
        Display::new().unwrap(),
        event_loop.handle(),
        event_loop.get_signal(),
        config.clone(),
        true,
        &cupertino_assets::Assets::builtin(),
    ) else {
        return;
    };
    if let Err(e) = headless::init(&mut state, (800, 600), 1.0) {
        common::egl_unavailable(format!("{e:#}"));
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let compositor = dir.path().join("compositor.json");
    state.start_settings_watch(
        SettingsFiles::default(),
        SettingsPaths {
            compositor: Some(compositor.clone()),
            system: None,
            keyboard: None,
            power: None,
            system_accessibility: None,
            data_control: None,
            desktop_control: None,
        },
    );
    // two mice, one with a stored resolution; a keyboard's pointer interface that never moved and a touchpad are not
    // listed
    state.pointer_devices.insert(
        "event3".into(),
        used(mouse(
            "046d:c077:Mouse A",
            "Mouse A",
            Some("400 *800"),
            &config,
        )),
    );
    state.pointer_devices.insert(
        "event4".into(),
        used(mouse("1234:5678:Mouse B", "Mouse B", None, &config)),
    );
    state.pointer_devices.insert(
        "event7".into(),
        mouse("1a2c:0001:Keyboard", "Keyboard", None, &config),
    );
    state.pointer_devices.insert(
        "event5".into(),
        PointerDevice::with_identity(
            true,
            &config.pointer,
            MouseIdentity {
                key: Some("0000:0000:Pad".into()),
                name: "Pad".into(),
                hwdb_dpi: None,
            },
        ),
    );
    cupertino_server::quit::start_session_service_on(&mut state, Some(bus.address.clone()));

    let address = bus.address.clone();
    let client = std::thread::spawn(move || {
        let conn = zbus::blocking::connection::Builder::address(address.as_str())
            .unwrap()
            .build()
            .unwrap();
        // no property cache: one that failed to fill before the service was up would keep failing
        let proxy = zbus::blocking::proxy::Builder::<zbus::blocking::Proxy>::new(&conn)
            .destination("org.cupertino.Session1")
            .unwrap()
            .path("/org/cupertino/Session1")
            .unwrap()
            .interface("org.cupertino.Session1")
            .unwrap()
            .cache_properties(zbus::proxy::CacheProperties::No)
            .build()
            .unwrap();
        let end = Instant::now() + common::HANG_GUARD;
        loop {
            let Err(e) = proxy.get_property::<Mice>("Mice") else {
                break;
            };
            assert!(
                Instant::now() < end,
                "the session interface did not appear: {e}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let first: Mice = proxy.get_property("Mice").unwrap();
        let read_only = proxy
            .set_property("Mice", Vec::<(String, String, u32, bool)>::new())
            .is_err();
        // the settings app stores a resolution for A and drops B's
        write_atomic(
            &compositor,
            br#"{"mouse_dpi": {"046d:c077:Mouse A": 1600}}"#,
        )
        .unwrap();
        let mut changed = first.clone();
        let end = Instant::now() + common::HANG_GUARD;
        while changed == first && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(20));
            changed = proxy.get_property("Mice").unwrap();
        }
        (first, read_only, changed)
    });
    while !client.is_finished() {
        headless::step(&mut state, &mut event_loop, Duration::from_millis(5)).unwrap();
    }
    let (first, read_only, changed) = client.join().unwrap();
    assert_eq!(
        first,
        vec![
            ("046d:c077:Mouse A".into(), "Mouse A".into(), 800, false),
            ("1234:5678:Mouse B".into(), "Mouse B".into(), 3200, true),
        ]
    );
    assert!(read_only, "the property cannot be set");
    assert_eq!(
        changed,
        vec![
            ("046d:c077:Mouse A".into(), "Mouse A".into(), 1600, true),
            ("1234:5678:Mouse B".into(), "Mouse B".into(), 1000, false),
        ]
    );
}
