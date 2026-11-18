//! Icons of files and folders, for the apps that list them: the icon of a file's type from the asset packs and the host
//! icon theme ([`cupertino_icons::document`]), a special folder's own icon, and the alias badge of a symbolic link, as
//! the renditions an [`Image`](cupertino_ui_controls::Image) picks from.
//!
//! A type gets the icon of its kind or of its nearest ancestor's kind ([`document_icon_name`]), then
//! `document.generic`, then the placeholder the packs draw for a name none of them provides.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use cupertino_apps::mime::{MimeDatabase, DIRECTORY};
use cupertino_assets::Assets;
use cupertino_icons::document::{self, badge_rect, Badge};
use cupertino_icons::SystemIcons;
use cupertino_ui_controls::ImageSource;
use cupertino_ui_core::next_resource_id;
use cupertino_ui_render::{ImageResource, PixelFormat};

pub use cupertino_icons::document::{document_icon_name, FILE_ICON_NAMES};

/// The scales each icon is drawn at: the image draws the smallest that covers its box at the window's scale.
pub const RENDITION_SCALES: [u32; 2] = [1, 2];

/// What one set of renditions shows: a special folder's own icon or the icon of a kind, with the alias badge or
/// without, at a size in points (as bits, so the key hashes).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct Drawn {
    folder: Option<&'static str>,
    kind: &'static str,
    alias: bool,
    points: u64,
}

/// File and folder icons as image renditions, by path, type and size, each drawn once for the life of the value: an
/// image keeps its resource id, so a repainted cell names the same pixels.
pub struct FileIcons {
    icons: document::FileIcons,
    /// Folders with an icon of their own (the home folder, the user directories) and its semantic name.
    folders: HashMap<PathBuf, &'static str>,
    /// The semantic name of each type asked for.
    kinds: HashMap<String, &'static str>,
    drawn: HashMap<Drawn, Arc<[ImageSource]>>,
}

impl FileIcons {
    /// File icons of the installed asset packs and the host icon theme, typed with `db`; `folders` wear their own icon.
    pub fn discover(db: MimeDatabase, folders: HashMap<PathBuf, &'static str>) -> Self {
        Self::new(&Assets::discover(), db, folders)
    }

    /// File icons of `assets` and the host icon theme. An icon of [`FILE_ICON_NAMES`] or of `folders` that nothing
    /// provides is reported once on stderr.
    pub fn new(assets: &Assets, db: MimeDatabase, folders: HashMap<PathBuf, &'static str>) -> Self {
        Self::of(
            document::FileIcons::new(assets, db, folders.clone()),
            folders,
        )
    }

    /// File icons of the built-in assets alone: every icon is the placeholder. What an app draws before it is given
    /// the icons of the session, and what its tests draw.
    pub fn builtin(db: MimeDatabase, folders: HashMap<PathBuf, &'static str>) -> Self {
        Self::without_host(&Assets::builtin(), db, folders)
    }

    /// File icons of `assets` alone, never of the machine's icon themes: for tests and captures.
    pub fn without_host(
        assets: &Assets,
        db: MimeDatabase,
        folders: HashMap<PathBuf, &'static str>,
    ) -> Self {
        let system = SystemIcons::without_host(assets);
        Self::of(
            document::FileIcons::with_system(system, db, folders.clone()),
            folders,
        )
    }

    fn of(icons: document::FileIcons, folders: HashMap<PathBuf, &'static str>) -> Self {
        Self {
            icons,
            folders,
            kinds: HashMap::new(),
            drawn: HashMap::new(),
        }
    }

    /// The MIME database the icons are resolved with.
    pub fn database(&self) -> &MimeDatabase {
        self.icons.database()
    }

