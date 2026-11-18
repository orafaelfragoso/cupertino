//! A window with a raised layer and a surface layer, drawn through the Wayland window system on a protocol server of
//! its own: the layers are drawn with the window's frame away from the UI thread, and commit before the window in the
//! same hand-over, so the compositor shows them together; a layer's renderer that panics there panics the instance
//! that submitted the frame, as the window's own renderer does. A frame the renderer cannot draw straight into the
//! window's buffers is drawn away from the UI thread too, and copied into them there.

#[allow(dead_code)]
mod support;

#[path = "../../cupertino-wayland-client/tests/support/mod.rs"]
mod wire;

use cupertino_diagnostic_events::{self as diag, Record};
use cupertino_keyboard::modifiers::Profile;
use cupertino_kit::controls::WINDOW_CONTENT_INSET;
use cupertino_kit::core::{
    Change, CommandRegistry, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx,
    SurfaceLayerMark, Widget,
};
use cupertino_kit::geometry::{Rect, Size};
use cupertino_kit::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_kit::platform::{DecorationMode, SurfaceLayerLook};
use cupertino_kit::prelude::*;
use cupertino_kit::render::{Paint, PaintOp, RenderRequest, Shape, UiRenderer};
use cupertino_kit::test::Runtime;
use cupertino_kit::theme::Color;
use cupertino_kit::Instance;
use cupertino_ui_render::{CpuFrame, MemoryTarget, PixelRect, RenderError, RenderFrameId};
use cupertino_ui_wayland::WaylandWindowSystem;
use std::cell::Cell;
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::ThreadId;
use std::time::{Duration, Instant};

/// The key of the window's surface layer.
const LAYER_KEY: u64 = 7;

/// The side of each square, in points.
const SIDE: f64 = 24.0;

thread_local! {
    /// The side of the square in the surface layer, in points, on this thread's runtime.
    static LAYER_SIDE: Cell<f64> = const { Cell::new(SIDE) };
    /// Whether the raised square is raised, as this thread's runtime lays it out.
    static RAISED: Cell<bool> = const { Cell::new(true) };
}

/// A square of one colour, `side` points large: raised, in a surface layer of its own, or neither.
struct Square {
    side: f64,
    raised: bool,
    layer: Option<u64>,
    color: Color,
}

impl<M: 'static> Widget<M> for Square {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.side),
            height: SizeSpec::Fixed(self.side),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.side, self.side),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        cx.push(PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, self.side, self.side)),
            paint: Paint::Solid(self.color),
        });
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn raised(&self, _: &()) -> bool {
        self.raised && RAISED.get()
    }

    fn surface_layer(&self, _: &()) -> Option<SurfaceLayerMark> {
        self.layer.map(|key| SurfaceLayerMark {
            key,
            look: SurfaceLayerLook::default(),
        })
    }
}

/// One window with a raised square, a square in a surface layer and one in the window's content.
struct Layered;

impl Application for Layered {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), (): ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let square = |raised, layer: Option<u64>, color| {
            Element::new(Square {
                side: layer.map_or(SIDE, |_| LAYER_SIDE.get()),
                raised,
                layer,
                color,
            })
        };
        vec![Window {
            key: WindowKey("layered".into()),
            spec: WindowSpec {
                title: "Layered".into(),
                app_id: "org.cupertino.tests.Layered".into(),
                size: Size::new(160.0, 120.0),
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
                spacing: 4.0,
                align: Align::Start,
                children: vec![
                    square(true, None, Color::new(1.0, 0.0, 0.0, 1.0)),
                    square(false, Some(LAYER_KEY), Color::new(0.0, 1.0, 0.0, 1.0)),
                    square(false, None, Color::new(0.0, 0.0, 1.0, 1.0)),
                ],
            }),
            menus: None,
        }]
    }
}

/// A CPU renderer that notes the thread of every frame it draws and, if it `breaks`, panics drawing the surface
/// layer's (a frame as wide as a square).
struct Noted {
    inner: Box<dyn UiRenderer<Frame = CpuFrame> + Send>,
    threads: Arc<Mutex<Vec<ThreadId>>>,
    breaks: bool,
    /// Whether it draws into memory, where the CPU renderer it wraps does.
    into_memory: bool,
}

