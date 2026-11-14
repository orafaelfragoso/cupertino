//! Output management against a headless compositor with two offscreen displays: a
//! `zwlr_output_manager_v1` client lists both heads, rearranges them, turns one off and on again,
//! sees stale configurations cancelled, hot-plugged heads come and go, and a display plugged in
//! again gets its stored arrangement back. Tests skip (pass) when no
//! EGL device is available.

use crate::common;

use std::io::ErrorKind;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use cupertino_defaults::displays::{display_key, DisplaysConfig};
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::backend::Backend;
use cupertino_server::config::Config;
use cupertino_server::displays::HeadMode;
use cupertino_server::state::RETIRED_GLOBAL_LIFETIME;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_output::{self, WlOutput};
use wayland_client::protocol::wl_registry;
use wayland_client::{event_created_child, Connection, Dispatch, Proxy, QueueHandle};
use wayland_protocols_wlr::output_management::v1::client::{
    zwlr_output_configuration_head_v1::ZwlrOutputConfigurationHeadV1,
    zwlr_output_configuration_v1::{self, ZwlrOutputConfigurationV1},
    zwlr_output_head_v1::{self, ZwlrOutputHeadV1},
    zwlr_output_manager_v1::{self, ZwlrOutputManagerV1},
    zwlr_output_mode_v1::{self, ZwlrOutputModeV1},
};

const LEFT: (i32, i32) = (1000, 700);
const RIGHT: (i32, i32) = (800, 600);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    /// A compositor with `headless-1` (`LEFT` pixels) and `headless-2` (`RIGHT` pixels), unscaled.
    fn new() -> Option<Harness> {
        Self::with(
            &[
                display_of("headless-1", LEFT),
                display_of("headless-2", RIGHT),
            ],
            Some(1.0),
        )
    }

    /// A compositor with `displays`, at `scale` or else at their own default scales.
    fn with(displays: &[HeadlessDisplay], scale: Option<f64>) -> Option<Harness> {
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
        match headless::init_displays(&mut state, displays, scale) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                common::egl_unavailable(format!("{e:#}"));
                None
            }
        }
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Run the compositor until `cond` holds for it and what `client` has been told.
    fn wait_until(
        &mut self,
        client: &Client,
        what: &str,
        mut cond: impl FnMut(&Cupertino, &Seen) -> bool,
    ) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if cond(&self.state, &client.seen.lock().unwrap()) {
                return;
            }
        }
        panic!(
            "timed out waiting for {what}; the client saw {:?}",
            client.seen.lock().unwrap()
        );
    }

    /// The logical rectangle of the output named `name`, as (x, y, w, h).
    fn output_rect(&self, name: &str) -> Option<(f64, f64, f64, f64)> {
        let entry = self
            .state
            .outputs
            .iter()
            .find(|o| o.output.name() == name)?;
        let rect = self
            .state
            .wm
            .output(entry.id)
            .expect("the window manager knows the output")
            .rect;
        Some((rect.x, rect.y, rect.w, rect.h))
    }
}

fn display_of(name: &str, size: (i32, i32)) -> HeadlessDisplay {
    HeadlessDisplay {
        name: name.into(),
        size,
        physical_mm: (0, 0),
    }
}

// ---- the client ----------------------------------------------------------------------------

/// A head as the client was told it.
#[derive(Clone, Debug, Default)]
struct SeenHead {
    name: String,
    enabled: bool,
    position: (i32, i32),
    scale: f64,
    /// Pixel size of the current mode.
    current: Option<(i32, i32)>,
    finished: bool,
}

