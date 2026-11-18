//! Two instances drawing their windows on the CPU pool through the Wayland window system, each on a UI thread and a
//! protocol server of its own: when one's renderer panics in a frame drawn away from its UI thread, the panic is that
//! instance's, recorded in its scope and resumed on its UI thread, and the other goes on drawing.

#[allow(dead_code)]
mod support;

#[path = "../../cupertino-wayland-client/tests/support/mod.rs"]
mod wire;

use cupertino_diagnostic_events::{self as diag, Record};
use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::controls::WINDOW_CONTENT_INSET;
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::DecorationMode;
use cupertino_kit::prelude::*;
use cupertino_kit::render::{RenderRequest, UiRenderer};
use cupertino_kit::test::Runtime;
use cupertino_kit::Instance;
use cupertino_ui_render::{CpuFrame, MemoryTarget, PixelRect, RenderError, RenderFrameId};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::num::NonZeroU32;
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The frame on which [`Breaks`] panics, counted from the window's first.
const BREAKS_AT: u32 = 2;

/// The frames the steady instance draws.
const STEADY_FRAMES: usize = 4;

/// How much wider, in points, each configure makes the window, so each draws a frame.
const WIDEN: i32 = 8;

/// One window with a label.
struct Still;

impl Application for Still {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey("still".into()),
            spec: WindowSpec {
                title: "Still".into(),
                app_id: "org.cupertino.tests.Still".into(),
                size: Size::new(160.0, 60.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::new(Stack {
                axis: Axis::Vertical,
                padding: WINDOW_CONTENT_INSET,
                spacing: 0.0,
                align: Align::Stretch,
                children: Vec::new(),
            }),
            menus: None,
        }]
    }
}

/// A CPU renderer that draws into the buffers it is lent and, given a frame to break at, panics drawing it.
struct Breaks {
    inner: Box<dyn UiRenderer<Frame = CpuFrame> + Send>,
    drawn: u32,
    breaks_at: Option<u32>,
}

impl UiRenderer for Breaks {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        self.inner.render(request)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.inner.frame(id)
    }

    fn draws_into_memory(&self) -> bool {
        self.inner.draws_into_memory()
    }

    fn render_into(
        &mut self,
        request: &RenderRequest<'_>,
        target: MemoryTarget<'_>,
    ) -> Result<(RenderFrameId, Vec<PixelRect>), RenderError> {
        self.drawn += 1;
        assert!(Some(self.drawn) != self.breaks_at, "the renderer broke");
        self.inner.render_into(request, target)
    }
}

/// An instance's window on a protocol server of its own, drawn by `Breaks`.
struct Drawn {
    server: wire::Harness,
    runtime: Runtime<Still, WaylandWindowSystem>,
    /// The width the server configured last, in points.
    width: i32,
}

impl Drawn {
    fn open(breaks_at: Option<u32>) -> Drawn {
        let (server, conn) = wire::Harness::start(wire::Offer {
            releases: true,
            ..wire::Offer::NONE
        });
        let mut options = support::options(support::text_system());
        let mut cpu = options.renderers;
        options.renderers = Box::new(move || {
            Box::new(Breaks {
                inner: cpu(),
                drawn: 0,
                breaks_at,
            })
        });
        let system = WaylandWindowSystem::new(conn, Profile::cupertino());
        let mut drawn = Drawn {
            server,
            runtime: Runtime::new(system, options),
            width: 160,
        };
        // the first commit maps the window; the second shows its first frame
        drawn.commits_reach(2);
        drawn
    }

    fn commits(&self) -> usize {
        self.server
            .presents()
            .iter()
            .filter(|p| *p == "Commit")
            .count()
    }

    /// Runs the window until the server has `count` commits of it.
    fn commits_reach(&mut self, count: usize) {
        let end = Instant::now() + support::PATIENCE;
        while self.commits() < count {
            self.runtime.step(NonZeroU32::new(64).unwrap()).unwrap();
            assert!(Instant::now() < end, "the window committed {count} frames");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The server configures the window at another width, and the window draws a frame for it.
    fn draw_again(&mut self) {
        let next = self.commits() + 1;
        self.width += WIDEN;
        let width = self.width;
        self.server.run(move |s| {
            s.resize(0, width, 60);
        });
        self.commits_reach(next);
    }
}

#[test]
fn a_panic_in_a_frame_drawn_on_the_cpu_pool_is_its_instances_and_the_other_goes_on_drawing() {
    diag::init(diag::component!(), "kit_test", diag::Sink::Memory);
    let (go_on, steady_waits) = mpsc::channel::<()>();
    let fragile = std::thread::Builder::new()
        .name("fragile-ui".into())
        .spawn(move || {
            let _fragile = Instance::new("fragile").enter();
            let mut drawn = Drawn::open(Some(BREAKS_AT));
            drawn.draw_again();
        })
        .unwrap();
    let steady = std::thread::Builder::new()
        .name("steady-ui".into())
        .spawn(move || {
            let _steady = Instance::new("steady").enter();
            let mut drawn = Drawn::open(None);
            for frame in 1..STEADY_FRAMES {
                if frame == 2 {
                    steady_waits.recv().unwrap();
                }
                drawn.draw_again();
            }
            drawn.commits()
        })
        .unwrap();
    let payload = fragile.join().expect_err("the fragile instance panics");
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("the renderer broke"));
    let panics: Vec<Record> = diag::recent()
        .iter()
        .map(|line| Record::parse(line.trim_end()).unwrap())
        .filter(|r| r.id.ends_with(".panic"))
        .collect();
    assert_eq!(panics.len(), 1, "{panics:?}");
    assert_eq!(panics[0].id, "fragile.panic");
    let thread = panics[0].fields.get("thread").and_then(|v| v.as_str());
    assert!(
        thread.is_some_and(|t| t.starts_with("kit-cpu")),
        "recorded on the pool thread that drew the frame: {thread:?}"
    );
    go_on.send(()).unwrap();
    let commits = steady.join().expect("the steady instance goes on");
    assert!(commits >= STEADY_FRAMES, "{commits}");
}
