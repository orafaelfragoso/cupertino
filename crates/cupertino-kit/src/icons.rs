//! The system's symbols from the asset packs, for the widgets' [`Icons`].

use cupertino_assets::{Assets, IconName};
use cupertino_icons::SystemIcons;
use cupertino_ui_core::{next_resource_id, IconImage, Icons, PlaceholderIcons};
use cupertino_ui_render::{ImageResource, PixelFormat};
use cupertino_ui_theme::Color;
use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

/// A symbol as asked for: its name, device pixels, colour in 8-bit steps, and whether it is mirrored.
type Request = (String, u32, [u8; 4], bool);

/// [`SystemIcons`] of the asset packs. Each request keeps its own id and pixels for the life of the app, so a node
/// repainted and a node reused from the paint cache never name two images with one id.
pub(crate) struct PackIcons {
    state: Mutex<(SystemIcons, HashMap<Request, IconImage>)>,
    /// What a name that is not a valid icon name draws.
    invalid: PlaceholderIcons,
}

impl PackIcons {
    pub(crate) fn new(assets: &Assets) -> Self {
        Self::of(SystemIcons::new(assets))
    }

    fn of(icons: SystemIcons) -> Self {
        Self {
            state: Mutex::new((icons, HashMap::new())),
            invalid: PlaceholderIcons::default(),
        }
    }
}

impl fmt::Debug for PackIcons {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("PackIcons")
    }
}

impl Icons for PackIcons {
    fn symbol(&self, name: &str, px: u32, color: Color, mirrored: bool) -> IconImage {
        if IconName::new(name).is_err() {
            return self.invalid.symbol(name, px, color, mirrored);
        }
        let rgba = [color.r, color.g, color.b, color.a];
        let request = (
            name.to_owned(),
            px,
            rgba.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8),
            mirrored,
        );
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let (icons, drawn) = &mut *state;
        if let Some(image) = drawn.get(&request) {
            return image.clone();
        }
        let pixmap = icons.symbol(name, px, rgba);
        let (width, height) = (pixmap.width(), pixmap.height());
        let mut pixels = pixmap.data().to_vec();
        if mirrored {
            for row in pixels.chunks_exact_mut(width as usize * 4) {
                row.as_chunks_mut::<4>().0.reverse();
            }
        }
        let image = IconImage {
            id: next_resource_id(),
            image: ImageResource {
                revision: 1,
                width,
                height,
                stride: width * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(pixels),
            },
        };
        drawn.insert(request, image.clone());
        image
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn icons() -> PackIcons {
        PackIcons::of(SystemIcons::without_host(&Assets::builtin()))
    }

    const BLACK: Color = Color::new(0.0, 0.0, 0.0, 1.0);

    #[test]
    fn a_request_keeps_its_id_and_a_mirrored_one_has_its_own_reversed_pixels() {
        let icons = icons();
        let plain = icons.symbol("symbol.chevron.left", 24, BLACK, false);
        assert_eq!(icons.symbol("symbol.chevron.left", 24, BLACK, false), plain);
        let mirrored = icons.symbol("symbol.chevron.left", 24, BLACK, true);
        assert_ne!(mirrored.id, plain.id);
        let row = plain.image.width as usize * 4;
        for (a, b) in plain
            .image
            .pixels
            .chunks_exact(row)
            .zip(mirrored.image.pixels.chunks_exact(row))
        {
            let reversed: Vec<[u8; 4]> = a.as_chunks::<4>().0.iter().rev().copied().collect();
            assert_eq!(b.as_chunks::<4>().0, &reversed[..]);
        }
        assert_ne!(
            icons.symbol("symbol.chevron.left", 32, BLACK, false).id,
            plain.id
        );
    }

    #[test]
    fn a_name_that_is_not_an_icon_name_draws_the_placeholder() {
        let image = icons().symbol("../not a name", 16, BLACK, false);
        assert_eq!((image.image.width, image.image.height), (16, 16));
    }
}
