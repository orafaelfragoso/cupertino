//! A popup of an application window with a material backdrop is drawn over that material (a
//! context menu's backdrop), against a headless compositor. The client is the toolkit the shell
//! processes use. Tests skip (pass) when no EGL device is available.

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cupertino_client::{Button, Client, Connection, Event, PointerEvent, TitlebarStyle, ToplevelConfig};
use cupertino_geometry::Rect;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::backend::input::ButtonState;
use smithay::input::pointer::CursorImageStatus;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

const OUTPUT: (i32, i32) = (1000, 700);
const SIZE: (u32, u32) = (500, 300);
/// The popup in the window surface's coordinates.
const POPUP: (f64, f64, f64, f64) = (100.0, 100.0, 160.0, 80.0);

struct Harness {
    state: Cupertino,
    event_loop: EventLoop<'static, Cupertino>,
}

impl Harness {
    fn new() -> Option<Harness> {
        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().ok()?;
        let display: Display<Cupertino> = Display::new().ok()?;
        let mut state = Cupertino::new(display, event_loop.handle(), event_loop.get_signal(), Config::default(), true, &cupertino_assets::Assets::discover()).ok()?;
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
            headless::step(&mut self.state, &mut self.event_loop, Duration::from_millis(5)).unwrap();
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

    fn screenshot(&mut self) -> image::RgbaImage {
        let path = std::env::temp_dir().join(format!("cupertino-test-{}-window-popup.png", std::process::id()));
        self.state.screenshot_requests.push(path.clone());
        self.state.request_redraw();
        let p = path.clone();
        self.wait_until("the screenshot", move |_| p.exists());
        let img = image::open(&path).unwrap().to_rgba8();
        let _ = std::fs::remove_file(path);
        img
    }
}

/// A client with one white unified window that opens a popup with a `Menu` material backdrop once it has
/// had a button press (the popup needs an input serial).
struct App {
    stop: Arc<AtomicBool>,
}

impl App {
    fn spawn(socket: PathBuf) -> App {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        std::thread::spawn(move || {
            let conn = Connection::from_socket(UnixStream::connect(socket).expect("connecting to the compositor")).unwrap();
            let mut client = Client::from_connection(conn).unwrap();
            let window = client
                .toplevel(&ToplevelConfig { app_id: "org.example.popup".into(), title: "Window".into(), size: SIZE, min_size: (100, 100), titlebar: TitlebarStyle::Unified })
                .unwrap();
            let mut popup = None;
            let mut backdrop = None;
            while !flag.load(Ordering::SeqCst) {
                for event in client.wait(Some(Duration::from_millis(10))).unwrap() {
                    match event {
                        Event::Draw(s) if s == window => client.draw(window, |pm, _| pm.fill(tiny_skia::Color::WHITE)).unwrap(),
                        Event::Draw(s) if Some(s) == popup => client.draw(s, |pm, _| pm.fill(tiny_skia::Color::TRANSPARENT)).unwrap(),
                        Event::Pointer { event: PointerEvent::Button { button: Button::Left, pressed: true, .. }, .. } if popup.is_none() => {
                            let (x, y, w, h) = POPUP;
                            let surface = client.popup(window, &Rect::new(x, y, w, h)).unwrap();
                            let b = client.backdrop(surface).unwrap();
                            b.set_material("Menu");
                            b.set_shape(&Rect::new(0.0, 0.0, w, h), 10.0).unwrap();
                            popup = Some(surface);
                            backdrop = Some(b);
                        }
                        _ => {}
                    }
                }
            }
            drop(backdrop);
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
fn a_window_popup_draws_its_material_over_the_window_and_only_there() {
    let Some(mut h) = Harness::new() else { return };
    h.state.cursor_status = CursorImageStatus::Hidden;
    let _app = App::spawn(h.socket_path());
    h.wait_until("the window to map", |s| s.windows.iter().any(|w| w.mapped));
    let t = h.state.now() + 5.0;
    h.state.fixed_time = Some(t);
    h.state.tick();
    let id = h.state.windows.iter().find(|w| w.mapped).expect("a mapped window").id;
    let frame = h.state.wm.window(id).unwrap().geometry();
    // a press inside the window gives the client the serial its popup needs
    h.state.pointer_moved_to((frame.x + 400.0, frame.y + 200.0).into(), 0, None);
    h.state.pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Pressed, 0);
    h.state.pointer_button(cupertino_server::events::BTN_LEFT, ButtonState::Released, 0);
    for _ in 0..200 {
        headless::step(&mut h.state, &mut h.event_loop, Duration::from_millis(5)).unwrap();
    }
    let shot = h.screenshot();
    let (px, py, pw, ph) = POPUP;
    let at = |x: f64, y: f64| shot.get_pixel((frame.x + x) as u32, (frame.y + y) as u32).0;
    let inside = at(px + pw / 2.0, py + ph / 2.0);
    let outside = at(px - 30.0, py + ph / 2.0);
    assert_eq!(outside[..3], [255, 255, 255], "the window is white outside the popup");
    assert!(inside[..3].iter().all(|c| *c < 250), "the popup's material darkens the white window under it: {inside:?}");
}
