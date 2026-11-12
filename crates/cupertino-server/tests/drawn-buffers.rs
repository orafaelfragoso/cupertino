//! A frame holds the client buffers it was drawn from, every surface of a window's tree included, so their explicit-sync
//! release points fire only once the frame is shown. Tests skip (pass) when no EGL device is available.

use crate::common;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use common::Client;
use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::frame::drawn_buffers;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::{Display, Resource};

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
        match headless::init(&mut state, (1000, 700), 1.0) {
            Ok(()) => Some(Harness { state, event_loop }),
            Err(e) => {
                eprintln!("skipping: no EGL device ({e:#})");
                None
            }
        }
    }

    fn wait_until(&mut self, what: &str, mut cond: impl FnMut(&mut Cupertino) -> bool) {
        let end = Instant::now() + common::HANG_GUARD;
        while Instant::now() < end {
            self.state.fixed_time = Some(100.0);
            headless::step(
                &mut self.state,
                &mut self.event_loop,
                Duration::from_millis(5),
            )
            .unwrap();
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
}

#[test]
fn a_frame_holds_the_buffers_of_a_window_and_its_subsurface() {
    let Some(mut h) = Harness::new() else { return };
    let mut renderer = headless::create_renderer().unwrap();
    let _game = Client::constraining_under_subsurface(h.socket_path(), "org.example.game");
    let mut held = Vec::new();
    h.wait_until("the window and its subsurface in a frame", |state| {
        let Some(composed) = state.compose_for_present(&mut renderer, 0) else {
            return false;
        };
        held = drawn_buffers(&composed.resources.surfaces);
        held.len() == 2
    });
    let ids: Vec<_> = held.iter().map(|b| b.id()).collect();
    assert_ne!(ids[0], ids[1], "two surfaces, two buffers");
}
