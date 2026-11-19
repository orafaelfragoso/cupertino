//! Cursor images: themed cursors loaded through `xcursor` (by name, for the cursor-shape
//! protocol), recoloured when the user picked other cursor colours, converted to memory render
//! buffers, with a built-in arrow as the fallback; and shake to locate, which grows the cursor
//! while the pointer is shaken (`cupertino_input::ShakeDetector`).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::Duration;

use cupertino_assets::CursorTheme;
use smithay::backend::allocator::Fourcc;
use smithay::backend::renderer::element::memory::MemoryRenderBuffer;
use smithay::input::pointer::CursorIcon;
use smithay::reexports::calloop::ping::make_ping;
use smithay::reexports::calloop::{LoopHandle, RegistrationToken};
use smithay::utils::{Logical, Physical, Point, Transform};
use xcursor::parser::{parse_xcursor, Image};

/// One cursor frame ready for rendering.
pub struct CursorFrame {
    /// The image at buffer scale 1.
    pub buffer: MemoryRenderBuffer,
    /// Hotspot in image pixels.
    pub hotspot: Point<i32, Physical>,
    /// Width and height of the image, pixels.
    pub pixels: (u32, u32),
    /// Nominal size of the image (the XCursor size it was drawn for), pixels.
    pub nominal: u32,
}

impl CursorFrame {
    /// Logical points per image pixel for a cursor drawn `size` points nominal (the nominal size times the
    /// cursor scale): the image's own scale, whatever the output's.
    fn points_per_pixel(&self, size: f64) -> f64 {
        size / f64::from(self.nominal)
    }

    /// The hotspot in logical points from the image's top-left corner, for a cursor drawn `size` points nominal.
    pub fn hotspot_at(&self, size: f64) -> Point<f64, Logical> {
        let k = self.points_per_pixel(size);
        Point::from((f64::from(self.hotspot.x) * k, f64::from(self.hotspot.y) * k))
    }

    /// The image's size in logical points for a cursor drawn `size` points nominal.
    pub fn size_at(&self, size: f64) -> (f64, f64) {
        let k = self.points_per_pixel(size);
        (f64::from(self.pixels.0) * k, f64::from(self.pixels.1) * k)
    }
}

/// Fill and outline colours (sRGB, straight alpha) of the cursors the server draws. The design
/// stores both and redraws its cursors with them once the user changed either.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorColors {
    pub fill: [u8; 4],
    pub outline: [u8; 4],
}

impl CursorColors {
    /// Black fill, white outline.
    pub const DEFAULT: CursorColors = CursorColors {
        fill: [0, 0, 0, 255],
        outline: [255, 255, 255, 255],
    };
}

