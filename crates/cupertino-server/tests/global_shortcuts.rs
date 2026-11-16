//! `cupertino_global_shortcuts_v1` against a headless compositor: the portal's connection binds a chord, the key press
//! and release reach it while another application's window has the keyboard, and that application gets neither; a
//! chord the server binds stays the server's; a connection without the role sees no global; and the chord goes back to
//! the focused client once the binding object is gone. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use common::{Client, Rounds};
use cupertino_protocols::client::global_shortcuts::cupertino_global_shortcuts_v1::{
    self, CupertinoGlobalShortcutsV1, Modifier,
};
use cupertino_server::authorization::ShellRole;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::backend::input::KeyState;
use smithay::input::keyboard::Keycode;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::{Connection, Dispatch, QueueHandle};

/// evdev codes
const KEY_K: u32 = 37;
const KEY_TAB: u32 = 15;
const KEY_LEFTCTRL: u32 = 29;
const KEY_LEFTALT: u32 = 56;
const KEY_SUPER: u32 = 125;
/// `XKB_KEY_k` and `XKB_KEY_Tab`.
const SYM_K: u32 = 0x6b;
const SYM_TAB: u32 = 0xff09;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Heard {
    Reserved(u32, u32),
    Done,
    Pressed(u32),
    Released(u32),
}

/// What the test asks the portal's stand-in to send.
enum Ask {
    Bind(u32, Modifier, u32),
    Destroy,
}

