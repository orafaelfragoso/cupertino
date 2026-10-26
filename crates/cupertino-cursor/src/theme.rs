//! Building a cursor theme: collect cursors (from vector PDFs with metadata and/or the procedural
//! set), rasterise them at every requested nominal size, bake the shadow, scale the hotspot and
//! write an XCursor theme (`index.theme`, `cursors/<name> files and alias symlinks), optionally
//! with a hyprcursor source theme.
//!
//! Scale: a nominal size `N` is `N / base_size` pixels per point (`base_size` 24 by default:
//! approximate, so 24 → 1 px per point, 32 → 1.33, 48 → 2, 64 → 2.67). Hotspots are metadata
//! points × scale, rounded, plus the padding the shadow added.

use crate::meta::{self, CursorMeta, HotspotSource, Shadow};
use crate::names::{self, Mapping};
use crate::plist;
use crate::procedural::{self, Kind};
use crate::raster::Poppler;
use crate::shadow;
use crate::xcursor::{self, XCursorImage};
use std::collections::{BTreeMap, BTreeSet};
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
    /// Bake a shadow where the metadata has shadow parameters.
    Auto,
    Never,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Procedural,
    /// Directory with one sub-directory per cursor, each holding `cursor.pdf` and optionally `cursor.meta`.
    Pdf(PathBuf),
}

#[derive(Clone, Debug)]
pub struct BuildOptions {
    pub output: PathBuf,
    pub name: String,
    pub sizes: Vec<u32>,
    pub base_size: u32,
    pub source: Source,
    /// Metadata JSON (shape); overrides the property lists.
    pub metadata: Option<PathBuf>,
    pub hotspot_source: HotspotSource,
    pub shadow: ShadowMode,
    /// Cursors without shadow metadata get [`Shadow::COMMON`].
    pub default_shadow: bool,
    /// The source's shadow y offset points up (the default interpretation).
    pub flip_shadow_y: bool,
    pub hyprcursor: bool,
    pub inherits: Option<String>,
    pub poppler: Poppler,
    /// Complete the theme with procedural cursors for names the source does not provide.
    pub fill_missing: bool,
    /// Frame delay in milliseconds for animations whose metadata has none (approximate).
    pub default_delay_ms: u32,
}

impl BuildOptions {
    pub fn new(output: impl Into<PathBuf>) -> Self {
        Self {
            output: output.into(),
            name: "Cupertino".into(),
            sizes: DEFAULT_SIZES.to_vec(),
            base_size: 24,
            source: Source::Procedural,
            metadata: None,
            hotspot_source: HotspotSource::Unscaled,
            shadow: ShadowMode::Auto,
            default_shadow: true,
            flip_shadow_y: true,
            hyprcursor: false,
            inherits: None,
            poppler: Poppler::default(),
            fill_missing: true,
            default_delay_ms: 100,
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.sizes.is_empty() || self.sizes.iter().any(|s| *s == 0 || *s > 512) {
            return Err("sizes must be between 1 and 512".into());
        }
        if self.base_size == 0 {
            return Err("base size must be positive".into());
        }
        if self.name.is_empty() || !self.name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') {
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Reference,
    Procedural,
}

#[derive(Clone)]
pub struct Cursor {
    /// File name of the primary XCursor file.
    pub name: String,
    pub aliases: Vec<String>,
    pub origin: Origin,
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
                    pixels: f.image.pixels().iter().map(|p| ((p.alpha() as u32) << 24) | ((p.red() as u32) << 16) | ((p.green() as u32) << 8) | p.blue() as u32).collect(),
                });
            }
        }
        xcursor::write(&images)
    }
}

fn find_mapping(primary: &str) -> Option<&'static Mapping> {
    names::SOURCE_MAP.iter().chain(names::PROCEDURAL_ONLY).find(|m| m.primary == primary)
}

