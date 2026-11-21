//! The server side of the tests: the real compositor, headless, drawing one offscreen output with the software
//! rasteriser, whose supervisor starts this test binary as its host program ([`super::host::MARK`]). The tests read
//! the server's own state and the output's pixels; Smithay stays behind the server ([`cupertino-layers`]).

use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use calloop::EventLoop;
use cupertino_assets::Assets;
use cupertino_server::backend::headless;
use cupertino_server::host::{HostKind, HostView};
use cupertino_server::state::{insert_client, ClientState};
use cupertino_server::supervisor::{HostProgram, ProcessInfo, RestartError};
use cupertino_server::{Config, Cupertino};
use wayland_server::Display;

use super::host::MARK;

/// The offscreen output's size in pixels, at scale 1.
pub const OUTPUT_SIZE: (i32, i32) = (800, 600);

/// How long a test waits for the server, its hosts and their components to do what it expects: a debug build drawing
/// with the software rasteriser on a loaded builder.
const WAIT: Duration = Duration::from_secs(60);

/// How long one turn of the server's loop waits for an event at most.
const TURN: Duration = Duration::from_millis(5);

/// The factor for the server's restart delays (its backoffs and quiet moments): a twentieth, so a test's rows of crashes
/// wait milliseconds instead of seconds while the server still counts and orders them as it does in a session.
const TIME_SCALE: f64 = 0.05;

/// The share of an area's pixels that must show a colour for the area to show it: all but antialiased edges.
const COVERED: f64 = 0.9;

/// A rectangle of the output, in pixels (points at scale 1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Area {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl Area {
    pub const fn new(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { x, y, w, h }
    }

    /// The whole output.
    pub const fn output() -> Self {
        Self::new(0, 0, OUTPUT_SIZE.0, OUTPUT_SIZE.1)
    }

    /// How many of its pixels must show a colour for the area to show it ([`COVERED`]).
    pub fn covered(&self) -> usize {
        (f64::from(self.w * self.h) * COVERED) as usize
    }

    /// Whether the area and `other` share a pixel.
    pub fn meets(&self, other: Area) -> bool {
        self.x < other.x + other.w
            && other.x < self.x + self.w
            && self.y < other.y + other.h
            && other.y < self.y + self.h
    }
}

/// The server, and the report its hosts write to.
pub struct Rig {
    event_loop: EventLoop<'static, Cupertino>,
    pub state: Cupertino,
    report: PathBuf,
}

impl Rig {
    /// A server whose shell is `components`, those of `alone` in separate hosts, run by this test binary as the host
    /// program, its first supervision pass run; `name` names the report.
    pub fn new(name: &str, components: &[&str], alone: &[&str]) -> Self {
        // the software rasteriser, as the server's program tests run the headless server
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        // the restart delays on the test's clock, which the hosts this rig starts inherit
        std::env::set_var(
            cupertino_server::restart_policy::TEST_TIME_SCALE_ENV,
            TIME_SCALE.to_string(),
        );
        let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("shell-host");
        std::fs::create_dir_all(&dir).expect("the report's folder");
        let report = dir.join(format!("{name}-{}.report", std::process::id()));
        let _ = std::fs::remove_file(&report);

        let event_loop: EventLoop<'static, Cupertino> = EventLoop::try_new().unwrap();
        let display: Display<Cupertino> = Display::new().unwrap();
        let config = Config {
            shell_components: components.iter().map(|c| c.to_string()).collect(),
            shell_alone: alone.iter().map(|c| c.to_string()).collect(),
            ..Config::default()
        };
        let mut state = Cupertino::new(
            display,
            event_loop.handle(),
            event_loop.get_signal(),
            config,
            false,
            &Assets::builtin(),
        )
        .expect("the server starts");
        headless::init(&mut state, OUTPUT_SIZE, 1.0).expect("an offscreen renderer");
        let args = ["--exact", "host_program", "--nocapture", "--", MARK];
        let mut args: Vec<String> = args.map(String::from).to_vec();
        args.push(report.display().to_string());
        state.supervisor.set_host_program(Some(HostProgram {
            path: std::env::current_exe().unwrap(),
            args,
            checked: false,
        }));
        state.apply_shell_plan();
        state.supervise_shell(Instant::now());
        Self {
            event_loop,
            state,
            report,
        }
    }

