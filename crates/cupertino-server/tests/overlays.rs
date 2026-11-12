//! Input-source HUD, caps-lock delay, key repeat advertised to clients and the accent picker's
//! text-input bridge, in-process on an offscreen EGL device (tests skip when there is none).

use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_defaults::keyboard::ComposeKey;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::input::Mods;
use cupertino_server::kbd::SourceSpec;
use cupertino_server::localed::SystemLayout;
use cupertino_server::Cupertino;
use cupertino_test_bus::PrivateBus;
use smithay::backend::input::{ButtonState, KeyState};
use smithay::input::keyboard::keysyms;
use smithay::input::keyboard::{Keycode, ModifiersState};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_keyboard, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

const OUTPUT: (i32, i32) = (1000, 700);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn with_config(tweak: impl FnOnce(&mut Config)) -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut cfg = Config::default();
        tweak(&mut cfg);
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            cfg,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .expect("creating the compositor");
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn new() -> Option<Harness> {
        Self::with_config(|_| {})
    }

    fn step(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(5),
        )
        .unwrap();
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&mut Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.step();
            if cond(&mut self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Fixed model time and one loop iteration.
    fn at(&mut self, t: f64) {
        let time = t;
        common::set_time(&mut self.state, &mut self.event_loop, time);
        self.step();
    }
}

// ---- a client window (optionally with a text input) ------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
enum TextEvent {
    Preedit(Option<String>),
    Commit(Option<String>),
    DeleteSurrounding(u32, u32),
    Done,
}

