//! The asset packs every Cupertino process draws its text, icons, cursors and wallpaper from.
//!
//! An asset pack is a directory described by `manifest.json` (version 2, or version 1 read as the subset of version 2
//! it is). [`Assets::discover`] looks for packs in order ([`search_path`]): `$CUPERTINO_ASSET_DIR`,
//! `$XDG_DATA_HOME/cupertino/assets` (`~/.local/share` when unset), `/usr/share/cupertino/assets/cupertino`,
//! `/usr/share/cupertino/assets`, then the built-in
//! fallback. Resolution is per name: for each logical name the first pack that provides it wins, so a pack may be
//! partial. A missing pack, an invalid manifest or entry and a missing file never stop a process; each is reported once on
//! stderr. The built-in fallback is the embedded Inter font ([`BUILTIN_UI_FAMILY`]); icons the packs lack are drawn as
//! placeholders by `cupertino-icons`, cursors fall back to the procedural theme in the server and the wallpaper to the
//! procedural one. Callers ask for logical names ([`IconName`], [`FontRole`]); the mapping to files lives only in the
//! packs. [`Assets::open`] is the strict check of one pack, for pack authors and tests. Tests and package builds that
//! must not depend on what is installed set `CUPERTINO_ASSET_SEARCH_PATH` ([`SEARCH_PATH_ENV`]), which replaces the whole
//! list.

mod error;
mod manifest;
mod name;
mod report;

pub use error::AssetError;
pub use manifest::{FontRole, RasterIcon};
pub use name::{IconName, CURSOR_NAMES, RASTER_NAMES, SYMBOL_NAMES};
pub use report::{PackEntry, PackReport, Provided};

use manifest::Manifest;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Name of the environment variable that names the first asset pack.
pub const ASSET_DIR_ENV: &str = "CUPERTINO_ASSET_DIR";
/// Name of the environment variable that replaces the search list: its colon-separated directories are the packs, in
/// order, and nothing else is searched. Set to an empty value it selects the built-in fallback alone. It is for tests
/// and package builds whose result must not depend on the packs installed on the machine.
pub const SEARCH_PATH_ENV: &str = "CUPERTINO_ASSET_SEARCH_PATH";
/// The system-wide pack, the last one before the built-in fallback.
pub const SYSTEM_PACK: &str = "/usr/share/cupertino/assets";
/// Family of the embedded built-in font (a subset of Inter, SIL Open Font License 1.1; see `fonts/OFL.txt`).
pub const BUILTIN_UI_FAMILY: &str = "Inter Variable";
/// The embedded built-in font: a variable font with `wght` and `opsz` axes and named weight instances.
pub const BUILTIN_FONT: &[u8] = include_bytes!("../fonts/InterVariable-subset.ttf");

/// The XCursor theme of an asset pack: `<search_dir>/<name>/{index.theme,cursors/}`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct CursorTheme {
    /// Theme name from the manifest.
    pub name: String,
    /// The directory that contains the theme directory (to be placed on an XCursor search path).
    pub search_dir: PathBuf,
}

/// A font to load: a file of a pack, or the embedded built-in font.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontSource {
    File(PathBuf),
    Builtin,
}

impl FontSource {
    /// A name for messages: the file, or the built-in font.
    pub fn label(&self) -> String {
        match self {
            FontSource::File(p) => p.display().to_string(),
            FontSource::Builtin => format!("the built-in font ({BUILTIN_UI_FAMILY})"),
        }
    }
}

/// One way a pack provides an icon, in the order the sources are tried.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconSource {
    /// A PNG chosen for the requested pixel size.
    Raster(RasterIcon),
    /// A full-colour SVG drawn at the requested size.
    Svg(PathBuf),
    /// A single-colour template SVG the caller tints.
    Symbol(PathBuf),
    /// Freedesktop icon names to resolve in the host icon theme (`theme` is the pack's `icon_theme.name`, `None` the
    /// user's theme).
    Theme {
        names: Vec<String>,
        theme: Option<String>,
    },
}

/// One asset pack: its directory, its manifest and the default icon map of its `icon_theme`.
#[derive(Debug, Clone)]
struct Pack {
    root: PathBuf,
    manifest: Manifest,
    icon_map: BTreeMap<String, Vec<String>>,
    /// The font files, in loading order, listed once when the pack is read.
    fonts: Vec<PathBuf>,
}

