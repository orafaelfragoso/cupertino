//! Rendering: re-exports the core rendering pipeline from `cupertino-server-render` alongside
//! the compositor-level scene assembly (`scene`, `layers`, `popup_shown`).

pub use cupertino_server_render::render::{
    chrome, clip, compose, content_blur, core, gles, image, lower, material, mesh, motion, passes,
    programs, reduce, screenshot, shader_rect, snapshots, supersample, ui, uploads, wallpaper,
    ClipElement, ClipShape, CupertinoElement, ElementCache, ElementKey, GlassContainer,
    GlassElement, GpuRecipe, LayerStack, MaterialElement, MaterialGl, MaterialStage, MeshElement,
    MeshGl, Programs, ReducedElement, SceneLayer, ShaderRect, SharedMaterial, SharedMesh, Sig,
};
pub mod layers;
pub mod popup_shown;
pub mod scene;
