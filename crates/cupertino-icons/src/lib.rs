//! Icons of the system and of applications. The system's own icons (folders, documents, the trash, symbols) come from
//! the asset packs by logical name ([`SystemIcons`], [`document`]): a pack's raster, SVG or template symbol file, or
//! freedesktop names resolved in the host icon theme (a pack's `theme` entries and default icon map); a name no pack
//! provides is drawn as a placeholder. An application's icon comes from its own `Icon=` entry: a name of the packs'
//! `application.` namespace (the session's own applications), an absolute path, or the `hicolor` theme, after the icon
//! theme of a pack that opts in with `icon_theme.applications` ([`IconLookup`], [`ThemeIcons`]). Files load into premultiplied RGBA pixels.
//!
//! [`thumbnail`] is the freedesktop.org thumbnail cache of pictures, which the file manager and Settings share.

pub mod document;
pub mod thumbnail;

use cupertino_assets::{Assets, IconName, IconSource};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Loaded icon: premultiplied RGBA, row-major, `width * height * 4` bytes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IconImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug)]
struct ThemeDir {
    path: String,
    size: u32,
    scale: u32,
    kind: DirType,
    min: u32,
    max: u32,
    threshold: u32,
}

impl ThemeDir {
    /// Distance of this directory's sizes from the wanted size (icon theme specification).
    fn distance(&self, size: u32, scale: u32) -> u32 {
        let want = size * scale;
        match self.kind {
            DirType::Fixed => (self.size * self.scale).abs_diff(want),
            DirType::Scalable => (self.min * self.scale)
                .saturating_sub(want)
                .max(want.saturating_sub(self.max * self.scale)),
            DirType::Threshold => {
                let lo = self.size.saturating_sub(self.threshold) * self.scale;
                let hi = (self.size + self.threshold) * self.scale;
                lo.saturating_sub(want).max(want.saturating_sub(hi))
            }
        }
    }
}

#[derive(Clone, Debug, Default)]
struct Theme {
    dirs: Vec<ThemeDir>,
    roots: Vec<PathBuf>,
    /// The `Inherits` of the theme's first `index.theme`.
    inherits: Vec<String>,
}

type IniGroups = HashMap<String, HashMap<String, String>>;

fn parse_index(text: &str) -> (Vec<String>, IniGroups) {
    let mut groups: HashMap<String, HashMap<String, String>> = HashMap::new();
    let mut current = String::new();
    for line in text.lines() {
        let line = line.trim();
        if let Some(g) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            current = g.to_string();
            groups.entry(current.clone()).or_default();
        } else if let Some((k, v)) = line.split_once('=') {
            if !current.is_empty() {
                groups
                    .entry(current.clone())
                    .or_default()
                    .insert(k.trim().to_string(), v.trim().to_string());
            }
        }
    }
    let dirs = groups
        .get("Icon Theme")
        .and_then(|g| g.get("Directories"))
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default();
    (dirs, groups)
}

fn load_theme(name: &str, bases: &[PathBuf]) -> Option<Theme> {
    let mut theme = Theme::default();
    let mut found = false;
    for base in bases {
        let root = base.join(name);
        let Ok(text) = std::fs::read_to_string(root.join("index.theme")) else {
            continue;
        };
        let (dirs, groups) = parse_index(&text);
        if !found {
            let inherits = groups.get("Icon Theme").and_then(|g| g.get("Inherits"));
            theme.inherits = inherits
                .map(|v| {
                    v.split(',')
                        .map(|s| s.trim().to_string())
                        .filter(|s| !s.is_empty())
                        .collect()
                })
                .unwrap_or_default();
            for d in dirs {
                let Some(g) = groups.get(&d) else { continue };
                let num = |k: &str| g.get(k).and_then(|v| v.parse::<u32>().ok());
                let size = num("Size").unwrap_or(0);
                let kind = match g.get("Type").map(String::as_str) {
                    Some("Fixed") => DirType::Fixed,
                    Some("Scalable") => DirType::Scalable,
                    _ => DirType::Threshold,
                };
                theme.dirs.push(ThemeDir {
                    path: d,
                    size,
                    scale: num("Scale").unwrap_or(1).max(1),
                    kind,
                    min: num("MinSize").unwrap_or(size),
                    max: num("MaxSize").unwrap_or(size),
                    threshold: num("Threshold").unwrap_or(2),
                });
            }
        }
        found = true;
        theme.roots.push(root);
    }
    found.then_some(theme)
}

