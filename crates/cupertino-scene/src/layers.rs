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
    /// Popups of the key window and of the layer-shell surfaces below the overlay layer (menus, popovers, tooltips):
    /// above every window and the bars, so a menu that opens low on the screen covers the dock rather than going under
    /// it. Another window's popups stay with that window.
    Popups,
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
    /// Compositor notices above all desktop content, including fullscreen windows and the cursor.
    /// Above the display filter, so notices are not inverted or tinted.
    /// Composition must omit this layer from lock-only frames.
    CompositorOverlay,
    /// Activation fade and power dimming: covers every frame, lock-only frames included.
    SessionOverlay,
}

/// Collects elements per layer and flattens them front to back.
#[derive(Debug)]
pub struct LayerStack<T> {
    layers: std::collections::BTreeMap<SceneLayer, Vec<T>>,
}

impl<T> Default for LayerStack<T> {
    fn default() -> Self {
        Self {
            layers: Default::default(),
        }
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
        self.layers
            .into_iter()
            .rev()
            .flat_map(|(l, v)| v.into_iter().map(move |i| (l, i)))
            .collect()
    }
}