#[derive(Default)]
struct Shared {
    mapped: bool,
    repeat_info: Option<(i32, i32)>,
    text_events: Vec<TextEvent>,
    entered: bool,
    /// `wl_keyboard.key` events as (evdev code, pressed).
    keys: Vec<(u32, bool)>,
    /// `wl_keyboard` key and modifiers events in the order they arrived.
    key_log: Vec<KeyLog>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum KeyLog {
    /// A key event: evdev code, pressed.
    Key(u32, bool),
    /// A modifiers event: the depressed and the locked mask.
    Mods(u32, u32),
}

struct Client {
    shared: Arc<Mutex<Shared>>,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    cursor: (i32, i32, i32, i32),
    text_input: Option<zwp_text_input_v3::ZwpTextInputV3>,
    buffer: Option<wl_buffer::WlBuffer>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Client {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_compositor::WlCompositor,
    wl_shm::WlShm,
    wl_shm_pool::WlShmPool,
    wl_buffer::WlBuffer,
    wl_surface::WlSurface,
    wl_seat::WlSeat,
    zwp_text_input_manager_v3::ZwpTextInputManagerV3
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Client {
    fn event(
        _: &mut Self,
        base: &xdg_wm_base::XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, ()> for Client {
    fn event(
        _: &mut Self,
        _: &xdg_toplevel::XdgToplevel,
        _: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for Client {
    fn event(
        state: &mut Self,
        surface: &xdg_surface::XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            if state.buffer.is_none() {
                let (w, h) = (300, 200);
                let file =
                    rustix::fs::memfd_create("overlay-test", rustix::fs::MemfdFlags::CLOEXEC)
                        .unwrap();
                let mut file = std::fs::File::from(file);
                file.set_len((w * h * 4) as u64).unwrap();
                let row = vec![0xd0u8; (w * 4) as usize];
                for _ in 0..h {
                    file.write_all(&row).unwrap();
                }
                let pool = state.shm.create_pool(file.as_fd(), w * h * 4, qh, ());
                let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Argb8888, qh, ());
                pool.destroy();
                state.surface.attach(Some(&buffer), 0, 0);
                state.surface.damage_buffer(0, 0, w, h);
                state.surface.commit();
                state.buffer = Some(buffer);
                state.shared.lock().unwrap().mapped = true;
            }
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Client {
    fn event(
        state: &mut Self,
        _: &wl_keyboard::WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_keyboard::Event::RepeatInfo { rate, delay } => {
                state.shared.lock().unwrap().repeat_info = Some((rate, delay))
            }
            wl_keyboard::Event::Key {
                key,
                state: key_state,
                ..
            } => {
                let pressed = matches!(
                    key_state,
                    wayland_client::WEnum::Value(wl_keyboard::KeyState::Pressed)
                );
                let mut shared = state.shared.lock().unwrap();
                shared.keys.push((key, pressed));
                shared.key_log.push(KeyLog::Key(key, pressed));
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_locked,
                ..
            } => state
                .shared
                .lock()
                .unwrap()
                .key_log
                .push(KeyLog::Mods(mods_depressed, mods_locked)),
            _ => {}
        }
    }
}

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for Client {
    fn event(
        state: &mut Self,
        ti: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut shared = state.shared.lock().unwrap();
        match event {
            zwp_text_input_v3::Event::Enter { .. } => {
                shared.entered = true;
                ti.enable();
                let (x, y, w, h) = state.cursor;
                ti.set_cursor_rectangle(x, y, w, h);
                ti.commit();
            }
            zwp_text_input_v3::Event::Leave { .. } => shared.entered = false,
            zwp_text_input_v3::Event::PreeditString { text, .. } => {
                shared.text_events.push(TextEvent::Preedit(text))
            }
            zwp_text_input_v3::Event::CommitString { text } => {
                shared.text_events.push(TextEvent::Commit(text))
            }
            zwp_text_input_v3::Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => shared
                .text_events
                .push(TextEvent::DeleteSurrounding(before_length, after_length)),
            zwp_text_input_v3::Event::Done { .. } => shared.text_events.push(TextEvent::Done),
            _ => {}
        }
    }
}

struct TestClient {
    stop: Arc<AtomicBool>,
    shared: Arc<Mutex<Shared>>,
    thread: Option<JoinHandle<()>>,
}

impl TestClient {
    fn spawn(socket: PathBuf, with_text_input: bool, with_keyboard: bool) -> TestClient {
        Self::spawn_as(socket, "overlay.test", with_text_input, with_keyboard)
    }

    fn spawn_as(
        socket: PathBuf,
        app_id: &'static str,
        with_text_input: bool,
        with_keyboard: bool,
    ) -> TestClient {
        let stop = Arc::new(AtomicBool::new(false));
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (s2, sh2) = (stop.clone(), shared.clone());
        let thread = std::thread::spawn(move || {
            let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<Client>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let _keyboard = with_keyboard.then(|| seat.get_keyboard(&qh, ()));
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            let text_input = with_text_input.then(|| {
                let manager: zwp_text_input_manager_v3::ZwpTextInputManagerV3 =
                    globals.bind(&qh, 1..=1, ()).unwrap();
                manager.get_text_input(&seat, &qh, ())
            });
            surface.commit();
            let mut client = Client {
                shared: sh2,
                shm,
                surface,
                cursor: (40, 30, 2, 18),
                text_input,
                buffer: None,
            };
            let _ = client.text_input.as_ref();
            while !s2.load(Ordering::SeqCst) {
                if queue.roundtrip(&mut client).is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        TestClient {
            stop,
            shared,
            thread: Some(thread),
        }
    }
}

impl Drop for TestClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take();
    }
}

fn mapped_window(h: &mut Harness, text_input: bool, keyboard: bool) -> TestClient {
    let client = TestClient::spawn(h.socket_path(), text_input, keyboard);
    let shared = client.shared.clone();
    h.wait_until("the client window", move |st| {
        shared.lock().unwrap().mapped && st.wm.key_window().is_some()
    });
    client
}

// ---- scripted keys ---------------------------------------------------------------------------------

#[test]
fn scripted_keys_reach_the_focused_client_as_real_key_events() {
    let Some(mut h) = Harness::new() else { return };
    let client = mapped_window(&mut h, false, true);
    let shared = client.shared.clone();
    h.wait_until("the keyboard focus", move |st| {
        st.seat.get_keyboard().unwrap().current_focus().is_some() && shared.lock().unwrap().mapped
    });
    h.state.scripted_type("Ab");
    h.state.scripted_key("Escape").unwrap();
    let shared = client.shared.clone();
    h.wait_until("the keys", move |_| shared.lock().unwrap().keys.len() >= 8);
    // evdev: Shift 42, a 30, b 48, Escape 1; a capital letter is typed with Shift held around its key
    let expected = [
        (42, true),
        (30, true),
        (30, false),
        (42, false),
        (48, true),
        (48, false),
        (1, true),
        (1, false),
    ];
    assert_eq!(client.shared.lock().unwrap().keys, expected);
}

// ---- keyboard -------------------------------------------------------------------------------------

#[test]
fn clients_get_the_verified_repeat_info() {
    let Some(mut h) = Harness::new() else { return };
    let client = TestClient::spawn(h.socket_path(), false, true);
    let shared = client.shared.clone();
    h.wait_until("repeat_info", move |_| {
        shared.lock().unwrap().repeat_info.is_some()
    });
    // 500 ms delay, 83 ms interval = 12 characters per second
    assert_eq!(client.shared.lock().unwrap().repeat_info, Some((12, 500)));
}

#[test]
fn repeat_info_follows_the_configuration() {
    let Some(mut h) = Harness::with_config(|c| {
        c.keyboard.repeat = cupertino_keyboard::RepeatConfig {
            initial_delay_ms: 300,
            interval_ms: 40,
        };
    }) else {
        return;
    };
    let client = TestClient::spawn(h.socket_path(), false, true);
    let shared = client.shared.clone();
    h.wait_until("repeat_info", move |_| {
        shared.lock().unwrap().repeat_info.is_some()
    });
    assert_eq!(client.shared.lock().unwrap().repeat_info, Some((25, 300)));
}

fn with_sources(c: &mut Config) {
    c.keyboard.sources = ["us", "de", "fr"]
        .iter()
        .map(|s| SourceSpec::parse(s).unwrap())
        .collect();
}

#[test]
fn input_source_hud_switches_the_xkb_group_when_the_modifiers_are_released() {
    let Some(mut h) = Harness::with_config(with_sources) else {
        return;
    };
    assert_eq!(h.state.active_xkb_group(), 0);
    let kb = h.state.seat.get_keyboard().unwrap();
    // Ctrl held while Space is pressed (the chord of the design assignment table)
    kb.set_modifier_state(ModifiersState {
        ctrl: true,
        ..Default::default()
    });
    h.at(5.0);
    h.state.scripted_key("Ctrl+space").unwrap();
    assert!(
        h.state.overlays.inputs.hud_open(),
        "first press opens the HUD"
    );
    assert_eq!(h.state.overlays.inputs.hud.as_ref().unwrap().selection, 1);
    assert_eq!(
        h.state.active_xkb_group(),
        0,
        "nothing is committed while Ctrl is held"
    );
    h.state.scripted_key("Ctrl+space").unwrap();
    assert_eq!(h.state.overlays.inputs.hud.as_ref().unwrap().selection, 2);
    let labels = h.state.overlays.inputs.hud.as_ref().unwrap().labels.clone();
    assert_eq!(labels.len(), 3);
    assert!(
        labels[0].contains("English"),
        "labels come from the keymap: {labels:?}"
    );
    // Ctrl released: commit
    kb.set_modifier_state(ModifiersState::default());
    h.state.input_sources_modifiers(Mods::NONE);
    assert_eq!(h.state.active_xkb_group(), 2);
    assert!(!h.state.overlays.inputs.hud_open());
    // the HUD fades out over 0.3 s and is then gone
    assert!(h.state.overlays.inputs.hud.is_some());
    h.at(5.1);
    assert!(h.state.overlays.inputs.hud.is_some());
    h.at(5.4);
    assert!(h.state.overlays.inputs.hud.is_none());
}

struct FakeLocaled {
    layout: &'static str,
    variant: &'static str,
    options: &'static str,
}

#[zbus::interface(name = "org.freedesktop.locale1")]
impl FakeLocaled {
    #[zbus(property, name = "X11Layout")]
    fn x11_layout(&self) -> String {
        self.layout.into()
    }

    #[zbus(property, name = "X11Variant")]
    fn x11_variant(&self) -> String {
        self.variant.into()
    }

    #[zbus(property, name = "X11Options")]
    fn x11_options(&self) -> String {
        self.options.into()
    }
}

#[test]
fn the_system_layouts_become_the_input_sources_with_their_options_and_the_set_compose_key() {
    let Some(bus) = PrivateBus::start() else {
        eprintln!("skipping: dbus-daemon is not available");
        return;
    };
    let localed = FakeLocaled {
        layout: "us,br",
        variant: ",abnt2",
        options: "grp:alt_shift_toggle,compose:menu,grp_led:scroll",
    };
    let _localed = zbus::blocking::connection::Builder::address(bus.address.as_str())
        .unwrap()
        .name("org.freedesktop.locale1")
        .unwrap()
        .serve_at("/org/freedesktop/locale1", localed)
        .unwrap()
        .build()
        .unwrap();
    let system = SystemLayout::read(Some(&bus.address)).unwrap();
    let Some(mut h) = Harness::with_config(|c| c.keyboard.compose = Some(ComposeKey::RightAlt))
    else {
        return;
    };
    h.state.apply_system_layout(system);
    let sources: Vec<&str> = h
        .state
        .config
        .keyboard
        .sources
        .iter()
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(sources, ["us", "br(abnt2)"]);
    assert_eq!(
        h.state.config.keyboard.xkb_options(),
        "grp_led:scroll,lv3:lalt_switch,compose:ralt"
    );
    const KEY_RIGHTALT: u32 = 100;
    const KEY_COMPOSE: u32 = 127;
    assert_eq!(
        h.state.xkb_first_keysym(KEY_RIGHTALT, 0),
        Some(keysyms::KEY_Multi_key)
    );
    assert_ne!(
        h.state.xkb_first_keysym(KEY_COMPOSE, 0),
        Some(keysyms::KEY_Multi_key),
        "the set compose key replaces the system's"
    );
    let kb = h.state.seat.get_keyboard().unwrap();
    kb.set_modifier_state(ModifiersState {
        ctrl: true,
        ..Default::default()
    });
    h.at(5.0);
    h.state.scripted_key("Ctrl+space").unwrap();
    kb.set_modifier_state(ModifiersState::default());
    h.state.input_sources_modifiers(Mods::NONE);
    assert_eq!(
        h.state.active_xkb_group(),
        1,
        "Ctrl+Space switches to br(abnt2)"
    );
}

#[test]
fn escape_closes_the_hud_without_switching_and_most_recent_source_comes_first() {
    let Some(mut h) = Harness::with_config(with_sources) else {
        return;
    };
    h.state.overlays.hold_modifiers = true;
    h.at(5.0);
    h.state.scripted_key("Ctrl+space").unwrap();
    assert!(h.state.overlays.inputs.hud_open());
    h.state.scripted_key("Escape").unwrap();
    assert!(!h.state.overlays.inputs.hud_open());
    assert_eq!(h.state.active_xkb_group(), 0);
    // switch to "de", then the history puts "de" first and "us" is the previous source
    h.state.scripted_key("Ctrl+space").unwrap();
    h.state.scripted_key("Return").unwrap();
    assert_eq!(h.state.active_xkb_group(), 1);
    h.at(6.0);
    h.state.scripted_key("Ctrl+space").unwrap();
    let hud = h.state.overlays.inputs.hud.as_ref().unwrap();
    assert_eq!(hud.order[0], "de");
    assert_eq!(
        hud.order[hud.selection], "us",
        "Ctrl+Space steps to the previously used source"
    );
    // the menu-order shortcut walks the menu instead
    h.state.scripted_key("Return").unwrap();
    assert_eq!(h.state.active_xkb_group(), 0);
    h.at(7.0);
    h.state.scripted_key("Ctrl+Alt+space").unwrap();
    let hud = h.state.overlays.inputs.hud.as_ref().unwrap();
    assert_eq!(
        hud.order[hud.selection], "de",
        "the source after \"us\" in menu order"
    );
}

#[test]
fn caps_lock_press_counts_after_75_ms() {
    let Some(mut h) = Harness::new() else { return };
    let kb = h.state.seat.get_keyboard().unwrap();
    let caps = Keycode::new(66);
    // brushed for 40 ms: both events vanish, the lock does not toggle
    assert!(h.state.caps_gate(caps, KeyState::Pressed, 1000));
    assert!(h.state.caps_gate(caps, KeyState::Released, 1040));
    for _ in 0..40 {
        h.step();
    }
    assert!(!kb.modifier_state().caps_lock);
    // held: the press is delivered once 75 ms passed
    assert!(h.state.caps_gate(caps, KeyState::Pressed, 2000));
    assert!(!kb.modifier_state().caps_lock);
    h.wait_until("the delayed caps-lock press", |st| {
        st.seat.get_keyboard().unwrap().modifier_state().caps_lock
    });
    assert!(
        !h.state.caps_gate(caps, KeyState::Released, 2200),
        "the release of a delivered press passes"
    );
    h.state.keyboard_event_now(caps, KeyState::Released, 2200);
    assert!(
        kb.modifier_state().caps_lock,
        "the lock stays on after the release"
    );
}

// ---- accent picker through the text-input bridge ----------------------------------------------------

fn accent_table() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/accents.json")
}

#[test]
fn accent_picker_commits_through_the_text_input_of_the_focused_client() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.accent.table = Some(accent_table()))
    else {
        return;
    };
    assert!(
        h.state.connect_input_method(),
        "the in-process input method connects"
    );
    let client = mapped_window(&mut h, true, false);
    let shared = client.shared.clone();
    h.wait_until("the text input to become active", move |st| {
        shared.lock().unwrap().entered && st.overlays.accent.bridge
    });
    h.wait_until("the cursor rectangle", |st| st.text_cursor.is_some());
    // the popup is anchored at the application's text cursor (surface-local (40, 30) of its window)
    let anchor = h.state.text_cursor_global().expect("cursor");
    let frame = h
        .state
        .wm
        .window(h.state.wm.key_window().unwrap())
        .unwrap()
        .geometry();
    assert!(
        anchor.x() >= frame.x + 40.0 && anchor.x() < frame.x + 60.0,
        "anchor {anchor:?} in {frame:?}"
    );

    assert!(h.state.accent_open('e'));
    let popup = h.state.overlays.accent.popup.clone().unwrap();
    assert_eq!(popup.candidates[..3], ["è", "é", "ê"]);
    // a screenshot shows the popup row above the cursor
    h.state.fixed_time = None;
    h.step();
    // number key 2 picks the second alternative: the typed base character is replaced
    h.state.scripted_key("2").unwrap();
    let shared = client.shared.clone();
    h.wait_until("the commit", move |_| {
        shared
            .lock()
            .unwrap()
            .text_events
            .iter()
            .any(|e| matches!(e, TextEvent::Commit(Some(t)) if t == "é"))
    });
    let ev = client.shared.lock().unwrap().text_events.clone();
    assert!(
        ev.contains(&TextEvent::DeleteSurrounding(1, 0)),
        "the base character is deleted first: {ev:?}"
    );
    assert!(!h.state.overlays.accent.is_open());
}

#[test]
fn accent_preview_uses_marked_text_and_escape_restores_the_base() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.accent.table = Some(accent_table()))
    else {
        return;
    };
    assert!(h.state.connect_input_method());
    let client = mapped_window(&mut h, true, false);
    let shared = client.shared.clone();
    h.wait_until("the text input to become active", move |st| {
        shared.lock().unwrap().entered && st.overlays.accent.bridge
    });
    assert!(h.state.accent_open('a'));
    h.state.scripted_key("Tab").unwrap();
    let shared = client.shared.clone();
    h.wait_until("the preview", move |_| {
        shared
            .lock()
            .unwrap()
            .text_events
            .iter()
            .any(|e| matches!(e, TextEvent::Preedit(Some(t)) if t == "à"))
    });
    h.state.scripted_key("Escape").unwrap();
    let shared = client.shared.clone();
    h.wait_until("the restore", move |_| {
        shared
            .lock()
            .unwrap()
            .text_events
            .iter()
            .any(|e| matches!(e, TextEvent::Commit(Some(t)) if t == "a"))
    });
    assert!(!h.state.overlays.accent.is_open());
}

#[test]
fn a_held_letter_key_opens_the_picker_on_its_first_repeat_and_swallows_the_release() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.accent.table = Some(accent_table()))
    else {
        return;
    };
    assert!(h.state.connect_input_method());
    let client = mapped_window(&mut h, true, true);
    let shared = client.shared.clone();
    h.wait_until("the text input to become active", move |st| {
        shared.lock().unwrap().entered && st.overlays.accent.bridge
    });
    let e = Keycode::new(26); // evdev KEY_E + 8
    h.at(100.0);
    h.state.keyboard_event_now(e, KeyState::Pressed, 0);
    assert!(
        h.state.overlays.accent.swallow_release.contains(&e.raw()),
        "an accented letter arms the trigger"
    );
    // the application gets the press and the release at once, so its own repeat never starts: no double insert
    let shared = client.shared.clone();
    h.wait_until("the typed letter", move |_| {
        shared.lock().unwrap().keys.len() >= 2
    });
    assert_eq!(
        client.shared.lock().unwrap().keys,
        [(18, true), (18, false)]
    );
    h.at(100.3);
    assert!(
        !h.state.overlays.accent.is_open(),
        "not before the repeat delay (500 ms)"
    );
    h.at(100.55);
    assert!(
        h.state.overlays.accent.is_open(),
        "the compositor's first repeat opens the popup"
    );
    // the physical release is not delivered a second time
    h.state.keyboard_event_now(e, KeyState::Released, 0);
    assert!(h.state.overlays.accent.swallow_release.is_empty());
    assert!(
        h.state.overlays.accent.is_open(),
        "the popup stays until a choice"
    );
    // a digit picks the first candidate
    h.state.scripted_key("1").unwrap();
    let shared = client.shared.clone();
    h.wait_until("the commit", move |_| {
        shared
            .lock()
            .unwrap()
            .text_events
            .iter()
            .any(|e| matches!(e, TextEvent::Commit(Some(t)) if t == "è"))
    });
    // a tapped key never opens it
    h.at(200.0);
    h.state.keyboard_event_now(e, KeyState::Pressed, 0);
    h.at(200.1);
    h.state.keyboard_event_now(e, KeyState::Released, 0);
    h.at(201.0);
    assert!(!h.state.overlays.accent.is_open());
}

#[test]
fn without_an_accent_table_the_bridge_stays_out_of_the_way() {
    let Some(mut h) = Harness::new() else { return };
    assert!(
        !h.state.connect_input_method(),
        "no input-method global without a table: installed input methods keep working"
    );
    assert!(!h.state.accent_open('e'));
}

#[test]
fn a_click_on_an_accent_candidate_commits_it_and_a_click_outside_closes_the_popup() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.accent.table = Some(accent_table()))
    else {
        return;
    };
    assert!(h.state.connect_input_method());
    let client = mapped_window(&mut h, true, false);
    let shared = client.shared.clone();
    h.wait_until("the text input to become active", move |st| {
        shared.lock().unwrap().entered && st.overlays.accent.bridge
    });
    h.wait_until("the cursor rectangle", |st| st.text_cursor.is_some());
    assert!(h.state.accent_open('e'));
    let popup = h.state.overlays.accent.popup.clone().unwrap();
    let inset = if h.state.config.shell.show_menu_bar {
        h.state.config.shell.menu_bar_height
    } else {
        0.0
    };
    let output = cupertino_geometry::Rect::new(0.0, 0.0, f64::from(OUTPUT.0), f64::from(OUTPUT.1));
    let layout = cupertino_server::kbd_ui::accent_layout(&popup, output, inset);
    let second = layout.cells[1];
    h.state.pointer_moved_to(
        smithay::utils::Point::from((second.x + second.w / 2.0, second.y + second.h / 2.0)),
        0,
        None,
    );
    h.state
        .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
    h.state.pointer_button(
        cupertino_server::events::BTN_LEFT,
        ButtonState::Released,
        10,
    );
    let shared = client.shared.clone();
    h.wait_until("the clicked candidate", move |_| {
        shared
            .lock()
            .unwrap()
            .text_events
            .iter()
            .any(|e| matches!(e, TextEvent::Commit(Some(t)) if t == "é"))
    });
    assert!(!h.state.overlays.accent.is_open());
    // outside: the popup closes and the press goes on
    assert!(h.state.accent_open('e'));
    h.state.pointer_moved_to(
        smithay::utils::Point::from((5.0, f64::from(OUTPUT.1) - 5.0)),
        20,
        None,
    );
    h.state
        .pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 20);
    assert!(!h.state.overlays.accent.is_open());
}