    /// The semantic name of the icon the item at `path` of type `mime` asks for: a special folder's own name, else
    /// the name of its type's kind. A name the packs lack is drawn as a more generic one.
    pub fn name(&mut self, path: &Path, mime: &str) -> &'static str {
        let (folder, kind) = self.names(path, mime);
        folder.unwrap_or(kind)
    }

    fn names(&mut self, path: &Path, mime: &str) -> (Option<&'static str>, &'static str) {
        let db = self.icons.database();
        let kind = match self.kinds.get(mime) {
            Some(&kind) => kind,
            None => {
                let kind = document_icon_name(db, mime);
                self.kinds.insert(mime.to_owned(), kind);
                kind
            }
        };
        let folder = (db.canonical(mime) == DIRECTORY)
            .then(|| self.folders.get(path).copied())
            .flatten();
        (folder, kind)
    }

    /// Whether an asset pack gives the icon `name` (a name [`FileIcons::name`] returns) as a file of its own, rather than
    /// through the host icon theme: an app that draws its own art for some kinds keeps a pack's.
    pub fn pack_has(&self, name: &str) -> bool {
        self.icons.pack_has(name)
    }

    /// The renditions of the icon of the item at `path` of type `mime`, `points` square, at each of
    /// [`RENDITION_SCALES`]; `alias` puts the alias badge on its bottom-left corner (a symbolic link).
    pub fn images(
        &mut self,
        path: &Path,
        mime: &str,
        alias: bool,
        points: f64,
    ) -> Vec<ImageSource> {
        let (folder, kind) = self.names(path, mime);
        let key = Drawn {
            folder,
            kind,
            alias,
            points: points.to_bits(),
        };
        if let Some(images) = self.drawn.get(&key) {
            return images.to_vec();
        }
        let images: Arc<[ImageSource]> = RENDITION_SCALES
            .iter()
            .map(|&scale| {
                let px = (points * f64::from(scale)).round().max(1.0) as u32;
                // a file of another size is fitted to the rendition, centred, with its aspect ratio kept
                let icon = self.icons.document(path, mime, px);
                let (iw, ih) = (icon.width(), icon.height());
                let mut pixels = if (iw, ih) == (px, px) {
                    icon.data().to_vec()
                } else {
                    let fit = f64::from(px) / f64::from(iw.max(ih));
                    let (w, h) = (
                        (f64::from(iw) * fit).round().max(1.0) as u32,
                        (f64::from(ih) * fit).round().max(1.0) as u32,
                    );
                    let mut fitted = vec![0; (px * px * 4) as usize];
                    let area = Area {
                        x: i64::from((px - w) / 2),
                        y: i64::from((px - h) / 2),
                        w,
                        h,
                    };
                    draw_over(&mut fitted, (px, px), icon.data(), (iw, ih), area);
                    fitted
                };
                if alias {
                    // the badge where the recipe puts it
                    let (x, y, side) = badge_rect(points);
                    let ratio = f64::from(px) / points;
                    let bpx = (side * ratio).round().max(1.0) as u32;
                    let badge = self.icons.badge(Badge::Alias, bpx);
                    let area = Area {
                        x: (x * ratio).round() as i64,
                        y: (y * ratio).round() as i64,
                        w: bpx,
                        h: bpx,
                    };
                    draw_over(
                        &mut pixels,
                        (px, px),
                        badge.data(),
                        (badge.width(), badge.height()),
                        area,
                    );
                }
                let (width, height) = (px, px);
                ImageSource {
                    id: next_resource_id(),
                    image: ImageResource {
                        revision: 1,
                        width,
                        height,
                        stride: width * 4,
                        format: PixelFormat::Rgba8Premultiplied,
                        pixels: Arc::from(pixels),
                    },
                }
            })
            .collect();
        self.drawn.insert(key, Arc::clone(&images));
        images.to_vec()
    }
}

/// A rectangle of device pixels: its top left corner and its size.
#[derive(Clone, Copy, Debug)]
struct Area {
    x: i64,
    y: i64,
    w: u32,
    h: u32,
}

/// Draw the premultiplied `src` over the premultiplied `dst`, stretched to `to`: each pixel of `to` takes the average of
/// the source pixels it covers, and pixels outside `dst` are dropped.
fn draw_over(dst: &mut [u8], (dw, dh): (u32, u32), src: &[u8], (sw, sh): (u32, u32), to: Area) {
    if sw == 0 || sh == 0 || to.w == 0 || to.h == 0 {
        return;
    }
    for ty in 0..to.h {
        let y = to.y + i64::from(ty);
        if y < 0 || y >= i64::from(dh) {
            continue;
        }
        let (y0, y1) = span(ty, to.h, sh);
        for tx in 0..to.w {
            let x = to.x + i64::from(tx);
            if x < 0 || x >= i64::from(dw) {
                continue;
            }
            let (x0, x1) = span(tx, to.w, sw);
            let mut sum = [0u32; 4];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let at = ((sy * sw + sx) * 4) as usize;
                    for (s, c) in sum.iter_mut().zip(&src[at..at + 4]) {
                        *s += u32::from(*c);
                    }
                }
            }
            let n = (y1 - y0) * (x1 - x0);
            let s = sum.map(|c| (c + n / 2) / n);
            let at = ((y as u64 * u64::from(dw) + x as u64) * 4) as usize;
            let keep = 255 - s[3];
            for (dc, sc) in dst[at..at + 4].iter_mut().zip(s) {
                *dc = (sc + (u32::from(*dc) * keep + 127) / 255).min(255) as u8;
            }
        }
    }
}

/// The source pixels that pixel `t` of a target `side` pixels long covers on a source `len` pixels long: at least one.
fn span(t: u32, side: u32, len: u32) -> (u32, u32) {
    let (t, side, len64) = (u64::from(t), u64::from(side), u64::from(len));
    let start = (t * len64 / side) as u32;
    let end = ((t + 1) * len64).div_ceil(side) as u32;
    (start.min(len - 1), end.clamp(start.min(len - 1) + 1, len))
}
