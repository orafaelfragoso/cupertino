//! The icons the dock shows, loaded for the toolkit's images on Kit's I/O pool: an application's own icon (an absolute
//! path or the `hicolor` theme), the icon a window set for itself, and the system's generic application and trash icons
//! from the asset directory, the themes' and the packs' read through the process's shared icons
//! ([`cupertino_kit::shared`]). Every surface of the dock draws them through CupertinoKit ([`crate::view`], the switcher's
//! panel of [`crate::switcher_kit`]). Nothing here runs on the UI thread but [`WindowImage::new`], which only reorders
//! the pixels the server sent.

use std::collections::HashMap;
use std::sync::Arc;

use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, IconImage};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_kit::shared;

/// Semantic names of the generic application icon and of the trash's two states.
const GENERIC_APPLICATION_ICON: &str = "application.generic";
/// The asset packs' icons a named icon may be, when the theme has none of the name: the special folders' and the kinds
/// of documents' (what a folder's stack shows).
const PACKED_NAMES: [&str; 17] = [
    "folder",
    "folder.home",
    "folder.desktop",
    "folder.documents",
    "folder.downloads",
    "folder.music",
    "folder.pictures",
    "folder.movies",
    "document.pdf",
    "document.image",
    "document.audio",
    "document.video",
    "document.archive",
    "document.text",
    "document.executable",
    "document.generic",
    "folder.applications",
];
const TRASH_EMPTY_ICON: &str = "trash.empty";
const TRASH_FULL_ICON: &str = "trash.full";

/// An icon whose drawn shape covers less than this share of its image is loaded again larger and cropped to the
/// shape, so every icon's artwork fills the square it is drawn in, whatever margin its file leaves.
const FILLED_SHARE: f64 = 0.97;
/// The most an icon is enlarged to fill its square: a shape smaller than a quarter of its image keeps its margin.
const MAX_FILL_SCALE: f64 = 4.0;
/// A pixel at least this opaque is part of an icon's shape.
const SHAPE_ALPHA: u8 = 8;

/// An image of premultiplied RGBA8 pixels, rows top to bottom.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Raster {
    /// A transparent image `width` × `height` pixels large.
    fn transparent(width: u32, height: u32) -> Self {
        Self {
            width,
            height,
            data: vec![0; width as usize * height as usize * 4],
        }
    }

    /// The pixel at `(x, y)`, the nearest edge pixel for one outside.
    fn at(&self, x: i64, y: i64) -> [u8; 4] {
        let x = x.clamp(0, i64::from(self.width) - 1) as usize;
        let y = y.clamp(0, i64::from(self.height) - 1) as usize;
        let i = (y * self.width as usize + x) * 4;
        [
            self.data[i],
            self.data[i + 1],
            self.data[i + 2],
            self.data[i + 3],
        ]
    }

    /// The image as the toolkit draws it.
    pub fn image(&self) -> ImageSource {
        ImageSource {
            id: next_resource_id(),
            image: ImageResource {
                revision: 0,
                width: self.width,
                height: self.height,
                stride: self.width * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(self.data.as_slice()),
            },
        }
    }
}

/// The square of `image` around its drawn shape, `(x, y, side)`: the bounding box of the pixels at least
/// [`SHAPE_ALPHA`] opaque, made square about its centre and kept inside the image. `None` when nothing is drawn.
pub fn shape_square(image: &Raster) -> Option<(u32, u32, u32)> {
    let (w, h) = (image.width, image.height);
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (i, px) in image.data.as_chunks::<4>().0.iter().enumerate() {
        if px[3] >= SHAPE_ALPHA {
            let (x, y) = (i as u32 % w, i as u32 / w);
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x + 1);
            y1 = y1.max(y + 1);
        }
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let side = (x1 - x0).max(y1 - y0).min(w).min(h);
    let centre = |lo: u32, hi: u32, limit: u32| -> u32 {
        let start = (lo + hi).saturating_sub(side) / 2;
        start.min(limit - side)
    };
    Some((centre(x0, x1, w), centre(y0, y1, h), side))
}

/// `image` cropped to the square `(x, y, side)`.
fn cropped(image: &Raster, (x, y, side): (u32, u32, u32)) -> Raster {
    let mut out = Raster::transparent(side, side);
    let row = side as usize * 4;
    for r in 0..side.min(image.height.saturating_sub(y)) {
        let from = ((y + r) as usize * image.width as usize + x as usize) * 4;
        let width = (side.min(image.width.saturating_sub(x)) as usize) * 4;
        out.data[r as usize * row..r as usize * row + width]
            .copy_from_slice(&image.data[from..from + width]);
    }
    out
}