// ---- the key filters (slow, sticky and mouse keys, globe key, caps-lock switch, compositor repeat) ------------

/// A key event from the device (evdev code), through the whole filter chain.
fn device_key(h: &mut Harness, evdev: u32, pressed: bool) {
    let state = if pressed {
        KeyState::Pressed
    } else {
        KeyState::Released
    };
    h.state.key_from_device(Keycode::new(evdev + 8), state, 0);
}

fn tap(h: &mut Harness, evdev: u32) {
    device_key(h, evdev, true);
    device_key(h, evdev, false);
}

fn focused_keyboard_client(h: &mut Harness) -> TestClient {
    let client = mapped_window(h, false, true);
    let shared = client.shared.clone();
    h.wait_until("the keyboard focus", move |st| {
        st.seat.get_keyboard().unwrap().current_focus().is_some() && shared.lock().unwrap().mapped
    });
    client
}

fn client_keys(h: &mut Harness, client: &TestClient, count: usize) -> Vec<(u32, bool)> {
    let shared = client.shared.clone();
    h.wait_until("the client's keys", move |_| {
        shared.lock().unwrap().keys.len() >= count
    });
    // late extra events would show up here
    for _ in 0..10 {
        h.step();
    }
    client.shared.lock().unwrap().keys.clone()
}