/// Everything the client was told, for the test thread to check.
#[derive(Debug, Default)]
struct Seen {
    /// In the order they were announced.
    heads: Vec<SeenHead>,
    /// Serial of the last `done`.
    serial: Option<u32>,
    /// `succeeded`, `failed` or `cancelled`, one per configuration, in order.
    results: Vec<&'static str>,
    /// `wl_output` globals announced and withdrawn after the client connected, in order.
    outputs: Vec<OutputGlobal>,
    /// Commands the compositor answered without an error.
    answered: usize,
    /// The compositor disconnected the client with a protocol error.
    disconnected: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OutputGlobal {
    Added(u32),
    Removed(u32),
}

impl Seen {
    fn head(&self, name: &str) -> Option<&SeenHead> {
        self.heads.iter().find(|h| h.name == name && !h.finished)
    }
}

struct ClientState {
    manager: ZwlrOutputManagerV1,
    registry: wl_registry::WlRegistry,
    /// Head objects, at the same index as their `Seen::heads` entry.
    heads: Vec<ZwlrOutputHeadV1>,
    /// Mode objects and their pixel sizes.
    modes: Vec<(ZwlrOutputModeV1, (i32, i32))>,
    seen: Arc<Mutex<Seen>>,
}

impl ClientState {
    fn head_index(&self, head: &ZwlrOutputHeadV1) -> usize {
        self.heads
            .iter()
            .position(|h| h == head)
            .expect("a head the manager announced")
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for ClientState {
    fn event(
        state: &mut Self,
        _: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let mut seen = state.seen.lock().unwrap();
        match event {
            wl_registry::Event::Global {
                name, interface, ..
            } if interface == WlOutput::interface().name => {
                seen.outputs.push(OutputGlobal::Added(name))
            }
            wl_registry::Event::GlobalRemove { name }
                if seen.outputs.contains(&OutputGlobal::Added(name)) =>
            {
                seen.outputs.push(OutputGlobal::Removed(name))
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &WlOutput,
        _: wl_output::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<ZwlrOutputManagerV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        _: &ZwlrOutputManagerV1,
        event: zwlr_output_manager_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_manager_v1::Event::Head { head } => {
                state.heads.push(head);
                state.seen.lock().unwrap().heads.push(SeenHead::default());
            }
            zwlr_output_manager_v1::Event::Done { serial } => {
                state.seen.lock().unwrap().serial = Some(serial)
            }
            _ => {}
        }
    }

    event_created_child!(ClientState, ZwlrOutputManagerV1, [
        zwlr_output_manager_v1::EVT_HEAD_OPCODE => (ZwlrOutputHeadV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputHeadV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        head: &ZwlrOutputHeadV1,
        event: zwlr_output_head_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let idx = state.head_index(head);
        if let zwlr_output_head_v1::Event::Mode { mode } = event {
            state.modes.push((mode, (0, 0)));
            return;
        }
        let current = match &event {
            zwlr_output_head_v1::Event::CurrentMode { mode } => Some(
                state
                    .modes
                    .iter()
                    .find(|(m, _)| m == mode)
                    .expect("a mode the head announced")
                    .1,
            ),
            _ => None,
        };
        let mut seen = state.seen.lock().unwrap();
        let seen = &mut seen.heads[idx];
        match event {
            zwlr_output_head_v1::Event::Name { name } => seen.name = name,
            zwlr_output_head_v1::Event::Enabled { enabled } => seen.enabled = enabled != 0,
            zwlr_output_head_v1::Event::CurrentMode { .. } => seen.current = current,
            zwlr_output_head_v1::Event::Position { x, y } => seen.position = (x, y),
            zwlr_output_head_v1::Event::Scale { scale } => seen.scale = scale,
            zwlr_output_head_v1::Event::Finished => {
                seen.finished = true;
                head.release();
            }
            _ => {}
        }
    }

    event_created_child!(ClientState, ZwlrOutputHeadV1, [
        zwlr_output_head_v1::EVT_MODE_OPCODE => (ZwlrOutputModeV1, ()),
    ]);
}

impl Dispatch<ZwlrOutputModeV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        mode: &ZwlrOutputModeV1,
        event: zwlr_output_mode_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_output_mode_v1::Event::Size { width, height } => {
                state
                    .modes
                    .iter_mut()
                    .find(|(m, _)| m == mode)
                    .expect("a mode the head announced")
                    .1 = (width, height);
            }
            zwlr_output_mode_v1::Event::Finished => {
                state.modes.retain(|(m, _)| m != mode);
                mode.release();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrOutputConfigurationV1, ()> for ClientState {
    fn event(
        state: &mut Self,
        config: &ZwlrOutputConfigurationV1,
        event: zwlr_output_configuration_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let result = match event {
            zwlr_output_configuration_v1::Event::Succeeded => "succeeded",
            zwlr_output_configuration_v1::Event::Failed => "failed",
            zwlr_output_configuration_v1::Event::Cancelled => "cancelled",
            _ => return,
        };
        state.seen.lock().unwrap().results.push(result);
        config.destroy();
    }
}

impl Dispatch<ZwlrOutputConfigurationHeadV1, ()> for ClientState {
    fn event(
        _: &mut Self,
        _: &ZwlrOutputConfigurationHeadV1,
        _: <ZwlrOutputConfigurationHeadV1 as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

/// How a configuration sets one head: `None` disables it, otherwise it is enabled with these.
#[derive(Clone, Copy, Default)]
struct HeadChange {
    position: Option<(i32, i32)>,
    scale: Option<f64>,
    /// A mode by its pixel size and refresh rate in mHz.
    custom_mode: Option<(i32, i32, i32)>,
}

type Command = Box<dyn FnOnce(&mut ClientState, &QueueHandle<ClientState>) + Send>;

/// An output-management client on its own thread; dropping it disconnects the client.
struct Client {
    seen: Arc<Mutex<Seen>>,
    commands: mpsc::Sender<Command>,
    stop: Arc<AtomicBool>,
}

impl Client {
    fn connect(socket: PathBuf) -> Client {
        let seen = Arc::new(Mutex::new(Seen::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let (commands, received) = mpsc::channel::<Command>();
        let (seen2, stop2) = (seen.clone(), stop.clone());
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(&socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let (globals, mut queue) = registry_queue_init::<ClientState>(&conn).unwrap();
            let qh = queue.handle();
            let manager: ZwlrOutputManagerV1 = globals.bind(&qh, 4..=4, ()).unwrap();
            let mut client = ClientState {
                manager,
                registry: globals.registry().clone(),
                heads: Vec::new(),
                modes: Vec::new(),
                seen: seen2,
            };
            while !stop2.load(Ordering::SeqCst) {
                let mut sent = 0;
                while let Ok(command) = received.try_recv() {
                    command(&mut client, &qh);
                    sent += 1;
                }
                if queue.roundtrip(&mut client).is_err() {
                    client.seen.lock().unwrap().disconnected = true;
                    break;
                }
                client.seen.lock().unwrap().answered += sent;
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Client {
            seen,
            commands,
            stop,
        }
    }

    /// Apply (or with `test_only`, test) a configuration naming every head in `heads` at `serial`.
    fn configure(
        &self,
        serial: u32,
        heads: Vec<(&'static str, Option<HeadChange>)>,
        test_only: bool,
    ) {
        self.commands
            .send(Box::new(move |client, qh| {
                let config = client.manager.create_configuration(serial, qh, ());
                let seen = client.seen.lock().unwrap();
                for (name, change) in heads {
                    let idx = seen
                        .heads
                        .iter()
                        .position(|h| h.name == name && !h.finished)
                        .expect("a listed head");
                    let head = &client.heads[idx];
                    match change {
                        None => config.disable_head(head),
                        Some(change) => {
                            let config_head = config.enable_head(head, qh, ());
                            if let Some((x, y)) = change.position {
                                config_head.set_position(x, y);
                            }
                            if let Some(scale) = change.scale {
                                config_head.set_scale(scale);
                            }
                            if let Some((w, h, refresh)) = change.custom_mode {
                                config_head.set_custom_mode(w, h, refresh);
                            }
                        }
                    }
                }
                if test_only {
                    config.test();
                } else {
                    config.apply();
                }
            }))
            .expect("the client thread runs");
    }

    /// Bind the `wl_output` global `name`, whether or not the client was told that it went away.
    fn bind_output(&self, name: u32) {
        self.commands
            .send(Box::new(move |client, qh| {
                client.registry.bind::<WlOutput, _, _>(name, 4, qh, ());
            }))
            .expect("the client thread runs");
    }

    fn serial(&self) -> u32 {
        self.seen
            .lock()
            .unwrap()
            .serial
            .expect("the heads were listed")
    }

    fn results(&self) -> Vec<&'static str> {
        self.seen.lock().unwrap().results.clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

fn listed(seen: &Seen) -> bool {
    seen.serial.is_some()
        && seen.head("headless-1").is_some_and(|h| h.current.is_some())
        && seen.head("headless-2").is_some_and(|h| h.current.is_some())
}

// ---- tests ---------------------------------------------------------------------------------

#[test]
fn both_heads_are_listed_and_can_be_rearranged() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    {
        let seen = client.seen.lock().unwrap();
        let (left, right) = (
            seen.head("headless-1").unwrap(),
            seen.head("headless-2").unwrap(),
        );
        assert!(left.enabled && right.enabled);
        assert_eq!(
            (left.position, left.current, left.scale),
            ((0, 0), Some(LEFT), 1.0)
        );
        assert_eq!(
            (right.position, right.current, right.scale),
            ((LEFT.0, 0), Some(RIGHT), 1.0),
            "a new display starts right of the others"
        );
    }
    assert_eq!(
        h.output_rect("headless-2"),
        Some((LEFT.0 as f64, 0.0, RIGHT.0 as f64, RIGHT.1 as f64))
    );

    // the smaller display moves to the left; the larger one follows it at scale 2
    let serial = client.serial();
    let left = HeadChange {
        position: Some((RIGHT.0, 0)),
        scale: Some(2.0),
        custom_mode: None,
    };
    let right = HeadChange {
        position: Some((0, 0)),
        scale: None,
        custom_mode: None,
    };
    client.configure(
        serial,
        vec![("headless-1", Some(left)), ("headless-2", Some(right))],
        false,
    );
    h.wait_until(&client, "the new arrangement", |_, seen| {
        seen.results == ["succeeded"]
            && seen.serial != Some(serial)
            && seen.head("headless-1").is_some_and(|h| h.scale == 2.0)
    });
    assert_eq!(
        h.output_rect("headless-1"),
        Some((
            RIGHT.0 as f64,
            0.0,
            LEFT.0 as f64 / 2.0,
            LEFT.1 as f64 / 2.0
        ))
    );
    assert_eq!(
        h.output_rect("headless-2"),
        Some((0.0, 0.0, RIGHT.0 as f64, RIGHT.1 as f64))
    );
    let seen = client.seen.lock().unwrap();
    assert_eq!(seen.head("headless-1").unwrap().position, (RIGHT.0, 0));
    assert_eq!(seen.head("headless-2").unwrap().position, (0, 0));
}

#[test]
fn a_built_in_panel_starts_at_its_default_size_and_takes_each_choice_exactly() {
    let panel = HeadlessDisplay {
        name: "eDP-1".into(),
        size: (3024, 1964),
        physical_mm: (302, 196),
    };
    let Some(mut h) = Harness::with(&[panel], None) else {
        return;
    };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "the panel", |_, seen| {
        seen.serial.is_some() && seen.head("eDP-1").is_some()
    });
    assert_eq!(
        client.seen.lock().unwrap().head("eDP-1").unwrap().scale,
        2.0
    );
    assert_eq!(h.output_rect("eDP-1"), Some((0.0, 0.0, 1512.0, 982.0)));
    // each scale goes over the wire in 256ths; the output still gets exactly the choice's points
    for (n, (width, height)) in [
        (1, (1352, 878)),
        (2, (1024, 665)),
        (3, (1800, 1169)),
        (4, (1147, 745)),
    ] {
        let serial = client.serial();
        let change = HeadChange {
            position: None,
            scale: Some(3024.0 / f64::from(width)),
            custom_mode: None,
        };
        client.configure(serial, vec![("eDP-1", Some(change))], false);
        h.wait_until(&client, "the new size", |state, seen| {
            seen.results.len() == n && state.outputs[0].rect.w == f64::from(width)
        });
        assert_eq!(
            client.seen.lock().unwrap().results.last(),
            Some(&"succeeded")
        );
        assert_eq!(
            h.output_rect("eDP-1"),
            Some((0.0, 0.0, f64::from(width), f64::from(height)))
        );
    }
}

#[test]
fn a_head_turns_off_and_on_but_the_last_one_stays_on() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));

    client.configure(
        client.serial(),
        vec![
            ("headless-1", Some(HeadChange::default())),
            ("headless-2", None),
        ],
        false,
    );
    h.wait_until(&client, "headless-2 off", |_, seen| {
        seen.results == ["succeeded"] && seen.head("headless-2").is_some_and(|h| !h.enabled)
    });
    assert_eq!(h.state.outputs.len(), 1, "a disabled head has no output");
    assert_eq!(h.output_rect("headless-2"), None);

    // turning the remaining one off as well is refused, in a test and for real
    client.configure(
        client.serial(),
        vec![("headless-1", None), ("headless-2", None)],
        true,
    );
    h.wait_until(&client, "the test to fail", |_, seen| {
        seen.results.len() == 2
    });
    client.configure(
        client.serial(),
        vec![("headless-1", None), ("headless-2", None)],
        false,
    );
    h.wait_until(&client, "the configuration to fail", |_, seen| {
        seen.results.len() == 3
    });
    assert_eq!(client.results(), ["succeeded", "failed", "failed"]);
    assert!(
        h.output_rect("headless-1").is_some(),
        "the last display stays on"
    );

    // turned on again, the head comes back where it was
    client.configure(
        client.serial(),
        vec![
            ("headless-1", Some(HeadChange::default())),
            ("headless-2", Some(HeadChange::default())),
        ],
        false,
    );
    h.wait_until(&client, "headless-2 on", |_, seen| {
        seen.results.len() == 4 && seen.head("headless-2").is_some_and(|h| h.enabled)
    });
    assert_eq!(client.results()[3], "succeeded");
    assert_eq!(
        h.output_rect("headless-2"),
        Some((LEFT.0 as f64, 0.0, RIGHT.0 as f64, RIGHT.1 as f64))
    );
}

#[test]
fn a_configuration_made_before_a_change_is_cancelled() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let stale = client.serial();
    let moved = HeadChange {
        position: Some((0, LEFT.1)),
        scale: None,
        custom_mode: None,
    };
    client.configure(
        stale,
        vec![
            ("headless-1", Some(HeadChange::default())),
            ("headless-2", Some(moved)),
        ],
        false,
    );
    h.wait_until(&client, "the first configuration", |_, seen| {
        seen.results.len() == 1 && seen.serial != Some(stale)
    });
    client.configure(
        stale,
        vec![
            ("headless-1", Some(HeadChange::default())),
            ("headless-2", Some(HeadChange::default())),
        ],
        false,
    );
    h.wait_until(&client, "the stale configuration", |_, seen| {
        seen.results.len() == 2
    });
    assert_eq!(client.results(), ["succeeded", "cancelled"]);
    assert_eq!(
        h.output_rect("headless-2"),
        Some((0.0, LEFT.1 as f64, RIGHT.0 as f64, RIGHT.1 as f64)),
        "the cancelled configuration changed nothing"
    );
}

#[test]
fn hot_plugged_heads_are_announced_and_finished() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let third = headless::add_display(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        Some(1.0),
    )
    .unwrap();
    h.wait_until(&client, "headless-3", |_, seen| {
        seen.head("headless-3")
            .is_some_and(|h| h.enabled && h.current == Some((640, 480)))
    });
    assert_eq!(
        client
            .seen
            .lock()
            .unwrap()
            .head("headless-3")
            .unwrap()
            .position,
        (LEFT.0 + RIGHT.0, 0)
    );
    assert_eq!(h.state.outputs.len(), 3);
    headless::remove_display(&mut h.state, third).unwrap();
    h.wait_until(&client, "headless-3 finished", |_, seen| {
        seen.head("headless-3").is_none()
    });
    assert_eq!(h.state.outputs.len(), 2);
}

#[test]
fn a_display_plugged_in_gets_its_stored_arrangement_back() {
    let Some(mut h) = Harness::new() else { return };
    let path = std::env::temp_dir().join(format!(
        "cupertino-test-{}-displays.conf",
        std::process::id()
    ));
    let key = display_key("headless-3", "Cupertino", "headless-3", None);
    std::fs::write(&path, format!("[{key}]\nenabled = true\nmode = 640x480@60000\nposition = 0,{}\nscale = 2\ntransform = normal\n", LEFT.1)).unwrap();
    h.state.displays.use_store(path.clone()).unwrap();
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));

    // the file's arrangement, not the default one right of the others
    let third =
        headless::add_display(&mut h.state, &display_of("headless-3", (640, 480)), None).unwrap();
    h.wait_until(&client, "headless-3", |_, seen| {
        seen.head("headless-3")
            .is_some_and(|h| h.enabled && h.scale == 2.0)
    });
    assert_eq!(
        client
            .seen
            .lock()
            .unwrap()
            .head("headless-3")
            .unwrap()
            .position,
        (0, LEFT.1)
    );
    assert_eq!(
        h.output_rect("headless-3"),
        Some((0.0, LEFT.1 as f64, 320.0, 240.0))
    );

    // an applied arrangement is written to the file and comes back on the next plug
    let moved = HeadChange {
        position: Some((LEFT.0 + RIGHT.0, 0)),
        scale: Some(1.0),
        custom_mode: None,
    };
    let unchanged = Some(HeadChange::default());
    client.configure(
        client.serial(),
        vec![
            ("headless-1", unchanged),
            ("headless-2", unchanged),
            ("headless-3", Some(moved)),
        ],
        false,
    );
    h.wait_until(&client, "the new arrangement", |_, seen| {
        seen.results == ["succeeded"]
    });
    let (stored, problems) = DisplaysConfig::load(&path).unwrap();
    assert!(problems.is_empty(), "{problems:?}");
    let settings = stored.get(&key).expect("the display's section");
    assert_eq!(
        (settings.position, settings.scale),
        ((LEFT.0 + RIGHT.0, 0), 1.0)
    );
    headless::remove_display(&mut h.state, third).unwrap();
    h.wait_until(&client, "headless-3 unplugged", |_, seen| {
        seen.head("headless-3").is_none()
    });
    headless::add_display(&mut h.state, &display_of("headless-3", (640, 480)), None).unwrap();
    h.wait_until(&client, "headless-3 again", |_, seen| {
        seen.head("headless-3").is_some_and(|h| h.enabled)
    });
    assert_eq!(
        h.output_rect("headless-3"),
        Some(((LEFT.0 + RIGHT.0) as f64, 0.0, 640.0, 480.0))
    );
    std::fs::remove_file(path).unwrap();
}

fn mode(width: i32, height: i32, refresh_mhz: i32) -> HeadMode {
    HeadMode {
        width,
        height,
        refresh_mhz,
    }
}

/// Make the offscreen displays fail to turn on in `modes` with an error of `kind`.
fn refuse(state: &mut Cupertino, kind: ErrorKind, modes: &[HeadMode]) {
    if let Some(Backend::Headless(b)) = state.backend.as_mut() {
        b.refused = modes.iter().map(|m| (*m, kind)).collect();
    }
}

#[test]
fn a_display_that_cannot_show_its_mode_falls_back_and_keeps_its_own_stored() {
    let Some(mut h) = Harness::new() else { return };
    let path = std::env::temp_dir().join(format!(
        "cupertino-test-{}-fallback-displays.conf",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    h.state.displays.use_store(path.clone()).unwrap();
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let offered = [mode(640, 480, 30_000), mode(320, 240, 60_000)];

    // the link cannot carry 640x480 at 60 Hz: the same size at 30 Hz is shown
    refuse(
        &mut h.state,
        ErrorKind::InvalidInput,
        &[mode(640, 480, 60_000)],
    );
    let third = headless::add_display_with_modes(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        &offered,
        Some(1.0),
    )
    .unwrap();
    let head = h.state.displays.head(third).unwrap();
    assert_eq!(head.state.mode, mode(640, 480, 30_000));
    assert!(head.output.is_some());

    // a configuration of the other heads stores the display's own mode, not the fallback
    let unchanged = Some(HeadChange::default());
    h.wait_until(&client, "headless-3", |_, seen| {
        seen.head("headless-3").is_some_and(|h| h.enabled)
    });
    client.configure(
        client.serial(),
        vec![
            ("headless-1", unchanged),
            ("headless-2", unchanged),
            ("headless-3", unchanged),
        ],
        false,
    );
    h.wait_until(&client, "the configuration", |_, seen| {
        seen.results == ["succeeded"]
    });
    let key = display_key("headless-3", "Cupertino", "headless-3", None);
    let (stored, _) = DisplaysConfig::load(&path).unwrap();
    let stored = stored.get(&key).expect("the display's section").mode;
    assert_eq!(
        (stored.width, stored.height, stored.refresh_mhz),
        (640, 480, 60_000)
    );

    // plugged in again where the link can carry it, the display shows its own mode
    headless::remove_display(&mut h.state, third).unwrap();
    refuse(&mut h.state, ErrorKind::InvalidInput, &[]);
    let third = headless::add_display_with_modes(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        &offered,
        Some(1.0),
    )
    .unwrap();
    assert_eq!(
        h.state.displays.head(third).unwrap().state.mode,
        mode(640, 480, 60_000)
    );

    // a display that can show none of its modes is announced but stays off
    headless::remove_display(&mut h.state, third).unwrap();
    refuse(
        &mut h.state,
        ErrorKind::InvalidInput,
        &[
            mode(640, 480, 60_000),
            mode(640, 480, 30_000),
            mode(320, 240, 60_000),
        ],
    );
    assert!(headless::add_display_with_modes(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        &offered,
        Some(1.0),
    )
    .is_err());
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_display_the_backend_cannot_show_is_off_but_stays_stored_on() {
    let Some(mut h) = Harness::new() else { return };
    let path = std::env::temp_dir().join(format!(
        "cupertino-test-{}-unsupported-displays.conf",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    h.state.displays.use_store(path.clone()).unwrap();
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let third = headless::add_display(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        Some(1.0),
    )
    .unwrap();
    h.state.head_unsupported(third);
    let head = h.state.displays.head(third).unwrap();
    assert!(!head.state.enabled);
    assert!(head.output.is_none());

    // a configuration of the other heads that leaves it off stores it on
    let unchanged = Some(HeadChange::default());
    h.wait_until(&client, "headless-3 off", |_, seen| {
        seen.head("headless-3").is_some_and(|h| !h.enabled)
    });
    let mut answers = 0;
    let mut configure = |h: &mut Harness, third: Option<HeadChange>| {
        client.configure(
            client.serial(),
            vec![
                ("headless-1", unchanged),
                ("headless-2", unchanged),
                ("headless-3", third),
            ],
            false,
        );
        answers += 1;
        h.wait_until(&client, "the configuration", |_, seen| {
            seen.results.len() == answers
                && seen
                    .head("headless-3")
                    .is_some_and(|h| h.enabled == third.is_some())
        });
        assert_eq!(
            client.seen.lock().unwrap().results.last(),
            Some(&"succeeded")
        );
        let key = display_key("headless-3", "Cupertino", "headless-3", None);
        let (stored, _) = DisplaysConfig::load(&path).unwrap();
        stored.get(&key).expect("the display's section").enabled
    };
    assert!(configure(&mut h, None));

    // turned on by the person, then off, it is stored off
    assert!(configure(&mut h, unchanged));
    assert!(!configure(&mut h, None));
    std::fs::remove_file(path).unwrap();
}

#[test]
fn a_client_binding_an_output_as_it_goes_away_stays_connected_until_the_global_is_gone() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let third = headless::add_display(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        Some(1.0),
    )
    .unwrap();
    h.wait_until(&client, "headless-3's output", |_, seen| {
        !seen.outputs.is_empty()
    });
    let outputs = client.seen.lock().unwrap().outputs.clone();
    let [OutputGlobal::Added(name)] = outputs[..] else {
        panic!("one output global was announced: {outputs:?}");
    };

    // the client binds the output after the compositor took it away
    headless::remove_display(&mut h.state, third).unwrap();
    let gone = Instant::now();
    client.bind_output(name);
    h.wait_until(&client, "the bind answered", |_, seen| {
        seen.answered == 1 || seen.disconnected
    });
    assert!(!client.seen.lock().unwrap().disconnected);

    // later the global is destroyed: binding it is a protocol error
    h.wait_until(&client, "the global's lifetime", |_, _| {
        gone.elapsed() > RETIRED_GLOBAL_LIFETIME + Duration::from_millis(100)
    });
    let late = Client::connect(h.socket_path());
    h.wait_until(&late, "both heads", |_, seen| listed(seen));
    late.bind_output(name);
    h.wait_until(&late, "the late bind refused", |_, seen| seen.disconnected);
}

#[test]
fn a_display_that_cannot_show_its_mode_announces_only_the_output_it_falls_back_to() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    refuse(
        &mut h.state,
        ErrorKind::InvalidInput,
        &[mode(640, 480, 60_000)],
    );
    headless::add_display_with_modes(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        &[mode(640, 480, 30_000)],
        Some(1.0),
    )
    .unwrap();
    // the head and the output global come on different queues, so either may arrive first
    h.wait_until(&client, "headless-3 and its output", |_, seen| {
        seen.head("headless-3").is_some_and(|h| h.enabled) && !seen.outputs.is_empty()
    });
    let settle = Instant::now() + Duration::from_millis(100);
    h.wait_until(&client, "a while for any later global", |_, _| {
        Instant::now() >= settle
    });
    let outputs = client.seen.lock().unwrap().outputs.clone();
    assert!(
        matches!(outputs[..], [OutputGlobal::Added(_)]),
        "{outputs:?}"
    );
}

#[test]
fn a_busy_display_is_not_tried_in_other_modes() {
    let Some(mut h) = Harness::new() else { return };
    refuse(
        &mut h.state,
        ErrorKind::ResourceBusy,
        &[mode(640, 480, 60_000)],
    );
    let error = headless::add_display_with_modes(
        &mut h.state,
        &display_of("headless-3", (640, 480)),
        &[mode(640, 480, 30_000)],
        Some(1.0),
    )
    .unwrap_err();
    assert_eq!(
        error.root_cause().to_string(),
        std::io::Error::from(ErrorKind::ResourceBusy).to_string()
    );
    let head = h
        .state
        .displays
        .heads
        .iter()
        .find(|h| h.info.connector == "headless-3")
        .expect("the display is listed");
    assert_eq!(
        (head.output, head.state.mode),
        (None, mode(640, 480, 60_000))
    );
}

/// A configuration that moves one display and gives the other a mode it cannot show fails as a whole: the first display
/// keeps its scale and place, the second its mode, the refused mode is not offered afterwards and nothing is stored.
#[test]
fn a_configuration_that_fails_part_way_keeps_the_last_valid_arrangement() {
    let Some(mut h) = Harness::new() else { return };
    let path = std::env::temp_dir().join(format!(
        "cupertino-test-{}-failed-displays.conf",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    h.state.displays.use_store(path.clone()).unwrap();
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let (left, right) = (h.output_rect("headless-1"), h.output_rect("headless-2"));
    refuse(
        &mut h.state,
        ErrorKind::InvalidInput,
        &[mode(640, 480, 60_000)],
    );
    let scaled = HeadChange {
        scale: Some(2.0),
        ..HeadChange::default()
    };
    let refused = HeadChange {
        custom_mode: Some((640, 480, 60_000)),
        ..HeadChange::default()
    };
    client.configure(
        client.serial(),
        vec![("headless-1", Some(scaled)), ("headless-2", Some(refused))],
        false,
    );
    h.wait_until(&client, "the answer", |_, seen| !seen.results.is_empty());
    assert_eq!(client.results(), ["failed"]);
    assert_eq!(h.output_rect("headless-1"), left, "the scale was undone");
    assert_eq!(h.output_rect("headless-2"), right);
    let head = |h: &Harness, name: &str| {
        h.state
            .displays
            .heads
            .iter()
            .find(|d| d.info.connector == name)
            .map(|d| (d.state, d.info.modes.clone()))
            .unwrap()
    };
    let (one, _) = head(&h, "headless-1");
    assert_eq!(one.scale, 1.0);
    let (two, offered) = head(&h, "headless-2");
    assert_eq!((two.mode.width, two.mode.height), RIGHT);
    assert!(
        !offered.contains(&mode(640, 480, 60_000)),
        "the refused mode is not offered: {offered:?}"
    );
    assert!(!path.exists(), "a failed configuration is not stored");
    // the same configuration without the refused mode applies
    client.configure(
        client.serial(),
        vec![
            ("headless-1", Some(scaled)),
            ("headless-2", Some(HeadChange::default())),
        ],
        false,
    );
    h.wait_until(&client, "the second answer", |_, seen| {
        seen.results.len() == 2
    });
    assert_eq!(client.results(), ["failed", "succeeded"]);
    assert_eq!(head(&h, "headless-1").0.scale, 2.0);
    let _ = std::fs::remove_file(path);
}

/// Displays that would cover the same points are refused as a whole, tested or applied, and nothing moves.
#[test]
fn overlapping_displays_are_refused_and_nothing_moves() {
    let Some(mut h) = Harness::new() else { return };
    let client = Client::connect(h.socket_path());
    h.wait_until(&client, "both heads", |_, seen| listed(seen));
    let right = h.output_rect("headless-2");
    let over = HeadChange {
        position: Some((LEFT.0 / 2, 0)),
        ..HeadChange::default()
    };
    let unchanged = Some(HeadChange::default());
    client.configure(
        client.serial(),
        vec![("headless-1", unchanged), ("headless-2", Some(over))],
        true,
    );
    client.configure(
        client.serial(),
        vec![("headless-1", unchanged), ("headless-2", Some(over))],
        false,
    );
    h.wait_until(&client, "both answers", |_, seen| seen.results.len() == 2);
    assert_eq!(client.results(), ["failed", "failed"]);
    assert_eq!(h.output_rect("headless-2"), right);
    // touching along an edge is an arrangement
    let below = HeadChange {
        position: Some((0, LEFT.1)),
        ..HeadChange::default()
    };
    client.configure(
        client.serial(),
        vec![("headless-1", unchanged), ("headless-2", Some(below))],
        false,
    );
    h.wait_until(&client, "the third answer", |_, seen| {
        seen.results.len() == 3
    });
    assert_eq!(client.results(), ["failed", "failed", "succeeded"]);
    assert_eq!(
        h.output_rect("headless-2"),
        Some((0.0, LEFT.1 as f64, RIGHT.0 as f64, RIGHT.1 as f64))
    );
}
