//! The seat keymap when a keyboard layout does not compile: the last keymap that compiled stays and the failure is
//! logged.

use std::io::Write;
use std::sync::{Arc, Mutex};

use cupertino_assets::Assets;
use cupertino_server::localed::SystemLayout;
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

/// The evdev code of the key between T and U on a US keyboard (Y there, Z on a German one).
const KEY_Y: u32 = 21;
const KEYSYM_Y: u32 = 0x79;
const KEYSYM_Z: u32 = 0x7a;

/// A layout name no keyboard description holds.
const MISSING_LAYOUT: &str = "cupertino-no-such-layout";

/// What the server logs, kept for the test to read.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Log {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
    }
}

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The server's environment: a home of its own, and no default layout from the environment.
fn private_environment(name: &str) {
    let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(home.join("runtime")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
    std::env::set_var("XDG_RUNTIME_DIR", home.join("runtime"));
    for name in [
        "XKB_DEFAULT_RULES",
        "XKB_DEFAULT_MODEL",
        "XKB_DEFAULT_LAYOUT",
        "XKB_DEFAULT_VARIANT",
        "XKB_DEFAULT_OPTIONS",
    ] {
        std::env::remove_var(name);
    }
}

fn layout(layout: &str, variant: &str) -> SystemLayout {
    SystemLayout {
        layout: layout.to_string(),
        variant: variant.to_string(),
        options: String::new(),
    }
}

#[test]
fn a_layout_that_does_not_compile_keeps_the_last_working_keymap() {
    private_environment("keymap-kept");
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    let mut state = Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        false,
        &Assets::builtin(),
    )
    .unwrap();

    state.apply_system_layout(layout("us,de", ","));
    assert_eq!(state.xkb_first_keysym(KEY_Y, 0), Some(KEYSYM_Y));
    assert_eq!(state.xkb_first_keysym(KEY_Y, 1), Some(KEYSYM_Z));
    assert!(!log.text().contains("do not compile"), "{}", log.text());

    state.apply_system_layout(layout(MISSING_LAYOUT, ""));
    assert_eq!(
        state.xkb_first_keysym(KEY_Y, 0),
        Some(KEYSYM_Y),
        "the US group of the last keymap stays"
    );
    assert_eq!(
        state.xkb_first_keysym(KEY_Y, 1),
        Some(KEYSYM_Z),
        "the German group of the last keymap stays"
    );
    let text = log.text();
    assert!(
        text.contains("the configured input sources do not compile; the last working keymap stays")
            && text.contains(MISSING_LAYOUT),
        "the failure is logged with the layout: {text}"
    );
}