/// Name of the icon theme applications' icons are looked up in: the base theme of the icon theme specification, where
/// applications install their own icons. Unless a pack opts in with `icon_theme.applications`, no other theme is
/// searched, so the look of the system never depends on the host's theme.
const APPLICATION_THEME: &str = "hicolor";

/// Semantic names of the asset directory that are application icons.
const APPLICATION_NAMESPACE: &str = "application.";

/// Lookup of an application's own icon files by name and size: the asset directory for names in the
/// `application.` namespace, else the pack's application icon theme (when one opts in) and the `hicolor` theme.
pub struct IconLookup {
    assets: Assets,
    bases: Vec<PathBuf>,
    /// The themes searched in order: the opted-in theme and the themes it inherits, then `hicolor`.
    themes: Vec<Theme>,
    host: HostThemes,
    cache: HashMap<(String, u32, u32), Option<PathBuf>>,
}

/// The icon theme searched when the pack names none.
const DEFAULT_HOST_THEME: &str = "Adwaita";

const EXTENSIONS: [&str; 2] = ["png", "svg"];

impl IconLookup {
    /// Searches `assets`, then `~/.icons`, `<data dir>/icons` for every XDG data directory and `/usr/share/pixmaps`.
    pub fn new(assets: &Assets) -> Self {
        Self::with_roots(assets, host_icon_bases())
    }

    /// Lookup of `assets` and explicit icon directories (tests).
    pub fn with_roots(assets: &Assets, bases: Vec<PathBuf>) -> Self {
        let mut host = HostThemes::new(bases.clone());
        let themes = match assets.application_icon_theme() {
            Some(theme) => host
                .chain(Some(theme))
                .iter()
                .filter_map(|name| load_theme(name, &bases))
                .collect(),
            None => load_theme(APPLICATION_THEME, &bases).into_iter().collect(),
        };
        Self {
            assets: assets.clone(),
            bases,
            themes,
            host,
            cache: HashMap::new(),
        }
    }

    fn search_theme(&self, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
        self.themes
            .iter()
            .find_map(|t| search(t, name, size, scale))
    }

    /// File of the icon `name` for `size` pixels at `scale`: a name of the `application.` namespace from the asset
    /// directory (`application.file-manager`), an absolute path as it is, else the application icon themes, then the
    /// flat directories (pixmaps and legacy `~/.icons`).
    pub fn find(&mut self, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
        if name.is_empty() {
            return None;
        }
        if name.starts_with(APPLICATION_NAMESPACE) {
            let sources = IconName::new(name)
                .map(|n| {
                    self.assets
                        .icon_sources(&n, size * scale)
                        .into_iter()
                        .map(|(_, s)| s)
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let host = &mut self.host;
            let own = sources.into_iter().find_map(|s| match s {
                IconSource::Raster(r) => Some(r.path),
                IconSource::Svg(p) => Some(p),
                IconSource::Theme { names, theme } => {
                    host.find(theme.as_deref(), &names, size * scale)
                }
                IconSource::Symbol(_) => None,
            });
            if own.is_some() {
                return own;
            }
        }
        if name.starts_with('/') {
            return Path::new(name).is_file().then(|| PathBuf::from(name));
        }
        let key = (name.to_string(), size, scale);
        if let Some(hit) = self.cache.get(&key) {
            return hit.clone();
        }
        let found = self.search_theme(name, size, scale).or_else(|| {
            self.bases.iter().find_map(|base| {
                EXTENSIONS
                    .iter()
                    .map(|ext| base.join(format!("{name}.{ext}")))
                    .find(|p| p.is_file())
            })
        });
        self.cache.insert(key, found.clone());
        found
    }
}

/// The file of `name` in `t` closest to `size` at `scale` (icon theme specification), PNG or SVG.
fn search(t: &Theme, name: &str, size: u32, scale: u32) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for d in &t.dirs {
        let dist = d.distance(size, scale);
        if best.as_ref().is_some_and(|(b, _)| *b <= dist) {
            continue;
        }
        for root in &t.roots {
            for ext in EXTENSIONS {
                let p = root.join(&d.path).join(format!("{name}.{ext}"));
                if p.is_file() {
                    best = Some((dist, p));
                    break;
                }
            }
            if best.as_ref().is_some_and(|(b, _)| *b == dist) {
                break;
            }
        }
        if best.as_ref().is_some_and(|(b, _)| *b == 0) {
            break;
        }
    }
    best.map(|(_, p)| p)
}

/// The icon themes of the host, loaded once each: the resolution of the packs' `theme` sources.
struct HostThemes {
    bases: Vec<PathBuf>,
    loaded: HashMap<String, Option<Theme>>,
}

impl HostThemes {
    fn new(bases: Vec<PathBuf>) -> Self {
        Self {
            bases,
            loaded: HashMap::new(),
        }
    }