impl Pack {
    /// Read the pack in `dir`; the notes are the entries that were dropped and the files the manifest names but lacks.
    fn read(dir: &Path) -> Result<(Self, Vec<String>), AssetError> {
        let manifest_path = dir.join("manifest.json");
        let text = std::fs::read_to_string(&manifest_path).map_err(|source| AssetError::Read {
            path: manifest_path.clone(),
            source,
        })?;
        let (manifest, mut notes) =
            Manifest::parse(&text).map_err(|reason| AssetError::Manifest {
                path: manifest_path,
                reason,
            })?;
        let mut icon_map = BTreeMap::new();
        if let Some(map) = manifest.icon_theme.as_ref().and_then(|t| t.map.as_ref()) {
            let path = dir.join(map);
            let parsed = std::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|t| {
                    toml::from_str::<BTreeMap<String, Vec<String>>>(&t).map_err(|e| e.to_string())
                });
            match parsed {
                Ok(m) => icon_map = m,
                Err(e) => notes.push(format!("icon_theme.map {}: {e}; ignored", path.display())),
            }
        }
        let fonts = match font_files(dir, &manifest) {
            Ok(fonts) => fonts,
            Err(e) => {
                notes.push(format!("{e}; the pack's fonts are not used"));
                Vec::new()
            }
        };
        let pack = Self {
            root: dir.to_path_buf(),
            manifest,
            icon_map,
            fonts,
        };
        notes.extend(pack.missing());
        Ok((pack, notes))
    }

    /// The sources this pack gives for `name` at `px` pixels whose files exist, in trial order: raster, svg, symbol,
    /// theme. A name the manifest does not list takes the names of the pack's default icon map.
    fn icon_sources(&self, name: &IconName, px: u32) -> Vec<IconSource> {
        let theme = self
            .manifest
            .icon_theme
            .as_ref()
            .and_then(|t| t.name.clone());
        let Some(entry) = self.manifest.icons.get(name) else {
            return self
                .icon_map
                .get(name.as_str())
                .map(|names| {
                    vec![IconSource::Theme {
                        names: names.clone(),
                        theme,
                    }]
                })
                .unwrap_or_default();
        };
        let mut out = Vec::new();
        let chosen = entry
            .raster
            .range(px..)
            .next()
            .or_else(|| entry.raster.iter().next_back());
        if let Some((&size, file)) = chosen.filter(|(_, f)| self.root.join(f).is_file()) {
            out.push(IconSource::Raster(RasterIcon {
                path: self.root.join(file),
                px: size,
            }));
        }
        out.extend(
            entry
                .svg
                .iter()
                .map(|f| self.root.join(f))
                .filter(|p| p.is_file())
                .map(IconSource::Svg),
        );
        out.extend(
            entry
                .symbol
                .iter()
                .map(|f| self.root.join(f))
                .filter(|p| p.is_file())
                .map(IconSource::Symbol),
        );
        if !entry.theme.is_empty() {
            out.push(IconSource::Theme {
                names: entry.theme.clone(),
                theme,
            });
        }
        out
    }

    /// The cursor theme, when one of its search directories holds it.
    fn cursor_theme(&self) -> Option<CursorTheme> {
        let cursors = self.manifest.cursors.as_ref()?;
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let mut dirs = cursors.path.iter().flat_map(|entry| -> Vec<PathBuf> {
            if entry == "$XCURSOR_PATH" {
                std::env::var("XCURSOR_PATH")
                    .map(|v| {
                        v.split(':')
                            .filter(|d| !d.is_empty())
                            .map(PathBuf::from)
                            .collect()
                    })
                    .unwrap_or_default()
            } else if let Some(rest) = entry.strip_prefix("~/") {
                home.iter().map(|h| h.join(rest)).collect()
            } else if Path::new(entry).is_absolute() {
                vec![PathBuf::from(entry)]
            } else {
                vec![self.root.join(entry)]
            }
        });
        dirs.find(|d| d.join(&cursors.theme).is_dir())
            .map(|search_dir| CursorTheme {
                name: cursors.theme.clone(),
                search_dir,
            })
    }

    fn wallpaper(&self, dark: bool) -> Option<PathBuf> {
        let w = self.manifest.wallpaper.as_ref()?;
        let file = if dark {
            w.dark.as_ref().unwrap_or(&w.light)
        } else {
            &w.light
        };
        Some(self.root.join(file))
    }

    /// Every file or directory the manifest names that is not there.
    fn missing(&self) -> Vec<String> {
        let gone = |what: String| format!("{what} is named by the manifest but missing");
        let mut missing = Vec::new();
        if self.manifest.version == 1 && !self.manifest.fonts.is_empty() && self.fonts.is_empty() {
            missing.push(gone("a font file in fonts/".to_string()));
        }
        missing.extend(
            self.fonts
                .iter()
                .filter(|f| !f.is_file())
                .map(|f| gone(f.display().to_string())),
        );
        if let Some(c) = self
            .manifest
            .cursors
            .as_ref()
            .filter(|_| self.cursor_theme().is_none())
        {
            missing.push(gone(format!("the cursor theme {:?}", c.theme)));
        }
        for dark in [false, true] {
            if let Some(w) = self.wallpaper(dark).filter(|w| !w.is_file()) {
                missing.push(gone(w.display().to_string()));
            }
        }
        for (name, entry) in &self.manifest.icons {
            let files = entry.raster.values().chain(&entry.svg).chain(&entry.symbol);
            missing.extend(
                files
                    .filter(|f| !self.root.join(f).is_file())
                    .map(|f| gone(format!("{f} (icon {name})"))),
            );
        }
        missing.dedup();
        missing
    }
}

