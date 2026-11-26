//! The icons of menu rows and of status items, read from the icon themes and the asset packs or decoded from what an
//! item sent. Everything here reads files or decodes images, so it runs on Kit's I/O pool (`cupertino_kit::task::io`),
//! never on the component's UI thread; the view draws what was loaded ([`crate::view::RowIcons`]).

use crate::dbus::sni::{self, StatusItem};
use crate::extras;
use crate::feed;
use cupertino_assets::Assets;
use cupertino_icons::{SystemIcons, ThemeIcons};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

pub use cupertino_icons::IconImage as Pixels;

/// How long loading one icon should take: a few files of a local icon theme, read and decoded.
pub const LOAD_DEADLINE: Duration = Duration::from_secs(2);

/// One icon of a menu row to load: what the row sent inline, else the name it gives, at `px` device pixels.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Request {
    /// Identifies the icon and its size in the view's cache.
    pub key: (u64, u32),
    pub data: Option<Vec<u8>>,
    pub name: Option<String>,
    /// The directories the row's exporter names for its icons (DBusMenu `IconThemePath`), searched for `name` before
    /// the icon themes.
    pub dirs: Vec<PathBuf>,
    pub px: u32,
}

/// The icon themes and the asset packs, looked up on the I/O pool. One loader serves the component's tasks in turn.
#[derive(Clone, Default)]
pub struct Loader {
    packs: Arc<Mutex<Option<Packs>>>,
}

struct Packs {
    themed: ThemeIcons,
    /// The template symbols of the asset packs, which rows name as `symbol.*`.
    system: SystemIcons,
}

impl Loader {
    /// Find the packs and the themes, reporting the bar's own symbols a pack lacks. Called on the I/O pool, once at
    /// the start; an icon asked for before that finds them itself.
    pub fn prepare(&self) {
        self.with(|_| ());
    }

    fn with<T>(&self, f: impl FnOnce(&mut Packs) -> T) -> T {
        let mut packs = self.packs.lock().unwrap_or_else(PoisonError::into_inner);
        let packs = packs.get_or_insert_with(|| {
            let assets = Assets::discover();
            Packs {
                themed: ThemeIcons::new(&assets),
                // the extras' symbols are drawn by the toolkit from the same packs; a missing one is reported here
                system: SystemIcons::required(&assets, &[], &extras::SYMBOL_NAMES),
            }
        });
        f(packs)
    }

    /// The themed icon `name` at `px` device pixels.
    pub fn themed(&self, name: &str, px: u32) -> Option<Pixels> {
        self.with(|packs| packs.themed.load(name, px))
            .map(|pixmap| Pixels {
                width: pixmap.width(),
                height: pixmap.height(),
                rgba: pixmap.data().to_vec(),
            })
    }

    /// The icon of a menu row: what it sent inline, else its themed name, a template symbol drawn in black (it takes
    /// the row's colour when painted); `None` without one.
    pub fn row_icon(&self, request: &Request) -> Option<Pixels> {
        let from_data = request
            .data
            .as_deref()
            .and_then(|data| cupertino_icons::decode_png(data, request.px));
        from_data.or_else(|| {
            let name = request.name.as_deref()?;
            if name.starts_with("symbol.") {
                let black = [0.0, 0.0, 0.0, 1.0];
                return self.with(|packs| {
                    let pixmap = packs.system.symbol(name, request.px, black);
                    Some(Pixels {
                        width: pixmap.width(),
                        height: pixmap.height(),
                        rgba: pixmap.data().to_vec(),
                    })
                });
            }
            self.named_icon(name, &request.dirs, request.px)
        })
    }

