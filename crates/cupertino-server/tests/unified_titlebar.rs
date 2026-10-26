//! The title bar style a first-party client chooses with `cupertino_shell_v1.set_titlebar_style`,
//! against a headless compositor: the unified window's surface fills the whole frame, its window
//! control buttons take the pointer over the surface and the rest of its top strip goes to the
//! client, while a standard window keeps the server's title bar above the surface. The clients are
//! the toolkit the shell processes use. Tests skip (pass) when no EGL device is available.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_client::{Client, Connection, Event, TitlebarStyle, ToplevelConfig};
use cupertino_decorations::{ButtonKind, Hit, TitlebarStyle as ServerStyle};
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::state::WindowTarget;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Point;

const OUTPUT: (i32, i32) = (1000, 700);
const SIZE: (u32, u32) = (500, 300);
/// Centre of the close button of a 52 pt unified bar: the 16 pt cell centred in the bar.
const UNIFIED_CLOSE_Y: f64 = 26.0;

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
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
        match headless::init(&mut state, OUTPUT, 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&Cupertino) -> bool) {
        let end = Instant::now() + Duration::from_secs(10);
        while Instant::now() < end {
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

    fn socket_path(&self) -> PathBuf {
        let dir = std::env::var_os("XDG_RUNTIME_DIR").expect("XDG_RUNTIME_DIR");
        PathBuf::from(dir).join(self.state.socket_name.as_ref().unwrap())
    }
}

/// A toolkit client with one window of `style`, drawn white on every request, until dropped.
struct App {
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf, app_id: &str, style: TitlebarStyle) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let app_id = app_id.to_string();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(
                UnixStream::connect(socket).expect("connecting to the compositor"),
            )
            .unwrap();
            let mut client = Client::from_connection(conn).unwrap();
            let window = client
                .toplevel(&ToplevelConfig {
                    app_id,
                    title: "Window".into(),
                    size: SIZE,
                    min_size: (100, 100),
                    titlebar: style,
                })
                .unwrap();
            while !flag.load(Ordering::SeqCst) {
                for event in client.wait(Some(Duration::from_millis(10))).unwrap() {
                    if matches!(event, Event::Draw(s) if s == window) {
                        client
                            .draw(window, |pm, _| pm.fill(tiny_skia::Color::WHITE))
                            .unwrap();
                    }
                }
            }
        });
        App { stop }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[test]
fn a_unified_window_fills_its_frame_and_only_its_buttons_take_the_pointer_over_the_surface() {
    let Some(mut h) = Harness::new() else { return };
    // the standard window first: the unified one maps above it and covers the points the test probes;
    // one application, so the unified window cascades 29 pt right and down and leaves the standard
    // title bar uncovered
    let _standard = App::spawn(
        h.socket_path(),
        "org.example.titlebars",
        TitlebarStyle::Standard,
    );
    h.wait_until("the standard window to map", |s| {
        s.windows.iter().any(|w| w.mapped)
    });
    let _unified = App::spawn(
        h.socket_path(),
        "org.example.titlebars",
        TitlebarStyle::Unified,
    );
    h.wait_until("both windows to map", |s| {
        s.windows.iter().filter(|w| w.mapped).count() == 2
    });
    // let the open transforms end, so the frames are where they will stay
    let t = h.state.now() + 5.0;
    h.state.fixed_time = Some(t);
    h.state.tick();

    let entry_of = |style: ServerStyle| {
        h.state
            .windows
            .iter()
            .find(|w| w.titlebar == style)
            .expect("a window of the style")
    };
    let (unified, standard) = (
        entry_of(ServerStyle::Unified),
        entry_of(ServerStyle::Standard),
    );
    assert_eq!(
        unified.insets.vertical(),
        0.0,
        "the unified surface starts at the top edge"
    );
    assert_eq!(standard.insets.vertical(), 28.0);
    let (unified_id, standard_id) = (unified.id, standard.id);
    let frame_of = |id| h.state.wm.window(id).unwrap().geometry();
    assert_eq!(
        (frame_of(unified_id).w, frame_of(unified_id).h),
        (f64::from(SIZE.0), f64::from(SIZE.1))
    );
    assert_eq!(
        (frame_of(standard_id).w, frame_of(standard_id).h),
        (f64::from(SIZE.0), f64::from(SIZE.1) + 28.0)
    );

    let layout =
        h.state
            .decor
            .for_style(ServerStyle::Unified)
            .layout(cupertino_geometry::Size::new(
                f64::from(SIZE.0),
                f64::from(SIZE.1),
            ));
    let close_x = layout.button(ButtonKind::Close).center.x;
    let at = |id, x: f64, y: f64| {
        let f = frame_of(id);
        h.state.window_target_under(Point::from((f.x + x, f.y + y)))
    };
    match at(unified_id, close_x, UNIFIED_CLOSE_Y) {
        Some(WindowTarget::Decoration {
            id,
            hit: Hit::Button(ButtonKind::Close),
            ..
        }) => assert_eq!(id, unified_id),
        other => panic!("the close button of the unified bar takes the pointer: {other:?}"),
    }
    assert!(
        matches!(at(unified_id, 300.0, 10.0), Some(WindowTarget::Surface { id, .. }) if id == unified_id),
        "the rest of the unified strip belongs to the client's surface"
    );
    assert!(
        matches!(
            at(standard_id, 300.0, 10.0),
            Some(WindowTarget::Decoration {
                hit: Hit::Titlebar,
                ..
            })
        ),
        "the standard title bar is the server's"
    );
}