/// The font files of the pack in `dir`, in loading order: version 2 lists them (the roles' files, then the fallback
/// faces); version 1 loads every font file of `fonts/`, sorted by path. Fails when `fonts/` exists but cannot be read.
fn font_files(dir: &Path, manifest: &Manifest) -> Result<Vec<PathBuf>, AssetError> {
    if manifest.version >= 2 {
        let role_files = FontRole::ALL
            .iter()
            .filter_map(|r| manifest.fonts.get(r))
            .flat_map(|f| f.files.iter().flatten());
        let mut files: Vec<PathBuf> = Vec::new();
        for f in role_files.chain(&manifest.fallback_fonts) {
            let p = dir.join(f);
            if !files.contains(&p) {
                files.push(p);
            }
        }
        return Ok(files);
    }
    let fonts = dir.join("fonts");
    let entries = match std::fs::read_dir(&fonts) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => {
            return Err(AssetError::Read {
                path: fonts,
                source,
            })
        }
    };
    let mut files = Vec::new();
    for entry in entries {
        let path = entry
            .map_err(|source| AssetError::Read {
                path: fonts.clone(),
                source,
            })?
            .path();
        let is_font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["ttf", "otf", "ttc", "otc"].contains(&e.to_ascii_lowercase().as_str())
        });
        if is_font && path.is_file() {
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

/// The pack directories [`Assets::discover`] tries, in order: `$CUPERTINO_ASSET_DIR` (when set and not empty), the
/// user pack under `$XDG_DATA_HOME` (`$HOME/.local/share` when unset), the system pack [`SYSTEM_PACK`]; or, when
/// `$CUPERTINO_ASSET_SEARCH_PATH` is set (even to an empty value), exactly the directories it lists.
pub fn search_path() -> Vec<PathBuf> {
    search_path_from(|name| std::env::var_os(name))
}

/// [`search_path`] with the environment read through `env`.
fn search_path_from(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Vec<PathBuf> {
    if let Some(list) = env(SEARCH_PATH_ENV) {
        return std::env::split_paths(&list)
            .filter(|d| !d.as_os_str().is_empty())
            .collect();
    }
    let var = |name: &str| env(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    let data_home = var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local/share")));
    var(ASSET_DIR_ENV)
        .into_iter()
        .chain(data_home.map(|d| d.join("cupertino/assets")))
        .chain([
            PathBuf::from(SYSTEM_PACK).join("cupertino"),
            PathBuf::from(SYSTEM_PACK),
        ])
        .collect()
}

/// The built-in default icon map: for every logical icon name the freedesktop names to look for in the user's icon
/// theme. It is the last source of every icon, so a name no pack provides is drawn from the host theme, and a
/// placeholder appears only when the host theme lacks the name too.
fn builtin_icon_map() -> &'static BTreeMap<String, Vec<String>> {
    static MAP: std::sync::OnceLock<BTreeMap<String, Vec<String>>> = std::sync::OnceLock::new();
    MAP.get_or_init(|| {
        toml::from_str(include_str!("../data/icon-map.toml"))
            .expect("data/icon-map.toml is valid (checked by a test)")
    })
}

/// The asset packs of a process, in lookup order, followed by the built-in fallback.
#[derive(Debug, Clone)]
pub struct Assets {
    packs: Vec<Pack>,
}

/// A picture the desktop can show: one image, or a light and a dark one that go together.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WallpaperFile {
    /// The file name without its extension (without `-dark` for a pair): `sunrise` for `sunrise.jpg` and
    /// `sunrise-dark.jpg`.
    pub name: String,
    pub light: PathBuf,
    /// The picture of the dark appearance; `None` when the one image serves both.
    pub dark: Option<PathBuf>,
    /// The picture's title and its photographer, from the pack's `wallpapers/credits.txt` when it names the file.
    pub credit: Option<Credit>,
}

/// Who made a picture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    pub title: String,
    pub author: String,
}