    fn theme(&mut self, name: &str) -> Option<&Theme> {
        let bases = &self.bases;
        self.loaded
            .entry(name.to_string())
            .or_insert_with(|| load_theme(name, bases))
            .as_ref()
    }

    /// `theme` (else [`DEFAULT_HOST_THEME`]), its `Inherits` chain (each theme once), then `hicolor`.
    fn chain(&mut self, theme: Option<&str>) -> Vec<String> {
        let mut chain: Vec<String> = Vec::new();
        let mut todo: Vec<String> = theme
            .map(str::to_string)
            .or_else(|| Some(DEFAULT_HOST_THEME.to_string()))
            .into_iter()
            .collect();
        while let Some(name) = todo.pop() {
            if chain.contains(&name) {
                continue;
            }
            let inherits = self
                .theme(&name)
                .map(|t| t.inherits.clone())
                .unwrap_or_default();
            chain.push(name);
            todo.extend(inherits.into_iter().rev());
        }
        if !chain.iter().any(|t| t == APPLICATION_THEME) {
            chain.push(APPLICATION_THEME.to_string());
        }
        chain
    }

    /// The first of `names` that a theme of the chain of `theme` has, for `size` pixels.
    fn find(&mut self, theme: Option<&str>, names: &[String], size: u32) -> Option<PathBuf> {
        let chain = self.chain(theme);
        names.iter().find_map(|name| {
            chain
                .iter()
                .find_map(|t| self.theme(t).and_then(|t| search(t, name, size, 1)))
        })
    }
}

/// The icon directories of the host: `~/.icons`, `<data dir>/icons` for every XDG data directory, `/usr/share/pixmaps`.
fn host_icon_bases() -> Vec<PathBuf> {
    let mut bases: Vec<PathBuf> = Vec::new();
    if let Some(h) = std::env::var_os("HOME") {
        bases.push(PathBuf::from(&h).join(".icons"));
    }
    bases.extend(
        cupertino_defaults::data_dirs()
            .into_iter()
            .map(|d| d.join("icons")),
    );
    bases.push(PathBuf::from("/usr/share/pixmaps"));
    bases
}

fn premultiply(rgba: &mut [u8]) {
    for px in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(px[3]);
        for c in &mut px[..3] {
            *c = ((u32::from(*c) * a + 127) / 255) as u8;
        }
    }
}

/// Load a PNG or SVG file rendered to fit `px` × `px` (SVG) or at its own size (PNG, resampled to
/// `px` when it is larger).
pub fn load_icon(path: &Path, px: u32) -> Option<IconImage> {
    let data = std::fs::read(path).ok()?;
    let px = px.max(1);
    if path.extension().and_then(|e| e.to_str()) == Some("svg") {
        return render_svg(&data, px);
    }
    decode_png(&data, px)
}

