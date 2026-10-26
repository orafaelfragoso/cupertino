//! Layer surfaces created without an output, against a headless compositor with two offscreen
//! displays: they go to the output under the pointer. The client is the toolkit the launcher uses,
//! creating its panel the way the launcher does. Tests skip (pass) when no EGL device is available.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use cupertino_client::{
    Anchor, Client, Event, KeyboardInteractivity, Layer, LayerSurfaceConfig, Margin, OutputGeometry,
};
use cupertino_server::backend::headless::{self, HeadlessDisplay};
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::desktop::layer_map_for_output;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use wayland_client::Connection;

const LEFT: (i32, i32) = (1000, 700);
const RIGHT: (i32, i32) = (800, 600);
const PANEL_NAMESPACE: &str = "cupertino-launcher";

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    /// A compositor with `headless-1` (`LEFT` pixels) and, right of it, `headless-2` (`RIGHT`), unscaled.
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            Config::default(),
            true,
            &cupertino_assets::Assets::discover(),
        )
        .ok()?;
        let displays = [
            HeadlessDisplay {
                name: "headless-1".into(),
                size: LEFT,
                physical_mm: (0, 0),
            },
            HeadlessDisplay {
                name: "headless-2".into(),
                size: RIGHT,
                physical_mm: (0, 0),
            },
        ];
        match headless::init_displays(&mut state, &displays, Some(1.0)) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }

    /// Run the compositor until `received` delivers.
    fn wait_for<T>(&mut self, what: &str, received: &mpsc::Receiver<T>) -> T {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
            if let Ok(value) = received.try_recv() {
                return value;
            }
        }
        panic!("timed out waiting for {what}");
    }

    /// Namespaces of the layer surfaces on the output named `name`.
    fn layers_on(&self, name: &str) -> Vec<String> {
        let entry = self
            .state
            .outputs
            .iter()
            .find(|o| o.output.name() == name)
            .expect("the output");
        layer_map_for_output(&entry.output)
            .layers()
            .map(|l| l.namespace().to_string())
            .collect()
    }
}

/// Connect a toolkit client that creates a launcher-like panel without an output and reports the
/// geometry of the outputs the server put it on, once configured. It stays connected until the
/// compositor goes away.
fn spawn_panel_client(socket: PathBuf) -> mpsc::Receiver<Vec<OutputGeometry>> {
    let (report, received) = mpsc::channel();
    std::thread::spawn(move || {
        let conn = Connection::from_socket(
            UnixStream::connect(&socket).expect("connecting to the compositor"),
        )
        .unwrap();
        let mut client = Client::from_connection(conn).unwrap();
        let panel = client
            .layer_surface(&LayerSurfaceConfig {
                namespace: PANEL_NAMESPACE.to_string(),
                output: None,
                layer: Layer::Overlay,
                anchor: Anchor::Top,
                size: (400, 300),
                margin: Margin {
                    top: 100,
                    ..Margin::default()
                },
                exclusive_zone: -1,
                keyboard: KeyboardInteractivity::None,
            })
            .unwrap();
        let mut reported = false;
        while let Ok(events) = client.wait(Some(Duration::from_millis(20))) {
            for event in events {
                if !reported
                    && event
                        == (Event::Configured {
                            surface: panel,
                            size: (400, 300),
                        })
                {
                    let ids = client.surface_outputs(panel).unwrap();
                    let outputs = client.outputs();
                    let geometries = ids
                        .iter()
                        .map(|id| {
                            outputs
                                .iter()
                                .find(|o| o.id == *id)
                                .and_then(|o| o.geometry)
                                .expect("a described output")
                        })
                        .collect();
                    report.send(geometries).unwrap();
                    reported = true;
                }
            }
        }
    });
    received
}

#[test]
fn a_panel_without_an_output_maps_on_the_output_under_the_pointer() {
    let Some(mut h) = Harness::new() else { return };
    h.state
        .pointer_moved_to((f64::from(LEFT.0) + 100.0, 200.0).into(), 0, None);
    let placed = spawn_panel_client(h.socket_path());
    let geometries = h.wait_for("the panel to be configured", &placed);
    assert_eq!(
        geometries,
        vec![OutputGeometry {
            x: LEFT.0,
            y: 0,
            width: RIGHT.0 as u32,
            height: RIGHT.1 as u32
        }],
        "the client is told the second output"
    );
    assert_eq!(h.layers_on("headless-2"), [PANEL_NAMESPACE]);
    assert!(h.layers_on("headless-1").is_empty());
}
