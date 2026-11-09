//! A headless compositor on a fixed model clock with real toplevel clients, to look at what it composes.
#![allow(dead_code)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use cupertino_server::backend::headless;
use cupertino_server::config::Config;
use cupertino_server::Cupertino;
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;

pub const SIZE: (i32, i32) = (1000, 700);

pub struct Desktop {
    pub state: Cupertino,
    pub event_loop: EventLoop<'static, Cupertino>,
    pub clients: Vec<super::Client>,
    pub scale: f64,
}

impl Desktop {
    pub fn new(scale: f64) -> Self {
        Self::sized(SIZE, scale)
    }

    pub fn sized(size: (i32, i32), scale: f64) -> Self {
        Self::with_displays(
            &[headless::HeadlessDisplay {
                name: "headless-1".into(),
                size,
                physical_mm: (0, 0),
            }],
            scale,
        )
    }

    pub fn with_displays(displays: &[headless::HeadlessDisplay], scale: f64) -> Self {
        Self::configured(Config::default(), displays, scale)
    }

    pub fn configured(config: Config, displays: &[headless::HeadlessDisplay], scale: f64) -> Self {
        let event_loop = EventLoop::try_new().unwrap();
        let mut state = Cupertino::new(
            Display::new().unwrap(),
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            true,
            &cupertino_assets::Assets::builtin(),
        )
        .unwrap();
        state.fixed_time = Some(0.0);
        headless::init_displays(&mut state, displays, Some(scale))
            .expect("these tests require an offscreen EGL renderer");
        Self {
            state,
            event_loop,
            clients: Vec::new(),
            scale,
        }
    }

    pub fn socket(&self) -> PathBuf {
        PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap())
    }

    pub fn pump(&mut self) {
        headless::step(
            &mut self.state,
            &mut self.event_loop,
            Duration::from_millis(2),
        )
        .unwrap();
    }

    pub fn window(&mut self) {
        let expected = self.clients.len() + 1;
        let socket = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR").unwrap())
            .join(self.state.socket_name.as_ref().unwrap());
        self.clients.push(super::Client::toplevel(socket));
        let deadline = Instant::now() + super::HANG_GUARD;
        while self.state.wm.windows().count() != expected {
            self.pump();
            assert!(Instant::now() < deadline, "the test client did not map");
        }
    }

    pub fn at(&mut self, milliseconds: u32) {
        super::set_time(
            &mut self.state,
            &mut self.event_loop,
            f64::from(milliseconds) / 1000.0,
        );
        self.state.tick();
        self.pump();
    }

    /// Lock the session with a client that covers the output.
    pub fn lock(&mut self) -> super::Client {
        let locker = super::Client::locker(super::session_agent(&self.state));
        let deadline = Instant::now() + super::HANG_GUARD;
        while !locker.locked() {
            self.pump();
            assert!(Instant::now() < deadline, "the session did not lock");
        }
        locker
    }
}

/// With `CUPERTINO_FRAME_COUNTS` set to a file, each desktop appends its outputs' frame work there when it goes: the
/// sum over a suite compares pacing changes by counts.
impl Drop for Desktop {
    fn drop(&mut self) {
        let Some(path) = std::env::var_os("CUPERTINO_FRAME_COUNTS") else {
            return;
        };
        let (composes, renders) = self.state.outputs.iter().fold((0, 0), |(c, r), o| {
            let n = o.scheduler.counts();
            (c + n.composes, r + n.renders)
        });
        let line = format!("{composes} {renders}\n");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
    }
}
