use crate::IconName;
use serde::Deserialize;
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// The manifest versions this crate reads. Version 1 is read as the subset of version 2 it is.
pub const VERSIONS: [u64; 2] = [1, 2];

/// The text roles of the font table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize)]
pub enum FontRole {
    #[serde(rename = "ui")]
    Ui,
    #[serde(rename = "ui-rounded")]
    UiRounded,
    #[serde(rename = "ui-compact")]
    UiCompact,
    #[serde(rename = "mono")]
    Mono,
    #[serde(rename = "serif")]
    Serif,
}

impl FontRole {
    pub const ALL: [FontRole; 5] = [
        FontRole::Ui,
        FontRole::UiRounded,
        FontRole::UiCompact,
        FontRole::Mono,
        FontRole::Serif,
    ];

    /// The role's key in the manifest.
    pub fn key(self) -> &'static str {
        match self {
            FontRole::Ui => "ui",
            FontRole::UiRounded => "ui-rounded",
            FontRole::UiCompact => "ui-compact",
            FontRole::Mono => "mono",
            FontRole::Serif => "serif",
        }
    }

    fn from_key(key: &str) -> Option<FontRole> {
        FontRole::ALL.into_iter().find(|r| r.key() == key)
    }
}

/// A raster icon file and the pixel size it was chosen at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RasterIcon {
    pub path: PathBuf,
    pub px: u32,
}

/// The font of one role: its family name (as stored in the font's name table) and, in version 2, its files.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FontEntry {
    pub family: String,
    /// Pack-relative files; `None` (version 1) means every font file in `fonts/`.
    pub files: Option<Vec<String>>,
}

/// The sources a pack gives for one icon name, each optional.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct IconEntry {
    /// Pixel size to pack-relative PNG.
    pub raster: BTreeMap<u32, String>,
    /// A full-colour SVG drawn at the requested size.
    pub svg: Option<String>,
    /// A single-colour template SVG the caller tints (only for `symbol.*` names).
    pub symbol: Option<String>,
    /// Freedesktop icon names, resolved in the host icon theme.
    pub theme: Vec<String>,
}

/// The host icon theme and the default map of a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct IconTheme {
    /// Theme to search; `None` is the user's theme.
    pub name: Option<String>,
    /// Pack-relative TOML file `"<logical name>" = ["<freedesktop name>", ...]`.
    pub map: Option<String>,
    /// Whether applications' icons are looked up in the theme too, before `hicolor`.
    pub applications: bool,
}

/// The cursor theme of a pack and where to look for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Cursors {
    pub theme: String,
    /// Search directories: pack-relative, absolute, `~/...` or `$XCURSOR_PATH`.
    pub path: Vec<String>,
}

/// The default wallpaper of a pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Wallpaper {
    pub light: String,
    pub dark: Option<String>,
}

/// A manifest in the version-2 model, whatever its version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Manifest {
    pub version: u64,
    pub name: Option<String>,
    pub credits: Option<String>,
    pub fonts: BTreeMap<FontRole, FontEntry>,
    pub fallback_fonts: Vec<String>,
    pub icons: BTreeMap<IconName, IconEntry>,
    pub icon_theme: Option<IconTheme>,
    pub cursors: Option<Cursors>,
    pub wallpaper: Option<Wallpaper>,
}