/// The most of a credits file that is read (bytes).
const MAX_CREDITS_BYTES: u64 = 1 << 20;

/// The credits in the file at `path`, reading at most [`MAX_CREDITS_BYTES`] of it; none when it cannot be read.
fn read_credits(path: &Path) -> std::collections::HashMap<String, Credit> {
    use std::io::Read;
    let Ok(file) = std::fs::File::open(path) else {
        return Default::default();
    };
    let mut bytes = Vec::new();
    if file
        .take(MAX_CREDITS_BYTES)
        .read_to_end(&mut bytes)
        .is_err()
    {
        return Default::default();
    }
    parse_credits(&String::from_utf8_lossy(&bytes))
}

/// The credits of `text`, by file name: a line `wallpapers/<file>` followed by an indented line
/// `"<title>" by <author> (<link>)`.
pub fn parse_credits(text: &str) -> std::collections::HashMap<String, Credit> {
    let mut out = std::collections::HashMap::new();
    let mut file: Option<String> = None;
    for line in text.lines() {
        if !line.starts_with(' ') && !line.starts_with('\t') {
            file = line.trim().strip_prefix("wallpapers/").map(str::to_string);
            continue;
        }
        let Some(name) = &file else { continue };
        let line = line.trim();
        if let Some((title, rest)) = line.strip_prefix('"').and_then(|l| l.split_once("\" by ")) {
            let author = rest.split(" (").next().unwrap_or(rest).trim();
            out.entry(name.clone()).or_insert(Credit {
                title: title.to_string(),
                author: author.to_string(),
            });
        }
    }
    out
}

/// The pictures in `dir` (PNG and JPEG, no hidden files), by name; `<name>-dark.<ext>` next to `<name>.<ext>` makes
/// them one pair. A directory that cannot be read has none.
pub fn pictures_in(dir: &Path) -> Vec<WallpaperFile> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<(String, PathBuf)> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .filter_map(|p| {
            let name = p.file_name()?.to_str()?;
            let (stem, ext) = name.rsplit_once('.')?;
            let ext = ext.to_ascii_lowercase();
            (!name.starts_with('.') && matches!(ext.as_str(), "png" | "jpg" | "jpeg"))
                .then(|| (stem.to_string(), p.clone()))
        })
        .collect();
    files.sort();
    let light_names: Vec<String> = files.iter().map(|(stem, _)| stem.clone()).collect();
    let mut out: Vec<WallpaperFile> = Vec::new();
    for (stem, path) in &files {
        match stem.strip_suffix("-dark") {
            Some(base) if light_names.iter().any(|n| n == base) => {}
            _ => out.push(WallpaperFile {
                name: stem.clone(),
                light: path.clone(),
                dark: files
                    .iter()
                    .find(|(other, _)| other.strip_suffix("-dark") == Some(stem.as_str()))
                    .map(|(_, dark)| dark.clone()),
                credit: None,
            }),
        }
    }
    out
}

impl Assets {
    /// The packs of [`search_path`]. Never fails: a directory that does not exist is skipped silently (most systems
    /// have only some of them), one that exists but cannot be used is skipped with a message on stderr.
    pub fn discover() -> Self {
        Self::from_dirs(&search_path())
    }

    /// The packs in `dirs`, in that order, as [`Assets::discover`] reads them.
    pub fn from_dirs(dirs: &[PathBuf]) -> Self {
        let mut packs = Vec::new();
        for dir in dirs.iter().filter(|d| d.exists()) {
            match Pack::read(dir) {
                Ok((pack, notes)) => {
                    for note in notes {
                        eprintln!("cupertino-assets: {}: {note}", dir.display());
                    }
                    packs.push(pack);
                }
                Err(e) => eprintln!("cupertino-assets: skipping the pack {}: {e}", dir.display()),
            }
        }
        Self { packs }
    }

    /// Only the built-in fallback: no pack.
    pub fn builtin() -> Self {
        Self { packs: Vec::new() }
    }

    /// One pack read as [`Assets::discover`] reads it, with the problems it reports: the entries that were dropped and
    /// the files the manifest names but lacks. Fails only when the manifest cannot be read, is not valid JSON or has an
    /// unsupported version.
    pub fn read_pack(dir: &Path) -> Result<(Self, Vec<String>), AssetError> {
        let (pack, notes) = Pack::read(dir)?;
        Ok((Self { packs: vec![pack] }, notes))
    }