fn finish_frame(src: Pixmap, shadow_params: Option<&Shadow>, hot_pt: (f64, f64), scale: f64, delay_ms: u32) -> Frame {
    let (image, origin) = match shadow_params {
        Some(s) => {
            let r = shadow::apply(&src, s, scale);
            (r.image, r.origin)
        }
        None => (src, (0, 0)),
    };
    let hx = ((hot_pt.0 * scale).round() as i64 + origin.0 as i64).clamp(0, image.width() as i64 - 1) as u32;
    let hy = ((hot_pt.1 * scale).round() as i64 + origin.1 as i64).clamp(0, image.height() as i64 - 1) as u32;
    Frame { image, hotspot: (hx, hy), delay_ms }
}

fn crop_frame(sheet: &Pixmap, index: u32, frame_h: u32) -> Pixmap {
    let mut out = Pixmap::new(sheet.width(), frame_h).expect("non-empty frame");
    out.draw_pixmap(0, -((index * frame_h) as i32), sheet.as_ref(), &tiny_skia::PixmapPaint::default(), tiny_skia::Transform::identity(), None);
    out
}

fn choose_shadow(o: &BuildOptions, m: &CursorMeta, has_meta: bool) -> Option<Shadow> {
    if o.shadow == ShadowMode::Never {
        return None;
    }
    m.shadow.or(if o.default_shadow && !has_meta { Some(Shadow::COMMON) } else { None })
}

/// Load metadata: the JSON file when given, else per-cursor property lists (read lazily).
fn load_metadata(o: &BuildOptions) -> Result<BTreeMap<String, CursorMeta>, String> {
    match &o.metadata {
        Some(p) => meta::from_json_file(p, o.flip_shadow_y),
        None => Ok(BTreeMap::new()),
    }
}

fn build_from_pdfs(o: &BuildOptions, dir: &Path, warnings: &mut Vec<String>) -> Result<Vec<Cursor>, String> {
    if !o.poppler.available() {
        return Err("poppler's pdftocairo and pdfinfo are required to rasterise PDF cursors (install poppler / poppler-utils), or use the procedural mode".into());
    }
    let json = load_metadata(o)?;
    let mut entries: Vec<_> = std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().filter(|e| e.path().join("cursor.pdf").is_file()).collect();
    entries.sort_by_key(|e| e.file_name());
    if entries.is_empty() {
        return Err(format!("no <cursor>/cursor.pdf found in {}", dir.display()));
    }
    let mut out = Vec::new();
    for e in entries {
        let name = e.file_name().to_string_lossy().into_owned();
        let pdf = e.path().join("cursor.pdf");
        let (meta, has_meta) = match json.get(&name) {
            Some(m) => (m.clone(), true),
            None => match std::fs::read(e.path().join("cursor.meta")).ok().filter(|b| b.starts_with(b"bplist00")) {
                Some(bytes) => match plist::raw_meta_from_bytes(&bytes) {
                    Ok(raw) => (CursorMeta::from_raw(&raw, o.flip_shadow_y), true),
                    Err(err) => {
                        warnings.push(format!("{name}: unreadable cursor.meta ({err}); using defaults"));
                        (CursorMeta { frames: 1, ..Default::default() }, false)
                    }
                },
                None => (CursorMeta { frames: 1, ..Default::default() }, false),
            },
        };
        let (page_w, page_h) = o.poppler.page_size(&pdf)?;
        let mut frames = meta.frames.max(1);
        if !has_meta && page_h >= 2.0 * page_w && (page_h / page_w).fract() == 0.0 {
            frames = (page_h / page_w) as u32;
            warnings.push(format!("{name}: no metadata; guessed {frames} stacked square frames from the page size (approximate)"));
        }
        let delay_ms = if frames > 1 && meta.delay_ms == 0 { o.default_delay_ms } else { meta.delay_ms };
        let (fw, fh) = (page_w, page_h / frames as f64);
        let hot = meta.hotspot_for(o.hotspot_source).unwrap_or_else(|| {
            warnings.push(format!("{name}: no hotspot in the metadata; using the frame centre"));
            (fw / 2.0, fh / 2.0)
        });
        let shadow_params = choose_shadow(o, &meta, has_meta);
        let mut sizes = Vec::new();
        for &nominal in &o.sizes {
            let scale = nominal as f64 / o.base_size as f64;
            let w = (fw * scale).round().max(1.0) as u32;
            let frame_h = (fh * scale).round().max(1.0) as u32;
            let sheet = o.poppler.render(&pdf, w, frame_h * frames)?;
            let list = (0..frames).map(|i| finish_frame(crop_frame(&sheet, i, frame_h), shadow_params.as_ref(), hot, scale, delay_ms)).collect();
            sizes.push(SizeVariant { nominal, frames: list });
        }
        let (primary, aliases) = match names::mapping_for_source(&name) {
            Some(m) => (m.primary.to_string(), m.aliases.iter().map(|s| s.to_string()).collect()),
            None => (name.clone(), Vec::new()),
        };
        out.push(Cursor { name: primary, aliases, origin: Origin::Reference, sizes });
    }
    Ok(out)
}

