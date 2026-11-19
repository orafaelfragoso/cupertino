//! `Cupertino`-side GPU reset recovery (rebuilding render resources and notifying clients).

pub use cupertino_server_backend::gpu_reset::*;

use smithay::backend::renderer::gles::GlesRenderer;

use crate::render::scene::RenderResources;
use crate::state::Cupertino;

impl Cupertino {
    /// Makes every GPU object of the scene again with `renderer`, the new one: the shaders and passes. The window
    /// snapshots and closing ghosts lived in the lost context and are dropped; a window closing later fades out from a
    /// capture made with the new renderer. Each output's next frame is full.
    pub fn rebuild_render(&mut self, renderer: &mut GlesRenderer) -> anyhow::Result<()> {
        self.fx.drop_snapshots();
        self.render = None;
        self.render = Some(RenderResources::new(renderer)?);
        for output in &mut self.outputs {
            output.tracker = smithay::backend::renderer::damage::OutputDamageTracker::from_output(
                &output.output,
            );
            // the offscreen frame of a supersampled output lived in the lost context too
            output.cache.supersample.reset();
            output.scheduler.reset();
        }
        self.request_redraw();
        // clients drawing on a GPU of their own may have lost their buffers' pixels too: they draw whole again
        self.sessions.buffers_lost();
        Ok(())
    }
}