const SHIFT: u32 = 42;
const KEY_A: u32 = 30;
const CTRL: u32 = 29;

#[test]
fn sticky_keys_latch_a_tapped_shift_for_the_next_key_only() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.access.sticky_keys = true) else {
        return;
    };
    let client = focused_keyboard_client(&mut h);
    let kb = h.state.seat.get_keyboard().unwrap();
    h.at(10.0);
    tap(&mut h, SHIFT);
    assert!(
        kb.modifier_state().shift,
        "the tapped Shift stays down (latched)"
    );
    tap(&mut h, KEY_A);
    assert!(!kb.modifier_state().shift, "consumed by the next key");
    tap(&mut h, KEY_A);
    // Shift is released after the first a, so the client sees "A" then "a"
    assert_eq!(
        client_keys(&mut h, &client, 6),
        [
            (SHIFT, true),
            (KEY_A, true),
            (KEY_A, false),
            (SHIFT, false),
            (KEY_A, true),
            (KEY_A, false)
        ]
    );
}

#[test]
fn a_modifier_held_when_the_device_was_read_does_not_latch() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.access.sticky_keys = true) else {
        return;
    };
    let client = focused_keyboard_client(&mut h);
    let kb = h.state.seat.get_keyboard().unwrap();
    h.at(10.0);
    // Ctrl still down from the chord that switched to this session: the device read reports it held
    h.state.key_filters.pass_unfiltered(CTRL);
    tap(&mut h, CTRL);
    assert!(
        !kb.modifier_state().ctrl,
        "a key no filter saw go down is released, not latched"
    );
    tap(&mut h, KEY_A);
    assert_eq!(
        client_keys(&mut h, &client, 4),
        [(CTRL, true), (CTRL, false), (KEY_A, true), (KEY_A, false)]
    );
}

