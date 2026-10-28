//! The renderer seam. A renderer draws one [`SceneFrame`] to a target and says what came of it. The trait names no
//! graphics API, protocol or window-system type: the resources the frame refers to, the targets it can draw to and the
//! completion it returns are the implementation's associated types, so a different GPU backend replaces an
//! implementation without touching how scenes are composed.
//!
//! Presenting what was rendered (page flips, buffer swaps, frame callbacks, presentation feedback) is not a renderer's
//! business: a successful render is not a successful presentation.

use cupertino_scene::SceneFrame;

/// What a render call achieved, in terms every presenter understands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderStatus {
    /// Pixels were drawn that differ from what the target already showed.
    Rendered,
    /// Nothing differs from what the target already showed: there is nothing to present.
    Empty,
}

/// The result of a render, with the implementation's own details (damage, queued state, pixels).
pub trait Completion {
    fn status(&self) -> RenderStatus;
}

/// Draws scene frames. Implementations are thread-affine and statically dispatched: no `Send`, no `dyn`.
pub trait Renderer {
    /// What the frame's typed handles resolve against, and the caches that outlive a frame; it may borrow them.
    type Resources<'a>
    where
        Self: 'a;
    /// Where the frame is drawn: an offscreen image, a window's framebuffer, a display output.
    type Target<'a>
    where
        Self: 'a;
    type Completion: Completion;
    type Error: std::error::Error + 'static;

    /// Draw `scene` to `target`. A failure is an error, never an empty success.
    fn render(
        &mut self,
        scene: &SceneFrame,
        resources: &mut Self::Resources<'_>,
        target: Self::Target<'_>,
    ) -> Result<Self::Completion, Self::Error>;
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use cupertino_geometry::Rect;
    use cupertino_scene::{
        FrameId, GlobalRect, LayerStack, Output, OutputId, OutputInvalidation, PhysicalSize,
        Transform,
    };

    use super::*;

    /// A renderer that draws nothing: the frame is empty when it holds no item, an error when told to fail.
    struct Counting;

    #[derive(Debug, PartialEq, Eq)]
    struct Refused;

    impl std::fmt::Display for Refused {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("refused")
        }
    }

    impl std::error::Error for Refused {}

    struct Done(usize);

    impl Completion for Done {
        fn status(&self) -> RenderStatus {
            if self.0 == 0 {
                RenderStatus::Empty
            } else {
                RenderStatus::Rendered
            }
        }
    }

    impl Renderer for Counting {
        type Resources<'a> = ();
        type Target<'a> = bool;
        type Completion = Done;
        type Error = Refused;

        fn render(&mut self, scene: &SceneFrame, _: &mut (), fail: bool) -> Result<Done, Refused> {
            if fail {
                return Err(Refused);
            }
            Ok(Done(scene.items().len()))
        }
    }

    fn frame() -> SceneFrame {
        SceneFrame::new(
            Output {
                id: OutputId(1),
                rect: GlobalRect(Rect::new(0.0, 0.0, 10.0, 10.0)),
                physical_size: PhysicalSize {
                    width: 10,
                    height: 10,
                },
                scale: 1.0,
                transform: Transform::Normal,
            },
            FrameId(1),
            Duration::ZERO,
            [0.0; 4],
            OutputInvalidation::Incremental,
            LayerStack::new(),
        )
    }

    #[test]
    fn empty_rendered_and_failed_are_three_different_results() {
        let mut renderer = Counting;
        let empty = renderer.render(&frame(), &mut (), false).unwrap();
        assert_eq!(empty.status(), RenderStatus::Empty);
        assert_eq!(
            renderer.render(&frame(), &mut (), true).err(),
            Some(Refused)
        );
    }
}
