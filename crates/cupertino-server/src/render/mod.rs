//! Rendering: the element enum, the layer stack that fixes the drawing order, custom GLES
//! elements (shadow / rounded rectangles / corner mask / material and glass passes) and the scene builder.
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
pub mod content_blur;
pub mod gles;
pub mod image;
pub mod layers;
pub mod lower;
pub mod material;
pub mod mesh;
pub mod motion;
pub mod passes;
pub mod popup_shown;
pub mod programs;
pub mod reduce;
pub mod scene;
pub mod screenshot;
pub mod shader_rect;
pub mod snapshots;
pub mod ui;
pub mod uploads;
pub mod wallpaper;

pub use clip::{ClipElement, ClipShape};
pub use material::glass::{GlassContainer, GlassElement};
pub use material::{GpuRecipe, MaterialElement, MaterialGl, MaterialStage, SharedMaterial};
pub use mesh::{MeshElement, MeshGl, SharedMesh};
pub use programs::Programs;
pub use reduce::ReducedElement;
pub use shader_rect::{ElementCache, ElementKey, ShaderRect, Sig};

render_elements! {
    /// Everything the compositor draws.
    pub CupertinoElement<=GlesRenderer>;
    /// Window content: surface element, rounded mask, uniform or animated rescale.
    Window = RescaleRenderElement<ClipElement<ReducedElement>>,
    /// Popups and other surfaces drawn without a mask (rescaled with their window).
    Popup = RescaleRenderElement<ReducedElement>,
    /// Plain surfaces: layer-shell surfaces, drag icon, client cursor.
    Surface = WaylandSurfaceRenderElement<GlesRenderer>,
    /// Shadows, wallpaper, highlights.
    Shader = ShaderRect,
    /// Material passes (menu bar strip, backdrops of the shell processes).
    Material = MaterialElement,
    /// Glass containers: the union of a surface's glass backdrops over the blurred backdrop.
    Glass = GlassElement,
    /// CPU-drawn buffers: themed cursor image, menu bar contents, popups.
    Image = MemoryRenderBufferRenderElement<GlesRenderer>,
    /// Title bars and rims, rescaled together with their window.
    Decoration = RescaleRenderElement<MemoryRenderBufferRenderElement<GlesRenderer>>,
    /// Title bars and rims of a stage placement, masked to its animated corner radius.
    ClippedDecoration = RescaleRenderElement<ClipElement<MemoryRenderBufferRenderElement<GlesRenderer>>>,
    /// Window snapshot drawn through an effect mesh (minimize, closing).
    Mesh = MeshElement,
    /// A layer's content drawn from its blurred copy, masked and rescaled as window content.
    Blurred = RescaleRenderElement<ClipElement<content_blur::BlurredContent>>,
}

pub use cupertino_scene::{LayerStack, SceneLayer};