#[test]
fn five_shift_presses_toggle_sticky_keys_and_a_double_tap_locks() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.access.sticky_keys_shortcut = true)
    else {
        return;
    };
    let kb = h.state.seat.get_keyboard().unwrap();
    h.at(10.0);
    for _ in 0..5 {
        tap(&mut h, SHIFT);
    }
    assert!(
        h.state.key_filters.sticky.enabled(),
        "five Shift releases within 30 s"
    );
    assert!(!kb.modifier_state().shift, "the toggling taps were plain");
    h.at(11.0);
    tap(&mut h, SHIFT);
    tap(&mut h, SHIFT);
    tap(&mut h, KEY_A);
    tap(&mut h, KEY_A);
    assert!(
        kb.modifier_state().shift,
        "locked: stays down across typed keys"
    );
    tap(&mut h, SHIFT);
    assert!(!kb.modifier_state().shift, "a third press unlocks");
}

#[test]
fn slow_keys_accept_a_key_only_after_the_delay() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.access.slow_keys = true) else {
        return;
    };
    let client = focused_keyboard_client(&mut h);
    assert_eq!(
        client.shared.lock().unwrap().repeat_info,
        Some((2, 420)),
        "the repeat runs at 420 ms with slow keys"
    );
    let kb = h.state.seat.get_keyboard().unwrap();
    let a = Keycode::new(KEY_A + 8);
    h.at(10.0);
    device_key(&mut h, KEY_A, true);
    h.at(10.1);
    device_key(&mut h, KEY_A, false);
    h.at(11.0);
    assert!(
        !kb.pressed_keys().contains(&a),
        "a press shorter than 250 ms vanishes"
    );
    device_key(&mut h, KEY_A, true);
    h.at(11.2);
    assert!(!kb.pressed_keys().contains(&a));
    h.at(11.26);
    assert!(
        kb.pressed_keys().contains(&a),
        "accepted once held for 250 ms"
    );
    device_key(&mut h, KEY_A, false);
    assert_eq!(
        client_keys(&mut h, &client, 2),
        [(KEY_A, true), (KEY_A, false)]
    );
}