    /// One turn of the server's loop: clients and hosts are read and answered, the model advances, a due frame is
    /// drawn.
    pub fn turn(&mut self) {
        headless::step(&mut self.state, &mut self.event_loop, TURN).expect("the loop turns");
    }

    /// Turn until `done` holds, which is asked before every turn.
    pub fn until(&mut self, what: &str, mut done: impl FnMut(&mut Self) -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done(self) {
            assert!(
                Instant::now() < deadline,
                "timed out waiting until {what}; the hosts reported:\n{}",
                self.lines().join("\n")
            );
            self.turn();
        }
    }

    /// Turn until a host reported `line`.
    pub fn until_reported(&mut self, line: &str) {
        self.until(&format!("reported {line:?}"), |rig| rig.reported(line));
    }

    /// The lines the hosts reported so far.
    pub fn lines(&self) -> Vec<String> {
        std::fs::read_to_string(&self.report)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// Whether a host reported `line`.
    pub fn reported(&self, line: &str) -> bool {
        self.lines().iter().any(|l| l == line)
    }

    /// The host of `kind`.
    pub fn host(&self, kind: HostKind) -> Option<HostView> {
        self.state
            .hosts
            .host_views()
            .into_iter()
            .find(|h| h.kind == kind)
    }

    /// The process of the host of `kind`, once it runs and bound `cupertino_host_v1`.
    pub fn host_pid(&self, kind: HostKind) -> Option<u32> {
        self.host(kind).filter(|h| h.bound).and_then(|h| h.pid)
    }

    /// The output as it shows now, without the pointer.
    pub fn image(&mut self) -> image::RgbaImage {
        let mut backend = self.state.backend.take().unwrap();
        let image = self
            .state
            .draw_output_image(backend.renderer(), 0, false, false)
            .unwrap();
        self.state.backend = Some(backend);
        image
    }

    /// Whether `area` of the output shows a colour that passes `test` over at least `pixels` of its pixels.
    pub fn shows(&mut self, area: Area, test: impl Fn([u8; 3]) -> bool, pixels: usize) -> bool {
        count(&self.image(), area, test) >= pixels
    }

    /// A connection of an ordinary client.
    pub fn client(&mut self) -> UnixStream {
        let (ours, theirs) = UnixStream::pair().unwrap();
        insert_client(&self.state.display_handle, ours, ClientState::default()).unwrap();
        theirs
    }

    /// Ask for `component` to be started again, as `cupertino shell restart` does; the answer comes on the channel.
    pub fn restart(
        &mut self,
        component: &str,
    ) -> mpsc::Receiver<Result<ProcessInfo, RestartError>> {
        let (tx, rx) = mpsc::channel();
        self.state.restart_shell_process(
            component,
            Box::new(move |answer| {
                let _ = tx.send(answer);
            }),
        );
        rx
    }

    /// Turn until the restart answered.
    pub fn answer(
        &mut self,
        rx: &mpsc::Receiver<Result<ProcessInfo, RestartError>>,
    ) -> Result<ProcessInfo, RestartError> {
        let mut answer = None;
        self.until("the restart is answered", |_| {
            answer = rx.try_recv().ok();
            answer.is_some()
        });
        answer.unwrap()
    }
}

/// How many pixels of `image` in `area` pass `test` (on red, green, blue).
pub fn count(image: &image::RgbaImage, area: Area, test: impl Fn([u8; 3]) -> bool) -> usize {
    let (x0, y0) = (area.x.max(0) as u32, area.y.max(0) as u32);
    let (x1, y1) = (
        ((area.x + area.w).max(0) as u32).min(image.width()),
        ((area.y + area.h).max(0) as u32).min(image.height()),
    );
    let mut n = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let [r, g, b, _] = image.get_pixel(x, y).0;
            if test([r, g, b]) {
                n += 1;
            }
        }
    }
    n
}

/// The pixel shows one primary colour: channel `channel` bright, the others dark.
pub fn primary(channel: usize) -> impl Fn([u8; 3]) -> bool {
    move |rgb| {
        (0..3).all(|c| {
            if c == channel {
                rgb[c] > 200
            } else {
                rgb[c] < 60
            }
        })
    }
}

/// The pixel shows the magenta of the test window.
pub fn magenta(rgb: [u8; 3]) -> bool {
    rgb[0] > 120 && rgb[2] > 120 && rgb[1] < 60
}
