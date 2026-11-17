//! Building a cursor theme: draw the procedural cursors at every requested nominal size, bake the shadow, scale
//! the hotspot and write an XCursor theme (`index.theme`, `cursors/<name>` files and alias symlinks), optionally
//! with a hyprcursor source theme.
//!
//! Scale: a nominal size `N` is `N / base_size` pixels per point (`base_size` 24 by default:
//! approximate, so 24 → 1 px per point, 32 → 1.33, 48 → 2, 64 → 2.67). Hotspots are points × scale,
//! rounded, plus the padding the shadow added.

use crate::names::{self, Mapping};
use crate::procedural::{self, Kind};
use crate::shadow::{self, Shadow};
use crate::xcursor::{self, XCursorImage};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tiny_skia::Pixmap;

/// Nominal sizes written by default: the base 24, the usual high-density steps and 96, four times
/// the base, so the largest user cursor scale (4) is drawn from an image rather than
/// scaled up. The shake-to-locate growth (up to 5) is transient and scales the largest
/// image; a 120 size would nearly double the procedural theme (18 MB instead of 9.9 MB).
pub const DEFAULT_SIZES: [u32; 5] = [24, 32, 48, 64, 96];
pub const MARKER_FILE: &str = ".generated-by-cupertino-cursor";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowMode {
    /// Bake the common shadow under every cursor.
    Auto,
    Never,
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub output: PathBuf,
    pub name: String,
    pub sizes: Vec<u32>,
    pub base_size: u32,
    pub shadow: ShadowMode,
    pub hyprcursor: bool,
    pub inherits: Option<String>,
}

impl BuildOptions {
    pub fn new(output: impl Into<PathBuf>) -> Self {
        Self {
            output: output.into(),
            name: "Cupertino".into(),
            sizes: DEFAULT_SIZES.to_vec(),
            base_size: 24,
            shadow: ShadowMode::Auto,
            hyprcursor: false,
            inherits: None,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.sizes.is_empty() || self.sizes.iter().any(|s| *s == 0 || *s > 512) {
            return Err("sizes must be between 1 and 512".into());
        }
        if self.base_size == 0 {
            return Err("base size must be positive".into());
        }
        if self.name.is_empty()
            || !self
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err("theme name may only contain letters, digits, '-' and '_'".into());
        }
        Ok(())
    }
}

#[derive(Clone)]
pub struct Frame {
    pub image: Pixmap,
    pub hotspot: (u32, u32),
    pub delay_ms: u32,
}

#[derive(Clone)]
pub struct SizeVariant {
    pub nominal: u32,
    pub frames: Vec<Frame>,
}

#[derive(Clone)]
pub struct Cursor {
    /// File name of the primary XCursor file.
    pub name: String,
    pub aliases: Vec<String>,
    pub sizes: Vec<SizeVariant>,
}

impl Cursor {
    pub fn to_xcursor(&self) -> Result<Vec<u8>, String> {
        let mut images = Vec::new();
        for v in &self.sizes {
            for f in &v.frames {
                images.push(XCursorImage {
                    nominal_size: v.nominal,
                    width: f.image.width(),
                    height: f.image.height(),
                    xhot: f.hotspot.0,
                    yhot: f.hotspot.1,
                    delay_ms: f.delay_ms,
                    pixels: f
                        .image
                        .pixels()
                        .iter()
                        .map(|p| {
                            ((p.alpha() as u32) << 24)
                                | ((p.red() as u32) << 16)
                                | ((p.green() as u32) << 8)
                                | p.blue() as u32
                        })
                        .collect(),
                });
            }
        }
        xcursor::write(&images)
    }
}

fn find_mapping(primary: &str) -> Option<&'static Mapping> {
    names::NAMES.iter().find(|m| m.primary == primary)
}

/// The cursor `src` with its shadow, cut back to the cursor's canvas: the image keeps the role's
/// size at every scale and the shadow falls in its transparent margin.
fn finish_frame(
    src: Pixmap,
    shadow_params: Option<&Shadow>,
    hot_pt: (f64, f64),
    scale: f64,
    delay_ms: u32,
) -> Frame {
    let image = match shadow_params {
        Some(s) => {
            let r = shadow::apply(&src, s, scale);
            let canvas = tiny_skia::IntRect::from_xywh(
                r.origin.0 as i32,
                r.origin.1 as i32,
                src.width(),
                src.height(),
            )
            .expect("a non-empty canvas");
            r.image
                .clone_rect(canvas)
                .expect("the canvas lies inside the shadowed image")
        }
        None => src,
    };
    let hx = ((hot_pt.0 * scale).round() as i64).clamp(0, image.width() as i64 - 1) as u32;
    let hy = ((hot_pt.1 * scale).round() as i64).clamp(0, image.height() as i64 - 1) as u32;
    Frame {
        image,
        hotspot: (hx, hy),
        delay_ms,
    }
}

