//! Rendering: the element enum, the layer stack that fixes the drawing order, custom GLES
//! elements (shadow / rounded rectangles / corner mask / material pass) and the scene builder.
//!
//! Per-output element order, bottom to top, is [`SceneLayer`]; [`LayerStack`] flattens it into the
//! front-to-back list the damage tracker expects.

use smithay::backend::renderer::element::memory::MemoryRenderBufferRenderElement;
use smithay::backend::renderer::element::surface::WaylandSurfaceRenderElement;
use smithay::backend::renderer::element::utils::RescaleRenderElement;
use smithay::backend::renderer::element::render_elements;
use smithay::backend::renderer::gles::GlesRenderer;

pub mod clip;
pub mod image;
pub mod material;
pub mod mesh;
pub mod motion;
pub mod programs;
pub mod scene;
pub mod screenshot;
pub mod shader_rect;
pub mod ui;

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

/// Drawing layers of one output, bottom to top.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SceneLayer {
    Wallpaper,
    /// Layer-shell `background` surfaces.
    LayerBackground,
    /// Layer-shell `bottom` surfaces.
    LayerBottom,
    /// Dimming behind the overview windows.
    OverviewBackdrop,
    /// Backdrop captures of the menu bars: what lies below the windows and their shadows.
    BackdropCapture,
    /// Workspace slide: windows with shadows, bottom to top.
    Windows,
    /// Tiling preview rectangle.
    SnapPreview,
    /// Layer-shell `top` surfaces.
    LayerTop,
    /// Workspaces bar of the overview.
    /// Notification banners (above windows).
    Banners,
    /// Input-source switcher HUD.
    InputHud,
    /// Accent picker popup.
    AccentPopup,
    /// Layer-shell `overlay` surfaces.
    LayerOverlay,
    DragIcon,
    /// Colour filter, invert and contrast of the whole output (a material pass over everything below).
    DisplayFilter,
    Cursor,
}

/// Collects elements per layer and flattens them front to back.
#[derive(Debug)]
pub struct LayerStack<T> {
    layers: std::collections::BTreeMap<SceneLayer, Vec<T>>,
}

impl<T> Default for LayerStack<T> {
    fn default() -> Self {
        Self { layers: Default::default() }
    }
}

impl<T> LayerStack<T> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Append to `layer`. Within a layer, elements are given front to back (first = topmost).
    pub fn push(&mut self, layer: SceneLayer, item: T) {
        self.layers.entry(layer).or_default().push(item);
    }

    pub fn extend(&mut self, layer: SceneLayer, items: impl IntoIterator<Item = T>) {
        self.layers.entry(layer).or_default().extend(items);
    }

    pub fn len(&self) -> usize {
        self.layers.values().map(Vec::len).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Front-to-back list: topmost layer first.
    pub fn flatten(self) -> Vec<T> {
        self.layers.into_values().rev().flatten().collect()
    }

    /// Items with their layer, front to back.
    pub fn flatten_with_layer(self) -> Vec<(SceneLayer, T)> {
        self.layers.into_iter().rev().flat_map(|(l, v)| v.into_iter().map(move |i| (l, i))).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layers_flatten_top_first() {
        let mut s = LayerStack::new();
        s.push(SceneLayer::Cursor, "cursor");
        s.push(SceneLayer::Wallpaper, "wallpaper");
        s.push(SceneLayer::Windows, "w-top");
        s.push(SceneLayer::Windows, "w-below");
        s.push(SceneLayer::Banners, "banners");
        s.push(SceneLayer::LayerTop, "layer-top");
        assert_eq!(s.flatten(), vec!["cursor", "banners", "layer-top", "w-top", "w-below", "wallpaper"]);
    }

    #[test]
    fn required_order_background_desktop_workspace_windows_shell_overlays_cursor() {
        use SceneLayer::*;
        let order = [Wallpaper, LayerBackground, LayerBottom, OverviewBackdrop, BackdropCapture, Windows, SnapPreview, LayerTop, Banners, InputHud, AccentPopup, LayerOverlay, DragIcon, DisplayFilter, Cursor];
        for w in order.windows(2) {
            assert!(w[0] < w[1], "{:?} must be below {:?}", w[0], w[1]);
        }
        // the material layers sit above the windows so their backdrop contains them
        assert!(Windows < Banners);
    }

    #[test]
    fn flatten_with_layer_keeps_layers() {
        let mut s = LayerStack::new();
        s.push(SceneLayer::Banners, 1);
        s.push(SceneLayer::Wallpaper, 2);
        assert_eq!(s.flatten_with_layer(), vec![(SceneLayer::Banners, 1), (SceneLayer::Wallpaper, 2)]);
    }
}
