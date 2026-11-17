//! The seat keymap when an input source's generated keymap is involved and a keymap does not compile: switching to a
//! generated keymap that does not compile keeps the system layouts, and switching from a generated keymap to system
//! layouts that do not compile keeps the generated one. Each failure is logged.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use cupertino_assets::Assets;
use cupertino_keyboard::keycodes::evdev_of;
use cupertino_keyboard::switcher::Step;
use cupertino_server::kbd::{InputSources, SourceSpec};
use cupertino_server::{Config, Cupertino};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

/// The evdev code of the key between T and U on a US keyboard (Y there, Z on a German one).
const KEY_Y: u32 = 21;
const KEYSYM_Y: u32 = 0x79;
const KEYSYM_Z: u32 = 0x7a;
const KEYSYM_E: u32 = 0x65;
const KEYSYM_X: u32 = 0x78;

/// A layout name no keyboard description holds.
const MISSING_LAYOUT: &str = "cupertino-no-such-layout";

/// A layout whose keys 0 and 3 type `e` and `x`, shifted `E` and `X`.
const LAYOUT: &str = r#"{
    "name": "Test",
    "id": 3,
    "language": "xx",
    "sets": [{
        "first": 0,
        "last": 0,
        "modmap": [0, 0, 1],
        "default_table": 0,
        "tables": [
            [{"c": "e"}, {"c": "a"}, {"c": "b"}, {"c": "x"}],
            [{"c": "E"}, {"c": "A"}, {"c": "B"}, {"c": "X"}]
        ],
        "records": [],
        "terminators": []
    }]
}"#;

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

/// The server's environment: a home of its own, and no default layout from the environment. Returns the home.
fn private_environment(name: &str) -> PathBuf {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join(name);
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
    home
}

fn server() -> Cupertino {
    let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
    let display: Display<Cupertino> = Display::new().unwrap();
    Cupertino::new(
        display,
        event_loop.handle(),
        event_loop.get_signal(),
        Config::default(),
        false,
        &Assets::builtin(),
    )
    .unwrap()
}

/// The decoded source of the test layout, written into `home`.
fn decoded_source(home: &Path) -> SourceSpec {
    let path = home.join("layout.json");
    std::fs::write(&path, LAYOUT).unwrap();
    SourceSpec::parse(&format!("decoded:{}", path.display())).expect("a decoded source")
}

/// Takes `sources` as the input sources from `keyboard.conf` and applies them to the seat.
fn use_sources(state: &mut Cupertino, sources: &[SourceSpec]) {
    state.config.keyboard.sources = sources.to_vec();
    state.overlays.inputs = InputSources::new(sources.to_vec());
    state.apply_keyboard_settings();
}

/// Presses the reverse input-source shortcut, which steps to the least recently used source, until `id` is the current
/// source.
fn switch_to(state: &mut Cupertino, id: &str) {
    for _ in 0..state.overlays.inputs.specs.len() {
        if state.overlays.inputs.current() == Some(id) {
            return;
        }
        state.input_source_step(false, Step::Previous);
    }
    assert_eq!(state.overlays.inputs.current(), Some(id));
}

/// Applies the system layouts US and German with the test layout as a third source, then breaks the test layout's
/// generated keymap with `break_keymap` and switches to it: the seat keeps the system layouts. Returns the log.
fn switch_to_a_broken_generated_keymap(name: &str, break_keymap: impl Fn(&mut String)) -> String {
    let home = private_environment(name);
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut state = server();

    let decoded = decoded_source(&home);
    let sources = [
        SourceSpec::parse("us").unwrap(),
        SourceSpec::parse("de").unwrap(),
        decoded.clone(),
    ];
    use_sources(&mut state, &sources);
    assert!(
        log.text().contains("decoded layout loaded"),
        "{}",
        log.text()
    );
    assert_eq!(state.xkb_first_keysym(KEY_Y, 0), Some(KEYSYM_Y));
    assert_eq!(state.xkb_first_keysym(KEY_Y, 1), Some(KEYSYM_Z));

    let generated = state
        .overlays
        .inputs
        .decoded
        .get_mut(&decoded.id)
        .expect("the generated keymap of the decoded source");
    break_keymap(&mut generated.keymap);
    switch_to(&mut state, &decoded.id);

    assert!(
        !state.overlays.inputs.decoded_active,
        "the seat keymap is not the generated one"
    );
    assert_eq!(
        state.xkb_first_keysym(KEY_Y, 0),
        Some(KEYSYM_Y),
        "the US group of the system keymap stays"
    );
    assert_eq!(
        state.xkb_first_keysym(KEY_Y, 1),
        Some(KEYSYM_Z),
        "the German group of the system keymap stays"
    );
    log.text()
}

#[test]
fn a_generated_keymap_that_does_not_compile_keeps_the_last_working_keymap() {
    let text = switch_to_a_broken_generated_keymap("keymap-decoded-broken", |keymap| {
        // a keymap cut off half way: its sections never close
        keymap.truncate(keymap.len() / 2);
    });
    assert!(
        text.contains("the generated keymap does not compile; the last working keymap stays"),
        "the failure is logged: {text}"
    );
}

#[test]
fn a_generated_keymap_holding_a_nul_keeps_the_last_working_keymap() {
    let text = switch_to_a_broken_generated_keymap("keymap-decoded-nul", |keymap| {
        keymap.insert(keymap.len() / 2, '\0');
    });
    assert!(
        text.contains("the generated keymap holds a NUL; the last working keymap stays"),
        "the failure is logged: {text}"
    );
}

#[test]
fn system_layouts_that_do_not_compile_keep_the_generated_keymap() {
    let home = private_environment("keymap-decoded-kept");
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_writer(move || writer.clone())
        .with_ansi(false)
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let mut state = server();

    let decoded = decoded_source(&home);
    let missing = SourceSpec::parse(MISSING_LAYOUT).unwrap();
    use_sources(&mut state, &[missing.clone(), decoded.clone()]);
    let (key_e, key_x) = (evdev_of(0).unwrap(), evdev_of(3).unwrap());

    switch_to(&mut state, &decoded.id);
    assert!(state.overlays.inputs.decoded_active, "{}", log.text());
    assert_eq!(state.xkb_first_keysym(key_e, 0), Some(KEYSYM_E));
    assert_eq!(state.xkb_first_keysym(key_x, 0), Some(KEYSYM_X));

    let before = log.text().len();
    switch_to(&mut state, &missing.id);
    assert!(
        state.overlays.inputs.decoded_active,
        "the seat keymap is still the generated one"
    );
    assert_eq!(
        state.xkb_first_keysym(key_e, 0),
        Some(KEYSYM_E),
        "the generated keymap stays"
    );
    assert_eq!(state.xkb_first_keysym(key_x, 0), Some(KEYSYM_X));
    let text = log.text();
    let since = &text[before..];
    assert!(
        since
            .contains("the configured input sources do not compile; the last working keymap stays")
            && since.contains(MISSING_LAYOUT),
        "the failure is logged with the layout: {since}"
    );
}