    /// The strict check of one pack: its manifest parses with a supported version, has no entry that had to be dropped,
    /// and every path it names exists.
    pub fn open(dir: &Path) -> Result<Self, AssetError> {
        let (pack, notes) = Pack::read(dir)?;
        if let Some(first) = notes.first() {
            return Err(AssetError::Manifest {
                path: dir.join("manifest.json"),
                reason: first.clone(),
            });
        }
        Ok(Self { packs: vec![pack] })
    }

    /// The pack directories in lookup order (empty for the built-in fallback alone).
    pub fn roots(&self) -> impl Iterator<Item = &Path> {
        self.packs.iter().map(|p| p.root.as_path())
    }

    /// The directory, name and credits file of each pack, in lookup order.
    pub fn credits(&self) -> impl Iterator<Item = (&Path, Option<&str>, Option<PathBuf>)> {
        self.packs.iter().map(|p| {
            (
                p.root.as_path(),
                p.manifest.name.as_deref(),
                p.manifest.credits.as_ref().map(|c| p.root.join(c)),
            )
        })
    }

    /// Every source the packs give for icon `name` at `px` pixels, pack by pack in lookup order, each with the pack it
    /// comes from, then the built-in default map's host-theme names (with an empty pack path). The caller takes the first one
    /// it can load; when none loads, it draws a placeholder.
    pub fn icon_sources(&self, name: &IconName, px: u32) -> Vec<(&Path, IconSource)> {
        let mut out: Vec<(&Path, IconSource)> = self
            .packs
            .iter()
            .flat_map(|p| {
                p.icon_sources(name, px)
                    .into_iter()
                    .map(move |s| (p.root.as_path(), s))
            })
            .collect();
        if let Some(names) = builtin_icon_map().get(name.as_str()) {
            out.push((
                Path::new(""),
                IconSource::Theme {
                    names: names.clone(),
                    theme: None,
                },
            ));
        }
        out
    }

    /// The cursor theme of the first pack whose theme is found; `None` means the built-in cursors.
    pub fn cursor_theme(&self) -> Option<CursorTheme> {
        self.packs.iter().find_map(Pack::cursor_theme)
    }

    /// The host icon theme of the first pack whose `icon_theme` sets `applications`: applications' icons are looked
    /// up there (and in the themes it inherits) before `hicolor`.
    pub fn application_icon_theme(&self) -> Option<&str> {
        self.packs
            .iter()
            .filter_map(|p| p.manifest.icon_theme.as_ref())
            .find(|t| t.applications)
            .and_then(|t| t.name.as_deref())
    }

    /// The pictures the packs ship in their `wallpapers` directory, the first pack's first; a name an earlier pack has
    /// is not listed again.
    pub fn wallpaper_files(&self) -> Vec<WallpaperFile> {
        let mut out: Vec<WallpaperFile> = Vec::new();
        for pack in &self.packs {
            let dir = pack.root.join("wallpapers");
            let credits = read_credits(&dir.join("credits.txt"));
            for mut file in pictures_in(&dir) {
                file.credit = file
                    .light
                    .file_name()
                    .and_then(|n| n.to_str())
                    .and_then(|n| credits.get(n))
                    .cloned();
                if !out.iter().any(|f| f.name == file.name) {
                    out.push(file);
                }
            }
        }
        out
    }

    /// The default desktop picture for the light or `dark` appearance (the light one when a pack has no dark one) of
    /// the first pack that has its file; `None` means the procedural wallpaper.
    pub fn wallpaper_path(&self, dark: bool) -> Option<PathBuf> {
        self.packs
            .iter()
            .filter_map(|p| p.wallpaper(dark))
            .find(|p| p.is_file())
    }

    /// Family name (as stored in the font's name table) that the first pack defining `role` assigns to it. `None` when
    /// no pack does: the text renderer then takes the first usable family it loaded.
    pub fn font_family(&self, role: FontRole) -> Option<&str> {
        self.packs
            .iter()
            .find_map(|p| p.manifest.fonts.get(&role))
            .map(|f| f.family.as_str())
    }

    /// The fonts to load, in order: the font files of every pack in lookup order (those that exist), then the built-in
    /// font, always last, so that it also serves the characters the packs' faces lack. A pack whose fonts could not be
    /// listed (reported when it was read) contributes none.
    pub fn fonts(&self) -> Vec<FontSource> {
        let files = self.packs.iter().flat_map(|p| {
            p.fonts
                .iter()
                .filter(|f| f.is_file())
                .cloned()
                .map(FontSource::File)
        });
        files.chain([FontSource::Builtin]).collect()
    }
}

#[cfg(test)]
mod tests;