    /// The icon of a status item at `px` device pixels: its pixmap, else the icon it names, looked up in its
    /// `IconThemePath` first ([`Loader::named_icon`]); its overlay, when it has one, over the bottom right corner.
    pub fn status_icon(&self, item: &StatusItem, px: u32) -> Option<Pixels> {
        let dirs = [PathBuf::from(&item.icon_theme_path)];
        let icon = feed::status_icon(item, px, |name, px| self.named_icon(name, &dirs, px))?;
        let overlay_px = ((f64::from(px) * OVERLAY_SHARE).round() as u32).max(1);
        let overlay = sni::best_pixmap(&item.overlay_pixmaps, overlay_px as i32)
            .and_then(|p| cupertino_icons::from_argb(p.width, p.height, &p.argb))
            .or_else(|| {
                Some(item.overlay_icon_name.as_str())
                    .filter(|name| !name.is_empty())
                    .and_then(|name| self.named_icon(name, &dirs, overlay_px))
            });
        Some(match overlay {
            Some(overlay) => overlaid(icon, &overlay),
            None => icon,
        })
    }

    /// The icon `name` at `px` device pixels, looked up first in `dirs`, the directories of a status item's or a
    /// menu's own icons (their `IconThemePath`; an empty path stands for none) in turn, then in the system's icon
    /// themes ([`Loader::themed`]). Each directory is searched by [`find_in_theme_path`].
    pub fn named_icon(&self, name: &str, dirs: &[PathBuf], px: u32) -> Option<Pixels> {
        dirs.iter()
            .filter_map(|dir| find_in_theme_path(dir, name, px))
            .find_map(|path| cupertino_icons::load_icon(&path, px))
            .or_else(|| self.themed(name, px))
    }
}

/// How much of a status icon's width and height its overlay covers, at its bottom right corner.
const OVERLAY_SHARE: f64 = 0.5;
/// Most entries read of one directory while an icon is looked up in an item's or a menu's own icons; past it the rest
/// are not looked at, so a directory of any size costs a bounded time.
const MAX_DIR_ENTRIES: usize = 256;
/// The file types an item's or a menu's own icons are read in.
const ICON_EXTENSIONS: [&str; 2] = ["png", "svg"];
/// The theme a base of themes is searched in first.
const BASE_THEME: &str = "hicolor";

/// How well a file of an icon directory fits `px`: lower is better.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Fit {
    Exact,
    Scalable,
    Larger(u32),
    Smaller(u32),
    Unsized,
}

/// The size a theme's size directory stands for (`22x22`, `22x22@2`, `22`); `None` for `scalable` and the like, and
/// for a name that is no size.
fn dir_size(name: &str) -> Option<u32> {
    let (size, scale) = match name.split_once('@') {
        Some((size, scale)) => (size, scale.parse::<u32>().ok()?),
        None => (name, 1),
    };
    let edge = match size.split_once('x') {
        Some((w, h)) if w == h => w,
        Some(_) => return None,
        None => size,
    };
    edge.parse::<u32>().ok()?.checked_mul(scale)
}

fn fit(size: Option<u32>, scalable_dir: bool, path: &Path, px: u32) -> Fit {
    let svg = path.extension().and_then(|e| e.to_str()) == Some("svg");
    match size {
        Some(size) if size == px => Fit::Exact,
        // a drawing in a directory of one size stands for that size, as a picture does
        _ if scalable_dir || (svg && size.is_none()) => Fit::Scalable,
        Some(size) if size > px => Fit::Larger(size - px),
        Some(size) => Fit::Smaller(px - size),
        None => Fit::Unsized,
    }
}

/// The directories under `dir`, at most [`MAX_DIR_ENTRIES`] of them, by name.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io), as every icon does"
)]
fn subdirs(dir: &Path) -> Vec<(String, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut dirs: Vec<(String, PathBuf)> = entries
        .take(MAX_DIR_ENTRIES)
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_ok_and(|t| t.is_dir()))
        .filter_map(|e| Some((e.file_name().into_string().ok()?, e.path())))
        .collect();
    dirs.sort();
    dirs
}