/// Decode encoded PNG bytes, as a menu item sends its icon inline: at the picture's own size, resampled to `px` when it
/// is larger.
pub fn decode_png(data: &[u8], px: u32) -> Option<IconImage> {
    let px = px.max(1);
    let img = image::load_from_memory(data).ok()?.to_rgba8();
    let img = if img.width() > px * 2 || img.height() > px * 2 {
        image::imageops::resize(&img, px, px, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let (width, height) = (img.width(), img.height());
    let mut rgba = img.into_raw();
    premultiply(&mut rgba);
    Some(IconImage {
        width,
        height,
        rgba,
    })
}

fn render_svg(data: &[u8], px: u32) -> Option<IconImage> {
    let tree = resvg::usvg::Tree::from_data(data, &resvg::usvg::Options::default()).ok()?;
    let size = tree.size();
    let s = (px as f32 / size.width()).min(px as f32 / size.height());
    let (w, h) = (
        ((size.width() * s).round() as u32).max(1),
        ((size.height() * s).round() as u32).max(1),
    );
    let mut pm = resvg::tiny_skia::Pixmap::new(w, h)?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(s, s),
        &mut pm.as_mut(),
    );
    Some(IconImage {
        width: w,
        height: h,
        rgba: pm.take(),
    })
}

/// Pixmap bytes of an ARGB32 (network byte order) icon, as delivered by status items, converted to
/// premultiplied RGBA.
pub fn from_argb(width: i32, height: i32, argb: &[u8]) -> Option<IconImage> {
    if width <= 0 || height <= 0 || argb.len() != (width * height * 4) as usize {
        return None;
    }
    let mut rgba = Vec::with_capacity(argb.len());
    for px in argb.as_chunks::<4>().0 {
        rgba.extend_from_slice(&[px[1], px[2], px[3], px[0]]);
    }
    premultiply(&mut rgba);
    Some(IconImage {
        width: width as u32,
        height: height as u32,
        rgba,
    })
}

/// Application icon pixmaps by the name or path of a desktop entry's `Icon=` (or a `file://` URI), for the renderers of
/// the shell surfaces. Only [`IconLookup`]'s places are searched, so a name that is not an application's own icon
/// (a host theme's stock icon) finds nothing.
pub struct ThemeIcons {
    lookup: IconLookup,
    application_dirs: Vec<PathBuf>,
    /// `Icon=` of a desktop entry by its id; `None` when no installed entry has the id or it names no icon.
    entry_icons: HashMap<String, Option<String>>,
}

impl ThemeIcons {
    pub fn new(assets: &Assets) -> Self {
        Self {
            lookup: IconLookup::new(assets),
            application_dirs: cupertino_apps::entry::xdg_application_dirs(),
            entry_icons: HashMap::new(),
        }
    }

    /// Icons of `assets` and under the explicit `icon_roots`, desktop entries under `application_dirs` (tests).
    pub fn with_roots(
        assets: &Assets,
        icon_roots: Vec<PathBuf>,
        application_dirs: Vec<PathBuf>,
    ) -> Self {
        Self {
            lookup: IconLookup::with_roots(assets, icon_roots),
            application_dirs,
            entry_icons: HashMap::new(),
        }
    }

    /// The icon of the installed application whose desktop entry has the id `entry` (`org.example.Mail`, a trailing
    /// `.desktop` is accepted) at `px` × `px` device pixels, from the entry's `Icon=`; `None` when there is no such
    /// entry, it names no icon, or that icon is not an application icon of the application icon themes or a file.
    pub fn load_for_entry(&mut self, entry: &str, px: u32) -> Option<tiny_skia::Pixmap> {
        let id = entry.strip_suffix(".desktop").unwrap_or(entry);
        let dirs = &self.application_dirs;
        let icon = self
            .entry_icons
            .entry(id.to_string())
            .or_insert_with(|| entry_icon(dirs, id))
            .clone()?;
        self.load(&icon, px)
    }

    /// The icon `name` at `px` × `px` device pixels; `None` when the application has no such icon.
    pub fn load(&mut self, name: &str, px: u32) -> Option<tiny_skia::Pixmap> {
        let name = name.strip_prefix("file://").unwrap_or(name);
        let path = self.lookup.find(name, px, 1)?;
        let img = load_icon(&path, px)?;
        let size = tiny_skia::IntSize::from_wh(img.width, img.height)?;
        tiny_skia::Pixmap::from_vec(img.rgba, size)
    }
}

/// `Icon=` of the first installed desktop file `<dir>/<id>.desktop`; hidden entries and non-applications give `None`.
fn entry_icon(dirs: &[PathBuf], id: &str) -> Option<String> {
    let text = dirs
        .iter()
        .find_map(|dir| std::fs::read_to_string(dir.join(format!("{id}.desktop"))).ok())?;
    let (locale, visibility) = (
        cupertino_apps::entry::Locale::from_env(),
        cupertino_apps::entry::Visibility::from_env(),
    );
    cupertino_apps::entry::parse_handler(id, &text, &locale, &visibility)
        .ok()?
        .entry
        .icon
}

/// Straight (not premultiplied) RGBA colour, components 0 to 1.
pub type Rgba = [f32; 4];

/// The system's own icons by logical name, from the asset packs, as pixmaps cached by size: the raster icons
/// (`folder`, `document.pdf`, `trash.full`, ...) and the template symbols (`symbol.house`, ...), which the client
/// tints. Each name takes the first source of the packs that loads (see [`Assets::icon_sources`]); a name none
/// provides is drawn as a placeholder, a rounded square. A file that is listed but cannot be decoded is reported on stderr.
pub struct SystemIcons {
    assets: Assets,
    host: HostThemes,
    rasters: HashMap<(IconName, u32), Option<tiny_skia::Pixmap>>,
    symbols: HashMap<(IconName, u32, [u8; 4]), Option<tiny_skia::Pixmap>>,
    placeholders: HashMap<(u32, Option<[u8; 4]>), tiny_skia::Pixmap>,
}

fn semantic(name: &str) -> IconName {
    IconName::new(name).unwrap_or_else(|e| panic!("{e}"))
}

fn to_pixmap(path: &Path, img: Option<IconImage>) -> Option<tiny_skia::Pixmap> {
    let pixmap = img.and_then(|img| {
        tiny_skia::Pixmap::from_vec(
            img.rgba,
            tiny_skia::IntSize::from_wh(img.width, img.height)?,
        )
    });
    if pixmap.is_none() {
        eprintln!("cupertino-icons: cannot decode {}", path.display());
    }
    pixmap
}

/// Replace the colour of a rendered black template by `color`, keeping its coverage.
fn tint(pixmap: &mut tiny_skia::Pixmap, color: Rgba) {
    for px in pixmap.data_mut().as_chunks_mut::<4>().0 {
        let a = f32::from(px[3]) / 255.0 * color[3];
        px[0] = (color[0] * a * 255.0).round() as u8;
        px[1] = (color[1] * a * 255.0).round() as u8;
        px[2] = (color[2] * a * 255.0).round() as u8;
        px[3] = (a * 255.0).round() as u8;
    }
}

/// The system menu's mark: a single-colour template on a 16 × 16 grid, the project's own drawing.
const SYSTEM_MARK_SVG: &[u8] = include_bytes!("../data/system-menu-mark.svg");

/// The system menu's mark drawn to fit `px` × `px` pixels in `color`.
pub fn system_mark(px: u32, color: Rgba) -> tiny_skia::Pixmap {
    let img = render_svg(SYSTEM_MARK_SVG, px.max(1)).expect("the built-in mark is a valid SVG");
    let size = tiny_skia::IntSize::from_wh(img.width, img.height).expect("a non-empty mark");
    let mut pixmap = tiny_skia::Pixmap::from_vec(img.rgba, size).expect("pixels of its size");
    tint(&mut pixmap, color);
    pixmap
}

fn quantise(color: Rgba) -> [u8; 4] {
    color.map(|c| (c.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The placeholder of a name no pack provides, `px` × `px`: an opaque mid-grey rounded square for a raster icon, the
/// outline of one in `color` for a template symbol.
fn placeholder(px: u32, color: Option<[u8; 4]>) -> tiny_skia::Pixmap {
    let px = px.max(1);
    let mut pm = tiny_skia::Pixmap::new(px, px).expect("a non-empty square");
    let side = px as f32;
    let inset = (side * 0.1).round();
    let r = side * 0.2;
    let mut pb = tiny_skia::PathBuilder::new();
    let (x0, y0, x1, y1) = (inset, inset, side - inset, side - inset);
    pb.move_to(x0 + r, y0);
    pb.line_to(x1 - r, y0);
    pb.quad_to(x1, y0, x1, y0 + r);
    pb.line_to(x1, y1 - r);
    pb.quad_to(x1, y1, x1 - r, y1);
    pb.line_to(x0 + r, y1);
    pb.quad_to(x0, y1, x0, y1 - r);
    pb.line_to(x0, y0 + r);
    pb.quad_to(x0, y0, x0 + r, y0);
    pb.close();
    let path = pb.finish().expect("a closed rounded square");
    let mut paint = tiny_skia::Paint {
        anti_alias: true,
        ..tiny_skia::Paint::default()
    };
    match color {
        None => {
            paint.set_color_rgba8(140, 140, 140, 255);
            pm.fill_path(
                &path,
                &paint,
                tiny_skia::FillRule::Winding,
                tiny_skia::Transform::identity(),
                None,
            );
        }
        Some(c) => {
            paint.set_color_rgba8(c[0], c[1], c[2], c[3]);
            let stroke = tiny_skia::Stroke {
                width: (side / 12.0).max(1.0),
                ..tiny_skia::Stroke::default()
            };
            pm.stroke_path(
                &path,
                &paint,
                &stroke,
                tiny_skia::Transform::identity(),
                None,
            );
        }
    }
    pm
}

impl SystemIcons {
    pub fn new(assets: &Assets) -> Self {
        Self::with_host(assets, host_icon_bases())
    }

    /// Icons of `assets` alone: no host icon theme is searched, so a name that only a `theme` source or the default
    /// icon map could provide is drawn as a placeholder. For a process that is started without host data (tests), so that
    /// an expected image can be computed with the same view.
    pub fn without_host(assets: &Assets) -> Self {
        Self::with_host(assets, Vec::new())
    }

    /// Icons of `assets` whose `theme` sources resolve under the icon directories `bases` (tests).
    fn with_host(assets: &Assets, bases: Vec<PathBuf>) -> Self {
        Self {
            assets: assets.clone(),
            host: HostThemes::new(bases),
            rasters: HashMap::new(),
            symbols: HashMap::new(),
            placeholders: HashMap::new(),
        }
    }

    /// Like [`SystemIcons::new`], for a process that draws exactly the raster icons `rasters` and the template symbols
    /// `symbols`: the names no asset pack provides are reported once on stderr at startup (they are drawn as placeholders).
    pub fn required(assets: &Assets, rasters: &[&str], symbols: &[&str]) -> Self {
        let mut icons = Self::new(assets);
        let missing = icons.unprovided(rasters, symbols);
        if !missing.is_empty() {
            eprintln!(
                "cupertino-icons: no asset pack provides the icon {}; it is drawn as a placeholder",
                missing
                    .iter()
                    .map(|n| format!("`{n}`"))
                    .collect::<Vec<_>>()
                    .join(", ")
            );
        }
        icons
    }

    fn unprovided<'a>(&mut self, rasters: &[&'a str], symbols: &[&'a str]) -> Vec<&'a str> {
        let mut out = Vec::new();
        for (names, symbol) in [(rasters, false), (symbols, true)] {
            for &name in names {
                let found = self
                    .assets
                    .icon_sources(&semantic(name), 32)
                    .into_iter()
                    .any(|(_, s)| match s {
                        IconSource::Raster(_) | IconSource::Svg(_) => !symbol,
                        IconSource::Symbol(_) => symbol,
                        IconSource::Theme { names, theme } => {
                            self.host.find(theme.as_deref(), &names, 32).is_some()
                        }
                    });
                if !found {
                    out.push(name);
                }
            }
        }
        out
    }

    /// The raster icon `name` from the first source that loads, `None` when none does. A file is used at the size
    /// nearest above `px` (the largest when none is that big), shrunk to `px` when it is more than twice as large.
    fn load_raster(&mut self, name: &IconName, px: u32) -> Option<tiny_skia::Pixmap> {
        for (_, source) in self.assets.icon_sources(name, px) {
            let path = match source {
                IconSource::Raster(r) => r.path,
                IconSource::Svg(p) => p,
                IconSource::Symbol(_) => continue,
                IconSource::Theme { names, theme } => {
                    match self.host.find(theme.as_deref(), &names, px) {
                        Some(p) => p,
                        None => continue,
                    }
                }
            };
            if let Some(pm) = to_pixmap(&path, load_icon(&path, px)) {
                return Some(pm);
            }
        }
        None
    }

    /// The template symbol `name` drawn to fit `px` × `px` in `color`, from the first source that loads.
    fn load_symbol(&mut self, name: &IconName, px: u32, color: Rgba) -> Option<tiny_skia::Pixmap> {
        for (_, source) in self.assets.icon_sources(name, px) {
            let path = match source {
                IconSource::Symbol(p) => p,
                IconSource::Theme { names, theme } => {
                    match self.host.find(theme.as_deref(), &names, px) {
                        Some(p) => p,
                        None => continue,
                    }
                }
                IconSource::Raster(_) | IconSource::Svg(_) => continue,
            };
            if let Some(mut pm) = to_pixmap(&path, load_icon(&path, px)) {
                tint(&mut pm, color);
                return Some(pm);
            }
        }
        None
    }

    /// Whether an asset pack gives the raster icon `name` as a file of its own (a PNG or a full-colour SVG), rather than
    /// through the host icon theme or not at all.
    pub fn pack_has_raster(&self, name: &str) -> bool {
        self.assets
            .icon_sources(&semantic(name), 32)
            .into_iter()
            .any(|(_, s)| matches!(s, IconSource::Raster(_) | IconSource::Svg(_)))
    }

    /// Whether a source of the packs gives the raster icon `name` (at `px` pixels): `false` means it is drawn as a placeholder.
    pub fn has_raster(&mut self, name: &str, px: u32) -> bool {
        let name = semantic(name);
        if !self.rasters.contains_key(&(name.clone(), px)) {
            let loaded = self.load_raster(&name, px);
            self.rasters.insert((name.clone(), px), loaded);
        }
        self.rasters[&(name, px)].is_some()
    }

    /// The raster icon `name` at `px` pixels, or its placeholder. Panics when `name` is not a valid logical name (a
    /// programming error).
    pub fn raster(&mut self, name: &str, px: u32) -> &tiny_skia::Pixmap {
        if self.has_raster(name, px) {
            return self.rasters[&(semantic(name), px)]
                .as_ref()
                .expect("checked by has_raster");
        }
        self.placeholders
            .entry((px, None))
            .or_insert_with(|| placeholder(px, None))
    }

    /// The template symbol `name` drawn to fit `px` × `px` pixels (aspect kept) in `color`, or its placeholder.
    pub fn symbol(&mut self, name: &str, px: u32, color: Rgba) -> &tiny_skia::Pixmap {
        let name = semantic(name);
        let key = (name.clone(), px, quantise(color));
        if !self.symbols.contains_key(&key) {
            let loaded = self.load_symbol(&name, px, color);
            self.symbols.insert(key.clone(), loaded);
        }
        match self.symbols[&key] {
            Some(_) => self.symbols[&key].as_ref().expect("matched as loaded"),
            None => self
                .placeholders
                .entry((px, Some(quantise(color))))
                .or_insert_with(|| placeholder(px, Some(quantise(color)))),
        }
    }
}