/// One procedural cursor at all sizes.
pub fn procedural_cursor(kind: Kind, o: &BuildOptions, aliases: Vec<String>) -> Cursor {
    let (hx, hy) = kind.hotspot();
    let shadow = if o.shadow == ShadowMode::Never {
        None
    } else {
        kind.shadow()
    };
    let sizes = o
        .sizes
        .iter()
        .map(|&nominal| {
            let scale = nominal as f64 / o.base_size as f64;
            let frames = (0..kind.frames())
                .map(|i| {
                    finish_frame(
                        procedural::render(kind, scale as f32, i),
                        shadow.as_ref(),
                        (hx, hy),
                        scale,
                        kind.delay_ms(),
                    )
                })
                .collect();
            SizeVariant { nominal, frames }
        })
        .collect();
    Cursor {
        name: kind.name().to_string(),
        aliases,
        sizes,
    }
}

/// Collect all cursors for `o`: every cursor of the procedural set with the aliases of [`names::NAMES`].
pub fn build_cursors(o: &BuildOptions) -> Result<Vec<Cursor>, String> {
    o.validate()?;
    let mut cursors = Vec::new();
    let mut taken = BTreeSet::new();
    for kind in Kind::ALL {
        let aliases: Vec<String> = find_mapping(kind.name())
            .map(|m| {
                m.aliases
                    .iter()
                    .filter(|a| !taken.contains(**a))
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();
        taken.insert(kind.name().to_string());
        taken.extend(aliases.iter().cloned());
        cursors.push(procedural_cursor(kind, o, aliases));
    }
    Ok(cursors)
}

#[derive(Debug, Default)]
pub struct Report {
    pub theme_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub symlinks: Vec<(PathBuf, String)>,
    pub hypr_dir: Option<PathBuf>,
}

fn index_theme(o: &BuildOptions) -> String {
    let mut s = format!(
        "[Icon Theme]\nName={}\nComment=Cursor theme generated by cupertino-cursor\n",
        o.name
    );
    if let Some(i) = &o.inherits {
        s.push_str(&format!("Inherits={i}\n"));
    }
    s
}

fn notice() -> &'static str {
    "Generated by cupertino-cursor from its built-in procedural vector cursors; contains no third-party artwork.\n"
}

fn clean_previous(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        if dir.join(MARKER_FILE).is_file() {
            std::fs::remove_dir_all(dir)
                .map_err(|e| format!("cannot replace {}: {e}", dir.display()))?;
        } else if std::fs::read_dir(dir)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
        {
            return Err(format!(
                "{} exists and was not generated by cupertino-cursor; refusing to overwrite it",
                dir.display()
            ));
        }
    }
    Ok(())
}

/// Build and write the theme.
pub fn build_theme(o: &BuildOptions) -> Result<Report, String> {
    let cursors = build_cursors(o)?;
    write_theme(o, &cursors)
}

pub fn write_theme(o: &BuildOptions, cursors: &[Cursor]) -> Result<Report, String> {
    let theme_dir = o.output.join(&o.name);
    clean_previous(&theme_dir)?;
    let cur_dir = theme_dir.join("cursors");
    std::fs::create_dir_all(&cur_dir).map_err(|e| format!("{}: {e}", cur_dir.display()))?;
    let mut report = Report {
        theme_dir: theme_dir.clone(),
        ..Default::default()
    };
    let write = |path: PathBuf, bytes: &[u8], report: &mut Report| -> Result<(), String> {
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        report.files.push(path);
        Ok(())
    };
    write(
        theme_dir.join("index.theme"),
        index_theme(o).as_bytes(),
        &mut report,
    )?;
    write(
        theme_dir.join("README.txt"),
        notice().as_bytes(),
        &mut report,
    )?;
    for c in cursors {
        let bytes = c.to_xcursor().map_err(|e| format!("{}: {e}", c.name))?;
        write(cur_dir.join(&c.name), &bytes, &mut report)?;
        for a in &c.aliases {
            let link = cur_dir.join(a);
            if link.symlink_metadata().is_ok() {
                continue;
            }
            std::os::unix::fs::symlink(&c.name, &link)
                .map_err(|e| format!("{}: {e}", link.display()))?;
            report.symlinks.push((link, c.name.clone()));
        }
    }
    std::fs::write(theme_dir.join(MARKER_FILE), "").map_err(|e| e.to_string())?;
    if o.hyprcursor {
        report.hypr_dir = Some(crate::hypr::write(o, cursors, notice())?);
    }
    Ok(report)
}