#[test]
fn mouse_keys_move_the_pointer_from_the_keypad_with_the_verified_ramp() {
    let Some(mut h) = Harness::with_config(|c| c.keyboard.access.mouse_keys = true) else {
        return;
    };
    let kb = h.state.seat.get_keyboard().unwrap();
    h.state
        .pointer_moved_to(smithay::utils::Point::from((100.0, 100.0)), 0, None);
    const KP6: u32 = 77;
    const KP2: u32 = 80;
    h.at(10.0);
    device_key(&mut h, KP6, true);
    assert_eq!(
        h.state.pointer.current_location().x,
        101.0,
        "the first move is one point"
    );
    assert!(
        !kb.pressed_keys().contains(&Keycode::new(KP6 + 8)),
        "the keypad key does not type"
    );
    h.at(11.0);
    assert_eq!(
        h.state.pointer.current_location().x,
        101.0,
        "no repeat before the 1 s delay"
    );
    h.at(11.1);
    // repeats at 1.05 s and 1.10 s move 2 and 3 points
    assert_eq!(h.state.pointer.current_location().x, 106.0);
    device_key(&mut h, KP6, false);
    h.at(12.0);
    assert_eq!(
        h.state.pointer.current_location().x,
        106.0,
        "released: no more motion"
    );
    device_key(&mut h, KP2, true);
    device_key(&mut h, KP2, false);
    assert_eq!(
        h.state.pointer.current_location().y,
        101.0,
        "keypad 2 moves down"
    );
}

#[test]
fn the_compositor_repeats_the_hud_arrow_at_the_configured_delay_and_interval() {
    let Some(mut h) = Harness::with_config(with_sources) else {
        return;
    };
    h.state.overlays.hold_modifiers = true;
    h.at(5.0);
    h.state.scripted_key("Ctrl+space").unwrap();
    assert_eq!(h.state.overlays.inputs.hud.as_ref().unwrap().selection, 1);
    const DOWN: u32 = 108;
    let selection = |h: &Harness| h.state.overlays.inputs.hud.as_ref().unwrap().selection;
    device_key(&mut h, DOWN, true);
    assert_eq!(selection(&h), 2, "Down moves the HUD selection");
    h.at(5.45);
    assert_eq!(selection(&h), 2);
    h.at(5.51);
    assert_eq!(selection(&h), 0, "the first repeat after 500 ms, wrapping");
    h.at(5.6);
    assert_eq!(selection(&h), 1, "then every 83 ms");
    device_key(&mut h, DOWN, false);
    h.at(7.0);
    assert_eq!(selection(&h), 1, "the release stops the repeat");
    device_key(&mut h, 103, true); // Up
    assert_eq!(selection(&h), 0);
    device_key(&mut h, 103, false);
}