/// One procedural cursor at all sizes.
pub fn procedural_cursor(kind: Kind, o: &BuildOptions, aliases: Vec<String>) -> Cursor {
    let (hx, hy) = kind.hotspot();
    let shadow = if o.shadow == ShadowMode::Never { None } else { kind.shadow() };
    let sizes = o
        .sizes
        .iter()
        .map(|&nominal| {
            let scale = nominal as f64 / o.base_size as f64;
            let frames = (0..kind.frames()).map(|i| finish_frame(procedural::render(kind, scale as f32, i), shadow.as_ref(), (hx, hy), scale, kind.delay_ms())).collect();
            SizeVariant { nominal, frames }
        })
        .collect();
    Cursor { name: kind.name().to_string(), aliases, origin: Origin::Procedural, sizes }
}

/// Collect all cursors for `o`. Returns the cursors and non-fatal warnings.
pub fn build_cursors(o: &BuildOptions) -> Result<(Vec<Cursor>, Vec<String>), String> {
    o.validate()?;
    let mut warnings = Vec::new();
    let mut cursors = match &o.source {
        Source::Procedural => Vec::new(),
        Source::Pdf(dir) => build_from_pdfs(o, dir, &mut warnings)?,
    };
    if matches!(o.source, Source::Procedural) || o.fill_missing {
        let mut taken: BTreeSet<String> = cursors.iter().flat_map(|c| std::iter::once(c.name.clone()).chain(c.aliases.iter().cloned())).collect();
        for kind in Kind::ALL {
            if taken.contains(kind.name()) {
                continue;
            }
            let aliases: Vec<String> = find_mapping(kind.name()).map(|m| m.aliases.iter().filter(|a| !taken.contains(**a)).map(|s| s.to_string()).collect()).unwrap_or_default();
            taken.insert(kind.name().to_string());
            taken.extend(aliases.iter().cloned());
            cursors.push(procedural_cursor(kind, o, aliases));
        }
    }
    Ok((cursors, warnings))
}

#[derive(Debug, Default)]
pub struct Report {
    pub theme_dir: PathBuf,
    pub files: Vec<PathBuf>,
    pub symlinks: Vec<(PathBuf, String)>,
    pub warnings: Vec<String>,
    pub hypr_dir: Option<PathBuf>,
}

fn index_theme(o: &BuildOptions) -> String {
    let mut s = format!("[Icon Theme]\nName={}\nComment=Cursor theme generated by cupertino-cursor\n", o.name);
    if let Some(i) = &o.inherits {
        s.push_str(&format!("Inherits={i}\n"));
    }
    s
}

fn notice(o: &BuildOptions) -> String {
    match &o.source {
        Source::Pdf(_) => "LOCAL USE ONLY.\n\nThis theme was rasterised from a local source cursor set. Those assets are\nthird-party: do not redistribute this directory, do not commit it and do not ship it. It is\nwritten to a git-ignored location for side-by-side evaluation on this machine.\n\nCursors without reference artwork (the plain arrow, the text cursor) come from the procedural\nset of cupertino-cursor.\n".to_string(),
        Source::Procedural => "Generated by cupertino-cursor from its built-in procedural vector cursors; contains no third-party artwork.\n".to_string(),
    }
}

