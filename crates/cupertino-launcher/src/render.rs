//! Panel layout and CPU rendering.
//!
//! [`PanelRenderer::layout`] turns the panel state into geometry in points (input field, the
//! visible window of header and result rows); [`PanelRenderer::render`] rasterises it to a
//! premultiplied RGBA [`Pixmap`] at a device scale, with the same three layers as the banner
//! renderer ([`Layers`]): shadow, background material and content.
//!
//! Provenance: colours (labels, selection, separator, rim) are design tokens; the background is a
//! material recipe of the specification looked up by name per appearance
//! ([`PanelStyle::material_for`], `Menu` is the fallback); the shadow is the
//! standard window row of the verified window shadow table; text uses the substitute
//! system UI font through `cupertino-decorations`' text renderer with the specification's tracking
//! table. Panel width, height limits, corner radius, placement, input and icon sizes follow the
//! reference analysis (labelled per field in [`PanelStyle::from_theme`]); row
//! heights and paddings are approximate. Everything is configurable in [`PanelStyle`].

use crate::engine::{ItemKind, ResultItem};
use crate::panel::Panel;
use cupertino_decorations::shadow::{ShadowParams, ShadowRasterizer};
use cupertino_decorations::shape::rounded_path;
use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::text::TextRenderer;
use cupertino_icons::SystemIcons;
use cupertino_geometry::{CornerStyle, Point, Radii, Rect, Size};
use cupertino_theme::recipe::{MaterialOptions, Recipe};
use cupertino_theme::window_shadow::ShadowOption;
use cupertino_theme::{Appearance, SemanticColor, Theme, Typography};
use tiny_skia::{FillRule, Mask, Paint, Pixmap, PixmapPaint, Stroke, Transform};

pub type Rgba = [f32; 4];

/// Layer surface that holds the panel at every size it can take: where it sits and where the
/// panel is inside it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceGeometry {
    /// Logical size of the surface, including the shadow margins.
    pub size: (u32, u32),
    /// Distance of the surface from the top of the output, for a surface anchored to the top and
    /// centred horizontally; negative while the shadow starts above the output.
    pub top_margin: i32,
    /// Top-left corner of the panel, in surface-local points.
    pub origin: Point,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PanelPalette {
    pub label: Rgba,
    pub secondary_label: Rgba,
    pub tertiary_label: Rgba,
    /// Selected-row wash: vibrant; text keeps its label colours.
    pub selection: Rgba,
    /// Accent colour (insertion point).
    pub accent: Rgba,
    pub separator: Rgba,
    pub rim: Rgba,
    pub fallback_background: Rgba,
    pub glyph_fill: Rgba,
}

impl PanelPalette {
    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Self {
        let dark = appearance.is_dark();
        let t = &theme.tokens;
        let sem = |c: SemanticColor, f: Rgba| t.semantic(appearance, c).map(|c| c.to_array()).unwrap_or(f);
        let accent = sem(SemanticColor::SystemBlue, [0.0, 0.48, 1.0, 1.0]);
        let window = sem(SemanticColor::WindowBackground, if dark { [0.2, 0.2, 0.2, 1.0] } else { [0.93, 0.93, 0.93, 1.0] });
        Self {
            label: sem(SemanticColor::Label, if dark { [1.0; 4] } else { [0.0, 0.0, 0.0, 0.85] }),
            secondary_label: sem(SemanticColor::SecondaryLabel, [0.5, 0.5, 0.5, 0.6]),
            tertiary_label: sem(SemanticColor::TertiaryLabel, [0.5, 0.5, 0.5, 0.4]),
            selection: sem(SemanticColor::QuaternaryLabel, [0.5, 0.5, 0.5, 0.16]),
            accent,
            separator: sem(SemanticColor::Grid, [0.5, 0.5, 0.5, 0.3]),
            rim: sem(SemanticColor::WindowRim, [1.0, 1.0, 1.0, 0.35]),
            fallback_background: [window[0], window[1], window[2], 0.94],
            glyph_fill: sem(SemanticColor::SystemFill, [0.5, 0.5, 0.5, 0.2]),
        }
    }
}

/// Panel metrics in points (provenance per field in [`PanelStyle::from_theme`]).
#[derive(Clone, Debug)]
pub struct PanelStyle {
    pub appearance: Appearance,
    pub width: f64,
    pub corner_radius: f64,
    pub corner_style: CornerStyle,
    pub input_height: f64,
    pub input_font_size: f64,
    pub input_icon_size: f64,
    pub side_padding: f64,
    pub list_padding: f64,
    pub header_height: f64,
    pub header_font_size: f64,
    pub row_height: f64,
    pub calculator_row_height: f64,
    pub row_font_size: f64,
    pub row_subtitle_size: f64,
    pub calculator_font_size: f64,
    pub icon_size: f64,
    pub selection_inset: f64,
    pub selection_radius: f64,
    pub max_rows: usize,
    pub placeholder: String,
    /// Distance of the panel's top edge from the top of the output, as a fraction of its height.
    pub top_fraction: f64,
    pub rim_width: f64,
    pub shadow_option: ShadowOption,
    pub palette: PanelPalette,
    pub material_name: String,
    pub material: Option<Recipe>,
    pub typography: Typography,
}

