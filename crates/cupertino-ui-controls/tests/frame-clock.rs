use crate::support;

use cupertino_ui_controls::{FrameClock, Progress};
use cupertino_ui_core::{Application, Command, Element, Window};
use cupertino_ui_platform::Timestamp;
use std::num::NonZeroU32;
use support::{window, Harness};

/// How many steps the motion takes: one per frame.
const STEPS: u32 = 5;

/// A motion of [`STEPS`] steps, started by its first message.
struct Motion;

impl Application for Motion {
    type Model = u32;
    type Message = ();

    fn init() -> (u32, Vec<Command<()>>) {
        (0, Vec::new())
    }

    fn update(steps: &mut u32, _: ()) -> Vec<Command<()>> {
        *steps += 1;
        Vec::new()
    }

    fn view(steps: &u32) -> Vec<Window<()>> {
        window(Element::from(FrameClock {
            content: Element::new(Progress {
                label: steps.to_string(),
                value: Some(0.5),
            }),
            on_frame: (*steps < STEPS).then_some(()),
        }))
    }
}

fn turns() -> NonZeroU32 {
    NonZeroU32::new(64).unwrap()
}

/// Each frame callback steps the motion once, and nothing else does: with the clock running on and no frame given,
/// it stands still, then takes one step for each frame the compositor gives until it stops asking.
#[test]
fn frame_clock_steps_once_per_frame_callback_and_never_between() {
    let mut h = Harness::<Motion>::animated();
    let at_start = *h.model();
    // time passes without a frame callback: no step
    h.clock().advance_to(Timestamp(500_000_000)).unwrap();
    h.rt.step(turns()).unwrap();
    assert_eq!(*h.model(), at_start, "no step without a frame");
    let window = h.window;
    let mut frames = 0;
    let give_frame = |h: &mut Harness<Motion>| {
        h.clock().advance_by(16_666_667);
        let now = cupertino_ui_core::Clock::now(h.clock());
        h.rt.window_system_mut().frame_ready(window, now).unwrap();
        h.rt.step(turns()).unwrap();
    };
    while *h.model() < STEPS {
        assert!(
            h.rt.window_system().frame_requested(window).unwrap(),
            "the clock asks for the next frame while the motion runs"
        );
        let before = *h.model();
        give_frame(&mut h);
        frames += 1;
        assert_eq!(*h.model(), before + 1, "frame {frames} steps once");
    }
    // the frame already asked for when the motion ended sends nothing and asks for no other
    if h.rt.window_system().frame_requested(window).unwrap() {
        give_frame(&mut h);
    }
    assert_eq!(*h.model(), STEPS);
    assert!(!h.rt.window_system().frame_requested(window).unwrap());
}
