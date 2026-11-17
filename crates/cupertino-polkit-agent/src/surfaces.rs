//! The layer surfaces of the prompt: a scrim on the overlay layer of every output, which takes no keys, and the panel
//! above them, which takes the keyboard exclusively, so nothing else hears what is typed while it is shown.

use cupertino_shell_client::{Anchor, KeyboardInteractivity, Layer, LayerSurfaceConfig, Margin};
use cupertino_wayland_client::OutputId;

/// Layer-shell namespace of the scrims and the panel.
pub const NAMESPACE: &str = "cupertino-polkit-agent";

/// The scrim over all of `output`.
pub fn scrim(output: OutputId) -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        namespace: NAMESPACE.into(),
        output: Some(output),
        layer: Layer::Overlay,
        anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
        size: (0, 0),
        margin: Margin::default(),
        exclusive_zone: -1,
        keyboard: KeyboardInteractivity::None,
    }
}

/// The panel of `size` points; without an anchor the server centres it, and without an output it goes on the active
/// one.
pub fn panel(size: (u32, u32)) -> LayerSurfaceConfig {
    LayerSurfaceConfig {
        namespace: NAMESPACE.into(),
        output: None,
        layer: Layer::Overlay,
        anchor: Anchor::empty(),
        size,
        margin: Margin::default(),
        exclusive_zone: -1,
        keyboard: KeyboardInteractivity::Exclusive,
    }
}
