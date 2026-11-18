//! A window suspended while its frame is away on a busy render thread: the UI thread does not wait for the renderer
//! the render thread draws the frame with, nor turns its loop over while the window waits for the frame, and the
//! renderer lets go of its layers once the frame is shown.

#[allow(dead_code)]
mod support;

use cupertino_kit::controls::WINDOW_CONTENT_INSET;
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::core::StepStatus;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{
    Capabilities, DecorationMode, PlatformEvent, Shown, Submitted, Timestamp, WindowEvent,
    WindowStates,
};
use cupertino_kit::prelude::*;
use cupertino_kit::render::{
    render_thread, CpuFrame, MemoryTarget, PixelRect, RenderError, RenderFrameId, RenderRequest,
    UiRenderer,
};
use cupertino_ui_test::{ManualClock, TestDriver};
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc, PoisonError};
use std::time::Duration;

/// The time between two frames the test hands the window, in nanoseconds.
const FRAME_STEP: u64 = 16_000_000;

/// The most turns the window may take to rest.
const FRAMES: usize = 64;

/// How long the test waits for the UI thread to handle the suspension before it calls it stopped.
const STOPPED: Duration = Duration::from_secs(10);

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
                size: Size::new(200.0, 80.0),
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
                children: vec![Element::new(Button {
                    label: "Still".into(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Bordered,
                    enabled: true,
                    busy: false,
                    on_press: None,
                })],
            }),
            menus: None,
        }]
    }
}

/// A renderer on the CPU that, as one on the GPU does, waits for the render thread to let go of its layers.
struct Waits {
    inner: Box<dyn UiRenderer<Frame = CpuFrame> + Send>,
    dropped: Arc<AtomicUsize>,
}

impl UiRenderer for Waits {
    type Frame = CpuFrame;

    fn render(&mut self, request: &RenderRequest<'_>) -> Result<RenderFrameId, RenderError> {
        self.inner.render(request)
    }

    fn frame(&self, id: RenderFrameId) -> Result<&CpuFrame, RenderError> {
        self.inner.frame(id)
    }

    fn drop_layers(&mut self) {
        render_thread::run(|_| ());
        self.inner.drop_layers();
        self.dropped.fetch_add(1, Ordering::SeqCst);
    }

    fn render_into(
        &mut self,
        request: &RenderRequest<'_>,
        target: MemoryTarget<'_>,
    ) -> Result<(RenderFrameId, Vec<PixelRect>), RenderError> {
        self.inner.render_into(request, target)
    }
}

/// The frames the window system took and the ones it said were shown.
#[derive(Default)]
struct Frames {
    taken: AtomicUsize,
    shown: AtomicUsize,
}

impl Frames {
    fn away(&self) -> bool {
        self.taken.load(Ordering::SeqCst) > self.shown.load(Ordering::SeqCst)
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(1000).unwrap()
}

/// Hands the window the frames it asks for, waiting for those away, until it rests.
fn settle(driver: &mut TestDriver<Still>, frames: &Frames, at: &mut u64) {
    let window = driver.runtime().windows()[0].1;
    for _ in 0..FRAMES {
        driver.run_until_idle(turns()).unwrap();
        if frames.away() {
            assert!(
                driver.window_system_mut().wait_for_wake(support::PATIENCE),
                "the frame away was not shown"
            );
            continue;
        }
        if !driver.window_system_mut().frame_requested(window).unwrap() {
            return;
        }
        *at += FRAME_STEP;
        driver.advance_to(Timestamp(*at)).unwrap();
        driver
            .window_system_mut()
            .frame_ready(window, Timestamp(*at))
            .unwrap();
    }
    panic!("the window keeps asking for frames");
}

#[test]
fn a_window_suspended_while_its_frame_waits_for_a_busy_render_thread_goes_on() {
    let (handled, suspended) = mpsc::channel::<usize>();
    let (open, gate) = mpsc::channel::<()>();
    let ui = std::thread::Builder::new()
        .name("still-ui".into())
        .spawn(move || {
            let dropped = Arc::new(AtomicUsize::new(0));
            let mut options = support::options(support::text_system());
            let mut cpu = options.renderers;
            let counted = Arc::clone(&dropped);
            options.renderers = Box::new(move || {
                Box::new(Waits {
                    inner: cpu(),
                    dropped: Arc::clone(&counted),
                })
            });
            let mut driver = TestDriver::<Still>::new(
                Capabilities::default(),
                options,
                ManualClock::new(Timestamp(0)),
            );
            let frames = Arc::new(Frames::default());
            let counts = Arc::clone(&frames);
            // the frame drawn and shown on the render thread, the renderer let go before the window hears of it
            driver
                .window_system_mut()
                .set_submit(Box::new(move |window, frame, returns| {
                    counts.taken.fetch_add(1, Ordering::SeqCst);
                    let (frame, returns, counts) =
                        (frame.clone(), returns.clone(), Arc::clone(&counts));
                    render_thread::hand_over(move || {
                        let mut renderer = frame
                            .renderer
                            .lock()
                            .unwrap_or_else(PoisonError::into_inner);
                        let shown = match renderer.render(&frame.request()) {
                            Ok(id) => Shown::Presented(id),
                            Err(e) => Shown::Failed(e),
                        };
                        drop(renderer);
                        counts.shown.fetch_add(1, Ordering::SeqCst);
                        returns.send(PlatformEvent::Window {
                            window,
                            event: WindowEvent::Shown(shown),
                        });
                    });
                    Submitted::Taken
                }));
            driver.run_until_idle(turns()).unwrap();
            let window = driver.runtime().windows()[0].1;
            let mut at = 0;
            driver
                .window_system_mut()
                .configure(window, None, WindowStates::default())
                .unwrap();
            settle(&mut driver, &frames, &mut at);
            assert!(frames.shown.load(Ordering::SeqCst) > 0, "a frame was shown");

            // the render thread busy, a frame of a new size waits behind its work
            render_thread::hand_over(move || {
                let _ = gate.recv();
            });
            driver
                .window_system_mut()
                .configure(
                    window,
                    Some(Size::new(240.0, 96.0)),
                    WindowStates::default(),
                )
                .unwrap();
            driver.run_until_idle(turns()).unwrap();
            assert!(frames.away(), "the frame waits for the render thread");
            // then the window is suspended
            let suspended = WindowStates {
                suspended: true,
                ..WindowStates::default()
            };
            driver
                .window_system_mut()
                .configure(window, Some(Size::new(240.0, 96.0)), suspended)
                .unwrap();
            let rested = driver.run_until_idle(turns()).unwrap();
            // the configure leaves the window waiting for its frame, not turning the loop over and over
            assert!(
                !matches!(rested, StepStatus::BudgetExhausted(_)),
                "{rested:?}"
            );
            handled.send(dropped.load(Ordering::SeqCst)).unwrap();
            settle(&mut driver, &frames, &mut at);
            dropped.load(Ordering::SeqCst)
        })
        .unwrap();
    let before_shown = suspended
        .recv_timeout(STOPPED)
        .expect("the UI thread handled the suspension while the frame was away");
    assert_eq!(before_shown, 0, "the layers wait for the frame away");
    open.send(()).unwrap();
    let dropped = ui.join().expect("the UI thread goes on");
    assert_eq!(dropped, 1, "the layers went once the frame was shown");
}