/// A cursor's name and the index of its image in the theme's list: each animation frame has a buffer of its own.
type BufferKey = (&'static str, usize);
type CursorBuffer = (Point<i32, Physical>, MemoryRenderBuffer);

/// The cursors the server draws, read from the XCursor theme of the asset directory only (no host theme).
pub struct CursorLoader {
    theme: Option<CursorTheme>,
    size: u32,
    colors: CursorColors,
    default_images: Vec<Image>,
    requests: Option<mpsc::Sender<&'static str>>,
    ready: mpsc::Receiver<(&'static str, Option<Vec<Image>>)>,
    pending: HashSet<&'static str>,
    cache: HashMap<&'static str, Vec<Image>>,
    buffers: HashMap<BufferKey, CursorBuffer>,
}

/// Built-in arrow (white with a black outline), `size` pixels tall.
pub fn fallback_arrow(size: u32) -> Image {
    let s = size.max(12) as usize;
    let mut px = vec![0u8; s * s * 4];
    let h = s as f32;
    // triangle (0,0) - (0.55h,0.8h)... classic pointer silhouette
    let inside = |x: f32, y: f32, grow: f32| -> bool {
        let (x, y) = (x / h, y / h);
        let g = grow / h;
        y >= -g
            && y <= 0.82 + g
            && x >= -g
            && x <= 0.58 * (y.max(0.0) + 0.0) + 0.18 + g * 2.0
            && x <= 0.70 * y + g * 2.0 + 0.02
    };
    for y in 0..s {
        for x in 0..s {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * s + x) * 4;
            if inside(fx, fy, -1.2) {
                px[i..i + 4].copy_from_slice(&[255, 255, 255, 255]);
            } else if inside(fx, fy, 0.8) {
                px[i..i + 4].copy_from_slice(&[0, 0, 0, 255]);
            }
        }
    }
    Image {
        size,
        width: s as u32,
        height: s as u32,
        xhot: 1,
        yhot: 1,
        delay: 0,
        pixels_rgba: px,
        pixels_argb: vec![],
    }
}

/// Redraw a black-filled, white-outlined cursor image with `colors`.
///
/// approximate: the design redraws its vector cursors with the two colours; a
/// raster theme has only pixels, so each pixel's lightness picks between the outline (light) and
/// the fill (dark) colour. Dark pixels that are not opaque are the drop shadow, which stays black:
/// a cursor's fill is always opaque, inside its outline. `pixels` is XCursor data: premultiplied
/// ARGB32, little-endian (bytes B, G, R, A).
pub fn recolor(pixels: &mut [u8], colors: CursorColors) {
    let channel = |c: [u8; 4], i: usize| f32::from(c[i]) / 255.0;
    for px in pixels.as_chunks_mut::<4>().0 {
        let a = f32::from(px[3]) / 255.0;
        if a == 0.0 {
            continue;
        }
        let light =
            ((f32::from(px[0]) + f32::from(px[1]) + f32::from(px[2])) / (3.0 * 255.0) / a).min(1.0);
        let dark = if px[3] == 255 {
            colors.fill
        } else {
            [0, 0, 0, 255]
        };
        let mix = |i: usize| channel(colors.outline, i) * light + channel(dark, i) * (1.0 - light);
        let alpha = a * mix(3);
        let q = |v: f32| (v * alpha * 255.0).round().clamp(0.0, 255.0) as u8;
        // straight sRGB components 0 = red, 1 = green, 2 = blue, written as B, G, R
        let (r, g, b) = (mix(0), mix(1), mix(2));
        *px = [q(b), q(g), q(r), (alpha * 255.0).round() as u8];
    }
}

impl CursorLoader {
    /// The asset packs' `theme`, at the nominal size `XCURSOR_SIZE` (default 24), drawn in `colors`. Without a theme,
    /// or with one that has neither a `default` nor a `left_ptr` cursor (logged), the cursors are the
    /// built-in procedural set (`cupertino-cursor`), and the clients the server starts keep their own cursor theme.
    /// Calls `on_loaded` on the event loop whenever a background cursor read completes.
    pub fn load<D: 'static>(
        mut theme: Option<CursorTheme>,
        colors: CursorColors,
        handle: &LoopHandle<'static, D>,
        mut on_loaded: impl FnMut(&mut D) + 'static,
    ) -> Self {
        let size = std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.parse().ok())
            .unwrap_or(24);
        let default_images = theme.as_ref().and_then(|t| {
            Self::read(Some(t), size, "default").or_else(|| Self::read(Some(t), size, "left_ptr"))
        });
        if default_images.is_none() {
            if let Some(t) = theme.take() {
                tracing::warn!(
                    theme = t.name,
                    "cursor theme has no default cursor; drawing the built-in cursors"
                );
            }
        }
        let default_images = default_images
            .or_else(|| Self::read(None, size, "default"))
            .unwrap_or_else(|| vec![fallback_arrow(size)]);
        let (requests, rx) = mpsc::channel();
        let (results, ready) = mpsc::channel();
        let reading_theme = theme.clone();
        let requests = make_ping().and_then(|(wake, source)| {
            let token = handle.insert_source(source, move |(), _, state| on_loaded(state))
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            match std::thread::Builder::new()
                .name("cursor-images".into())
                .spawn(move || {
                    for name in rx {
                        if results
                            .send((name, Self::read(reading_theme.as_ref(), size, name)))
                            .is_err()
                        {
                            break;
                        }
                        wake.ping();
                    }
                }) {
                Ok(_) => Ok(requests),
                Err(e) => {
                    handle.remove(token);
                    Err(e)
                }
            }
        }).inspect_err(|e| tracing::warn!(error = %e, "cannot start the cursor reader; using the default cursor")).ok();
        Self {
            theme,
            size,
            colors,
            default_images,
            requests,
            ready,
            pending: HashSet::new(),
            cache: HashMap::new(),
            buffers: HashMap::new(),
        }
    }

    /// Draw the cursors in `colors` from now on (`cursor_fill`, `cursor_outline` changed): the images are
    /// recoloured as they are next used.
    pub fn set_colors(&mut self, colors: CursorColors) {
        if self.colors != colors {
            self.colors = colors;
            self.cache.clear();
            self.buffers.clear();
        }
    }

    /// The theme the cursors are loaded from, `None` for the built-in arrow. The clients the server starts get its
    /// name as `XCURSOR_THEME` and its directory as `XCURSOR_PATH`, so their own cursors match the ones the server
    /// draws and they find no other theme.
    pub fn theme(&self) -> Option<&CursorTheme> {
        self.theme.as_ref()
    }

    /// The images of the cursor `name`: the file of the theme (aliases are symlinks inside the theme), or without a
    /// theme the procedural cursor. `None` when the theme has no such file or the procedural set no such cursor.
    fn read(theme: Option<&CursorTheme>, size: u32, name: &str) -> Option<Vec<Image>> {
        let Some(theme) = theme else {
            return Self::procedural(size, name);
        };
        let path: PathBuf = theme
            .search_dir
            .join(&theme.name)
            .join("cursors")
            .join(name);
        parse_xcursor(&std::fs::read(path).ok()?).filter(|images| !images.is_empty())
    }

    /// The procedural cursor `name` at one, two and four times the nominal size (output scales and the cursor scale
    /// pick from them as from a theme's sizes).
    fn procedural(size: u32, name: &str) -> Option<Vec<Image>> {
        let kind = cupertino_cursor::procedural::Kind::from_name(name)?;
        let mut options = cupertino_cursor::theme::BuildOptions::new(PathBuf::new());
        options.sizes = vec![size, size * 2, size * 4];
        let file = cupertino_cursor::theme::procedural_cursor(kind, &options, Vec::new())
            .to_xcursor()
            .unwrap_or_else(|e| panic!("procedural cursor {name}: {e}"));
        parse_xcursor(&file)
    }

    /// Nominal cursor size at scale 1, pixels per logical point.
    pub fn size(&self) -> u32 {
        self.size
    }

    /// `XCURSOR_SIZE` for the clients the server starts, so the cursors they draw follow the user
    /// cursor scale: the nominal size times `scale`, rounded.
    pub fn client_size(&self, scale: f64) -> u32 {
        (f64::from(self.size) * scale).round() as u32
    }

    fn cache_images(&mut self, name: &'static str, images: Option<Vec<Image>>) {
        let mut images = images.unwrap_or_else(|| self.default_images.clone());
        if self.colors != CursorColors::DEFAULT {
            for image in &mut images {
                recolor(&mut image.pixels_rgba, self.colors);
            }
        }
        self.cache.insert(name, images);
    }

    /// While a new shape is being read the default arrow remains usable. Each name can be queued
    /// only once, even when a mounted theme stops answering.
    fn ready_name(&mut self, icon: CursorIcon) -> &'static str {
        while let Ok((name, images)) = self.ready.try_recv() {
            self.pending.remove(name);
            self.cache_images(name, images);
        }
        let name = icon.name();
        if self.cache.contains_key(name) {
            return name;
        }
        let default = CursorIcon::Default.name();
        if name != default && self.pending.insert(name) {
            if let Some(reader) = &self.requests {
                let _ = reader.send(name);
            }
        }
        if !self.cache.contains_key(default) {
            self.cache_images(default, None);
        }
        default
    }

    /// Frame for `icon` drawn `pixels` tall (nominal size times cursor scale times output scale)
    /// at animation time `time`: the smallest image of the theme at least that size, else the
    /// largest, so the renderer scales down where it can.
    pub fn frame(&mut self, icon: CursorIcon, pixels: u32, time: Duration) -> CursorFrame {
        let name = self.ready_name(icon);
        let index = pick_frame(&self.cache[name], pixels, time.as_millis() as u32);
        // the image stays in the cache: only its pixels' first frame makes a buffer of them
        let img = &self.cache[name][index];
        let key = (name, index);
        let entry = self.buffers.entry(key).or_insert_with(|| {
            let buf = MemoryRenderBuffer::from_slice(
                &img.pixels_rgba,
                Fourcc::Argb8888,
                (img.width as i32, img.height as i32),
                1,
                Transform::Normal,
                None,
            );
            (Point::from((img.xhot as i32, img.yhot as i32)), buf)
        });
        CursorFrame {
            buffer: entry.1.clone(),
            hotspot: entry.0,
            pixels: (img.width, img.height),
            nominal: img.size,
        }
    }
}

/// The image for `size`: the smallest nominal size at least `size`, else the largest; then the
/// animation frame of that size at `millis`.
fn pick_frame(images: &[Image], size: u32, millis: u32) -> usize {
    let largest = images
        .iter()
        .map(|i| i.size)
        .max()
        .expect("at least one image");
    let nominal = images
        .iter()
        .map(|i| i.size)
        .filter(|s| *s >= size)
        .min()
        .unwrap_or(largest);
    let same: Vec<usize> = (0..images.len())
        .filter(|&i| images[i].size == nominal)
        .collect();
    let total: u32 = same.iter().map(|&i| images[i].delay).sum();
    if total == 0 {
        return same[0];
    }
    let mut m = millis % total;
    for &i in &same {
        if m < images[i].delay {
            return i;
        }
        m -= images[i].delay;
    }
    same[0]
}

/// Shake to locate: the detector and the timer that runs its evaluations while the cursor is grown.
#[derive(Debug, Default)]
pub struct CursorShake {
    pub detector: cupertino_input::ShakeDetector,
    pub timer: Option<RegistrationToken>,
}
