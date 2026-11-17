//! The system's symbols from the asset packs, and the running application's own icon, for the widgets' [`Icons`].

use cupertino_assets::{Assets, IconName};
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_theme::ui::Color;
use cupertino_ui_core::{next_resource_id, IconImage, Icons, PlaceholderIcons};
use cupertino_ui_render::{ImageResource, PixelFormat};
use std::collections::HashMap;
use std::fmt;
use std::hash::Hash;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, PoisonError};

/// A symbol as asked for: its name, device pixels, colour in 8-bit steps, and whether it is mirrored.
type Request = (String, u32, [u8; 4], bool);

/// [`SystemIcons`] of the asset packs. Each request keeps its own id and pixels for the life of the app, so a node
/// repainted and a node reused from the paint cache never name two images with one id.
///
/// One can serve every window and every Kit instance of a process. Its caches only take whole images, which never
/// change afterwards (their pixels are shared), and a cache's lock is held only to look an image up or put one in; the
/// packs and the host's themes are read and drawn under locks of their own, never a cache's. Two threads drawing the
/// same request at once share the first image put in.
pub(crate) struct PackIcons {
    /// What reads and draws the packs' icons.
    packs: Mutex<SystemIcons>,
    drawn: Mutex<HashMap<Request, IconImage>>,
    /// What a name that is not a valid icon name draws.
    invalid: PlaceholderIcons,
    /// The application's icon: where to find its desktop entry, the entry's `Icon=` once read, the host's icon
    /// themes for an icon the packs do not have, and the images drawn by size.
    application: Application,
}

/// The pieces [`PackIcons::application`] works from. Nothing is read until an alert first asks for the icon.
struct Application {
    /// The desktop entry id: the running program's file name.
    entry: Option<String>,
    dirs: Vec<PathBuf>,
    icon: OnceLock<Option<String>>,
    /// The packs the host's themes are searched with, the first time an icon is not in the packs.
    assets: Assets,
    themes: OnceLock<Mutex<ThemeIcons>>,
    drawn: Mutex<HashMap<u32, Option<IconImage>>>,
    /// The icons of installed applications by name and size.
    named: Mutex<HashMap<(String, u32), IconImage>>,
}

/// The pack's icon for an application without one of its own.
const GENERIC_APPLICATION: &str = "application.generic";

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The value `cache` holds for `key`, else `draw`'s, drawn with no lock held and put in unless another thread put one in
/// first, whose value is then the one given.
fn cached<K: Eq + Hash, V: Clone>(
    cache: &Mutex<HashMap<K, V>>,
    key: K,
    draw: impl FnOnce() -> V,
) -> V {
    if let Some(value) = lock(cache).get(&key) {
        return value.clone();
    }
    let value = draw();
    lock(cache).entry(key).or_insert(value).clone()
}

impl PackIcons {
    pub(crate) fn new(assets: &Assets) -> Self {
        let entry = std::env::current_exe()
            .ok()
            .and_then(|exe| Some(exe.file_name()?.to_str()?.to_owned()));
        Self::of(
            SystemIcons::new(assets),
            Application {
                entry,
                dirs: cupertino_apps::entry::xdg_application_dirs(),
                icon: OnceLock::new(),
                assets: assets.clone(),
                themes: OnceLock::new(),
                drawn: Mutex::new(HashMap::new()),
                named: Mutex::new(HashMap::new()),
            },
        )
    }

    fn of(icons: SystemIcons, application: Application) -> Self {
        Self {
            packs: Mutex::new(icons),
            drawn: Mutex::new(HashMap::new()),
            invalid: PlaceholderIcons::default(),
            application,
        }
    }

    /// `Icon=` of the application's desktop entry, read once.
    fn entry_icon(&self) -> Option<&str> {
        let app = &self.application;
        app.icon
            .get_or_init(|| {
                let id = app.entry.as_deref()?;
                let text = app.dirs.iter().find_map(|dir| {
                    std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok()
                })?;
                cupertino_apps::entry::parse_handler(
                    id,
                    &text,
                    &cupertino_apps::entry::Locale::from_env(),
                    &cupertino_apps::entry::Visibility::from_env(),
                )
                .ok()?
                .entry
                .icon
            })
            .as_deref()
    }

    /// The packs' raster icon `name` at `px`, when a pack has one.
    fn pack_raster(&self, name: &str, px: u32) -> Option<IconImage> {
        let mut packs = lock(&self.packs);
        if !packs.has_raster(name, px) {
            return None;
        }
        let p = packs.raster(name, px);
        Some(image_of(p.width(), p.height(), p.data()))
    }

    /// The host themes' icon `name` at `px`, or that of a file it names.
    fn theme_icon(&self, name: &str, px: u32) -> Option<IconImage> {
        let app = &self.application;
        let themes = app
            .themes
            .get_or_init(|| Mutex::new(ThemeIcons::new(&app.assets)));
        let p = lock(themes).load(name, px)?;
        Some(image_of(p.width(), p.height(), p.data()))
    }

    /// The packs' generic application icon at `px` (or its placeholder).
    fn generic(&self, px: u32) -> IconImage {
        let mut packs = lock(&self.packs);
        let p = packs.raster(GENERIC_APPLICATION, px);
        image_of(p.width(), p.height(), p.data())
    }

    /// The application's icon at `px`: its entry's icon from the packs, else from the host's themes or a file, else
    /// the packs' generic application icon (or its placeholder).
    fn draw_application(&self, px: u32) -> Option<IconImage> {
        let icon = self.entry_icon();
        let packs = icon
            .filter(|n| IconName::new(n).is_ok())
            .and_then(|name| self.pack_raster(name, px));
        let themes = || icon.and_then(|name| self.theme_icon(name, px));
        Some(packs.or_else(themes).unwrap_or_else(|| self.generic(px)))
    }

    /// The icon of the application `name` at `px`: a pack's icon of that name, else the host's themes or a file, else
    /// the packs' generic application icon.
    fn draw_named(&self, name: &str, px: u32) -> IconImage {
        let found = (!name.is_empty())
            .then(|| {
                IconName::new(name)
                    .ok()
                    .and_then(|_| self.pack_raster(name, px))
                    .or_else(|| self.theme_icon(name, px))
            })
            .flatten();
        found.unwrap_or_else(|| self.generic(px))
    }
}

/// Premultiplied RGBA pixels as an image with an id of its own.
fn image_of(width: u32, height: u32, data: &[u8]) -> IconImage {
    IconImage {
        id: next_resource_id(),
        image: ImageResource {
            revision: 1,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(data),
        },
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
        cached(&self.drawn, request, || {
            let mut packs = lock(&self.packs);
            let pixmap = packs.symbol(name, px, rgba);
            let (width, height) = (pixmap.width(), pixmap.height());
            let mut pixels = pixmap.data().to_vec();
            drop(packs);
            if mirrored {
                for row in pixels.chunks_exact_mut(width as usize * 4) {
                    row.as_chunks_mut::<4>().0.reverse();
                }
            }
            IconImage {
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
    }

    fn named(&self, name: &str, px: u32) -> Option<IconImage> {
        let px = px.max(1);
        Some(cached(
            &self.application.named,
            (name.to_owned(), px),
            || self.draw_named(name, px),
        ))
    }

    fn application(&self, px: u32) -> Option<IconImage> {
        let px = px.max(1);
        cached(&self.application.drawn, px, || self.draw_application(px))
    }
}