impl Noted {
    fn note(&self, request: &RenderRequest<'_>) {
        self.threads
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(std::thread::current().id());
        let layer = f64::from(request.width) == SIDE * request.scale;
        assert!(!(self.breaks && layer), "the layer's renderer broke");
    }
}

/// A runtime of [`Layered`] on the connection `conn` whose renderers note the threads they draw on into `threads`;
/// with `breaking`, the surface layer's renderer panics as it draws, and without `into_memory` none draws into memory.
fn layered(
    conn: cupertino_wayland_client::Connection,
    threads: &Arc<Mutex<Vec<ThreadId>>>,
    breaking: bool,
    into_memory: bool,
) -> Runtime<Layered, WaylandWindowSystem> {
    let mut options = support::options(support::text_system());
    let mut cpu = options.renderers;
    let noted = Arc::clone(threads);
    options.renderers = Box::new(move || {
        Box::new(Noted {
            inner: cpu(),
            threads: Arc::clone(&noted),
            breaks: breaking,
            into_memory,
        })
    });
    let system = WaylandWindowSystem::new(conn, Profile::cupertino());
    Runtime::new(system, options)
}

/// What a server needs to offer surface layers, raised layers and shared memory in the renderer's byte order.
const LAYERING: wire::Offer = wire::Offer {
    layers: Some(6),
    app_effects: Some(21),
    releases: true,
    subsurfaces: true,
    ..wire::Offer::NONE
};

/// A server that offers surface layers, raised layers and shared memory in the renderer's byte order.
fn layering_server() -> (wire::Harness, cupertino_wayland_client::Connection) {
    wire::Harness::start(LAYERING)
}

impl UiRenderer for Noted {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        self.note(request);
        self.inner.render(request)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.inner.frame(id)
    }

    fn draws_into_memory(&self) -> bool {
        self.into_memory && self.inner.draws_into_memory()
    }

    fn render_into(
        &mut self,
        request: &RenderRequest<'_>,
        target: MemoryTarget<'_>,
    ) -> Result<(RenderFrameId, Vec<PixelRect>), RenderError> {
        self.note(request);
        self.inner.render_into(request, target)
    }
}

