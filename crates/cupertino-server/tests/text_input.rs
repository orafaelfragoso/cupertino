//! The text input of the focused client and the input method of the seat (`zwp_text_input_v3` and
//! `zwp_input_method_v2`): enable and disable, the state a field sends, the input method's preedit, commit and delete
//! as one `done`, and a focus change. A synthetic input method on a connection of the compositor's own stands in for
//! a real one. In-process on an offscreen EGL device (tests skip when there is none).
use crate::common;

use std::io::Write;
use std::os::fd::AsFd;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::state::ClientState;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_output, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::wp::text_input::zv3::client::{
    zwp_text_input_manager_v3, zwp_text_input_v3,
};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_manager_v2, zwp_input_method_v2, zwp_input_popup_surface_v2,
};

const OUTPUT: (i32, i32) = (1000, 700);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    /// A compositor with the text input and input method globals (they come with an accent table), without its own
    /// input method connected: the seat's single input method slot is free for the synthetic one.
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> =
            EventLoop::try_new().expect("creating the event loop");
        let display: Display<Cupertino> = Display::new().expect("creating the Wayland display");
        let mut cfg = Config::default();
        cfg.keyboard.accent.table =
            Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/accents.json"));
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

    /// Wait for each of `clients`' [`common::Rounds::barrier`] together: they have every event sent before, and the
    /// server handled their answers and what they were told to send.
    fn sync(&mut self, clients: &[&common::Rounds]) {
        let barriers: Vec<_> = clients.iter().map(|r| r.barrier()).collect();
        self.wait_until("the clients' roundtrips", move |_| {
            barriers.iter().all(|b| b())
        });
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }
}

// ---- the synthetic input method ------------------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum ImSeen {
    Activate,
    Deactivate,
    Surrounding(String, u32, u32),
    Cause(u32),
    Content(u32, u32),
    Done,
}

enum ImDo {
    Preedit(String, i32, i32),
    Commit(String),
    Delete(u32, u32),
    /// `commit` with the serial of the `done` events seen so far, or a stale one.
    Apply {
        stale: bool,
    },
}

#[derive(Default)]
struct ImShared {
    seen: Vec<ImSeen>,
    bound: bool,
    /// What the input method's popup surface heard: `enter`, `leave` and `scale N` (`preferred_buffer_scale`).
    popup: Vec<String>,
}

