use crate::{
    Assets, CursorTheme, FontRole, IconName, IconSource, BUILTIN_UI_FAMILY, CURSOR_NAMES,
    RASTER_NAMES, SYMBOL_NAMES,
};
use serde::Serialize;
use std::fmt;
use std::path::PathBuf;

/// What reading a pack gave, as `cupertino-assets check` and `cupertino appearance assets check` report it: the
/// problems, then per logical name what provides it, the pack or the built-in fallback.
#[derive(Debug, Serialize)]
pub struct PackReport {
    pub packs: Vec<PackEntry>,
    pub problems: Vec<String>,
    pub fonts: Vec<Provided>,
    pub icons: Vec<Provided>,
    /// The cursor theme and its search directory; `None` draws the procedural cursors.
    pub cursor_theme: Option<CursorTheme>,
    /// The cursors the theme lacks, each drawn with the theme's `default`.
    pub cursors: Vec<Provided>,
    pub wallpapers: Vec<Provided>,
    /// What the built-in fallback provides.
    pub fallbacks: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct PackEntry {
    pub root: PathBuf,
    pub name: Option<String>,
    pub credits: Option<PathBuf>,
}

#[derive(Debug, Serialize)]
pub struct Provided {
    pub name: String,
    pub provider: String,
}

fn provided(name: impl Into<String>, provider: impl Into<String>) -> Provided {
    Provided {
        name: name.into(),
        provider: provider.into(),
    }
}

impl PackReport {
    pub fn new(assets: &Assets, notes: &[String]) -> Self {
        let mut fallbacks = Vec::new();
        let packs = assets
            .credits()
            .map(|(root, name, credits)| PackEntry {
                root: root.to_path_buf(),
                name: name.map(str::to_string),
                credits,
            })
            .collect();
        let fonts = FontRole::ALL
            .into_iter()
            .map(|role| match assets.font_family(role) {
                Some(family) => provided(role.key(), family),
                None if role == FontRole::Ui => {
                    fallbacks.push("font ui".to_string());
                    provided("ui", format!("built-in ({BUILTIN_UI_FAMILY})"))
                }
                None => provided(role.key(), "not set (optional)"),
            })
            .collect();
        let icons = RASTER_NAMES
            .iter()
            .chain(&SYMBOL_NAMES)
            .map(|name| {
                let icon = IconName::new(name).expect("the logical names are valid");
                let provider = match assets.icon_sources(&icon, 32).into_iter().next() {
                    Some((_, IconSource::Raster(r))) => format!("raster {}", r.path.display()),
                    Some((_, IconSource::Svg(p))) => format!("svg {}", p.display()),
                    Some((_, IconSource::Symbol(p))) => format!("symbol {}", p.display()),
                    Some((_, IconSource::Theme { names, theme })) => format!(
                        "host theme {} ({})",
                        theme.as_deref().unwrap_or("of the user"),
                        names.join(", ")
                    ),
                    None => {
                        fallbacks.push(format!("icon {name}"));
                        "placeholder (built-in)".to_string()
                    }
                };
                provided(*name, provider)
            })
            .collect();
        let cursor_theme = assets.cursor_theme();
        let cursors = match &cursor_theme {
            Some(theme) => {
                let dir = theme.search_dir.join(&theme.name).join("cursors");
                CURSOR_NAMES
                    .iter()
                    .filter(|name| !dir.join(name).exists())
                    .map(|name| {
                        fallbacks.push(format!("cursor {name}"));
                        provided(*name, "the theme's `default`")
                    })
                    .collect()
            }
            None => {
                fallbacks.push("cursor theme".to_string());
                Vec::new()
            }
        };
        let wallpapers = [("light", false), ("dark", true)]
            .into_iter()
            .map(|(label, dark)| match assets.wallpaper_path(dark) {
                Some(p) => provided(label, p.display().to_string()),
                None => {
                    fallbacks.push(format!("wallpaper {label}"));
                    provided(label, "procedural (built-in)")
                }
            })
            .collect();
        Self {
            packs,
            problems: notes.to_vec(),
            fonts,
            icons,
            cursor_theme,
            cursors,
            wallpapers,
            fallbacks,
        }
    }
}

impl fmt::Display for PackReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let list = |f: &mut fmt::Formatter<'_>, title: &str, items: &[Provided]| {
            writeln!(f, "{title}:")?;
            items
                .iter()
                .try_for_each(|p| writeln!(f, "  {}: {}", p.name, p.provider))
        };
        for pack in &self.packs {
            writeln!(
                f,
                "pack {} ({})",
                pack.root.display(),
                pack.name.as_deref().unwrap_or("unnamed")
            )?;
            match &pack.credits {
                Some(c) => writeln!(f, "  credits: {}", c.display())?,
                None => writeln!(f, "  credits: none")?,
            }
        }
        writeln!(f, "problems: {}", self.problems.len())?;
        for n in &self.problems {
            writeln!(f, "  {n}")?;
        }
        list(f, "fonts", &self.fonts)?;
        list(f, "icons", &self.icons)?;
        writeln!(f, "cursors:")?;
        match &self.cursor_theme {
            Some(theme) => {
                writeln!(
                    f,
                    "  theme {} in {}",
                    theme.name,
                    theme.search_dir.display()
                )?;
                for c in &self.cursors {
                    writeln!(f, "  {}: {}", c.name, c.provider)?;
                }
            }
            None => writeln!(f, "  procedural (built-in)")?,
        }
        list(f, "wallpaper", &self.wallpapers)?;
        writeln!(f, "falls back: {}", self.fallbacks.len())?;
        for fb in &self.fallbacks {
            writeln!(f, "  {fb}")?;
        }
        Ok(())
    }
}