#[test]
fn a_globe_key_tap_switches_the_input_source_and_a_hold_or_chord_does_not() {
    let Some(mut h) = Harness::with_config(with_sources) else {
        return;
    };
    const FN: u32 = 464;
    h.at(5.0);
    device_key(&mut h, FN, true);
    h.at(5.1);
    device_key(&mut h, FN, false);
    assert_eq!(
        h.state.active_xkb_group(),
        1,
        "a tap selects the next source in menu order, at once"
    );
    h.at(6.0);
    device_key(&mut h, FN, true);
    h.at(6.6);
    device_key(&mut h, FN, false);
    assert_eq!(
        h.state.active_xkb_group(),
        1,
        "held longer than the 500 ms repeat delay: debounced"
    );
    h.at(7.0);
    device_key(&mut h, FN, true);
    tap(&mut h, KEY_A);
    device_key(&mut h, FN, false);
    assert_eq!(
        h.state.active_xkb_group(),
        1,
        "used with another key: no tap"
    );
}

#[test]
fn caps_lock_switch_taps_change_the_source_and_holds_turn_caps_lock_on() {
    let Some(mut h) = Harness::with_config(|c| {
        with_sources(c);
        c.keyboard.caps_switch = true;
    }) else {
        return;
    };
    let kb = h.state.seat.get_keyboard().unwrap();
    let caps = Keycode::new(66);
    h.state.key_from_device(caps, KeyState::Pressed, 1000);
    h.state.key_from_device(caps, KeyState::Released, 1200);
    assert_eq!(
        h.state.active_xkb_group(),
        1,
        "a tap switches to the previous source"
    );
    assert!(!kb.modifier_state().caps_lock);
    h.state.key_from_device(caps, KeyState::Pressed, 2000);
    h.wait_until("caps lock after the 500 ms hold", |st| {
        st.seat.get_keyboard().unwrap().modifier_state().caps_lock
    });
    h.state.key_from_device(caps, KeyState::Released, 2600);
    assert_eq!(h.state.active_xkb_group(), 1, "a hold does not switch");
    assert!(kb.modifier_state().caps_lock);
}

#[test]
fn a_decoded_layout_source_replaces_the_keymap_while_it_is_current() {
    let layout = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../cupertino-keyboard/tests/test_layout.json");
    let decoded = format!("decoded:{}", layout.display());
    let id = decoded.clone();
    let Some(mut h) = Harness::with_config(move |c| {
        c.keyboard.sources = ["us(basic)", decoded.as_str()]
            .iter()
            .map(|s| SourceSpec::parse(s).unwrap())
            .collect()
    }) else {
        return;
    };
    assert!(
        h.state.overlays.inputs.decoded.contains_key(&id),
        "the generated keymap is ready at startup"
    );
    let e = Keycode::new(18 + 8);
    // the third level of the E key: dead acute in the test layout, nothing in the system "us(basic)" layout
    let alt_sym = |h: &mut Harness| {
        let kb = h.state.seat.get_keyboard().unwrap();
        kb.with_xkb_state(&mut h.state, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            // SAFETY: the design does not outlive the lock guard
            let keymap = unsafe { xkb.keymap() };
            keymap
                .key_get_syms_by_level(e, 0, 2)
                .first()
                .map(|s| s.raw())
        })
    };
    assert_eq!(alt_sym(&mut h), None);
    h.state
        .input_source_step(false, cupertino_keyboard::Step::Next);
    assert!(
        h.state.overlays.inputs.decoded_active,
        "the generated keymap is the seat keymap"
    );
    assert_eq!(
        alt_sym(&mut h),
        Some(smithay::input::keyboard::keysyms::KEY_dead_acute)
    );
    assert!(
        h.state.overlays.inputs.hud.is_none(),
        "an immediate switch shows no HUD"
    );
    // back to the system layout
    h.state
        .input_source_step(false, cupertino_keyboard::Step::Next);
    assert!(!h.state.overlays.inputs.decoded_active);
    assert_eq!(alt_sym(&mut h), None);
    assert_eq!(h.state.active_xkb_group(), 0);
}

// ---- the chord rewrite for third-party clients -----------------------------------------------

const SUPER: u32 = 125;
const KEY_C: u32 = 46;
const KEY_LEFT: u32 = 105;
const KEY_HOME: u32 = 102;
const KEY_H: u32 = 35;
const MOD_SHIFT: u32 = 1;
const MOD_CTRL: u32 = 1 << 2;
const MOD_SUPER: u32 = 1 << 6;

/// A focused client with this application id; its key log is empty once it has the keyboard.
fn client_as(h: &mut Harness, app_id: &'static str) -> TestClient {
    let client = TestClient::spawn_as(h.socket_path(), app_id, false, true);
    let shared = client.shared.clone();
    h.wait_until("the client window", move |st| {
        shared.lock().unwrap().mapped
            && st.wm.key_window().is_some()
            && st.seat.get_keyboard().unwrap().current_focus().is_some()
    });
    for _ in 0..10 {
        h.step();
    }
    client.shared.lock().unwrap().key_log.clear();
    client
}