/// A relative path with plain components only (no `..`, no root).
fn is_plain_relative(p: &str) -> bool {
    !p.is_empty()
        && std::path::Path::new(p)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// What reading a manifest gave: the manifest and the problems that dropped single entries (each one logged by the
/// caller), or the error that makes the whole pack unusable (invalid JSON, an unsupported version).
pub(crate) type Parsed = Result<(Manifest, Vec<String>), String>;

/// Collects the entry-level problems of one manifest.
struct Reader {
    notes: Vec<String>,
}

impl Reader {
    fn note(&mut self, msg: String) {
        self.notes.push(msg);
    }

    fn path(&mut self, what: &str, v: &Value) -> Option<String> {
        match v.as_str() {
            Some(p) if is_plain_relative(p) => Some(p.to_string()),
            _ => {
                self.note(format!(
                    "{what}: {v} is not a relative path inside the pack; ignored"
                ));
                None
            }
        }
    }

    fn strings(&mut self, what: &str, v: &Value) -> Vec<String> {
        match v.as_array() {
            Some(items) if items.iter().all(Value::is_string) => items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect(),
            _ => {
                self.note(format!("{what}: expected a list of strings; ignored"));
                Vec::new()
            }
        }
    }

    fn unknown(&mut self, what: &str, obj: &Map<String, Value>, known: &[&str]) {
        for key in obj.keys().filter(|k| !known.contains(&k.as_str())) {
            self.note(format!("{what}: unknown key {key:?} ignored"));
        }
    }

    fn fonts(&mut self, v: &Value) -> BTreeMap<FontRole, FontEntry> {
        let mut out = BTreeMap::new();
        let Some(obj) = v.as_object() else {
            self.note("fonts: expected an object; ignored".to_string());
            return out;
        };
        for (key, entry) in obj {
            let Some(role) = FontRole::from_key(key) else {
                self.note(format!("fonts: unknown role {key:?} ignored"));
                continue;
            };
            let parsed = match entry {
                Value::String(family) => Some(FontEntry {
                    family: family.clone(),
                    files: None,
                }),
                Value::Object(o) => {
                    self.unknown(&format!("fonts.{key}"), o, &["family", "files"]);
                    let family = o.get("family").and_then(Value::as_str).map(str::to_string);
                    let files = o
                        .get("files")
                        .map(|f| self.strings(&format!("fonts.{key}.files"), f));
                    let files = files.map(|fs| {
                        fs.iter()
                            .filter_map(|p| {
                                self.path(&format!("fonts.{key}.files"), &Value::String(p.clone()))
                            })
                            .collect()
                    });
                    family.map(|family| FontEntry { family, files })
                }
                _ => None,
            };
            match parsed {
                Some(f) if !f.family.is_empty() => {
                    out.insert(role, f);
                }
                _ => self.note(format!("fonts.{key}: needs a family name; ignored")),
            }
        }
        out
    }

    fn icon(&mut self, name: &IconName, entry: &Value) -> Option<IconEntry> {
        let what = format!("icons.{name}");
        let Some(o) = entry.as_object() else {
            self.note(format!("{what}: expected an object; ignored"));
            return None;
        };
        self.unknown(&what, o, &["raster", "svg", "symbol", "theme"]);
        let mut icon = IconEntry::default();
        let dir = format!("icons/{name}");
        match o.get("raster") {
            // version 1: a list of pixel sizes, the files are icons/<name>/<px>.png
            Some(Value::Array(sizes)) => {
                for s in sizes {
                    match s
                        .as_u64()
                        .and_then(|n| u32::try_from(n).ok())
                        .filter(|&n| n > 0)
                    {
                        Some(px) => {
                            icon.raster.insert(px, format!("{dir}/{px}.png"));
                        }
                        None => {
                            self.note(format!("{what}.raster: {s} is not a pixel size; ignored"))
                        }
                    }
                }
            }
            Some(Value::Object(map)) => {
                for (px, file) in map {
                    match px.parse::<u32>().ok().filter(|&n| n > 0) {
                        Some(n) => {
                            if let Some(p) = self.path(&format!("{what}.raster.{px}"), file) {
                                icon.raster.insert(n, p);
                            }
                        }
                        None => self.note(format!(
                            "{what}.raster: {px:?} is not a pixel size; ignored"
                        )),
                    }
                }
            }
            Some(other) => self.note(format!(
                "{what}.raster: {other} is neither a size list nor a size map; ignored"
            )),
            None => {}
        }
        if let Some(svg) = o.get("svg") {
            icon.svg = self.path(&format!("{what}.svg"), svg);
        }
        match o.get("symbol") {
            // version 1: `true` means icons/<name>/symbol.svg
            Some(Value::Bool(true)) => icon.symbol = Some(format!("{dir}/symbol.svg")),
            Some(Value::Bool(false)) | None => {}
            Some(p) => icon.symbol = self.path(&format!("{what}.symbol"), p),
        }
        if icon.symbol.is_some() && !name.as_str().starts_with("symbol.") {
            self.note(format!(
                "{what}.symbol: only `symbol.*` names are template symbols; ignored"
            ));
            icon.symbol = None;
        }
        if let Some(t) = o.get("theme") {
            icon.theme = self.strings(&format!("{what}.theme"), t);
        }
        if icon.raster.is_empty()
            && icon.svg.is_none()
            && icon.symbol.is_none()
            && icon.theme.is_empty()
        {
            self.note(format!("{what}: provides no source; ignored"));
            return None;
        }
        Some(icon)
    }

    fn icons(&mut self, v: &Value) -> BTreeMap<IconName, IconEntry> {
        let mut out = BTreeMap::new();
        let Some(obj) = v.as_object() else {
            self.note("icons: expected an object; ignored".to_string());
            return out;
        };
        for (key, entry) in obj {
            let Ok(name) = IconName::new(key) else {
                self.note(format!(
                    "icons: {key:?} is an invalid semantic icon name; ignored"
                ));
                continue;
            };
            if let Some(icon) = self.icon(&name, entry) {
                out.insert(name, icon);
            }
        }
        out
    }

    fn cursors(&mut self, v: &Value) -> Option<Cursors> {
        let theme_name = |t: &str| !t.is_empty() && is_plain_relative(t) && !t.contains('/');
        match v {
            // version 1: the theme name, in the pack's cursors/
            Value::String(t) if theme_name(t) => Some(Cursors {
                theme: t.clone(),
                path: vec!["cursors".to_string()],
            }),
            Value::Object(o) => {
                self.unknown("cursors", o, &["theme", "path"]);
                let theme = o
                    .get("theme")
                    .and_then(Value::as_str)
                    .filter(|t| theme_name(t));
                let Some(theme) = theme else {
                    self.note("cursors.theme: needs a theme name; ignored".to_string());
                    return None;
                };
                let path = o.get("path").map_or_else(
                    || vec!["cursors".to_string()],
                    |p| self.strings("cursors.path", p),
                );
                Some(Cursors {
                    theme: theme.to_string(),
                    path,
                })
            }
            other => {
                self.note(format!("cursors: {other} is not a theme name; ignored"));
                None
            }
        }
    }

    fn wallpapers(&mut self, v: &Value) -> Option<Wallpaper> {
        let Some(default) = v.as_object().and_then(|o| {
            self.unknown("wallpapers", o, &["default"]);
            o.get("default")
        }) else {
            self.note("wallpapers: needs a `default` entry; ignored".to_string());
            return None;
        };
        let Some(d) = default.as_object() else {
            self.note("wallpapers.default: expected an object; ignored".to_string());
            return None;
        };
        self.unknown("wallpapers.default", d, &["light", "dark"]);
        let light = d
            .get("light")
            .and_then(|p| self.path("wallpapers.default.light", p))?;
        let dark = d
            .get("dark")
            .and_then(|p| self.path("wallpapers.default.dark", p));
        Some(Wallpaper { light, dark })
    }
}

impl Manifest {
    /// Read `text`. Invalid JSON or an unsupported version is an error; every other problem drops the entry it is in
    /// and is returned as a note.
    pub fn parse(text: &str) -> Parsed {
        let value: Value = serde_json::from_str(text).map_err(|e| e.to_string())?;
        let obj = value
            .as_object()
            .ok_or("the manifest is not a JSON object")?;
        let version = obj
            .get("version")
            .and_then(Value::as_u64)
            .ok_or("the manifest has no version")?;
        if !VERSIONS.contains(&version) {
            return Err(format!(
                "contract version {version} is not supported (expected 1 or 2)"
            ));
        }
        let mut r = Reader { notes: Vec::new() };
        let known: &[&str] = if version == 1 {
            &["version", "fonts", "icons", "cursors", "wallpaper"]
        } else {
            &[
                "version",
                "name",
                "credits",
                "fonts",
                "fallback_fonts",
                "icons",
                "icon_theme",
                "cursors",
                "wallpapers",
            ]
        };
        r.unknown("manifest", obj, known);
        let fonts = obj.get("fonts").map(|v| r.fonts(v)).unwrap_or_default();
        let fallback_fonts = obj
            .get("fallback_fonts")
            .map(|v| r.strings("fallback_fonts", v))
            .unwrap_or_default();
        let fallback_fonts = fallback_fonts
            .iter()
            .filter_map(|p| r.path("fallback_fonts", &Value::String(p.clone())))
            .collect();
        let icons = obj.get("icons").map(|v| r.icons(v)).unwrap_or_default();
        let icon_theme = obj.get("icon_theme").and_then(|v| match v.as_object() {
            Some(o) => {
                r.unknown("icon_theme", o, &["name", "map", "applications"]);
                let map = o.get("map").and_then(|m| r.path("icon_theme.map", m));
                let applications = match o.get("applications") {
                    None => false,
                    Some(Value::Bool(b)) => *b,
                    Some(_) => {
                        r.note(
                            "icon_theme.applications: expected true or false; ignored".to_string(),
                        );
                        false
                    }
                };
                Some(IconTheme {
                    name: o.get("name").and_then(Value::as_str).map(str::to_string),
                    map,
                    applications,
                })
            }
            None => {
                r.note("icon_theme: expected an object; ignored".to_string());
                None
            }
        });
        let cursors = obj.get("cursors").and_then(|v| r.cursors(v));
        let wallpaper = match (version, obj.get("wallpaper"), obj.get("wallpapers")) {
            (1, Some(w), _) => r
                .path("wallpaper", w)
                .map(|light| Wallpaper { light, dark: None }),
            (2, _, Some(w)) => r.wallpapers(w),
            _ => None,
        };
        let name = obj.get("name").and_then(Value::as_str).map(str::to_string);
        let credits = obj.get("credits").and_then(|c| r.path("credits", c));
        Ok((
            Manifest {
                version,
                name,
                credits,
                fonts,
                fallback_fonts,
                icons,
                icon_theme,
                cursors,
                wallpaper,
            },
            r.notes,
        ))
    }
}
