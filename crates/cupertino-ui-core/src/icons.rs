//! Images for widgets to draw: the process-wide resource ids that name them, and the source of the system's symbols.

use cupertino_ui_render::{ImageResource, PixelFormat, ResourceId};
use cupertino_ui_theme::Color;
use std::collections::HashMap;
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

static NEXT_RESOURCE: AtomicU64 = AtomicU64::new(1);

/// A resource id no other image of this process has. An id names one image for good: new pixels take a new id.
pub fn next_resource_id() -> ResourceId {
    ResourceId(NEXT_RESOURCE.fetch_add(1, Ordering::Relaxed))
}

/// A symbol drawn for one size and colour, and the id that names its pixels.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconImage {
    pub id: ResourceId,
    pub image: ImageResource,
}

/// The system's symbols by name (`symbol.house`, ...), drawn at device pixels.
pub trait Icons: Send + Sync + fmt::Debug {
    /// The symbol `name` drawn in `color` to fit `px` × `px` device pixels, mirrored left to right when `mirrored`.
    /// The same request gives the same image and id while the source keeps it.
    fn symbol(&self, name: &str, px: u32, color: Color, mirrored: bool) -> IconImage;

    /// The running application's own icon, drawn to fit `px` × `px` device pixels, as an alert shows it; `None` when
    /// the source has no icon for it.
    fn application(&self, _px: u32) -> Option<IconImage> {
        None
    }

    /// The icon of the installed application `name` (its desktop entry's `Icon=` value), drawn to fit `px` × `px`
    /// device pixels; the generic application icon when the source has none for the name. `None` when the source
    /// draws no application icons.
    fn named(&self, _name: &str, _px: u32) -> Option<IconImage> {
        None
    }
}

/// Draws every symbol, and the application's icon, as the outline of a square, for tests and for apps without asset
/// packs.
#[derive(Debug, Default)]
pub struct PlaceholderIcons {
    drawn: Mutex<HashMap<(u32, [u8; 4]), IconImage>>,
}

impl Icons for PlaceholderIcons {
    fn symbol(&self, _: &str, px: u32, color: Color, _: bool) -> IconImage {
        let px = px.max(1);
        let tint = color
            .premultiplied()
            .map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8);
        let mut drawn = self.drawn.lock().unwrap_or_else(|e| e.into_inner());
        drawn
            .entry((px, tint))
            .or_insert_with(|| {
                let edge = (px / 12).max(1);
                let side = px as usize;
                let mut pixels = vec![0u8; side * side * 4];
                for (i, pixel) in pixels.as_chunks_mut::<4>().0.iter_mut().enumerate() {
                    let (x, y) = ((i % side) as u32, (i / side) as u32);
                    if x < edge || y < edge || x >= px - edge || y >= px - edge {
                        *pixel = tint;
                    }
                }
                IconImage {
                    id: next_resource_id(),
                    image: ImageResource {
                        revision: 1,
                        width: px,
                        height: px,
                        stride: px * 4,
                        format: PixelFormat::Rgba8Premultiplied,
                        pixels: Arc::from(pixels),
                    },
                }
            })
            .clone()
    }

    fn application(&self, px: u32) -> Option<IconImage> {
        Some(self.symbol("", px, Color::new(0.5, 0.5, 0.5, 1.0), false))
    }
}