/// Runs `runtime` until the server has `count` commits that showed a buffer.
fn commits_reach(
    server: &wire::Harness,
    runtime: &mut Runtime<Layered, WaylandWindowSystem>,
    count: usize,
) {
    let end = Instant::now() + support::PATIENCE;
    while server.commits().len() < count {
        runtime.step(NonZeroU32::new(64).unwrap()).unwrap();
        assert!(
            Instant::now() < end,
            "the server saw {:?}, not {count} commits",
            server.commits()
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_raised_layer_and_a_surface_layer_commit_before_their_window_in_its_hand_over() {
    let (server, conn) = layering_server();
    let threads = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = layered(conn, &threads, false, true);
    commits_reach(&server, &mut runtime, 3);
    assert_eq!(
        server.commits()[..3],
        ["layer", "subsurface", "window"],
        "the surface layer and the raised layer commit before their window"
    );
    let ui = std::thread::current().id();
    let threads = threads.lock().unwrap_or_else(PoisonError::into_inner);
    assert!(
        threads.len() >= 3,
        "the window and its layers were drawn: {threads:?}"
    );
    assert!(
        threads.iter().all(|&t| t != ui),
        "every frame was drawn away from the UI thread: {threads:?}"
    );
}

/// Runs `runtime` until `done` holds for what the server saw of every commit, and returns that.
fn every_commit_until(
    server: &wire::Harness,
    runtime: &mut Runtime<Layered, WaylandWindowSystem>,
    done: impl Fn(&[String]) -> bool,
) -> Vec<String> {
    let end = Instant::now() + support::PATIENCE;
    loop {
        let seen = server.every_commit();
        if done(&seen) {
            return seen;
        }
        runtime.step(NonZeroU32::new(64).unwrap()).unwrap();
        assert!(Instant::now() < end, "the server saw {seen:?}");
        std::thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn a_raised_layer_hidden_by_the_frame_a_configure_brings_stays_hidden() {
    let (server, conn) = wire::Harness::start(wire::Offer {
        frames: true,
        ..LAYERING
    });
    let threads = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = layered(conn, &threads, false, true);
    // the first frame, which shows the raised layer, is away when the configure of the frame that hides it arrives
    let end = Instant::now() + support::PATIENCE;
    while threads
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .is_empty()
    {
        runtime.step(NonZeroU32::new(1).unwrap()).unwrap();
        assert!(Instant::now() < end, "the first frame is drawn");
        std::thread::sleep(Duration::from_millis(1));
    }
    RAISED.set(false);
    server.run(|s| {
        s.resize(0, 200, 140);
    });
    let hidden = |seen: &[String]| seen.iter().position(|c| c == "subsurface unmapped");
    let seen = every_commit_until(&server, &mut runtime, |seen| {
        hidden(seen).is_some_and(|at| seen[at..].iter().any(|c| c == "window"))
    });
    let at = hidden(&seen).unwrap();
    assert!(
        seen[..at].iter().any(|c| c == "subsurface"),
        "the first frame shows the raised layer: {seen:?}"
    );
    assert!(
        !seen[at..].iter().any(|c| c == "subsurface"),
        "no frame of the raised layer commits after the frame that hides it: {seen:?}"
    );
}

#[test]
fn a_surface_layer_of_a_fractional_size_is_drawn_at_the_size_its_buffers_are() {
    // at scale 2 a layer 24.4 points large is drawn at 24 whole points, as its surface's buffers are
    LAYER_SIDE.set(24.4);
    let (server, conn) = wire::Harness::start(wire::Offer {
        scale: Some(2),
        ..LAYERING
    });
    let threads = Arc::new(Mutex::new(Vec::new()));
    let mut runtime = layered(conn, &threads, false, true);
    commits_reach(&server, &mut runtime, 3);
    assert_eq!(
        server.commits()[..3],
        ["layer", "subsurface", "window"],
        "the layer and its window show"
    );
}

#[test]
fn a_panic_drawing_a_layer_away_from_the_ui_thread_is_the_submitting_instances() {
    diag::init(diag::component!(), "kit_test", diag::Sink::Memory);
    let ui = std::thread::Builder::new()
        .name("layered-ui".into())
        .spawn(move || {
            let _layered = Instance::new("layered").enter();
            let (server, conn) = layering_server();
            let threads = Arc::new(Mutex::new(Vec::new()));
            let mut runtime = layered(conn, &threads, true, true);
            commits_reach(&server, &mut runtime, 1);
        })
        .unwrap();
    let payload = ui.join().expect_err("the instance panics");
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    assert_eq!(message, Some("the layer's renderer broke"));
    let panics: Vec<Record> = diag::recent()
        .iter()
        .map(|line| Record::parse(line.trim_end()).unwrap())
        .filter(|r| r.id == "layered.panic")
        .collect();
    assert_eq!(panics.len(), 1, "{panics:?}");
    let thread = panics[0].fields.get("thread").and_then(|v| v.as_str());
    assert!(
        thread.is_some_and(|t| t.starts_with("kit-cpu")),
        "recorded on the pool thread that drew the layer: {thread:?}"
    );
}

#[test]
fn a_frame_not_drawn_straight_into_its_buffer_is_drawn_and_copied_away_from_the_ui_thread() {
    // (whether the server takes buffers in the renderer's byte order, whether the renderer draws into memory)
    for (abgr, into_memory) in [(false, true), (true, false)] {
        let (server, conn) = wire::Harness::start(wire::Offer {
            releases: abgr,
            ..wire::Offer::NONE
        });
        let threads = Arc::new(Mutex::new(Vec::new()));
        let mut runtime = layered(conn, &threads, false, into_memory);
        commits_reach(&server, &mut runtime, 1);
        assert_eq!(server.commits(), ["window"], "{abgr} {into_memory}");
        let ui = std::thread::current().id();
        let threads = threads.lock().unwrap_or_else(PoisonError::into_inner);
        assert!(
            !threads.is_empty() && threads.iter().all(|&t| t != ui),
            "the frame was drawn away from the UI thread ({abgr} {into_memory}): {threads:?}"
        );
    }
}