#[derive(Default)]
struct Portal {
    heard: Arc<Mutex<Vec<Heard>>>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Portal {
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

impl Dispatch<CupertinoGlobalShortcutsV1, ()> for Portal {
    fn event(
        state: &mut Self,
        _: &CupertinoGlobalShortcutsV1,
        event: cupertino_global_shortcuts_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let heard = match event {
            cupertino_global_shortcuts_v1::Event::Reserved { modifiers, keysym } => {
                Heard::Reserved(modifiers.into(), keysym)
            }
            cupertino_global_shortcuts_v1::Event::Done => Heard::Done,
            cupertino_global_shortcuts_v1::Event::Pressed { id, .. } => Heard::Pressed(id),
            cupertino_global_shortcuts_v1::Event::Released { id, .. } => Heard::Released(id),
            _ => return,
        };
        state.heard.lock().unwrap().push(heard);
    }
}

/// The portal's stand-in on a thread of its own: it binds the global when it is offered (`bound` says whether it was)
/// and records what the server sends it.
struct PortalClient {
    heard: Arc<Mutex<Vec<Heard>>>,
    asks: mpsc::Sender<Ask>,
    bound: Arc<Mutex<Option<bool>>>,
    stop: Arc<AtomicBool>,
    rounds: Rounds,
}

impl PortalClient {
    fn over(stream: std::os::unix::net::UnixStream) -> PortalClient {
        let heard = Arc::new(Mutex::new(Vec::new()));
        let bound = Arc::new(Mutex::new(None));
        let stop = Arc::new(AtomicBool::new(false));
        let rounds = Rounds::default();
        let (asks, asked) = mpsc::channel::<Ask>();
        let (heard2, bound2, stop2, rounds2) =
            (heard.clone(), bound.clone(), stop.clone(), rounds.clone());
        std::thread::spawn(move || {
            let _ended = rounds2.ended_on_drop();
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<Portal>(&conn).unwrap();
            let qh = queue.handle();
            let mut state = Portal { heard: heard2 };
            let mut object: Option<CupertinoGlobalShortcutsV1> = globals.bind(&qh, 1..=1, ()).ok();
            *bound2.lock().unwrap() = Some(object.is_some());
            while !stop2.load(Ordering::SeqCst) {
                while let Ok(ask) = asked.try_recv() {
                    match (ask, &object) {
                        (Ask::Bind(id, mods, sym), Some(o)) => o.bind(id, mods, sym),
                        (Ask::Destroy, Some(o)) => {
                            o.destroy();
                            object = None;
                        }
                        _ => {}
                    }
                }
                if queue.roundtrip(&mut state).is_err() {
                    break;
                }
                rounds2.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        PortalClient {
            heard,
            asks,
            bound,
            stop,
            rounds,
        }
    }

    fn heard(&self) -> Vec<Heard> {
        self.heard.lock().unwrap().clone()
    }

    /// The presses and releases it heard, without the reserved chords.
    fn keys(&self) -> Vec<Heard> {
        self.heard()
            .into_iter()
            .filter(|h| matches!(h, Heard::Pressed(_) | Heard::Released(_)))
            .collect()
    }
}

impl Drop for PortalClient {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
    now: f64,
    clients: Vec<Rounds>,
}

impl Harness {
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
        match headless::init(&mut state, (1000, 700), 1.0) {
            Ok(()) => Some(Harness {
                state,
                event_loop,
                now: 100.0,
                clients: Vec::new(),
            }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.state.fixed_time = Some(self.now);
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state) {
                return;
            }
        }
        panic!("timed out waiting for {what}");
    }

    fn settle(&mut self) {
        let barriers: Vec<_> = self.clients.iter().map(Rounds::barrier).collect();
        self.wait_until("the clients' roundtrips", move |_| {
            barriers.iter().all(|b| b())
        });
    }

    fn advance(&mut self, seconds: f64) {
        self.now += seconds;
        self.settle();
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    fn key(&mut self, evdev: u32, pressed: bool) {
        self.state.key_from_device(
            Keycode::new(evdev + 8),
            if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            },
            (self.now * 1000.0) as u32,
        );
        self.settle();
    }

    /// The portal's stand-in on a connection the server made for a process with `roles`.
    fn portal(&mut self, roles: &[ShellRole]) -> (PortalClient, common::Holder) {
        let (stream, holder) = common::held_shell_connection(&self.state, roles);
        let portal = PortalClient::over(stream);
        self.clients.push(portal.rounds.clone());
        let bound = portal.bound.clone();
        self.wait_until("the stand-in to see the globals", move |_| {
            bound.lock().unwrap().is_some()
        });
        self.settle();
        (portal, holder)
    }

    /// An application whose window has the keyboard.
    fn focused_app(&mut self) -> Client {
        let app = Client::listening(self.socket_path(), "org.example.editor");
        self.clients.push(app.rounds());
        self.wait_until("the window to map", |s| s.wm.windows().count() == 1);
        self.advance(1.0);
        let window = self.state.wm.windows().next().unwrap().id;
        self.state.focus_window_by_input(window);
        self.advance(1.0);
        app
    }
}

const CTRL_ALT: Modifier = Modifier::Ctrl.union(Modifier::Alt);

#[test]
fn a_bound_chord_reaches_the_portal_and_not_the_focused_application() {
    let Some(mut h) = Harness::new() else { return };
    let app = h.focused_app();
    let (portal, _holder) = h.portal(&[ShellRole::Capture, ShellRole::GlobalShortcuts]);
    assert_eq!(*portal.bound.lock().unwrap(), Some(true));
    // the server's own chords come first, Super+Tab among them
    let heard = portal.heard();
    assert_eq!(heard.last(), Some(&Heard::Done), "{heard:?}");
    assert!(
        heard.contains(&Heard::Reserved(Modifier::Logo.bits(), SYM_TAB)),
        "{heard:?}"
    );

    portal.asks.send(Ask::Bind(3, CTRL_ALT, SYM_K)).unwrap();
    portal
        .asks
        .send(Ask::Bind(4, Modifier::Logo, SYM_TAB))
        .unwrap();
    h.settle();
    h.key(KEY_LEFTCTRL, true);
    h.key(KEY_LEFTALT, true);
    h.key(KEY_K, true);
    assert_eq!(portal.keys(), [Heard::Pressed(3)]);
    // the modifiers go up before the key: the release still ends the shortcut
    h.key(KEY_LEFTALT, false);
    h.key(KEY_LEFTCTRL, false);
    h.key(KEY_K, false);
    assert_eq!(portal.keys(), [Heard::Pressed(3), Heard::Released(3)]);
    let keys = app.keys();
    assert!(
        !keys.iter().any(|(key, _)| *key == KEY_K),
        "the focused application got K: {keys:?}"
    );
    assert!(keys.contains(&(KEY_LEFTCTRL, true)), "{keys:?}");

    // Super+Tab is the server's: no shortcut fires for it
    h.key(KEY_SUPER, true);
    h.key(KEY_TAB, true);
    h.key(KEY_TAB, false);
    h.key(KEY_SUPER, false);
    assert_eq!(portal.keys(), [Heard::Pressed(3), Heard::Released(3)]);

    // without the object the chord is the focused application's again
    portal.asks.send(Ask::Destroy).unwrap();
    h.settle();
    assert!(h.state.key_router.global.is_empty());
    h.key(KEY_LEFTCTRL, true);
    h.key(KEY_LEFTALT, true);
    h.key(KEY_K, true);
    h.key(KEY_K, false);
    h.key(KEY_LEFTALT, false);
    h.key(KEY_LEFTCTRL, false);
    assert!(app.keys().contains(&(KEY_K, true)), "{:?}", app.keys());
    assert_eq!(portal.keys(), [Heard::Pressed(3), Heard::Released(3)]);
}

#[test]
fn only_the_portals_connection_sees_the_global_and_its_chords_end_with_it() {
    let Some(mut h) = Harness::new() else { return };
    let (other, _held) = h.portal(&[ShellRole::Capture]);
    assert_eq!(*other.bound.lock().unwrap(), Some(false));
    let public =
        PortalClient::over(std::os::unix::net::UnixStream::connect(h.socket_path()).unwrap());
    h.clients.push(public.rounds.clone());
    let bound = public.bound.clone();
    h.wait_until("the public client to see the globals", move |_| {
        bound.lock().unwrap().is_some()
    });
    assert_eq!(*public.bound.lock().unwrap(), Some(false));

    let (portal, holder) = h.portal(&[ShellRole::GlobalShortcuts]);
    portal.asks.send(Ask::Bind(1, CTRL_ALT, SYM_K)).unwrap();
    h.settle();
    assert_eq!(h.state.key_router.global.len(), 1);
    // the process quits: its connection goes, and its chords with it
    drop(portal);
    holder.end();
    h.wait_until("the chords to go", |s| s.key_router.global.is_empty());
}

#[test]
fn the_portals_prompt_windows_are_out_of_desktop_controls_reach() {
    let Some(mut h) = Harness::new() else { return };
    let prompt = Client::listening(
        h.socket_path(),
        cupertino_server::global_shortcuts::PORTAL_PROMPT_APP_ID,
    );
    h.clients.push(prompt.rounds());
    let chooser = Client::listening(h.socket_path(), "org.cupertino.Portal");
    h.clients.push(chooser.rounds());
    h.wait_until("the windows to map", |s| s.wm.windows().count() == 2);
    h.advance(1.0);
    let by_app = |s: &Cupertino, app_id: &str| {
        s.windows
            .iter()
            .find(|w| {
                smithay::wayland::compositor::with_states(w.surface(), |states| {
                    states
                        .data_map
                        .get::<smithay::wayland::shell::xdg::XdgToplevelSurfaceData>()
                        .and_then(|d| d.lock().unwrap().app_id.clone())
                })
                .as_deref()
                    == Some(app_id)
            })
            .unwrap()
            .surface()
            .clone()
    };
    let (prompt_surface, chooser_surface) = (
        by_app(
            &h.state,
            cupertino_server::global_shortcuts::PORTAL_PROMPT_APP_ID,
        ),
        by_app(&h.state, "org.cupertino.Portal"),
    );
    // the app id alone names no prompt: no connection of this process holds the portal's role yet
    assert!(!h.state.is_portal_prompt(&prompt_surface));
    let (portal, _holder) = h.portal(&[ShellRole::Capture, ShellRole::GlobalShortcuts]);
    assert_eq!(*portal.bound.lock().unwrap(), Some(true));
    assert!(h.state.is_portal_prompt(&prompt_surface));
    // the portal's choosers stay within reach
    assert!(!h.state.is_portal_prompt(&chooser_surface));
}