impl PanelStyle {
    /// Material of the panel per appearance: the design panel's private
    /// "launcher background" material resolves to the inline-sidebar recipe in the light catalog and
    /// the sidebar recipe in the dark catalog.
    pub fn material_for(appearance: Appearance) -> &'static str {
        if appearance.is_dark() {
            "Sidebar"
        } else {
            "InlineSidebar"
        }
    }
    pub const MATERIAL_FALLBACK: &'static str = "Menu";

    /// Collapsed panel height = the input row.
    pub const COLLAPSED_HEIGHT: f64 = 52.0;
    /// Smallest height of the panel while results are shown.
    pub const MIN_EXPANDED_HEIGHT: f64 = 430.0;

    /// Values labelled come from the analysis; the rest is approximate.
    pub fn from_theme(theme: &Theme, appearance: Appearance, reduce_transparency: bool) -> Self {
        let opts = MaterialOptions { reduce_transparency, ..Default::default() };
        let primary = Self::material_for(appearance);
        let (name, material) = match theme.recipe(appearance, primary, &opts) {
            Ok(r) => (primary, Some(r)),
            Err(_) => (Self::MATERIAL_FALLBACK, theme.recipe(appearance, Self::MATERIAL_FALLBACK, &opts).ok()),
        };
        let typ = theme.tokens.typography().clone();
        Self {
            appearance,
            width: 600.0,
            corner_radius: 16.0,                           //   (continuous corners)
            corner_style: CornerStyle::continuous(),
            input_height: Self::COLLAPSED_HEIGHT,
            input_font_size: 26.0,                         // large-title text style (26 pt: documented size)
            input_icon_size: 24.0,                         // approximate (symbol uses the field font)
            side_padding: 20.0,                            //   (560 pt field in a 600 pt panel)
            list_padding: 8.0,                             // approximate
            header_height: 32.0,                           // derived from header paddings 12 + 6 + one caption line
            header_font_size: typ.sizes.small,
            row_height: 40.0,                              // approximate (unknown in)
            calculator_row_height: 64.0,                   // approximate
            row_font_size: 13.0,                           // body text style (note)
            row_subtitle_size: typ.sizes.small,
            calculator_font_size: 26.0,                    // approximate
            icon_size: 32.0,                               //   (thumbnails up to 32 x 48)
            selection_inset: 8.0,                          // approximate; 8 + 12 (cell inset) = 20 = side padding
            selection_radius: 12.0,                        //   (continuous)
            max_rows: 9,
            placeholder: "Search".into(),
            top_fraction: 0.2,                             //   top edge at one fifth of the output height
            rim_width: theme.shapes.window_rim_width_px,
            shadow_option: ShadowOption::Standard,
            palette: PanelPalette::from_theme(theme, appearance),
            material_name: name.into(),
            material,
            typography: typ,
        }
    }

    fn px(&self, size: f64) -> f32 {
        self.typography.substitute_size(size) as f32
    }

    fn tracking(&self, size: f64) -> f32 {
        self.typography.letter_spacing(size) as f32
    }

    /// Baseline offset that centres a line of `size` in `height`.
    fn centered_baseline(&self, size: f64, height: f64) -> f64 {
        let m = &self.typography.metrics;
        let px = self.typography.substitute_size(size);
        let (asc, desc) = (m.ascender / m.units_per_em * px, m.descender.abs() / m.units_per_em * px);
        (height - (asc + desc)) / 2.0 + asc
    }

    /// Shadow of the panel (the standard window row).
    fn shadow_params(&self) -> ShadowParams {
        let hc = matches!(self.appearance, Appearance::LightHighContrast | Appearance::DarkHighContrast);
        ShadowParams::lookup(self.shadow_option, true, self.appearance.is_dark(), hc)
    }

    /// Tallest layout the panel takes: the input row and a full window of the tallest rows,
    /// never below the expanded minimum.
    fn max_height(&self) -> f64 {
        let tallest = self.row_height.max(self.calculator_row_height).max(self.header_height);
        (self.input_height + 1.0 + 2.0 * self.list_padding + self.max_rows as f64 * tallest).max(Self::MIN_EXPANDED_HEIGHT)
    }

    /// Surface for the panel on an output of `output` points. It is as large as the panel can get
    /// plus the shadow margins, so typing never resizes it. The margins are those of scale 1 in
    /// whole points; at a higher scale they are never larger, so the shadow always fits.
    pub fn surface_geometry(&self, output: Size) -> SurfaceGeometry {
        let m = self.shadow_params().margins_px(1.0);
        let size = (self.width.ceil() as u32 + m.left + m.right, self.max_height().ceil() as u32 + m.top + m.bottom);
        let top_margin = self.placement(output, 0.0).y() as i32 - m.top as i32;
        SurfaceGeometry { size, top_margin, origin: Point::new(f64::from(m.left), f64::from(m.top)) }
    }

    /// Panel rectangle for an output of `output` points: horizontally centred, top edge at
    /// `top_fraction` of the height.
    pub fn placement(&self, output: Size, panel_height: f64) -> Rect {
        Rect::new(((output.width - self.width) / 2.0).round(), (output.height * self.top_fraction).round(), self.width, panel_height)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RowKind {
    Header(String),
    Item { flat: usize },
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowLayout {
    pub kind: RowKind,
    /// Relative to the panel's top-left corner.
    pub rect: Rect,
    pub calculator: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PanelLayout {
    pub size: Size,
    pub input: Rect,
    /// The visible window of rows.
    pub rows: Vec<RowLayout>,
    pub total_rows: usize,
    pub scroll_row: usize,
}

impl PanelLayout {
    /// Flat result index under `p` (panel coordinates), for hover and click.
    pub fn item_at(&self, p: Point) -> Option<usize> {
        self.rows.iter().find_map(|r| match r.kind {
            RowKind::Item { flat } if r.rect.contains(p) => Some(flat),
            _ => None,
        })
    }
}

/// Semantic name of the generic application icon.
const GENERIC_APPLICATION_ICON: &str = "application.generic";

/// Source of application icon pixmaps for an icon name or path.
pub trait IconSource {
    fn load(&mut self, name: &str, size_px: u32) -> Option<Pixmap>;
}

impl IconSource for cupertino_icons::ThemeIcons {
    fn load(&mut self, name: &str, size_px: u32) -> Option<Pixmap> {
        cupertino_icons::ThemeIcons::load(self, name, size_px)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layers {
    pub shadow: bool,
    pub background: bool,
    pub content: bool,
}

impl Layers {
    pub const ALL: Layers = Layers { shadow: true, background: true, content: true };
    /// Input, rows and rim only, for a compositor that draws shadow and material itself.
    pub const CONTENT_ONLY: Layers = Layers { shadow: false, background: false, content: true };
}

/// The wallpaper (or whatever is below) around the panel, in device pixels, for the CPU material.
pub struct Backdrop<'a> {
    pub image: &'a Pixmap,
    pub panel_origin: (u32, u32),
}

pub struct RenderOptions<'a> {
    pub scale: f32,
    pub caret_visible: bool,
    pub backdrop: Option<Backdrop<'a>>,
    pub layers: Layers,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self { scale: 1.0, caret_visible: true, backdrop: None, layers: Layers::ALL }
    }
}

pub struct PanelImage {
    pub pixmap: Pixmap,
    /// Device-pixel distance from the image edges to the panel rectangle (shadow margins).
    pub margins: [u32; 4],
}

impl PanelImage {
    pub fn panel_origin(&self) -> (u32, u32) {
        (self.margins[0], self.margins[1])
    }
}

pub struct PanelRenderer {
    pub style: PanelStyle,
    text: TextRenderer,
    shadows: ShadowRasterizer,
    /// Application icons (an entry's own `Icon=`).
    pub icons: Box<dyn IconSource>,
    system: SystemIcons,
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint { anti_alias: true, ..Paint::default() };
    p.set_color(tiny_skia::Color::from_rgba(c[0].clamp(0.0, 1.0), c[1].clamp(0.0, 1.0), c[2].clamp(0.0, 1.0), c[3].clamp(0.0, 1.0)).unwrap_or(tiny_skia::Color::TRANSPARENT));
    p
}

fn fill_rounded(pm: &mut Pixmap, rect: Rect, radius: f64, style: CornerStyle, color: Rgba, scale: f64) {
    if let Some(path) = rounded_path(rect, Radii::uniform(radius), style, scale) {
        pm.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
    }
}

fn line(pm: &mut Pixmap, pts: &[(f32, f32)], width: f32, color: Rgba) {
    let mut pb = tiny_skia::PathBuilder::new();
    for (i, (x, y)) in pts.iter().enumerate() {
        if i == 0 {
            pb.move_to(*x, *y);
        } else {
            pb.line_to(*x, *y);
        }
    }
    if let Some(p) = pb.finish() {
        let stroke = Stroke { width, line_cap: tiny_skia::LineCap::Round, line_join: tiny_skia::LineJoin::Round, ..Stroke::default() };
        pm.stroke_path(&p, &paint(color), &stroke, Transform::identity(), None);
    }
}

fn circle_stroke(pm: &mut Pixmap, cx: f32, cy: f32, r: f32, width: f32, color: Rgba) {
    if let Some(p) = tiny_skia::PathBuilder::from_circle(cx, cy, r) {
        let stroke = Stroke { width, ..Stroke::default() };
        pm.stroke_path(&p, &paint(color), &stroke, Transform::identity(), None);
    }
}

impl PanelRenderer {
    /// Fails only when no usable font is left ([`TextRenderer::new`]). The icons the panel draws (the generic
    /// application, folder and document icons and the search symbol) are reported when no asset pack provides them, and
    /// drawn as placeholders.
    pub fn new(style: PanelStyle, assets: &Assets) -> Result<Self, AssetError> {
        let system = SystemIcons::required(assets, &[GENERIC_APPLICATION_ICON, "folder", "document.generic"], &["symbol.magnifyingglass"]);
        Ok(Self { style, text: TextRenderer::new(assets, FontRole::Ui)?, shadows: ShadowRasterizer::new(), icons: Box::new(cupertino_icons::ThemeIcons::new(assets)), system })
    }

    pub fn font_family(&self) -> Option<&str> {
        self.text.family()
    }

    /// Width in points. The optical size is that of the point size (the scale-1 request), which is what `draw_text`
    /// selects at any output scale, so widths measured here fit what is drawn.
    fn measure(&mut self, text: &str, size: f64, weight: u16) -> f64 {
        self.text.set_scale(1.0);
        let (px, ls) = (self.style.px(size), self.style.tracking(size));
        self.text.measure(text, px, weight, ls) as f64
    }

    fn fit(&mut self, text: &str, size: f64, weight: u16, max: f64) -> (String, f64) {
        self.text.set_scale(1.0);
        let (px, ls) = (self.style.px(size), self.style.tracking(size));
        let (s, w) = self.text.fit(text, px, weight, ls, max as f32);
        (s, w as f64)
    }

    /// Geometry of the panel for the current state of `panel` (honours `style.max_rows`).
    pub fn layout(&self, panel: &Panel) -> PanelLayout {
        let s = &self.style;
        let results = panel.results();
        let total_rows = panel.total_rows();
        let input = Rect::new(0.0, 0.0, s.width, s.input_height);
        let mut rows = Vec::new();
        let mut y = s.input_height;
        if total_rows > 0 {
            y += 1.0 + s.list_padding;
            let scroll = panel.scroll_row().min(total_rows.saturating_sub(1));
            let mut row_index = 0usize;
            let mut flat = 0usize;
            for section in &results.sections {
                if row_index >= scroll && rows.len() < s.max_rows {
                    rows.push(RowLayout { kind: RowKind::Header(section.title.clone()), rect: Rect::new(0.0, y, s.width, s.header_height), calculator: false });
                    y += s.header_height;
                }
                row_index += 1;
                for item in &section.items {
                    if row_index >= scroll && rows.len() < s.max_rows {
                        let calc = item.kind == ItemKind::Calculator;
                        let h = if calc { s.calculator_row_height } else { s.row_height };
                        rows.push(RowLayout { kind: RowKind::Item { flat }, rect: Rect::new(0.0, y, s.width, h), calculator: calc });
                        y += h;
                    }
                    row_index += 1;
                    flat += 1;
                }
            }
            y += s.list_padding;
            y = y.max(PanelStyle::MIN_EXPANDED_HEIGHT);
        }
        PanelLayout { size: Size::new(s.width, y), input, rows, total_rows, scroll_row: panel.scroll_row() }
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(&mut self, pm: &mut Pixmap, text: &str, x: f64, baseline: f64, size: f64, weight: u16, color: Rgba, scale: f32) -> f64 {
        let px = self.style.px(size) * scale;
        let ls = self.style.tracking(size) * scale;
        self.text.set_scale(scale);
        self.text.draw(pm, text, x as f32 * scale, baseline as f32 * scale, px, weight, ls, color);
        self.measure(text, size, weight)
    }

    /// Longest tail of a path (whole components) that fits, with a leading `…/`.
    fn fit_path(&mut self, path: &str, size: f64, max: f64) -> (String, f64) {
        let full = self.measure(path, size, 400);
        if full <= max {
            return (path.to_string(), full);
        }
        let parts: Vec<&str> = path.split('/').collect();
        for skip in 1..parts.len() {
            let cand = format!("…/{}", parts[skip..].join("/"));
            let w = self.measure(&cand, size, 400);
            if w <= max {
                return (cand, w);
            }
        }
        self.fit(path, size, 400, max)
    }

    /// Draw `text` with the characters at `highlights` in bold. Returns the width in points.
    #[allow(clippy::too_many_arguments)]
    fn draw_highlighted(&mut self, pm: &mut Pixmap, text: &str, highlights: &[usize], x: f64, baseline: f64, size: f64, color: Rgba, scale: f32) -> f64 {
        let chars: Vec<char> = text.chars().collect();
        let mut cx = x;
        let mut i = 0;
        while i < chars.len() {
            let bold = highlights.binary_search(&i).is_ok();
            let mut j = i;
            while j < chars.len() && highlights.binary_search(&j).is_ok() == bold {
                j += 1;
            }
            let seg: String = chars[i..j].iter().collect();
            let weight = if bold { 700 } else { 400 };
            cx += self.draw_text(pm, &seg, cx, baseline, size, weight, color, scale);
            i = j;
        }
        cx - x
    }

    /// The icon of a folder, file or web-search result from the asset directory (`folder`, `document.generic`, and
    /// the search symbol in the secondary label colour), fitted to `r` (a placeholder when no asset pack provides it).
    /// File results are not typed yet (the launcher holds no MIME database).
    fn draw_system_icon(&mut self, pm: &mut Pixmap, item: &ResultItem, r: Rect, scale: f32) {
        let side = (r.width() as f32 * scale).round().max(1.0) as u32;
        let (name, tint) = match item.kind {
            ItemKind::Folder => ("folder", None),
            ItemKind::File => ("document.generic", None),
            _ => ("symbol.magnifyingglass", Some(self.style.palette.secondary_label)),
        };
        let icon = match tint {
            Some(color) => self.system.symbol(name, side, color),
            None => self.system.raster(name, side),
        };

        let k = side as f32 / icon.width().max(icon.height()) as f32;
        let rr = r.scale(f64::from(scale));
        let (dx, dy) = ((side as f32 - icon.width() as f32 * k) / 2.0, (side as f32 - icon.height() as f32 * k) / 2.0);
        let pp = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..PixmapPaint::default() };
        pm.draw_pixmap(0, 0, icon.as_ref(), &pp, Transform::from_row(k, 0.0, 0.0, k, rr.x() as f32 + dx, rr.y() as f32 + dy), None);
    }

    fn draw_icon(&mut self, pm: &mut Pixmap, item: &ResultItem, r: Rect, scale: f32) {
        if matches!(item.kind, ItemKind::Folder | ItemKind::File | ItemKind::WebSearch) {
            self.draw_system_icon(pm, item, r, scale);
            return;
        }
        let rr = r.scale(scale as f64);
        let radius = rr.width() * 0.225;
        let Some(path) = rounded_path(rr, Radii::uniform(radius), CornerStyle::continuous(), 1.0) else { return };
        if item.kind == ItemKind::Calculator {
            self.draw_calculator_icon(pm, &path, rr, scale);
            return;
        }
        let size_px = (r.width() as f32 * scale).round().max(1.0) as u32;
        // an application without an icon of its own gets the generic application icon of the asset packs
        let src = match item.icon.as_deref().and_then(|n| self.icons.load(n, size_px)) {
            Some(own) => own,
            None => self.system.raster(GENERIC_APPLICATION_ICON, size_px).clone(),
        };
        let Some(mut mask) = Mask::new(pm.width(), pm.height()) else { return };
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        let (sx, sy) = (rr.width() as f32 / src.width() as f32, rr.height() as f32 / src.height() as f32);
        let ts = Transform::from_row(sx, 0.0, 0.0, sy, rr.x() as f32, rr.y() as f32);
        let pp = PixmapPaint { quality: tiny_skia::FilterQuality::Bicubic, ..PixmapPaint::default() };
        pm.draw_pixmap(0, 0, src.as_ref(), &pp, ts, Some(&mask));
    }

    /// The calculator result's icon: a grey rounded square with an equals sign. Drawn, not from the asset directory: the
    /// contract has no calculator icon (approximate).
    fn draw_calculator_icon(&mut self, pm: &mut Pixmap, path: &tiny_skia::Path, rr: Rect, scale: f32) {
        pm.fill_path(path, &paint([0.35, 0.35, 0.37, 1.0]), FillRule::Winding, Transform::identity(), None);
        let size = rr.width() / f64::from(scale) * 0.5;
        let w = self.measure("=", size, 600) as f32;
        let px = self.style.px(size) * scale;
        let baseline = rr.y() as f32 + rr.height() as f32 / 2.0 + px * 0.36;
        let ls = self.style.tracking(size) * scale;
        self.text.set_scale(scale);
        self.text.draw(pm, "=", rr.x() as f32 + (rr.width() as f32 - w * scale) / 2.0, baseline, px, 600, ls, [1.0, 1.0, 1.0, 0.95]);
    }

    fn background(&self, pm: &mut Pixmap, origin: (u32, u32), size_px: (u32, u32), radius_px: f64, backdrop: Option<&Backdrop>, scale: f32) {
        let rect = Rect::new(origin.0 as f64, origin.1 as f64, size_px.0 as f64, size_px.1 as f64);
        let Some(shape) = rounded_path(rect, Radii::uniform(radius_px), self.style.corner_style, 1.0) else { return };
        #[cfg(feature = "material")]
        if let (Some(b), Some(recipe)) = (backdrop, self.style.material.as_ref()) {
            use cupertino_glass::{cpu::render_regions, MaterialRegion};
            let (w, h) = (b.image.width(), b.image.height());
            let mut wall = image::RgbaImage::new(w, h);
            for (dst, src) in wall.pixels_mut().zip(b.image.pixels()) {
                let c = src.demultiply();
                *dst = image::Rgba([c.red(), c.green(), c.blue(), 255]);
            }
            let region = MaterialRegion::new([b.panel_origin.0 as f32, b.panel_origin.1 as f32, size_px.0 as f32, size_px.1 as f32], recipe);
            let out = render_regions(&wall, &[region], scale, [0.5, 0.5, 0.5], false, false);
            let Some(mut mask) = Mask::new(pm.width(), pm.height()) else { return };
            mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
            let Some(mut layer) = Pixmap::new(pm.width(), pm.height()) else { return };
            let dx = origin.0 as i64 - b.panel_origin.0 as i64;
            let dy = origin.1 as i64 - b.panel_origin.1 as i64;
            for y in origin.1..(origin.1 + size_px.1).min(pm.height()) {
                for x in origin.0..(origin.0 + size_px.0).min(pm.width()) {
                    let (sx, sy) = ((x as i64 - dx) as u32, (y as i64 - dy) as u32);
                    if sx >= w || sy >= h {
                        continue;
                    }
                    let p = out.get_pixel(sx, sy).0;
                    layer.pixels_mut()[(y * pm.width() + x) as usize] = tiny_skia::ColorU8::from_rgba(p[0], p[1], p[2], 255).premultiply();
                }
            }
            pm.draw_pixmap(0, 0, layer.as_ref(), &PixmapPaint::default(), Transform::identity(), Some(&mask));
            return;
        }
        let _ = (backdrop, scale);
        pm.fill_path(&shape, &paint(self.style.palette.fallback_background), FillRule::Winding, Transform::identity(), None);
    }

    /// Rasterise the panel at `opts.scale` device pixels per point.
    pub fn render(&mut self, panel: &Panel, layout: &PanelLayout, opts: &RenderOptions) -> PanelImage {
        let s = self.style.clone();
        let scale = opts.scale;
        let sc = scale as f64;
        let params = s.shadow_params();
        let m = params.margins_px(sc);
        let margins = [m.left, m.top, m.right, m.bottom];
        let pw = (layout.size.width * sc).round() as u32;
        let ph = (layout.size.height * sc).round() as u32;
        let (w, h) = (pw + m.left + m.right, ph + m.top + m.bottom);
        let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("non-empty panel");
        let origin = (m.left, m.top);
        let radius_px = s.corner_radius * sc;

        if opts.layers.shadow {
            let img = self.shadows.render(&params, layout.size, s.corner_radius, s.corner_style, sc);
            for (dst, a) in pm.pixels_mut().iter_mut().zip(img.data.chunks_exact(4)) {
                *dst = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, a[3]).unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
            }
            if let Some(path) = rounded_path(Rect::new(origin.0 as f64, origin.1 as f64, pw as f64, ph as f64), Radii::uniform(radius_px), s.corner_style, 1.0) {
                let mut p = Paint { blend_mode: tiny_skia::BlendMode::Clear, anti_alias: true, ..Paint::default() };
                p.set_color(tiny_skia::Color::BLACK);
                pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
            }
        }
        if opts.layers.background {
            self.background(&mut pm, origin, (pw, ph), radius_px, opts.backdrop.as_ref(), scale);
        }
        if !opts.layers.content {
            return PanelImage { pixmap: pm, margins };
        }

        let (ox, oy) = (origin.0 as f64 / sc, origin.1 as f64 / sc);
        let stroke = (1.7 * scale).max(1.0);

        let mid_y = oy + layout.input.height() / 2.0;
        let ic = s.input_icon_size;
        let (cx, cy) = (((ox + s.side_padding + ic * 0.42) * sc) as f32, ((mid_y - ic * 0.06) * sc) as f32);
        let r = (ic * 0.32 * sc) as f32;
        circle_stroke(&mut pm, cx, cy, r, stroke, s.palette.secondary_label);
        let d = r * 0.72;
        line(&mut pm, &[(cx + d, cy + d), (cx + r * 1.65, cy + r * 1.65)], stroke, s.palette.secondary_label);

        let text_x = ox + s.side_padding + ic + 12.0;
        let base = oy + s.centered_baseline(s.input_font_size, layout.input.height());
        let query = panel.query();
        if query.is_empty() {
            self.draw_text(&mut pm, &s.placeholder, text_x, base, s.input_font_size, 400, s.palette.tertiary_label, scale);
            if opts.caret_visible {
                self.caret(&mut pm, text_x, base, scale);
            }
        } else {
            self.draw_text(&mut pm, query, text_x, base, s.input_font_size, 400, s.palette.label, scale);
            if opts.caret_visible {
                let before: String = query.chars().take(panel.cursor()).collect();
                let off = self.measure(&before, s.input_font_size, 400);
                self.caret(&mut pm, text_x + off, base, scale);
            }
        }

        if layout.total_rows > 0 {
            let sep_y = ((oy + layout.input.height()) * sc) as f32;
            line(&mut pm, &[((ox * sc) as f32, sep_y + 0.5), (((ox + layout.size.width) * sc) as f32, sep_y + 0.5)], 1.0, s.palette.separator);
        }

        for row in &layout.rows {
            let rect = row.rect.translate(ox, oy);
            match &row.kind {
                RowKind::Header(title) => {
                    let base = rect.y() + s.centered_baseline(s.header_font_size, rect.height()) + 2.0;
                    self.draw_text(&mut pm, title, rect.x() + s.side_padding, base, s.header_font_size, 600, s.palette.secondary_label, scale);
                }
                RowKind::Item { flat } => {
                    let Some(item) = panel.results().get(*flat) else { continue };
                    let selected = *flat == panel.selected();
                    if selected {
                        let sel = Rect::new(rect.x() + s.selection_inset, rect.y(), rect.width() - 2.0 * s.selection_inset, rect.height());
                        fill_rounded(&mut pm, sel, s.selection_radius, CornerStyle::continuous(), s.palette.selection, sc);
                    }
                    let (fg, fg2) = (s.palette.label, s.palette.secondary_label);
                    let icon_size = if row.calculator { s.icon_size + 8.0 } else { s.icon_size };
                    let icon = Rect::new(rect.x() + s.side_padding, rect.y() + (rect.height() - icon_size) / 2.0, icon_size, icon_size);
                    self.draw_icon(&mut pm, item, icon, scale);
                    let tx = icon.max_x() + 12.0;
                    let right = rect.max_x() - s.side_padding;
                    if row.calculator {
                        let size = s.calculator_font_size;
                        let (title, _) = self.fit(&item.title, size, 400, right - tx);
                        let base = rect.y() + s.centered_baseline(size, rect.height() * 0.62);
                        self.draw_text(&mut pm, &title, tx, base, size, 400, fg, scale);
                        let (sub, _) = self.fit(&item.subtitle, s.row_subtitle_size, 400, right - tx);
                        self.draw_text(&mut pm, &sub, tx, rect.y() + rect.height() - 12.0, s.row_subtitle_size, 400, fg2, scale);
                    } else {
                        let base = rect.y() + s.centered_baseline(s.row_font_size, rect.height());
                        let sub_budget = (right - tx) * 0.42;
                        let is_path = matches!(item.kind, ItemKind::File | ItemKind::Folder);
                        let sub_w = if item.subtitle.is_empty() {
                            0.0
                        } else if is_path {
                            self.fit_path(&item.subtitle, s.row_subtitle_size, sub_budget).1
                        } else {
                            self.fit(&item.subtitle, s.row_subtitle_size, 400, sub_budget).1
                        };
                        let title_budget = right - tx - if sub_w > 0.0 { sub_w + 14.0 } else { 0.0 };
                        let (title, _) = self.fit(&item.title, s.row_font_size, 700, title_budget);
                        let highlights: &[usize] = if title.ends_with('…') && title != item.title { &[] } else { &item.highlights };
                        self.draw_highlighted(&mut pm, &title, highlights, tx, base, s.row_font_size, fg, scale);
                        if sub_w > 0.0 {
                            let (sub, w) = if is_path { self.fit_path(&item.subtitle, s.row_subtitle_size, sub_budget) } else { self.fit(&item.subtitle, s.row_subtitle_size, 400, sub_budget) };
                            let base = rect.y() + s.centered_baseline(s.row_subtitle_size, rect.height());
                            self.draw_text(&mut pm, &sub, right - w, base, s.row_subtitle_size, 400, fg2, scale);
                        }
                    }
                }
            }
        }

        if layout.total_rows > s.max_rows && !layout.rows.is_empty() {
            let top = layout.rows.first().map_or(0.0, |r| r.rect.y()) + oy;
            let bottom = layout.rows.last().map_or(0.0, |r| r.rect.max_y()) + oy;
            let track = bottom - top;
            let thumb = (track * s.max_rows as f64 / layout.total_rows as f64).max(16.0);
            let y = top + (track - thumb) * layout.scroll_row as f64 / (layout.total_rows - s.max_rows) as f64;
            fill_rounded(&mut pm, Rect::new(ox + layout.size.width - 6.0, y, 3.0, thumb), 1.5, CornerStyle::Circular, s.palette.tertiary_label, sc);
        }

        if s.rim_width > 0.0 {
            let half = 0.5;
            let rect = Rect::new(origin.0 as f64 + half, origin.1 as f64 + half, pw as f64 - 2.0 * half, ph as f64 - 2.0 * half);
            if let Some(path) = rounded_path(rect, Radii::uniform(radius_px - half), s.corner_style, 1.0) {
                let st = Stroke { width: (s.rim_width as f32 * scale.max(1.0)).max(1.0), ..Stroke::default() };
                pm.stroke_path(&path, &paint(s.palette.rim), &st, Transform::identity(), None);
            }
        }
        PanelImage { pixmap: pm, margins }
    }

    fn caret(&self, pm: &mut Pixmap, x: f64, baseline: f64, scale: f32) {
        let s = &self.style;
        let px = s.typography.substitute_size(s.input_font_size);
        let (top, bottom) = (baseline - px * 0.85, baseline + px * 0.22);
        let xx = (x * scale as f64 + 0.5) as f32;
        line(pm, &[(xx, (top * scale as f64) as f32), (xx, (bottom * scale as f64) as f32)], (1.4 * scale).max(1.0), s.palette.accent);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::tests_support::test_launcher;
    use crate::panel::{Key, Mods, Now};

    /// Needs the asset directory (`CUPERTINO_ASSET_DIR`); fails with the build instructions when it is missing.
    fn renderer() -> PanelRenderer {
        let theme = Theme::load().unwrap();
        let assets = Assets::discover();
        PanelRenderer::new(PanelStyle::from_theme(&theme, Appearance::Light, false), &assets).unwrap_or_else(|e| panic!("{e}"))
    }

    fn panel_with(query: &str) -> (Panel, crate::engine::Launcher) {
        let mut l = test_launcher();
        let mut p = Panel::new();
        let now = Now { mono: 0.0, unix: 1.0e6 };
        p.show(&l, now);
        for c in query.chars() {
            p.handle_key(Key::Char(c), Mods::NONE, &mut l, now);
        }
        (p, l)
    }

    #[test]
    fn style_uses_tokens_and_a_recipe() {
        let theme = Theme::load().unwrap();
        let s = PanelStyle::from_theme(&theme, Appearance::Dark, false);
        assert_eq!(s.palette.label, theme.tokens.semantic(Appearance::Dark, SemanticColor::Label).unwrap().to_array());
        assert_eq!(s.palette.selection, theme.tokens.semantic(Appearance::Dark, SemanticColor::QuaternaryLabel).unwrap().to_array());
        assert!(s.material.is_some(), "recipe {}", s.material_name);
        assert_eq!(s.placeholder, "Search");
    }

    #[test]
    fn layout_grows_with_results_and_respects_max_rows() {
        let r = renderer();
        let (empty, _) = panel_with("");
        let l0 = r.layout(&empty);
        assert_eq!(l0.size.height, r.style.input_height);
        assert!(l0.rows.is_empty());
        let (p, _) = panel_with("e");
        let l1 = r.layout(&p);
        assert!(l1.size.height > l0.size.height + 100.0);
        assert!(l1.rows.len() <= r.style.max_rows);
        assert!(matches!(l1.rows[0].kind, RowKind::Header(_)));
        let bottom = l1.rows.last().unwrap().rect.max_y();
        let content = bottom + r.style.list_padding;
        assert!(l1.size.height >= content);
        assert!((l1.size.height - content).abs() < 1e-9 || l1.size.height == PanelStyle::MIN_EXPANDED_HEIGHT);
        let item_rows: Vec<usize> = l1.rows.iter().filter_map(|r| if let RowKind::Item { flat } = r.kind { Some(flat) } else { None }).collect();
        assert_eq!(item_rows, (0..item_rows.len()).collect::<Vec<_>>());
    }

    #[test]
    fn folders_files_searches_and_applications_without_icons_are_drawn_from_the_assets() {
        let mut r = renderer();
        for kind in [ItemKind::Folder, ItemKind::File, ItemKind::WebSearch, ItemKind::Application] {
            let item = ResultItem { id: "x".into(), kind, title: "Zed".into(), subtitle: String::new(), icon: None, score: 0.0, highlights: Vec::new(), action: crate::engine::Action::OpenUrl(String::new()) };
            let mut pm = Pixmap::new(64, 64).unwrap();
            r.draw_icon(&mut pm, &item, Rect::new(0.0, 0.0, 32.0, 32.0), 2.0);
            assert!(pm.pixels().iter().filter(|p| p.alpha() > 0).count() > 100, "{kind:?} draws an icon");
        }
        let mut folder = Pixmap::new(64, 64).unwrap();
        let item = ResultItem { id: "x".into(), kind: ItemKind::Folder, title: "Zed".into(), subtitle: String::new(), icon: None, score: 0.0, highlights: Vec::new(), action: crate::engine::Action::OpenUrl(String::new()) };
        r.draw_icon(&mut folder, &item, Rect::new(0.0, 0.0, 32.0, 32.0), 2.0);
        let letter = folder.pixels().iter().any(|p| p.alpha() == 255 && p.red() == p.green() && p.green() == p.blue() && p.red() > 240);
        assert!(!letter, "no placeholder letter on the folder");
    }

    #[test]
    fn calculator_row_is_taller() {
        let r = renderer();
        let (p, _) = panel_with("3*7");
        let l = r.layout(&p);
        let calc = l.rows.iter().find(|r| r.calculator).expect("calculator row");
        assert_eq!(calc.rect.height(), r.style.calculator_row_height);
    }

    #[test]
    fn item_at_maps_points_to_flat_indices() {
        let r = renderer();
        let (p, _) = panel_with("e");
        let l = r.layout(&p);
        let row = l.rows.iter().find(|r| matches!(r.kind, RowKind::Item { flat: 1 })).unwrap();
        assert_eq!(l.item_at(row.rect.center()), Some(1));
        let header = l.rows.iter().find(|r| matches!(r.kind, RowKind::Header(_))).unwrap();
        assert_eq!(l.item_at(header.rect.center()), None);
        assert_eq!(l.item_at(Point::new(-5.0, -5.0)), None);
    }

    #[test]
    fn scrolled_layout_starts_at_the_scroll_row() {
        let many: Vec<_> = (0..40).map(|i| crate::engine::tests_support::app(&format!("t{i}"), &format!("Tool {i:02}"), "", "t")).collect();
        let cfg = crate::engine::LauncherConfig { max_apps: 30, ..Default::default() };
        let mut l = crate::engine::Launcher::new(cfg, many, crate::frecency::Frecency::in_memory());
        let r = renderer();
        let mut p = Panel::new();
        p.max_rows = r.style.max_rows;
        let now = Now { mono: 0.0, unix: 1.0e6 };
        p.show(&l, now);
        for c in "tool".chars() {
            p.handle_key(Key::Char(c), Mods::NONE, &mut l, now);
        }
        for _ in 0..15 {
            p.handle_key(Key::Down, Mods::NONE, &mut l, now);
        }
        let layout = r.layout(&p);
        assert!(layout.scroll_row > 0);
        assert!(layout.rows.iter().any(|r| matches!(r.kind, RowKind::Item { flat: 15 })), "selected item is in the window");
        assert_eq!(layout.rows.len(), r.style.max_rows);
    }

    #[test]
    fn placement_centres_horizontally() {
        let theme = Theme::load().unwrap();
        let s = PanelStyle::from_theme(&theme, Appearance::Light, false);
        let r = s.placement(Size::new(1920.0, 1080.0), 300.0);
        assert_eq!(r.x(), 660.0);
        assert_eq!(r.y(), 216.0);
        assert_eq!((r.width(), r.height()), (600.0, 300.0));
    }

    #[test]
    fn surface_holds_the_tallest_panel_with_its_shadow() {
        let theme = Theme::load().unwrap();
        let s = PanelStyle::from_theme(&theme, Appearance::Light, false);
        // 1280x800 points
        assert_eq!(
            s.surface_geometry(Size::new(1280.0, 800.0)),
            SurfaceGeometry { size: (712, 757), top_margin: 122, origin: Point::new(56.0, 38.0) }
        );
        // 3840x2160 pixels at scale 2 are 1920x1080 points
        assert_eq!(
            s.surface_geometry(Size::new(1920.0, 1080.0)),
            SurfaceGeometry { size: (712, 757), top_margin: 178, origin: Point::new(56.0, 38.0) }
        );
    }

    #[test]
    fn render_draws_text_selection_and_shadow() {
        let mut r = renderer();
        let (p, _) = panel_with("fire");
        let l = r.layout(&p);
        let img = r.render(&p, &l, &RenderOptions { scale: 2.0, ..Default::default() });
        let (ox, oy) = img.panel_origin();
        let px = |x: u32, y: u32| img.pixmap.pixels()[(y * img.pixmap.width() + x) as usize];
        assert_eq!(px(0, 0).alpha(), 0);
        assert!(px(ox + 600, oy + 20).alpha() > 200, "background");
        let sel = l.rows.iter().find(|r| matches!(r.kind, RowKind::Item { flat: 0 })).unwrap();
        let c = px(ox + 400, oy + (sel.rect.center().y * 2.0) as u32);
        assert!(c.blue().abs_diff(c.red()) < 16, "selected row is a neutral wash, not the accent: {c:?}");
        let below = px(ox + 600, oy + (l.size.height * 2.0) as u32 + 6);
        assert!(below.alpha() > 0, "shadow below the panel");
        let dark = img.pixmap.pixels().iter().filter(|p| p.alpha() > 0 && p.red() < 80 && p.green() < 80 && p.blue() < 80).count();
        assert!(dark > 100, "query text pixels: {dark}");
    }

    #[test]
    fn caret_follows_the_cursor() {
        let mut r = renderer();
        let (mut p, mut l) = panel_with("abc");
        let layout = r.layout(&p);
        let a = r.render(&p, &layout, &RenderOptions { caret_visible: true, ..Default::default() });
        let b = r.render(&p, &layout, &RenderOptions { caret_visible: false, ..Default::default() });
        assert_ne!(a.pixmap.data(), b.pixmap.data(), "caret drawn");
        let now = Now { mono: 0.0, unix: 1.0e6 };
        p.handle_key(Key::Home, Mods::NONE, &mut l, now);
        let layout = r.layout(&p);
        let c = r.render(&p, &layout, &RenderOptions { caret_visible: true, ..Default::default() });
        assert_ne!(a.pixmap.data(), c.pixmap.data());
    }

    #[test]
    fn content_only_layer_has_no_shadow_or_background() {
        let mut r = renderer();
        let (p, _) = panel_with("zz");
        let l = r.layout(&p);
        let img = r.render(&p, &l, &RenderOptions { layers: Layers::CONTENT_ONLY, ..Default::default() });
        assert_eq!(img.pixmap.pixels()[(img.pixmap.width() * (img.margins[1] + 30) + img.margins[0] + 600) as usize].alpha(), 0);
        assert!(img.pixmap.pixels().iter().any(|p| p.alpha() > 0));
    }

    #[test]
    fn long_paths_drop_leading_components() {
        let mut r = renderer();
        let (t, w) = r.fit_path("/home/user/Documents/Projects/very/deep/folder", 12.0, 150.0);
        assert!(t.starts_with("…/") && t.ends_with("folder"), "{t}");
        assert!(w <= 150.0);
        let (t, _) = r.fit_path("~/Documents", 12.0, 400.0);
        assert_eq!(t, "~/Documents");
    }

    #[test]
    fn highlight_segments_cover_the_whole_title() {
        let mut r = renderer();
        let mut pm = Pixmap::new(300, 40).unwrap();
        let full = r.measure("Visual Studio Code", 14.0, 400);
        let drawn = r.draw_highlighted(&mut pm, "Visual Studio Code", &[0, 7, 14], 4.0, 24.0, 14.0, [0.0, 0.0, 0.0, 1.0], 1.0);
        assert!(drawn >= full * 0.95 && drawn < full * 1.3, "{drawn} vs {full}");
        assert!(pm.pixels().iter().any(|p| p.alpha() > 0));
    }

    #[test]
    fn verified_metrics_of_the_reference_panel() {
        let theme = Theme::load().unwrap();
        let s = PanelStyle::from_theme(&theme, Appearance::Light, false);
        assert_eq!((s.width, s.input_height, s.corner_radius, s.top_fraction), (600.0, 52.0, 16.0, 0.2));
        assert_eq!((s.input_font_size, s.side_padding, s.icon_size, s.selection_radius), (26.0, 20.0, 32.0, 12.0));
        assert_eq!(s.shadow_option, ShadowOption::Standard);
        assert_eq!(PanelStyle::MIN_EXPANDED_HEIGHT, 430.0);
        // The field is 560 pt wide inside the 600 pt panel.
        assert_eq!(s.width - 2.0 * s.side_padding, 560.0);
    }

    #[test]
    fn material_follows_the_appearance() {
        // light -> inline-sidebar recipe, dark -> sidebar recipe.
        let theme = Theme::load().unwrap();
        let light = PanelStyle::from_theme(&theme, Appearance::Light, false);
        let dark = PanelStyle::from_theme(&theme, Appearance::Dark, false);
        assert_eq!((light.material_name.as_str(), dark.material_name.as_str()), ("InlineSidebar", "Sidebar"));
        let (l, d) = (light.material.as_ref().unwrap(), dark.material.as_ref().unwrap());
        assert_eq!((l.material.as_str(), d.material.as_str()), ("InlineSidebar", "Sidebar"));
        let opaque = PanelStyle::from_theme(&theme, Appearance::Light, true);
        assert!(opaque.material.as_ref().unwrap().opaque);
    }

    #[test]
    fn collapsed_height_is_the_input_row_and_results_reach_the_minimum() {
        let r = renderer();
        let (empty, _) = panel_with("");
        assert_eq!(r.layout(&empty).size.height, 52.0);
        let few = crate::engine::tests_support::app("solo", "Solo", "", "solo");
        let l = crate::engine::Launcher::new(Default::default(), vec![few], crate::frecency::Frecency::in_memory());
        let mut p = Panel::new();
        let now = Now { mono: 0.0, unix: 1.0e6 };
        p.show(&l, now);
        let mut l = l;
        for c in "solo".chars() {
            p.handle_key(Key::Char(c), Mods::NONE, &mut l, now);
        }
        assert_eq!(r.layout(&p).size.height, 430.0, "a short result list still fills the minimum expanded height");
    }
}