/// `image` scaled to `side` × `side` pixels, each pixel sampled bilinearly at its centre, the edge pixels repeated
/// past the image's edge.
fn bilinear(image: &Raster, side: u32) -> Raster {
    let mut out = Raster::transparent(side, side);
    let (kx, ky) = (
        f64::from(image.width) / f64::from(side),
        f64::from(image.height) / f64::from(side),
    );
    for (i, px) in out.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i as u32 % side, i as u32 / side);
        let u = (f64::from(x) + 0.5) * kx - 0.5;
        let v = (f64::from(y) + 0.5) * ky - 0.5;
        let (x0, y0) = (u.floor(), v.floor());
        let (fx, fy) = (u - x0, v - y0);
        let (x0, y0) = (x0 as i64, y0 as i64);
        let corners = [
            (image.at(x0, y0), (1.0 - fx) * (1.0 - fy)),
            (image.at(x0 + 1, y0), fx * (1.0 - fy)),
            (image.at(x0, y0 + 1), (1.0 - fx) * fy),
            (image.at(x0 + 1, y0 + 1), fx * fy),
        ];
        for (c, value) in px.iter_mut().enumerate() {
            let sum: f64 = corners.iter().map(|(p, w)| f64::from(p[c]) * w).sum();
            *value = sum.round().clamp(0.0, 255.0) as u8;
        }
    }
    out
}

/// A square image at exactly `px` a side: resampled down when larger, scaled up bilinearly when smaller.
fn sized(image: Raster, px: u32) -> Raster {
    let px = px.max(1);
    if image.width >= px {
        return resampled(&image, px);
    }
    bilinear(&image, px)
}

/// The icon `load` gives at `px` a side, its drawn shape filling the square: when the shape covers less than
/// [`FILLED_SHARE`] of the image, the icon is loaded again as much larger as its margin and cropped to the shape, so
/// it stays as sharp as when drawn at its own size.
pub fn filled(px: u32, mut load: impl FnMut(u32) -> Option<Raster>) -> Option<Raster> {
    let first = load(px)?;
    let Some(shape) = shape_square(&first) else {
        return Some(first);
    };
    let share = f64::from(shape.2) / f64::from(first.width.max(1));
    if share >= FILLED_SHARE || share * MAX_FILL_SCALE < 1.0 {
        return Some(first);
    }
    let larger = (f64::from(px) / share).ceil() as u32;
    let image = match load(larger) {
        Some(image) if image.width >= larger => image,
        _ => return Some(sized(cropped(&first, shape), px)),
    };
    match shape_square(&image) {
        Some(square) => Some(sized(cropped(&image, square), px)),
        None => Some(first),
    }
}

/// A square image at half its side, each pixel the mean of the (up to) four it covers.
fn halved(image: &Raster) -> Raster {
    let side = image.width;
    let half = side.div_ceil(2);
    let src = &image.data;
    let mut out = Raster::transparent(half, half);
    let at = |x: u32, y: u32| (y.min(side - 1) * side + x.min(side - 1)) as usize * 4;
    for (i, px) in out.data.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (2 * (i as u32 % half), 2 * (i as u32 / half));
        let corners = [at(x, y), at(x + 1, y), at(x, y + 1), at(x + 1, y + 1)];
        for (c, v) in px.iter_mut().enumerate() {
            let sum: u32 = corners.iter().map(|&o| u32::from(src[o + c])).sum();
            *v = ((sum + 2) / 4) as u8;
        }
    }
    out
}

/// A square image larger than `px` at `px` a side: halved while it is at least twice as large, then bilinear.
fn resampled(image: &Raster, px: u32) -> Raster {
    let px = px.max(1);
    let mut image = image.clone();
    while image.width >= 2 * px {
        image = halved(&image);
    }
    if image.width == px {
        return image;
    }
    bilinear(&image, px)
}

/// The icon a window set for itself: a theme name, an image, or both. Sent with each icon job of its application.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowImage {
    pub name: Option<String>,
    pub image: Option<Arc<Raster>>,
}

impl WindowImage {
    /// The icon of the server's `name` and square `side` × `side` image of premultiplied ARGB8888 pixels,
    /// little-endian; a colour channel over its alpha is cut to it. `None` when it names nothing and has no image.
    pub fn new(name: Option<String>, image: Option<(u32, &[u8])>) -> Option<Self> {
        let image = image
            .filter(|(side, argb)| *side > 0 && argb.len() == *side as usize * *side as usize * 4)
            .map(|(side, argb)| {
                let data = argb
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .flat_map(|&[b, g, r, a]| [r.min(a), g.min(a), b.min(a), a])
                    .collect();
                Arc::new(Raster {
                    width: side,
                    height: side,
                    data,
                })
            });
        (name.is_some() || image.is_some()).then_some(Self { name, image })
    }

    /// The image at `px`: cropped to its shape, resampled when larger, as it is when not.
    fn at(&self, px: u32) -> Option<Raster> {
        let image = self.image.as_deref()?;
        let shape = shape_square(image).map_or_else(|| image.clone(), |s| cropped(image, s));
        Some(if shape.width <= px {
            shape
        } else {
            resampled(&shape, px)
        })
    }
}

