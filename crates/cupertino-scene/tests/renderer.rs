//! A renderer draws a scene frame into a target and reports whether the failure is worth a retry or a lost device.

use std::fmt;
use std::time::Duration;

use cupertino_scene::renderer::{Completion, Failure, RenderFailure, RenderStatus, Renderer};
use cupertino_scene::{
    FrameId, GlobalRect, LayerStack, Output, OutputId, OutputInvalidation, PhysicalSize,
    SceneFrame, Transform,
};

#[derive(Debug)]
struct Fault(Failure);

impl fmt::Display for Fault {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.0)
    }
}

impl std::error::Error for Fault {}

impl RenderFailure for Fault {
    fn failure(&self) -> Failure {
        self.0
    }
}

struct Done(RenderStatus);

impl Completion for Done {
    fn status(&self) -> RenderStatus {
        self.0
    }
}

/// Counts frames into the target; fails once when the resources say so.
struct Counting;

impl Renderer for Counting {
    type Resources<'a> = Option<Failure>;
    type Target<'a> = &'a mut Vec<FrameId>;
    type Completion = Done;
    type Error = Fault;

    fn render(
        &mut self,
        scene: &SceneFrame,
        resources: &mut Option<Failure>,
        target: &mut Vec<FrameId>,
    ) -> Result<Done, Fault> {
        if let Some(failure) = resources.take() {
            return Err(Fault(failure));
        }
        target.push(scene.id);
        Ok(Done(if scene.items().is_empty() {
            RenderStatus::Empty
        } else {
            RenderStatus::Rendered
        }))
    }
}

fn empty_frame(id: u64) -> SceneFrame {
    SceneFrame::new(
        Output {
            id: OutputId(1),
            rect: GlobalRect(cupertino_geometry::Rect::new(0.0, 0.0, 100.0, 100.0)),
            physical_size: PhysicalSize {
                width: 100,
                height: 100,
            },
            scale: 1.0,
            transform: Transform::Normal,
        },
        FrameId(id),
        Duration::ZERO,
        [0.0; 4],
        OutputInvalidation::Full,
        LayerStack::new(),
    )
}

/// What a caller does with a failure: draw the next frame again, or rebuild the device.
fn recover<R: Renderer>(error: &R::Error) -> &'static str {
    match error.failure() {
        Failure::Transient => "retry",
        Failure::ContextLost => "rebuild",
    }
}

#[test]
fn a_renderer_draws_a_frame_into_its_target_and_reports_what_it_drew() {
    let mut renderer = Counting;
    let mut drawn = Vec::new();
    let done = renderer
        .render(&empty_frame(4), &mut None, &mut drawn)
        .expect("renders");
    assert_eq!(done.status(), RenderStatus::Empty);
    assert_eq!(drawn, [FrameId(4)]);
}

#[test]
fn a_failure_says_whether_the_device_survived() {
    let mut renderer = Counting;
    let mut drawn = Vec::new();
    let transient = renderer
        .render(&empty_frame(1), &mut Some(Failure::Transient), &mut drawn)
        .err()
        .expect("fails");
    assert_eq!(recover::<Counting>(&transient), "retry");
    let lost = renderer
        .render(&empty_frame(2), &mut Some(Failure::ContextLost), &mut drawn)
        .err()
        .expect("fails");
    assert_eq!(recover::<Counting>(&lost), "rebuild");
    assert!(drawn.is_empty(), "a failed frame draws nothing");
}