/// The client's key and modifiers events once `count` have arrived.
fn key_log(h: &mut Harness, client: &TestClient, count: usize) -> Vec<KeyLog> {
    let shared = client.shared.clone();
    h.wait_until("the client's key log", move |_| {
        shared.lock().unwrap().key_log.len() >= count
    });
    for _ in 0..10 {
        h.step();
    }
    client.shared.lock().unwrap().key_log.clone()
}

/// Presses `keys` in order and releases them in reverse.
fn chord(h: &mut Harness, keys: &[u32]) {
    for key in keys {
        device_key(h, *key, true);
    }
    for key in keys.iter().rev() {
        device_key(h, *key, false);
    }
}

#[test]
fn super_c_reaches_a_third_party_client_as_ctrl_c_and_the_real_modifiers_come_back() {
    let Some(mut h) = Harness::new() else { return };
    let client = client_as(&mut h, "org.example.editor");
    chord(&mut h, &[SUPER, KEY_C]);
    use KeyLog::{Key, Mods};
    assert_eq!(
        key_log(&mut h, &client, 8),
        [
            Key(SUPER, true),
            Mods(MOD_SUPER, 0),
            Mods(MOD_CTRL, 0),
            Key(KEY_C, true),
            Key(KEY_C, false),
            Mods(MOD_SUPER, 0),
            Key(SUPER, false),
            Mods(0, 0)
        ]
    );
}

#[test]
fn ctrl_c_is_never_rewritten() {
    let Some(mut h) = Harness::new() else { return };
    let client = client_as(&mut h, "org.example.editor");
    chord(&mut h, &[CTRL, KEY_C]);
    use KeyLog::{Key, Mods};
    assert_eq!(
        key_log(&mut h, &client, 6),
        [
            Key(CTRL, true),
            Mods(MOD_CTRL, 0),
            Key(KEY_C, true),
            Key(KEY_C, false),
            Key(CTRL, false),
            Mods(0, 0)
        ]
    );
}

#[test]
fn releasing_super_before_c_leaves_no_stray_key_and_ends_with_the_real_modifiers() {
    let Some(mut h) = Harness::new() else { return };
    let client = client_as(&mut h, "org.example.editor");
    device_key(&mut h, SUPER, true);
    device_key(&mut h, KEY_C, true);
    device_key(&mut h, SUPER, false);
    device_key(&mut h, KEY_C, false);
    use KeyLog::{Key, Mods};
    assert_eq!(
        key_log(&mut h, &client, 8),
        [
            Key(SUPER, true),
            Mods(MOD_SUPER, 0),
            Mods(MOD_CTRL, 0),
            Key(KEY_C, true),
            Key(SUPER, false),
            Mods(0, 0),
            Key(KEY_C, false),
            Mods(0, 0)
        ]
    );
}

#[test]
fn terminals_get_ctrl_shift_c_and_first_party_clients_the_real_chord() {
    let Some(mut h) = Harness::new() else { return };
    let term = client_as(&mut h, "foot");
    chord(&mut h, &[SUPER, KEY_C]);
    use KeyLog::{Key, Mods};
    assert_eq!(
        key_log(&mut h, &term, 8)[2..4],
        [Mods(MOD_CTRL | MOD_SHIFT, 0), Key(KEY_C, true)]
    );
    drop(term);
    let Some(mut h) = Harness::new() else { return };
    let own = client_as(&mut h, "cupertino-files");
    chord(&mut h, &[SUPER, KEY_C]);
    assert_eq!(
        key_log(&mut h, &own, 6),
        [
            Key(SUPER, true),
            Mods(MOD_SUPER, 0),
            Key(KEY_C, true),
            Key(KEY_C, false),
            Key(SUPER, false),
            Mods(0, 0)
        ]
    );
}

#[test]
fn super_left_moves_to_the_line_start_only_while_a_text_input_is_active() {
    let Some(mut h) = Harness::new() else { return };
    let client = client_as(&mut h, "org.example.editor");
    chord(&mut h, &[SUPER, KEY_LEFT]);
    use KeyLog::{Key, Mods};
    assert_eq!(
        key_log(&mut h, &client, 6),
        [
            Key(SUPER, true),
            Mods(MOD_SUPER, 0),
            Key(KEY_LEFT, true),
            Key(KEY_LEFT, false),
            Key(SUPER, false),
            Mods(0, 0)
        ]
    );
    client.shared.lock().unwrap().key_log.clear();
    h.state.overlays.accent.bridge = true;
    chord(&mut h, &[SUPER, KEY_LEFT]);
    assert_eq!(
        key_log(&mut h, &client, 8),
        [
            Key(SUPER, true),
            Mods(MOD_SUPER, 0),
            Mods(0, 0),
            Key(KEY_HOME, true),
            Key(KEY_HOME, false),
            Mods(MOD_SUPER, 0),
            Key(SUPER, false),
            Mods(0, 0)
        ]
    );
}

#[test]
fn a_chord_the_router_claims_is_never_forwarded() {
    let Some(mut h) = Harness::new() else { return };
    let client = client_as(&mut h, "org.example.editor");
    // Super+H hides the application
    chord(&mut h, &[SUPER, KEY_H]);
    for _ in 0..10 {
        h.step();
    }
    let log = client.shared.lock().unwrap().key_log.clone();
    assert!(
        !log.iter()
            .any(|e| matches!(e, KeyLog::Key(KEY_H, _) | KeyLog::Mods(MOD_CTRL, _))),
        "{log:?}"
    );
}