/// One icon to load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Job {
    /// An application's icon: its desktop entry's theme name (`entry`), else the icon its window set, else the theme's
    /// icon named as the application, else the generic application icon.
    App {
        app: String,
        entry: Option<String>,
        window: Option<WindowImage>,
    },
    /// The theme's icon of this name, else the asset packs' of the name, else the generic application icon.
    Named(String),
    /// The trash, full or empty.
    Trash(bool),
}

/// Icon images by name and pixel size, filled to their squares once for the instance: an application's own icon (an
/// absolute path or the `hicolor` theme) and the system's icons from the asset directory, both read through the
/// process's shared icons. It reads files: it lives on Kit's I/O pool.
pub struct Icons {
    cache: HashMap<(String, u32), Option<Raster>>,
    /// The system's icons by name and pixel size, filled to their squares.
    system_cache: HashMap<(&'static str, u32), Raster>,
}

/// The pixels of a shared icon.
fn raster(image: &IconImage) -> Raster {
    Raster {
        width: image.image.width,
        height: image.image.height,
        data: image.image.pixels.to_vec(),
    }
}

/// The pixel size at which [`Icons::new`] looks whether the asset packs give the dock's own icons.
const REPORTED_PX: u32 = 32;

impl Icons {
    /// Nothing loaded yet. The generic application icon and the trash's are reported when no asset pack provides them;
    /// they are then drawn as placeholders.
    #[expect(
        clippy::new_without_default,
        reason = "it reads the asset packs: made on Kit's I/O pool, never by default"
    )]
    pub fn new() -> Self {
        let missing: Vec<String> = [GENERIC_APPLICATION_ICON, TRASH_EMPTY_ICON, TRASH_FULL_ICON]
            .into_iter()
            .filter(|name| !shared::pack_has_raster(name, REPORTED_PX))
            .map(|name| format!("`{name}`"))
            .collect();
        if !missing.is_empty() {
            eprintln!(
                "cupertino-dock: no asset pack provides the icon {}; it is drawn as a placeholder",
                missing.join(", ")
            );
        }
        Self {
            cache: HashMap::new(),
            system_cache: HashMap::new(),
        }
    }

    /// What `job` loads at `px` a side.
    pub fn load(&mut self, job: &Job, px: u32) -> Raster {
        match job {
            Job::App { app, entry, window } => self.app(app, entry.as_deref(), window.as_ref(), px),
            Job::Named(name) => self.named(name, px),
            Job::Trash(full) => self.system_icon(
                if *full {
                    TRASH_FULL_ICON
                } else {
                    TRASH_EMPTY_ICON
                },
                px,
            ),
        }
    }

    /// The theme's icon `name` at `px`, filled to its square, loaded once; `None` when the theme has none.
    fn themed(&mut self, name: &str, px: u32) -> Option<Raster> {
        self.cache
            .entry((name.to_owned(), px))
            .or_insert_with(|| {
                filled(px, |side| {
                    shared::theme_icon(name, side).as_ref().map(raster)
                })
            })
            .clone()
    }

    /// The system's icon `name` at `px`, filled to its square.
    fn system_icon(&mut self, name: &'static str, px: u32) -> Raster {
        self.system_cache
            .entry((name, px))
            .or_insert_with(|| {
                filled(px, |side| {
                    shared::pack_raster(name, side).as_ref().map(raster)
                })
                .unwrap_or_else(|| Raster::transparent(px.max(1), px.max(1)))
            })
            .clone()
    }

    /// The theme's icon `name`, else the asset packs' icon of that name (a folder's or a kind of document's,
    /// `folder.downloads` or `document.pdf`), else the asset `application.generic`.
    fn named(&mut self, name: &str, px: u32) -> Raster {
        if let Some(icon) = self.themed(name, px) {
            return icon;
        }
        if let Some(packed) = PACKED_NAMES.iter().find(|n| **n == name) {
            if shared::pack_has_raster(packed, px) {
                return self.system_icon(packed, px);
            }
        }
        self.system_icon(GENERIC_APPLICATION_ICON, px)
    }

    /// The icon of an application: its desktop entry's, else the one its window set for itself (a theme name, then
    /// its image), else the theme's icon named as the application, else the asset `application.generic`.
    fn app(
        &mut self,
        app: &str,
        entry: Option<&str>,
        window: Option<&WindowImage>,
        px: u32,
    ) -> Raster {
        if let Some(icon) = entry.and_then(|name| self.themed(name, px)) {
            return icon;
        }
        if let Some(window) = window {
            if let Some(icon) = window.name.as_deref().and_then(|n| self.themed(n, px)) {
                return icon;
            }
            if let Some(icon) = window.at(px) {
                return icon;
            }
        }
        self.themed(app, px)
            .unwrap_or_else(|| self.system_icon(GENERIC_APPLICATION_ICON, px))
    }
}