/// The file of icon `stem` directly in `dir`, by the first extension that has one.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io), as every icon does"
)]
fn icon_file(dir: &Path, stem: &str) -> Option<PathBuf> {
    ICON_EXTENSIONS
        .iter()
        .map(|ext| dir.join(format!("{stem}.{ext}")))
        .find(|p| p.is_file())
}

/// The best file of icon `stem` in the theme directory `theme` (`<size>/<context>/<stem>` or `<size>/<stem>`).
fn in_theme(theme: &Path, stem: &str, px: u32) -> Option<PathBuf> {
    let mut best: Option<(Fit, PathBuf)> = None;
    for (name, size_dir) in subdirs(theme) {
        let size = dir_size(&name);
        let scalable = name == "scalable";
        let contexts = subdirs(&size_dir).into_iter().map(|(_, path)| path);
        for dir in std::iter::once(size_dir.clone()).chain(contexts) {
            if let Some(path) = icon_file(&dir, stem) {
                let fit = fit(size, scalable, &path, px);
                if best.as_ref().is_none_or(|(b, _)| fit < *b) {
                    best = Some((fit, path));
                }
            }
        }
    }
    best.map(|(_, path)| path)
}

/// The file of icon `name` at `px` device pixels in `dir`, a directory of an item's or a menu's own icons: laid flat in
/// it (`<dir>/<name>.png`), else in the `hicolor` theme under it (`<dir>/hicolor/<size>/<context>/<name>.svg`), else in
/// it as a theme directory (`<dir>/<size>/<context>/<name>.png`), else in the other themes under it; within a theme
/// the size nearest `px` wins, and at most [`MAX_DIR_ENTRIES`] entries of a directory are read. `None` for an empty or
/// missing directory and for a name that is a path.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io), as every icon does"
)]
pub fn find_in_theme_path(dir: &Path, name: &str, px: u32) -> Option<PathBuf> {
    if dir.as_os_str().is_empty() || name.is_empty() || name.contains('/') || !dir.is_dir() {
        return None;
    }
    let stem = ICON_EXTENSIONS
        .iter()
        .find_map(|ext| name.strip_suffix(&format!(".{ext}")))
        .unwrap_or(name);
    if let Some(flat) = icon_file(dir, stem) {
        return Some(flat);
    }
    if let Some(found) = in_theme(&dir.join(BASE_THEME), stem, px) {
        return Some(found);
    }
    if let Some(found) = in_theme(dir, stem, px) {
        return Some(found);
    }
    subdirs(dir)
        .into_iter()
        .filter(|(name, _)| name != BASE_THEME)
        .find_map(|(_, theme)| in_theme(&theme, stem, px))
}

/// `icon` with `overlay` drawn over its bottom right corner, scaled to [`OVERLAY_SHARE`] of its size (both
/// premultiplied).
pub fn overlaid(mut icon: Pixels, overlay: &Pixels) -> Pixels {
    let (w, h) = (icon.width as usize, icon.height as usize);
    let ow = ((w as f64 * OVERLAY_SHARE).round() as usize).max(1);
    let oh = ((h as f64 * OVERLAY_SHARE).round() as usize).max(1);
    if overlay.width == 0 || overlay.height == 0 || ow > w || oh > h {
        return icon;
    }
    let (sw, sh) = (overlay.width as usize, overlay.height as usize);
    for y in 0..oh {
        for x in 0..ow {
            // the overlay's pixel nearest the centre of this one
            let sx = ((2 * x + 1) * sw / (2 * ow)).min(sw - 1);
            let sy = ((2 * y + 1) * sh / (2 * oh)).min(sh - 1);
            let src = &overlay.rgba[(sy * sw + sx) * 4..][..4];
            let at = ((h - oh + y) * w + (w - ow + x)) * 4;
            let dst = &mut icon.rgba[at..at + 4];
            let keep = 255 - u32::from(src[3]);
            for c in 0..4 {
                dst[c] =
                    (u32::from(src[c]) + (u32::from(dst[c]) * keep + 127) / 255).min(255) as u8;
            }
        }
    }
    icon
}
