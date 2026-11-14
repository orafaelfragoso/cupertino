/// How many times a step ran and how many bytes it moved or allocated.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tally {
    pub count: u64,
    pub bytes: u64,
}

impl Tally {
    pub(crate) fn add(&mut self, bytes: u64) {
        self.count += 1;
        self.bytes += bytes;
    }
}

/// What the last [`CpuRenderer::render`](crate::CpuRenderer) call did, counted rather than timed, so a timing run can
/// tell a slower frame from a heavier one. Copies and allocations are named by site and kept apart.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RenderStats {
    pub dirty_rects: u32,
    pub dirty_pixels: u64,
    /// Nodes whose own operations were painted, and those painted as a group layer.
    pub nodes_painted: u32,
    pub groups_painted: u32,
    /// Groups laid over from the image kept of them ([`crate::Layer::Opacity`], [`crate::Layer::Transform`]), their
    /// subtrees not painted.
    pub groups_kept: u32,
    /// Nodes not painted, or painted without their own operations, because an opaque fill after them covers the
    /// damaged rectangle.
    pub hidden: u32,
    pub fills: u32,
    pub strokes: u32,
    pub images: u32,
    pub glyphs: u32,
    /// Allocations: scratch images for nodes and groups, operation layers, clip masks.
    pub scratch_images: Tally,
    pub layers: Tally,
    pub masks: Tally,
    /// Copies by site.
    pub clear: Tally,
    pub node_composite: Tally,
    pub group_composite: Tally,
    pub layer_composite: Tally,
    pub crop: Tally,
    pub mask_apply: Tally,
    /// Rectangles of pixels a node's fills cover fully, written as rows instead of through a scratch image.
    pub spans: Tally,
    /// Cache fills this frame: images decoded to premultiplied pixels, glyphs rasterized.
    pub image_decode: Tally,
    pub glyph_rasterize: Tally,
    /// Pixels painted into kept scroll layers: what came into view and what changed in them.
    pub layer_pixels: u64,
    /// Kept scroll layers laid over the frame.
    pub scroll_composite: Tally,
    /// Of those, the parts laid over the colour the frame was to be set to there (its background) in one pass, that
    /// colour not written first.
    pub scroll_on_background: Tally,
    /// Retained across frames, after this one.
    pub retained_glyphs: Tally,
    pub retained_images: Tally,
    pub retained_layers: Tally,
    pub frame_bytes: u64,
}
