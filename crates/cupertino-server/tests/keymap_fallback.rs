//! The seat keymap at start when the environment's default layout does not compile: the seat starts with the
//! fallback US keymap and the failure is logged.

use std::io::Write;
use std::sync::{Arc, Mutex};

use cupertino_assets::Assets;
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

/// The evdev code of the key between T and U on a US keyboard.
const KEY_Y: u32 = 21;
const KEYSYM_Y: u32 = 0x79;

/// What the server logs, kept for the test to read.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);

impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_default_layout_that_does_not_compile_starts_the_fallback_keymap() {
    let home = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("keymap-fallback");
    std::fs::create_dir_all(home.join("config")).unwrap();
    std::fs::create_dir_all(home.join("runtime")).unwrap();
    std::env::set_var("HOME", &home);
    std::env::set_var("XDG_CONFIG_HOME", home.join("config"));
    std::env::set_var("XDG_RUNTIME_DIR", home.join("runtime"));
    std::env::set_var("XKB_DEFAULT_LAYOUT", "cupertino-no-such-layout");
    std::env::set_var("XKB_DEFAULT_OPTIONS", "cupertino-no-such-option");
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
    .expect("the server starts");

    assert_eq!(state.xkb_first_keysym(KEY_Y, 0), Some(KEYSYM_Y));
    let text = String::from_utf8_lossy(&log.0.lock().unwrap()).into_owned();
    assert!(
        text.contains("the default keyboard layout does not compile"),
        "the failure is logged: {text}"
    );
}
