//! A window that moves on every frame, whose frame is away on another thread, on a compositor that answers every frame
//! request at once: the window asks for no frame it cannot draw until its frame is back, so the compositor does not turn
//! its loop over and over meanwhile, and once the frame is back it draws the next.

#[allow(dead_code)]
mod support;

use cupertino_kit::controls::{FrameClock, WINDOW_CONTENT_INSET};
use cupertino_kit::core::CommandRegistry;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{DecorationMode, Timestamp, WindowId};
use cupertino_kit::prelude::*;
use cupertino_ui_core::platform::{
    PlatformEvent, Shown, Submission, Submitted, WindowEvent, WindowStates,
};
use cupertino_ui_test::{Returns, TestDriver};
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex, PoisonError};

/// The time between two frames the compositor hands the window, in nanoseconds.
const FRAME_STEP: u64 = 16_000_000;

/// The most frames the compositor hands the window while its frame is away before the test calls it a busy loop.
const FRAMES: usize = 32;

/// One window whose label counts the frames it was handed.
struct Ticking;

impl Application for Ticking {
    type Model = u32;
    type Message = ();

    fn init() -> (u32, Vec<Command<()>>) {
        (0, Vec::new())
    }

    fn update(ticks: &mut u32, (): ()) -> Vec<Command<()>> {
        *ticks += 1;
        Vec::new()
    }

    fn view(ticks: &u32) -> Vec<Window<()>> {
        vec![Window {
            key: WindowKey("ticking".into()),
            spec: WindowSpec {
                title: "Ticking".into(),
                app_id: "org.cupertino.tests.Ticking".into(),
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
            content: FrameClock {
                content: Element::new(Stack {
                    axis: Axis::Vertical,
                    padding: WINDOW_CONTENT_INSET,
                    spacing: 0.0,
                    align: Align::Stretch,
                    children: vec![Element::new(Button {
                        label: ticks.to_string(),
                        role: ButtonRole::Normal,
                        style: ButtonStyle::Bordered,
                        enabled: true,
                        busy: false,
                        on_press: None,
                    })],
                }),
                on_frame: Some(()),
                on_layers_settled: None,
            }
            .into(),
            menus: None,
        }]
    }
}

/// The frames the window system took and has not shown yet.
type Away = Arc<Mutex<Vec<(WindowId, Submission, Returns)>>>;

fn turns() -> NonZeroU32 {
    NonZeroU32::new(1000).unwrap()
}

/// Draws the frames away and says they were shown, as the thread that took them does.
fn show(driver: &mut TestDriver<Ticking>, away: &Away) {
    let frames = std::mem::take(&mut *away.lock().unwrap_or_else(PoisonError::into_inner));
    assert!(!frames.is_empty(), "a frame is away");
    for (window, frame, returns) in frames {
        let mut renderer = frame
            .renderer
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let shown = match renderer.render(&frame.request()) {
            Ok(id) => Shown::Presented(id),
            Err(e) => Shown::Failed(e),
        };
        returns.send(PlatformEvent::Window {
            window,
            event: WindowEvent::Shown(shown),
        });
    }
    driver.run_until_idle(turns()).unwrap();
}

/// Hands the window a frame when it asked for one, as a compositor with nothing to wait for does; whether it asked.
fn answer(driver: &mut TestDriver<Ticking>, window: WindowId, at: &mut u64) -> bool {
    if !driver.window_system_mut().frame_requested(window).unwrap() {
        return false;
    }
    *at += FRAME_STEP;
    driver.advance_to(Timestamp(*at)).unwrap();
    driver
        .window_system_mut()
        .frame_ready(window, Timestamp(*at))
        .unwrap();
    driver.run_until_idle(turns()).unwrap();
    true
}

fn away_count(away: &Away) -> usize {
    away.lock().unwrap_or_else(PoisonError::into_inner).len()
}

#[test]
fn a_window_whose_frame_is_away_asks_for_no_frame_until_it_is_back() {
    let mut driver = support::driver::<Ticking>(support::text_system());
    let away = Away::default();
    let taken = Arc::clone(&away);
    driver
        .window_system_mut()
        .set_submit(Box::new(move |window, frame, returns| {
            taken.lock().unwrap_or_else(PoisonError::into_inner).push((
                window,
                frame.clone(),
                returns.clone(),
            ));
            Submitted::Taken
        }));
    driver.run_until_idle(turns()).unwrap();
    let window = driver.runtime().windows()[0].1;
    let mut at = 0;
    driver
        .window_system_mut()
        .configure(window, None, WindowStates::default())
        .unwrap();
    driver.run_until_idle(turns()).unwrap();
    show(&mut driver, &away);

    // the frame the window asks for once its first is shown moves it, and that frame goes away
    assert!(
        answer(&mut driver, window, &mut at),
        "the window asks for a frame"
    );
    assert_eq!(away_count(&away), 1, "the window's next frame is away");
    let mut handed = 0;
    while handed < FRAMES && answer(&mut driver, window, &mut at) {
        handed += 1;
    }
    assert!(
        handed <= 1,
        "the window took {handed} frames it could not draw while its frame was away"
    );
    assert_eq!(
        away_count(&away),
        1,
        "no other frame went while one was away"
    );

    // back, the frame lets the window draw the next
    show(&mut driver, &away);
    assert!(
        answer(&mut driver, window, &mut at),
        "the window asks for a frame"
    );
    assert_eq!(away_count(&away), 1, "the window drew its next frame");
}
