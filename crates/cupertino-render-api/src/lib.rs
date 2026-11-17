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

/// What a failed render means for the frames after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// This frame was not drawn; the next one may be.
    Transient,
    /// The device context is gone: every later frame fails until the renderer and its resources are created again.
    ContextLost,
}

/// A render error that says whether rendering can go on.
pub trait RenderFailure: std::error::Error + 'static {
    fn failure(&self) -> Failure;
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
    type Error: RenderFailure;

    /// Draw `scene` to `target`. A failure is an error, never an empty success.
    fn render(
        &mut self,
        scene: &SceneFrame,
        resources: &mut Self::Resources<'_>,
        target: Self::Target<'_>,
    ) -> Result<Self::Completion, Self::Error>;
}