fn clean_previous(dir: &Path) -> Result<(), String> {
    if dir.exists() {
        if dir.join(MARKER_FILE).is_file() {
            std::fs::remove_dir_all(dir).map_err(|e| format!("cannot replace {}: {e}", dir.display()))?;
        } else if std::fs::read_dir(dir).map_err(|e| e.to_string())?.next().is_some() {
            return Err(format!("{} exists and was not generated by cupertino-cursor; refusing to overwrite it", dir.display()));
        }
    }
    Ok(())
}

/// Build and write the theme.
pub fn build_theme(o: &BuildOptions) -> Result<Report, String> {
    let (cursors, warnings) = build_cursors(o)?;
    write_theme(o, &cursors, warnings)
}

pub fn write_theme(o: &BuildOptions, cursors: &[Cursor], warnings: Vec<String>) -> Result<Report, String> {
    let theme_dir = o.output.join(&o.name);
    clean_previous(&theme_dir)?;
    let cur_dir = theme_dir.join("cursors");
    std::fs::create_dir_all(&cur_dir).map_err(|e| format!("{}: {e}", cur_dir.display()))?;
    let mut report = Report { theme_dir: theme_dir.clone(), warnings, ..Default::default() };
    let write = |path: PathBuf, bytes: &[u8], report: &mut Report| -> Result<(), String> {
        std::fs::write(&path, bytes).map_err(|e| format!("{}: {e}", path.display()))?;
        report.files.push(path);
        Ok(())
    };
    write(theme_dir.join("index.theme"), index_theme(o).as_bytes(), &mut report)?;
    write(theme_dir.join("README.txt"), notice(o).as_bytes(), &mut report)?;
    for c in cursors {
        let bytes = c.to_xcursor().map_err(|e| format!("{}: {e}", c.name))?;
        write(cur_dir.join(&c.name), &bytes, &mut report)?;
        for a in &c.aliases {
            let link = cur_dir.join(a);
            if link.symlink_metadata().is_ok() {
                continue;
            }
            std::os::unix::fs::symlink(&c.name, &link).map_err(|e| format!("{}: {e}", link.display()))?;
            report.symlinks.push((link, c.name.clone()));
        }
    }
    std::fs::write(theme_dir.join(MARKER_FILE), "").map_err(|e| e.to_string())?;
    if o.hyprcursor {
        report.hypr_dir = Some(crate::hypr::write(o, cursors, &notice(o))?);
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quick(output: &Path) -> BuildOptions {
        let mut o = BuildOptions::new(output);
        o.sizes = vec![24, 32];
        o
    }

    #[test]
    fn procedural_theme_has_every_required_name() {
        let d = tempfile::tempdir().unwrap();
        let o = quick(d.path());
        let r = build_theme(&o).unwrap();
        let cur = r.theme_dir.join("cursors");
        for n in names::REQUIRED_NAMES {
            let p = cur.join(n);
            assert!(p.exists(), "{n} missing");
            let bytes = std::fs::read(&p).unwrap();
            let imgs = xcursor::parse(&bytes).unwrap_or_else(|e| panic!("{n}: {e}"));
            assert!(!imgs.is_empty());
        }
        assert!(std::fs::read_to_string(r.theme_dir.join("index.theme")).unwrap().contains("Name=Cupertino"));
        assert!(r.theme_dir.join(MARKER_FILE).is_file());
    }

    #[test]
    fn aliases_are_symlinks_to_the_primary_file() {
        let d = tempfile::tempdir().unwrap();
        let r = build_theme(&quick(d.path())).unwrap();
        let cur = r.theme_dir.join("cursors");
        let link = cur.join("left_ptr");
        assert!(link.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read_link(&link).unwrap(), Path::new("default"));
        assert_eq!(std::fs::read(&link).unwrap(), std::fs::read(cur.join("default")).unwrap());
        assert!(!cur.join("default").symlink_metadata().unwrap().file_type().is_symlink());
        assert!(r.symlinks.iter().any(|(p, t)| p.ends_with("hand2") && t == "pointer"));
    }

    #[test]
    fn sizes_hotspots_and_animation_are_written() {
        let d = tempfile::tempdir().unwrap();
        let mut o = quick(d.path());
        o.sizes = vec![24, 32, 48, 64];
        let r = build_theme(&o).unwrap();
        let cur = r.theme_dir.join("cursors");
        let imgs = xcursor::parse(&std::fs::read(cur.join("pointer")).unwrap()).unwrap();
        assert_eq!(imgs.iter().map(|i| i.nominal_size).collect::<Vec<_>>(), vec![24, 32, 48, 64]);
        let hot24 = (imgs[0].xhot as f64, imgs[0].yhot as f64);
        let hot48 = (imgs[2].xhot as f64, imgs[2].yhot as f64);
        assert!((hot48.0 / hot24.0 - 2.0).abs() < 0.2 && (hot48.1 / hot24.1 - 2.0).abs() < 0.4, "{hot24:?} {hot48:?}");
        assert!(imgs[2].width > imgs[0].width * 3 / 2);
        let anim = xcursor::parse(&std::fs::read(cur.join("progress")).unwrap()).unwrap();
        assert_eq!(anim.len(), 4 * 12);
        assert!(anim.iter().all(|f| f.delay_ms == 80));
        assert_ne!(anim[0].pixels, anim[3].pixels, "the spinner changes between frames");
        assert_eq!(std::fs::read(cur.join("wait")).unwrap(), std::fs::read(cur.join("progress")).unwrap());
    }

    #[test]
    fn shadow_can_be_disabled() {
        let d = tempfile::tempdir().unwrap();
        let (with, _) = build_cursors(&quick(d.path())).unwrap();
        let mut o = quick(d.path());
        o.shadow = ShadowMode::Never;
        let (without, _) = build_cursors(&o).unwrap();
        let w = |cs: &[Cursor]| cs.iter().find(|c| c.name == "default").unwrap().sizes[0].frames[0].image.width();
        assert!(w(&with) > w(&without), "padding for the shadow");
        assert_eq!(w(&without), 32);
    }

    #[test]
    fn refuses_to_overwrite_foreign_directories_but_replaces_its_own() {
        let d = tempfile::tempdir().unwrap();
        let o = quick(d.path());
        std::fs::create_dir_all(d.path().join("Cupertino")).unwrap();
        std::fs::write(d.path().join("Cupertino/precious.txt"), "x").unwrap();
        assert!(build_theme(&o).unwrap_err().contains("refusing"));
        std::fs::remove_dir_all(d.path().join("Cupertino")).unwrap();
        build_theme(&o).unwrap();
        std::fs::write(d.path().join("Cupertino/cursors/stale"), "x").unwrap();
        build_theme(&o).unwrap();
        assert!(!d.path().join("Cupertino/cursors/stale").exists(), "own output is replaced");
    }

    #[test]
    fn options_are_validated() {
        let d = tempfile::tempdir().unwrap();
        let mut o = quick(d.path());
        o.sizes = vec![];
        assert!(build_theme(&o).is_err());
        o.sizes = vec![24];
        o.name = "../x".into();
        assert!(build_theme(&o).is_err());
        o.name = "Ok".into();
        o.base_size = 0;
        assert!(build_theme(&o).is_err());
    }

    #[test]
    fn pdf_source_without_poppler_or_files_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let mut o = quick(d.path());
        o.source = Source::Pdf(d.path().join("src"));
        o.poppler = Poppler { pdftocairo: "/nonexistent".into(), pdfinfo: "/nonexistent".into() };
        assert!(build_theme(&o).unwrap_err().contains("poppler"));
        o.poppler = Poppler::default();
        if o.poppler.available() {
            std::fs::create_dir_all(d.path().join("src/empty")).unwrap();
            assert!(build_theme(&o).unwrap_err().contains("no <cursor>/cursor.pdf"));
        }
    }

    #[test]
    fn inherits_is_written_when_requested() {
        let d = tempfile::tempdir().unwrap();
        let mut o = quick(d.path());
        o.inherits = Some("Adwaita".into());
        let r = build_theme(&o).unwrap();
        assert!(std::fs::read_to_string(r.theme_dir.join("index.theme")).unwrap().contains("Inherits=Adwaita"));
    }
}