struct ImState {
    shared: Arc<Mutex<ImShared>>,
    dones: u32,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ImState {
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

macro_rules! ignore_im_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for ImState {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_im_events!(
    wl_seat::WlSeat,
    wl_compositor::WlCompositor,
    wl_output::WlOutput,
    zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2
);

impl Dispatch<wl_surface::WlSurface, ()> for ImState {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let heard = match event {
            wl_surface::Event::Enter { .. } => "enter".to_string(),
            wl_surface::Event::Leave { .. } => "leave".to_string(),
            wl_surface::Event::PreferredBufferScale { factor } => format!("scale {factor}"),
            _ => return,
        };
        state.shared.lock().unwrap().popup.push(heard);
    }
}

impl Dispatch<zwp_input_method_manager_v2::ZwpInputMethodManagerV2, ()> for ImState {
    fn event(
        _: &mut Self,
        _: &zwp_input_method_manager_v2::ZwpInputMethodManagerV2,
        _: zwp_input_method_manager_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<zwp_input_method_v2::ZwpInputMethodV2, ()> for ImState {
    fn event(
        state: &mut Self,
        _: &zwp_input_method_v2::ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_input_method_v2::Event;
        let seen = match event {
            Event::Activate => ImSeen::Activate,
            Event::Deactivate => ImSeen::Deactivate,
            Event::SurroundingText {
                text,
                cursor,
                anchor,
            } => ImSeen::Surrounding(text, cursor, anchor),
            Event::TextChangeCause { cause } => ImSeen::Cause(enum_value(cause)),
            Event::ContentType { hint, purpose } => {
                ImSeen::Content(enum_value(hint), enum_value(purpose))
            }
            Event::Done => {
                state.dones += 1;
                ImSeen::Done
            }
            _ => return,
        };
        state.shared.lock().unwrap().seen.push(seen);
    }
}

fn enum_value<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(v) => v.into(),
        WEnum::Unknown(v) => v,
    }
}

struct InputMethod {
    shared: Arc<Mutex<ImShared>>,
    tx: Sender<ImDo>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    rounds: common::Rounds,
}

impl InputMethod {
    /// Connects on a connection of the compositor's own (only such a client may be the input method) and binds the
    /// seat's input method.
    fn connect(h: &mut Harness) -> InputMethod {
        let (server_end, client_end) = UnixStream::pair().unwrap();
        let data = Arc::new(ClientState {
            internal: true,
            ..Default::default()
        });
        h.state
            .display_handle
            .insert_client(server_end, data)
            .expect("adding the input method's connection");
        let shared = Arc::new(Mutex::new(ImShared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx): (Sender<ImDo>, Receiver<ImDo>) = channel();
        let rounds = common::Rounds::default();
        let (sh, st, rt) = (shared.clone(), stop.clone(), rounds.clone());
        let thread = std::thread::spawn(move || {
            let _ended = rt.ended_on_drop();
            let conn = Connection::from_socket(client_end).unwrap();
            let (globals, mut queue) = registry_queue_init::<ImState>(&conn).unwrap();
            let qh = queue.handle();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let manager: zwp_input_method_manager_v2::ZwpInputMethodManagerV2 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let im = manager.get_input_method(&seat, &qh, ());
            // the output, so the popup can be told it is on it
            let _output: wl_output::WlOutput = globals.bind(&qh, 1..=4, ()).unwrap();
            // a candidate popup, through which the compositor learns the field's cursor rectangle
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let popup_surface = compositor.create_surface(&qh, ());
            let _popup = im.get_input_popup_surface(&popup_surface, &qh, ());
            let mut state = ImState {
                shared: sh.clone(),
                dones: 0,
            };
            sh.lock().unwrap().bound = true;
            while !st.load(Ordering::SeqCst) {
                while let Ok(todo) = rx.try_recv() {
                    match todo {
                        ImDo::Preedit(text, begin, end) => im.set_preedit_string(text, begin, end),
                        ImDo::Commit(text) => im.commit_string(text),
                        ImDo::Delete(before, after) => im.delete_surrounding_text(before, after),
                        ImDo::Apply { stale } => im.commit(if stale {
                            state.dones.wrapping_sub(1)
                        } else {
                            state.dones
                        }),
                    }
                }
                if queue.roundtrip(&mut state).is_err() {
                    break;
                }
                rt.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let im = InputMethod {
            shared,
            tx,
            stop,
            thread: Some(thread),
            rounds,
        };
        let shared = im.shared.clone();
        h.wait_until("the input method to bind", move |_| {
            shared.lock().unwrap().bound
        });
        h.sync(&[&im.rounds]);
        im
    }

    fn send(&self, todo: ImDo) {
        self.tx.send(todo).unwrap();
    }

    fn seen(&self) -> Vec<ImSeen> {
        self.shared.lock().unwrap().seen.clone()
    }

    fn clear(&self) {
        self.shared.lock().unwrap().seen.clear();
    }

    fn popup(&self) -> Vec<String> {
        self.shared.lock().unwrap().popup.clone()
    }
}

impl Drop for InputMethod {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take();
    }
}

// ---- a client window with a text field ---------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
enum TiSeen {
    Enter,
    Leave,
    Preedit(Option<String>, i32, i32),
    Commit(Option<String>),
    Delete(u32, u32),
    /// The serial of `done`, and the number of commits the client had sent then.
    Done(u32, u32),
}

enum TiDo {
    /// Enable with surrounding text (text, cursor), a content type and a cursor rectangle, and commit.
    Enable(String, i32),
    /// Send new surrounding text and commit.
    Surrounding(String, i32),
    Disable,
}

#[derive(Default)]
struct TiShared {
    mapped: bool,
    seen: Vec<TiSeen>,
}

struct TiState {
    shared: Arc<Mutex<TiShared>>,
    shm: wl_shm::WlShm,
    surface: wl_surface::WlSurface,
    buffer: Option<wl_buffer::WlBuffer>,
    commits: u32,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for TiState {
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
        impl Dispatch<$t, ()> for TiState {
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
    xdg_toplevel::XdgToplevel,
    zwp_text_input_manager_v3::ZwpTextInputManagerV3
);

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for TiState {
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

impl Dispatch<xdg_surface::XdgSurface, ()> for TiState {
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
                    rustix::fs::memfd_create("text-input-test", rustix::fs::MemfdFlags::CLOEXEC)
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

impl Dispatch<zwp_text_input_v3::ZwpTextInputV3, ()> for TiState {
    fn event(
        state: &mut Self,
        _: &zwp_text_input_v3::ZwpTextInputV3,
        event: zwp_text_input_v3::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        use zwp_text_input_v3::Event;
        let seen = match event {
            Event::Enter { .. } => TiSeen::Enter,
            Event::Leave { .. } => TiSeen::Leave,
            Event::PreeditString {
                text,
                cursor_begin,
                cursor_end,
            } => TiSeen::Preedit(text, cursor_begin, cursor_end),
            Event::CommitString { text } => TiSeen::Commit(text),
            Event::DeleteSurroundingText {
                before_length,
                after_length,
            } => TiSeen::Delete(before_length, after_length),
            Event::Done { serial } => TiSeen::Done(serial, state.commits),
            _ => return,
        };
        state.shared.lock().unwrap().seen.push(seen);
    }
}

struct Field {
    shared: Arc<Mutex<TiShared>>,
    tx: Sender<TiDo>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    rounds: common::Rounds,
}

impl Field {
    /// A mapped window with a text input; the harness waits until it is the key window.
    fn open(h: &mut Harness, app_id: &'static str) -> Field {
        let socket = h.socket_path();
        let shared = Arc::new(Mutex::new(TiShared::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx): (Sender<TiDo>, Receiver<TiDo>) = channel();
        let rounds = common::Rounds::default();
        let (sh, st, rt) = (shared.clone(), stop.clone(), rounds.clone());
        let thread = std::thread::spawn(move || {
            let _ended = rt.ended_on_drop();
            let stream = UnixStream::connect(&socket).expect("connecting to the compositor");
            let conn = Connection::from_socket(stream).unwrap();
            let (globals, mut queue) = registry_queue_init::<TiState>(&conn).unwrap();
            let qh = queue.handle();
            let compositor: wl_compositor::WlCompositor = globals.bind(&qh, 4..=6, ()).unwrap();
            let shm: wl_shm::WlShm = globals.bind(&qh, 1..=1, ()).unwrap();
            let wm_base: xdg_wm_base::XdgWmBase = globals.bind(&qh, 1..=5, ()).unwrap();
            let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ()).unwrap();
            let surface = compositor.create_surface(&qh, ());
            let xdg = wm_base.get_xdg_surface(&surface, &qh, ());
            let toplevel = xdg.get_toplevel(&qh, ());
            toplevel.set_app_id(app_id.to_string());
            let manager: zwp_text_input_manager_v3::ZwpTextInputManagerV3 =
                globals.bind(&qh, 1..=1, ()).unwrap();
            let ti = manager.get_text_input(&seat, &qh, ());
            surface.commit();
            let mut state = TiState {
                shared: sh,
                shm,
                surface,
                buffer: None,
                commits: 0,
            };
            while !st.load(Ordering::SeqCst) {
                while let Ok(todo) = rx.try_recv() {
                    match todo {
                        TiDo::Enable(text, cursor) => {
                            ti.enable();
                            ti.set_surrounding_text(text, cursor, cursor);
                            ti.set_text_change_cause(zwp_text_input_v3::ChangeCause::Other);
                            ti.set_content_type(
                                zwp_text_input_v3::ContentHint::Spellcheck,
                                zwp_text_input_v3::ContentPurpose::Normal,
                            );
                            ti.set_cursor_rectangle(40, 30, 2, 18);
                        }
                        TiDo::Surrounding(text, cursor) => {
                            ti.set_surrounding_text(text, cursor, cursor);
                            ti.set_text_change_cause(zwp_text_input_v3::ChangeCause::InputMethod);
                        }
                        TiDo::Disable => ti.disable(),
                    }
                    ti.commit();
                    state.commits += 1;
                }
                if queue.roundtrip(&mut state).is_err() {
                    break;
                }
                rt.tick();
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        let field = Field {
            shared,
            tx,
            stop,
            thread: Some(thread),
            rounds,
        };
        let shared = field.shared.clone();
        let window_of = |s: &Cupertino| {
            s.wm.windows()
                .find(|w| s.wm.app(w.app).is_some_and(|a| a.identity == app_id))
                .map(|w| w.id)
        };
        h.wait_until("the window to map", move |st| {
            shared.lock().unwrap().mapped && window_of(st).is_some()
        });
        // only a process's first window takes the keyboard by itself, and the test clients share one process
        let window = window_of(&h.state).unwrap();
        h.sync(&[&field.rounds]);
        if h.state.wm.key_window() != Some(window) {
            h.state.focus_window_by_input(window);
        }
        let shared = field.shared.clone();
        h.wait_until("the window to take the keyboard", move |st| {
            st.wm.key_window() == Some(window)
                && shared.lock().unwrap().seen.contains(&TiSeen::Enter)
        });
        h.sync(&[&field.rounds]);
        field
    }

    fn send(&self, todo: TiDo) {
        self.tx.send(todo).unwrap();
    }

    fn seen(&self) -> Vec<TiSeen> {
        self.shared.lock().unwrap().seen.clone()
    }

    fn clear(&self) {
        self.shared.lock().unwrap().seen.clear();
    }
}

impl Drop for Field {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.thread.take();
    }
}

fn wait_im(h: &mut Harness, im: &InputMethod, what: &str, cond: impl Fn(&[ImSeen]) -> bool) {
    let end = Instant::now() + common::HANG_GUARD;
    while Instant::now() < end {
        h.step();
        if cond(&im.seen()) {
            return;
        }
    }
    panic!(
        "timed out waiting for {what}; the input method saw {:?}",
        im.seen()
    );
}

fn wait_field(h: &mut Harness, field: &Field, what: &str, cond: impl Fn(&[TiSeen]) -> bool) {
    let end = Instant::now() + common::HANG_GUARD;
    while Instant::now() < end {
        h.step();
        if cond(&field.seen()) {
            return;
        }
    }
    panic!(
        "timed out waiting for {what}; the field saw {:?}",
        field.seen()
    );
}

/// The input method was activated and has seen the `done` that completes the activation's state (a keyboard enter
/// sends a deactivation with a `done` of its own first).
fn activated(seen: &[ImSeen]) -> bool {
    seen.iter()
        .rposition(|e| *e == ImSeen::Activate)
        .is_some_and(|at| seen[at..].contains(&ImSeen::Done))
}

// ---- tests -------------------------------------------------------------------------------------

#[test]
fn enabling_activates_the_input_method_with_the_fields_state_and_disabling_deactivates_it() {
    let Some(mut h) = Harness::new() else {
        return;
    };
    let im = InputMethod::connect(&mut h);
    let field = Field::open(&mut h, "text-input.one");
    im.clear();
    field.send(TiDo::Enable("héllo".into(), 3));
    wait_im(&mut h, &im, "the activation", activated);
    let seen = im.seen();
    assert_eq!(seen[0], ImSeen::Activate, "{seen:?}");
    assert!(
        seen.contains(&ImSeen::Surrounding("héllo".into(), 3, 3)),
        "surrounding text in UTF-8 bytes: {seen:?}"
    );
    assert!(seen.contains(&ImSeen::Content(
        zwp_text_input_v3::ContentHint::Spellcheck.bits(),
        zwp_text_input_v3::ContentPurpose::Normal.into()
    )));
    assert_eq!(seen.last(), Some(&ImSeen::Done));
    // the cursor rectangle reaches the compositor, for the input method's popup
    h.wait_until("the cursor rectangle", |st| st.text_cursor.is_some());

    im.clear();
    field.send(TiDo::Disable);
    wait_im(&mut h, &im, "the deactivation", |s| {
        s.contains(&ImSeen::Deactivate) && s.contains(&ImSeen::Done)
    });
    assert_eq!(im.seen(), [ImSeen::Deactivate, ImSeen::Done]);
}

#[test]
fn a_preedit_commit_and_delete_reach_the_field_as_one_done_with_its_serial() {
    let Some(mut h) = Harness::new() else {
        return;
    };
    let im = InputMethod::connect(&mut h);
    let field = Field::open(&mut h, "text-input.one");
    field.send(TiDo::Enable("ab".into(), 2));
    wait_im(&mut h, &im, "the activation", activated);
    field.clear();
    // a preedit with its cursor after the first character, in bytes, a commit and a deletion of the "b"
    im.send(ImDo::Delete(1, 0));
    im.send(ImDo::Commit("日".into()));
    im.send(ImDo::Preedit("にほ".into(), 3, 3));
    im.send(ImDo::Apply { stale: false });
    wait_field(&mut h, &field, "the batch", |s| {
        s.iter().any(|e| matches!(e, TiSeen::Done(..)))
    });
    let seen = field.seen();
    let done = seen
        .iter()
        .position(|e| matches!(e, TiSeen::Done(..)))
        .unwrap();
    let batch = &seen[..done];
    assert!(
        batch.contains(&TiSeen::Preedit(Some("にほ".into()), 3, 3)),
        "{seen:?}"
    );
    assert!(
        batch.contains(&TiSeen::Commit(Some("日".into()))),
        "{seen:?}"
    );
    assert!(batch.contains(&TiSeen::Delete(1, 0)), "{seen:?}");
    let TiSeen::Done(serial, commits) = seen[done] else {
        unreachable!()
    };
    assert_eq!(
        serial, commits,
        "the batch answers the field's latest state"
    );

    // the field sends its new state; an input method that commits for an older one gets its batch through as stale
    field.clear();
    field.send(TiDo::Surrounding("a日".into(), 4));
    // the server has the field's new state, then the input method has heard of it
    h.sync(&[&field.rounds]);
    h.sync(&[&im.rounds]);
    im.send(ImDo::Commit("本".into()));
    im.send(ImDo::Apply { stale: true });
    wait_field(&mut h, &field, "the stale batch", |s| {
        s.iter().any(|e| matches!(e, TiSeen::Done(..)))
    });
    let seen = field.seen();
    assert!(seen.contains(&TiSeen::Commit(Some("本".into()))));
    let Some(TiSeen::Done(serial, commits)) = seen.last().cloned() else {
        panic!("{seen:?}")
    };
    assert_ne!(
        serial, commits,
        "a stale batch carries a serial the field can tell"
    );
}

#[test]
fn a_focus_change_deactivates_the_input_method_and_its_edits_never_reach_the_old_field() {
    let Some(mut h) = Harness::new() else {
        return;
    };
    let im = InputMethod::connect(&mut h);
    let first = Field::open(&mut h, "text-input.one");
    first.send(TiDo::Enable("one".into(), 3));
    wait_im(&mut h, &im, "the activation", activated);
    im.send(ImDo::Preedit("ka".into(), 2, 2));
    im.send(ImDo::Apply { stale: false });
    wait_field(&mut h, &first, "the preedit", |s| {
        s.iter().any(|e| matches!(e, TiSeen::Done(..)))
    });
    im.clear();
    first.clear();

    // a second window takes the keyboard: the first field leaves and the input method is turned off for it
    let second = Field::open(&mut h, "text-input.two");
    wait_field(&mut h, &first, "the leave", |s| s.contains(&TiSeen::Leave));
    wait_im(&mut h, &im, "the deactivation", |s| {
        s.contains(&ImSeen::Deactivate)
    });

    // the input method's late edit goes nowhere, and the new field starts afresh once it enables
    first.clear();
    im.send(ImDo::Commit("x".into()));
    im.send(ImDo::Apply { stale: false });
    // the server handled the late edit, then the fields heard all it sent them
    h.sync(&[&im.rounds]);
    h.sync(&[&first.rounds, &second.rounds]);
    assert!(
        !first
            .seen()
            .iter()
            .any(|e| matches!(e, TiSeen::Commit(_) | TiSeen::Done(..))),
        "{:?}",
        first.seen()
    );
    assert!(
        !second.seen().iter().any(|e| matches!(e, TiSeen::Commit(_))),
        "nothing reaches a field that has not enabled"
    );
    im.clear();
    second.send(TiDo::Enable("two".into(), 0));
    wait_im(&mut h, &im, "the new activation", activated);
    let seen = im.seen();
    assert_eq!(seen.first(), Some(&ImSeen::Activate));
    assert!(seen.contains(&ImSeen::Surrounding("two".into(), 0, 0)));
}

#[test]
fn edits_after_the_field_disables_never_reach_it() {
    let Some(mut h) = Harness::new() else {
        return;
    };
    let im = InputMethod::connect(&mut h);
    let field = Field::open(&mut h, "text-input.one");
    field.send(TiDo::Enable("".into(), 0));
    wait_im(&mut h, &im, "the activation", activated);
    // the window taking the keyboard already deactivated the input method once: forget that, so the wait below is
    // for the deactivation the field's disable causes, after the server has handled that disable
    im.clear();
    field.send(TiDo::Disable);
    h.sync(&[&field.rounds]);
    wait_im(&mut h, &im, "the deactivation", |s| {
        s.contains(&ImSeen::Deactivate) && s.contains(&ImSeen::Done)
    });
    field.clear();
    im.send(ImDo::Preedit("late".into(), 4, 4));
    im.send(ImDo::Commit("late".into()));
    im.send(ImDo::Apply { stale: false });
    // the server handled the late edits, then the field heard all it sent it
    h.sync(&[&im.rounds]);
    h.sync(&[&field.rounds]);
    assert!(
        !field.seen().iter().any(|e| matches!(
            e,
            TiSeen::Preedit(..) | TiSeen::Commit(_) | TiSeen::Done(..)
        )),
        "{:?}",
        field.seen()
    );
}

/// The input method's popup is on the output of the field's text cursor while the input method is active, at that
/// output's whole-number scale, and on none once the field disables its text input.
#[test]
fn the_input_methods_popup_is_on_the_output_of_the_text_cursor_while_active() {
    let Some(mut h) = Harness::new() else {
        return;
    };
    let im = InputMethod::connect(&mut h);
    let field = Field::open(&mut h, "text-input.popup");
    assert!(
        im.popup().is_empty(),
        "not shown before an activation: {:?}",
        im.popup()
    );
    field.send(TiDo::Enable("text".into(), 2));
    wait_im(&mut h, &im, "the activation", activated);
    let shared = im.shared.clone();
    h.wait_until("the popup to enter the output", move |_| {
        shared.lock().unwrap().popup.contains(&"enter".to_string())
    });
    // at scale 1, which a wl_surface of version 6 needs not be told
    assert!(
        !im.popup().iter().any(|e| e.starts_with("scale")),
        "{:?}",
        im.popup()
    );

    // the output's scale changes: the popup is asked for the new one
    let head = h.state.displays.heads[0].id;
    let mut state = h.state.displays.heads[0].state;
    state.scale = 2.0;
    h.state
        .configure_heads(&[(head, state)], false)
        .expect("the scale applies");
    h.state.heads_changed();
    let shared = im.shared.clone();
    h.wait_until("the popup's new scale", move |_| {
        shared
            .lock()
            .unwrap()
            .popup
            .contains(&"scale 2".to_string())
    });

    field.send(TiDo::Disable);
    let shared = im.shared.clone();
    h.wait_until("the popup to leave the output", move |_| {
        shared.lock().unwrap().popup.last() == Some(&"leave".to_string())
    });
    h.sync(&[&im.rounds]);
    assert_eq!(im.popup(), ["enter", "scale 2", "leave"]);
}
