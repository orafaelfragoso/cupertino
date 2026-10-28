//! Rendering: the element enum, the layer stack that fixes the drawing order, custom GLES
//! elements (shadow / rounded rectangles / corner mask / material pass) and the scene builder.
//!
//! Per-output element order, bottom to top, is [`SceneLayer`]; [`LayerStack`] flattens it into the
//! front-to-back list the damage tracker expects.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::render_elements;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::gles::GlesRenderer;

pub mod clip;
pub mod compose;
pub mod image;
#[cfg(scene_oracle)]
pub mod legacy_scene;
pub mod lower;
pub mod material;
pub mod mesh;
pub mod motion;
pub mod programs;
pub mod scene;
pub mod screenshot;
pub mod shader_rect;
pub mod ui;
pub mod wallpaper;

pub use clip::{ClipElement, ClipShape};
pub use material::{GpuRecipe, MaterialElement, MaterialGl, MaterialStage, SharedMaterial};
pub use mesh::{MeshElement, MeshGl, SharedMesh};
pub use programs::Programs;
pub use shader_rect::{ElementCache, ElementKey, ShaderRect, Sig};

render_elements! {
    /// Everything the compositor draws.
    pub CupertinoElement<=GlesRenderer>;
    /// Window content: surface element, rounded mask, uniform or animated rescale.
    Window = RescaleRenderElement<ClipElement<WaylandSurfaceRenderElement<GlesRenderer>>>,
    /// Popups and other surfaces drawn without a mask (rescaled with their window).
    Popup = RescaleRenderElement<WaylandSurfaceRenderElement<GlesRenderer>>,
    /// Plain surfaces: layer-shell surfaces, drag icon, client cursor.
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    /// Shadows, wallpaper, highlights.
    Shader = ShaderRect,
    /// Material passes (menu bar strip, backdrops of the shell processes).
    Material = MaterialElement,
    /// CPU-drawn buffers: themed cursor image, menu bar contents, popups.
    Image = MemoryRenderBufferRenderElement<GlesRenderer>,
    /// Title bars and rims, rescaled together with their window.
    Decoration = RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>,
    /// Title bars and rims of a stage placement, masked to its animated corner radius.
    ClippedDecoration = RescaleRenderElement<ClipElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
    /// Window snapshot drawn through an effect mesh (minimize, closing).
    Mesh = MeshElement,
}

pub use cupertino_scene::{LayerStack, SceneLayer};
