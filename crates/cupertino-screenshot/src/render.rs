//! CPU drawing of the overlay, the toolbar's controls, the Options menu and the thumbnail into `tiny-skia` pixmaps at a
//! surface's device scale. The toolbar's and the menu's materials are the server's backdrops; nothing here draws them.
//!
//! The toolbar glyphs are simple vector drawings standing in for the design's symbols (approximate: the symbols
//! are named in, their artwork is not used). Text is 13 pt in the toolbar and the menu (approximate).

use std::fmt;

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::shape::rounded_path;
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::shadow::blurred_rect;
use cupertino_geometry::{CornerStyle, Insets, Point, Radii, Rect};
use cupertino_theme::{AccentColor, Appearance, SemanticColor, Theme, Typography};
use tiny_skia::{
    BlendMode, Color, FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, PathBuilder, Pixmap,
    PixmapPaint, Stroke, StrokeDash, Transform,
};

use crate::selection::{self, Handle};
use crate::session::Style;
use crate::settings::Mode;
use crate::thumbnail::Look;
use crate::toolbar::{self, Item, MenuRow, MENU_FONT_SIZE, MENU_RADIUS, MENU_WIDTH};
use crate::values::{
    BORDER_DASH, BORDER_WIDTH, CROSSHAIR_ARM, CROSSHAIR_CENTRE, CROSSHAIR_CORE, CROSSHAIR_GAP,
    CROSSHAIR_HALO, DIM, HANDLE_FILL, HANDLE_STROKE, LABEL_BACKDROP, LABEL_FONT_SIZE, LABEL_RADIUS,
    LABEL_TEXT, PICK_HIGHLIGHT, SIMPLE_TINT, THUMBNAIL_BORDER, THUMBNAIL_BORDER_COLOR,
    THUMBNAIL_RADIUS, THUMBNAIL_SHADOW_ALPHA, THUMBNAIL_SHADOW_OUTSET, THUMBNAIL_SHADOW_SIGMA,
};

/// Diameter of the round handle drawn in each 16 pt handle square (approximate: the handle path is not known).
const HANDLE_DOT: f64 = 9.0;
/// Text size and weight of the toolbar and menu (approximate).
const TEXT_WEIGHT: u16 = 400;
const HEADER_WEIGHT: u16 = 600;
/// Fill behind the chosen mode button (approximate).
const SELECTED_INSET: f64 = 4.0;
const SELECTED_RADIUS: f64 = 6.0;

/// A colour token missing from the design tokens of an appearance.
#[derive(Debug)]
pub struct MissingColor(pub &'static str);

impl fmt::Display for MissingColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the design tokens have no colour {}", self.0)
    }
}

impl std::error::Error for MissingColor {}

/// Colours of the toolbar and the menu, from the design tokens of the session's appearance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub text: Rgba,
    pub secondary: Rgba,
    pub disabled: Rgba,
    /// Behind the chosen mode button.
    pub selected: Rgba,
    pub separator: Rgba,
    /// The highlighted menu row, with white text on it.
    pub highlight: Rgba,
}

impl Palette {
    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Result<Self, MissingColor> {
        let get = |c: SemanticColor, name: &'static str| {
            theme
                .tokens
                .semantic(appearance, c)
                .map(|c| c.to_array())
                .ok_or(MissingColor(name))
        };
        Ok(Self {
            text: get(SemanticColor::ControlText, "control-text")?,
            secondary: get(SemanticColor::SecondaryLabel, "secondary-label")?,
            disabled: get(SemanticColor::DisabledControlText, "disabled-control-text")?,
            selected: get(SemanticColor::SecondarySystemFill, "secondary-system-fill")?,
            separator: get(SemanticColor::Grid, "grid")?,
            highlight: theme
                .tokens
                .accent(appearance, AccentColor::Blue)
                .map(|c| c.to_array())
                .ok_or(MissingColor("control-accent-blue"))?,
        })
    }
}

fn skia(c: Rgba) -> Color {
    Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        c[3].clamp(0.0, 1.0),
    )
    .expect("clamped channels are colours")
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(skia(c));
    p.anti_alias = true;
    p
}

fn with_alpha(c: Rgba, alpha: f64) -> Rgba {
    [c[0], c[1], c[2], c[3] * alpha as f32]
}

/// `r` in device pixels as a `tiny-skia` rectangle; `None` when it is empty.
fn device(r: &Rect, s: f64) -> Option<tiny_skia::Rect> {
    tiny_skia::Rect::from_xywh(
        (r.x() * s) as f32,
        (r.y() * s) as f32,
        (r.width() * s) as f32,
        (r.height() * s) as f32,
    )
}

/// Fill `r` without anti-aliasing: on whole device pixels, the edges a pixel grid draws. (Anti-aliased thin rectangles
/// also take `tiny-skia`'s hairline path, whose debug assertions fail for some sub-pixel positions.)
fn fill_rect(pm: &mut Pixmap, r: &Rect, s: f64, c: Rgba) {
    fill_rect_blended(pm, r, s, c, BlendMode::SourceOver);
}

/// [`fill_rect`] with a blend mode; [`BlendMode::Clear`] cuts a hole.
fn fill_rect_blended(pm: &mut Pixmap, r: &Rect, s: f64, c: Rgba, blend: BlendMode) {
    let snapped = Rect::from_ltrb(
        (r.min_x() * s).round(),
        (r.min_y() * s).round(),
        (r.max_x() * s).round(),
        (r.max_y() * s).round(),
    );
    if let Some(r) = device(&snapped, 1.0) {
        let mut p = paint(c);
        p.anti_alias = false;
        p.blend_mode = blend;
        pm.fill_rect(r, &p, Transform::identity(), None);
    }
}

fn fill_rounded(pm: &mut Pixmap, r: &Rect, radius: f64, s: f64, c: Rgba) {
    if let Some(path) = rounded_path(*r, Radii::uniform(radius), CornerStyle::Circular, s) {
        pm.fill_path(
            &path,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn circle(center: Point, radius: f64, s: f64) -> Option<tiny_skia::Path> {
    PathBuilder::from_circle(
        (center.x * s) as f32,
        (center.y * s) as f32,
        (radius * s) as f32,
    )
}

fn stroke(width: f64, s: f64) -> Stroke {
    Stroke {
        width: (width * s) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    }
}

/// A polyline through `points` (logical) at scale `s`.
fn polyline(points: &[(f64, f64)], s: f64, close: bool) -> Option<tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    let (first, rest) = points.split_first()?;
    pb.move_to((first.0 * s) as f32, (first.1 * s) as f32);
    for p in rest {
        pb.line_to((p.0 * s) as f32, (p.1 * s) as f32);
    }
    if close {
        pb.close();
    }
    pb.finish()
}

fn stroke_path(pm: &mut Pixmap, path: Option<tiny_skia::Path>, width: f64, s: f64, c: Rgba) {
    if let Some(path) = path {
        pm.stroke_path(
            &path,
            &paint(c),
            &stroke(width, s),
            Transform::identity(),
            None,
        );
    }
}

/// What the overlay of one output shows. Rectangles and points are global; `bounds` is the output.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayView {
    pub bounds: Rect,
    pub style: Style,
    pub mode: Mode,
    pub selection: Option<Rect>,
    /// The size readout is shown while the selection is spanned or resized.
    pub sizing: bool,
    pub picked: Option<Rect>,
    pub pointer: Option<Point>,
    /// Draw the crosshair at the pointer (the system pointer is hidden then).
    pub crosshair: bool,
}

/// What the toolbar shows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ToolbarView {
    pub mode: Mode,
    pub hovered: Option<Item>,
    /// Seconds left on the timer, shown on the Capture button while it runs.
    pub countdown: Option<u32>,
}

/// The renderer: text, palette and the cached thumbnail shadow.
pub struct Painter {
    pub palette: Palette,
    typography: Typography,
    text: TextRenderer,
    shadow: Option<((u32, u32), Pixmap)>,
}

impl Painter {
    pub fn new(
        palette: Palette,
        typography: Typography,
        assets: &Assets,
    ) -> Result<Self, AssetError> {
        let text = TextRenderer::new(assets, FontRole::Ui)?;
        Ok(Self {
            palette,
            typography,
            text,
            shadow: None,
        })
    }

    /// Width of `text` in points at `size`.
    fn measure(&mut self, text: &str, size: f64, weight: u16) -> f64 {
        self.text.set_scale(1.0);
        f64::from(self.text.measure(text, size as f32, weight, 0.0))
    }

    /// Draw `text` with its left edge at `x`, centred vertically in the band `top..top + height` (logical points).
    #[allow(clippy::too_many_arguments)]
    fn text_in_band(
        &mut self,
        pm: &mut Pixmap,
        text: &str,
        x: f64,
        top: f64,
        height: f64,
        size: f64,
        weight: u16,
        s: f64,
        color: Rgba,
    ) {
        let m = &self.typography.metrics;
        let (ascent, descent) = (
            m.ascender / m.units_per_em * size,
            m.descender.abs() / m.units_per_em * size,
        );
        let baseline = top + (height - (ascent + descent)) / 2.0 + ascent;
        self.text.set_scale(s as f32);
        self.text.draw(
            pm,
            text,
            (x * s) as f32,
            (baseline * s) as f32,
            (size * s) as f32,
            weight,
            0.0,
            color,
        );
    }

    // ---- overlay ---------------------------------------------------------------------------------------------------

    /// Draw the overlay of one output into `pm` (the output's size at scale `s`).
    pub fn overlay(&mut self, pm: &mut Pixmap, s: f64, v: &OverlayView) {
        let local = |r: &Rect| r.translate(-v.bounds.x(), -v.bounds.y());
        let full = Rect::new(0.0, 0.0, v.bounds.width(), v.bounds.height());
        let selection = v
            .selection
            .and_then(|r| r.intersection(&v.bounds).map(|_| local(&r)));
        match (v.style, v.mode) {
            (Style::Interactive, Mode::Selection) => {
                fill_rect(pm, &full, s, DIM);
                if let Some(sel) = selection {
                    // the hole: the design's destination-out mask layer
                    fill_rect_blended(pm, &sel, s, [0.0, 0.0, 0.0, 1.0], BlendMode::Clear);
                    self.border(pm, &sel, s);
                    for h in Handle::ALL {
                        let c = h.anchor(&sel);
                        if let Some(path) = circle(c, HANDLE_DOT / 2.0, s) {
                            pm.fill_path(
                                &path,
                                &paint(HANDLE_FILL),
                                FillRule::Winding,
                                Transform::identity(),
                                None,
                            );
                            pm.stroke_path(
                                &path,
                                &paint([1.0, 1.0, 1.0, 1.0]),
                                &stroke(HANDLE_STROKE, s),
                                Transform::identity(),
                                None,
                            );
                        }
                    }
                }
            }
            (Style::Simple, Mode::Selection) => {
                if let Some(sel) = selection {
                    fill_rect(pm, &sel, s, SIMPLE_TINT);
                }
            }
            (_, Mode::Window) => {
                if let Some(r) = v.picked.and_then(|w| w.intersection(&v.bounds)) {
                    fill_rect(pm, &local(&r), s, PICK_HIGHLIGHT);
                }
            }
            (_, Mode::Screen) => {}
        }
        let pointer = v
            .pointer
            .filter(|p| v.bounds.contains(*p))
            .map(|p| Point::new(p.x - v.bounds.x(), p.y - v.bounds.y()));
        if let (true, Some(sel), Some(p)) = (v.sizing, v.selection, pointer) {
            let text = selection::label_text(&sel);
            let width = self.measure(&text, LABEL_FONT_SIZE, TEXT_WEIGHT);
            let r = selection::label_rect(width, p, &full);
            fill_rounded(pm, &r, LABEL_RADIUS, s, LABEL_BACKDROP);
            let x = r.x() + (r.width() - width) / 2.0;
            self.text_in_band(
                pm,
                &text,
                x,
                r.y(),
                r.height(),
                LABEL_FONT_SIZE,
                TEXT_WEIGHT,
                s,
                LABEL_TEXT,
            );
        }
        if let (true, Some(p)) = (v.crosshair, pointer) {
            crosshair(pm, p, s);
        }
    }

    /// The interactive selection's border: black under white dashes.
    fn border(&self, pm: &mut Pixmap, sel: &Rect, s: f64) {
        let Some(path) = device(sel, s).map(PathBuilder::from_rect) else {
            return;
        };
        let mut solid = stroke(BORDER_WIDTH, s);
        solid.line_cap = LineCap::Butt;
        pm.stroke_path(
            &path,
            &paint([0.0, 0.0, 0.0, 1.0]),
            &solid,
            Transform::identity(),
            None,
        );
        let mut dashed = solid.clone();
        dashed.dash = StrokeDash::new(BORDER_DASH.iter().map(|d| d * s as f32).collect(), 0.0);
        pm.stroke_path(
            &path,
            &paint([1.0, 1.0, 1.0, 1.0]),
            &dashed,
            Transform::identity(),
            None,
        );
    }

    // ---- toolbar ---------------------------------------------------------------------------------------------------

    /// Draw the toolbar's controls into `pm` (the 628 x 104 surface at scale `s`).
    pub fn toolbar(&mut self, pm: &mut Pixmap, s: f64, v: &ToolbarView) {
        let p = self.palette;
        for sep in toolbar::separators() {
            let line = Rect::new(sep.center().x - 0.5, sep.y(), 1.0, sep.height());
            fill_rect(pm, &line, s, p.separator);
        }
        for (item, r) in toolbar::items() {
            let color = if !item.enabled() {
                p.disabled
            } else if v.hovered == Some(item) {
                p.text
            } else {
                p.secondary
            };
            if item == Item::Mode(v.mode) {
                fill_rounded(
                    pm,
                    &r.inset(Insets::uniform(SELECTED_INSET)),
                    SELECTED_RADIUS,
                    s,
                    p.selected,
                );
            }
            match item {
                Item::Close => close_glyph(pm, &r, s, color),
                Item::Mode(Mode::Screen) => screen_glyph(pm, r.center(), s, color, false),
                Item::Mode(Mode::Window) => window_glyph(pm, r.center(), s, color, false),
                Item::Mode(Mode::Selection) => selection_glyph(pm, r.center(), s, color, false),
                Item::RecordScreen => screen_glyph(pm, r.center(), s, color, true),
                Item::RecordWindow => window_glyph(pm, r.center(), s, color, true),
                Item::StopRecording => stop_glyph(pm, r.center(), s, color),
                Item::RecordSelection => selection_glyph(pm, r.center(), s, color, true),
                Item::Options => {
                    let label = "Options";
                    let w = self.measure(label, MENU_FONT_SIZE, TEXT_WEIGHT);
                    // the popup arrow asset is 8 x 4, 4 pt after the title (approximate)
                    let x = r.x() + (r.width() - w - 12.0) / 2.0;
                    let text = if v.hovered == Some(item) {
                        p.text
                    } else {
                        p.secondary
                    };
                    self.text_in_band(
                        pm,
                        label,
                        x,
                        r.y(),
                        r.height(),
                        MENU_FONT_SIZE,
                        TEXT_WEIGHT,
                        s,
                        text,
                    );
                    let (ax, ay) = (x + w + 4.0, r.center().y - 2.0);
                    stroke_path(
                        pm,
                        polyline(&[(ax, ay), (ax + 4.0, ay + 4.0), (ax + 8.0, ay)], s, false),
                        1.5,
                        s,
                        text,
                    );
                }
                Item::Capture => {
                    let label = v
                        .countdown
                        .map_or_else(|| "Capture".to_string(), |n| n.to_string());
                    let w = self.measure(&label, MENU_FONT_SIZE, TEXT_WEIGHT);
                    self.text_in_band(
                        pm,
                        &label,
                        r.x() + (r.width() - w) / 2.0,
                        r.y(),
                        r.height(),
                        MENU_FONT_SIZE,
                        TEXT_WEIGHT,
                        s,
                        p.text,
                    );
                }
            }
        }
    }

    // ---- menu ------------------------------------------------------------------------------------------------------

    /// Draw the Options menu's rows into `pm` (a [`MENU_WIDTH`]-wide surface at scale `s`).
    pub fn menu(&mut self, pm: &mut Pixmap, s: f64, rows: &[MenuRow], hovered: Option<usize>) {
        let p = self.palette;
        let (rects, _) = toolbar::menu_layout(rows);
        for (i, (row, r)) in rows.iter().zip(&rects).enumerate() {
            match row {
                MenuRow::Separator => fill_rect(
                    pm,
                    &Rect::new(10.0, r.center().y - 0.5, MENU_WIDTH - 20.0, 1.0),
                    s,
                    p.separator,
                ),
                MenuRow::Header(title) => self.text_in_band(
                    pm,
                    title,
                    10.0,
                    r.y(),
                    r.height(),
                    MENU_FONT_SIZE,
                    HEADER_WEIGHT,
                    s,
                    p.secondary,
                ),
                MenuRow::Choice { label, checked, .. } => {
                    let lit = hovered == Some(i);
                    if lit {
                        fill_rounded(
                            pm,
                            &Rect::new(5.0, r.y(), MENU_WIDTH - 10.0, r.height()),
                            MENU_RADIUS / 2.0,
                            s,
                            p.highlight,
                        );
                    }
                    let color = if lit { [1.0, 1.0, 1.0, 1.0] } else { p.text };
                    if *checked {
                        let (x, y) = (12.0, r.center().y);
                        stroke_path(
                            pm,
                            polyline(&[(x, y), (x + 3.5, y + 3.5), (x + 10.0, y - 4.0)], s, false),
                            1.6,
                            s,
                            color,
                        );
                    }
                    self.text.set_scale(s as f32);
                    let (text, _) = self.text.fit(
                        label,
                        (MENU_FONT_SIZE * s) as f32,
                        TEXT_WEIGHT,
                        0.0,
                        ((MENU_WIDTH - 40.0) * s) as f32,
                    );
                    self.text_in_band(
                        pm,
                        &text,
                        28.0,
                        r.y(),
                        r.height(),
                        MENU_FONT_SIZE,
                        TEXT_WEIGHT,
                        s,
                        color,
                    );
                }
            }
        }
    }

    // ---- thumbnail -------------------------------------------------------------------------------------------------

    /// Draw the thumbnail `image` (already at the device size of `size` points) with its shadow, offset and alpha
    /// from `look`, into a surface of `size` plus the shadow outset on every side.
    pub fn thumbnail(
        &mut self,
        pm: &mut Pixmap,
        s: f64,
        image: &Pixmap,
        size: (f64, f64),
        look: Look,
    ) {
        let outset = THUMBNAIL_SHADOW_OUTSET;
        let rect = Rect::new(outset + look.offset, outset, size.0, size.1);
        let shadow = self.shadow_for(pm.width(), pm.height(), s, size);
        let shadow_paint = PixmapPaint {
            opacity: look.alpha as f32,
            ..PixmapPaint::default()
        };
        pm.draw_pixmap(
            (look.offset * s).round() as i32,
            0,
            shadow.as_ref(),
            &shadow_paint,
            Transform::identity(),
            None,
        );
        let Some(path) = rounded_path(
            rect,
            Radii::uniform(THUMBNAIL_RADIUS),
            CornerStyle::Circular,
            s,
        ) else {
            return;
        };
        let mut mask = Mask::new(pm.width(), pm.height()).expect("the surface has a size");
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        let image_paint = PixmapPaint {
            opacity: look.alpha as f32,
            quality: FilterQuality::Nearest,
            ..PixmapPaint::default()
        };
        pm.draw_pixmap(
            (rect.x() * s).round() as i32,
            (rect.y() * s).round() as i32,
            image.as_ref(),
            &image_paint,
            Transform::identity(),
            Some(&mask),
        );
        pm.stroke_path(
            &path,
            &paint(with_alpha(THUMBNAIL_BORDER_COLOR, look.alpha)),
            &stroke(THUMBNAIL_BORDER, s),
            Transform::identity(),
            None,
        );
    }

    /// The shadow of a thumbnail of `size` points in a surface of `width` x `height` pixels, computed once per size.
    fn shadow_for(&mut self, width: u32, height: u32, s: f64, size: (f64, f64)) -> &Pixmap {
        if self
            .shadow
            .as_ref()
            .is_none_or(|(key, _)| *key != (width, height))
        {
            let mut pm = Pixmap::new(width, height).expect("the surface has a size");
            let rect = Rect::new(
                THUMBNAIL_SHADOW_OUTSET * s,
                THUMBNAIL_SHADOW_OUTSET * s,
                size.0 * s,
                size.1 * s,
            );
            let sigma = THUMBNAIL_SHADOW_SIGMA * s;
            let data = pm.data_mut();
            for y in 0..height {
                for x in 0..width {
                    let a = (blurred_rect(
                        Point::new(f64::from(x) + 0.5, f64::from(y) + 0.5),
                        rect,
                        sigma,
                    ) * THUMBNAIL_SHADOW_ALPHA
                        * 255.0)
                        .round() as u8;
                    let i = ((y * width + x) * 4) as usize;
                    data[i..i + 4].copy_from_slice(&[0, 0, 0, a]);
                }
            }
            self.shadow = Some(((width, height), pm));
        }
        &self.shadow.as_ref().expect("computed above").1
    }
}

/// `image` reduced to `width` x `height` pixels by averaging the source pixels each target pixel covers, so a capture
/// a tenth of its size stays smooth.
pub fn downscale(image: &Pixmap, width: u32, height: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(width, height)?;
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let src = image.data();
    let dst = out.data_mut();
    for y in 0..height as usize {
        let (y0, y1) = (
            y * sh / height as usize,
            ((y + 1) * sh).div_ceil(height as usize).min(sh),
        );
        for x in 0..width as usize {
            let (x0, x1) = (
                x * sw / width as usize,
                ((x + 1) * sw).div_ceil(width as usize).min(sw),
            );
            let mut sum = [0u32; 4];
            for yy in y0..y1 {
                for xx in x0..x1 {
                    let i = (yy * sw + xx) * 4;
                    for c in 0..4 {
                        sum[c] += u32::from(src[i + c]);
                    }
                }
            }
            let n = ((y1 - y0) * (x1 - x0)).max(1) as u32;
            let o = (y * width as usize + x) * 4;
            for c in 0..4 {
                dst[o + c] = ((sum[c] + n / 2) / n) as u8;
            }
        }
    }
    Some(out)
}

/// The crosshair with its centre pixel at `p` (logical, surface-local), from the 1x asset's geometry.
fn crosshair(pm: &mut Pixmap, p: Point, s: f64) {
    // asset pixel c spans c..c+1, so the centre pixel's middle is at CROSSHAIR_CENTRE + 0.5
    let mid = CROSSHAIR_CENTRE + 0.5;
    let (from, to) = (CROSSHAIR_ARM.0 - mid, CROSSHAIR_ARM.1 + 1.0 - mid);
    let arms = [(from, -CROSSHAIR_GAP), (CROSSHAIR_GAP, to)];
    for (halo, color) in [(1.0, CROSSHAIR_HALO), (0.0, CROSSHAIR_CORE)] {
        for (a, b) in arms {
            fill_rect(
                pm,
                &Rect::new(
                    p.x + a - halo,
                    p.y - 0.5 - halo,
                    b - a + 2.0 * halo,
                    1.0 + 2.0 * halo,
                ),
                s,
                color,
            );
            fill_rect(
                pm,
                &Rect::new(
                    p.x - 0.5 - halo,
                    p.y + a - halo,
                    1.0 + 2.0 * halo,
                    b - a + 2.0 * halo,
                ),
                s,
                color,
            );
        }
    }
}

fn close_glyph(pm: &mut Pixmap, r: &Rect, s: f64, color: Rgba) {
    let c = r.center();
    let radius = r.width().min(r.height()) / 2.0;
    if let Some(path) = circle(c, radius, s) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    let d = radius * 0.4;
    let mut cut = paint([0.0, 0.0, 0.0, 1.0]);
    cut.blend_mode = BlendMode::Clear;
    for (a, b) in [
        ((c.x - d, c.y - d), (c.x + d, c.y + d)),
        ((c.x - d, c.y + d), (c.x + d, c.y - d)),
    ] {
        if let Some(path) = polyline(&[a, b], s, false) {
            pm.stroke_path(&path, &cut, &stroke(1.5, s), Transform::identity(), None);
        }
    }
}

/// A 24 x 17 frame centred on `c`, the base of the mode glyphs.
fn frame(c: Point) -> Rect {
    Rect::new(c.x - 12.0, c.y - 8.5, 24.0, 17.0)
}

fn badge(pm: &mut Pixmap, c: Point, s: f64, color: Rgba) {
    let at = Point::new(c.x + 11.0, c.y + 7.0);
    if let Some(path) = circle(at, 4.5, s) {
        let mut clear = paint([0.0, 0.0, 0.0, 1.0]);
        clear.blend_mode = BlendMode::Clear;
        pm.fill_path(
            &path,
            &clear,
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    if let Some(path) = circle(at, 3.0, s) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn outline(pm: &mut Pixmap, r: &Rect, radius: f64, s: f64, color: Rgba) {
    if let Some(path) = rounded_path(*r, Radii::uniform(radius), CornerStyle::Circular, s) {
        pm.stroke_path(
            &path,
            &paint(color),
            &stroke(1.3, s),
            Transform::identity(),
            None,
        );
    }
}

/// A screen with a menu bar and a dock.
fn screen_glyph(pm: &mut Pixmap, c: Point, s: f64, color: Rgba, record: bool) {
    let f = frame(c);
    outline(pm, &f, 2.0, s, color);
    fill_rect(pm, &Rect::new(f.x(), f.y(), f.width(), 2.5), s, color);
    fill_rect(
        pm,
        &Rect::new(c.x - 5.0, f.max_y() - 4.0, 10.0, 2.0),
        s,
        color,
    );
    if record {
        badge(pm, c, s, color);
    }
}

/// A window with a title bar.
fn window_glyph(pm: &mut Pixmap, c: Point, s: f64, color: Rgba, record: bool) {
    let f = frame(c).inset(Insets::uniform(1.5));
    outline(pm, &f, 2.5, s, color);
    fill_rect(pm, &Rect::new(f.x(), f.y() + 4.0, f.width(), 1.2), s, color);
    if record {
        badge(pm, c, s, color);
    }
}

/// A dashed rectangle.
fn selection_glyph(pm: &mut Pixmap, c: Point, s: f64, color: Rgba, record: bool) {
    let f = frame(c).inset(Insets::uniform(1.5));
    if let Some(path) = device(&f, s).map(PathBuilder::from_rect) {
        let mut dashed = stroke(1.3, s);
        dashed.line_cap = LineCap::Butt;
        dashed.dash = StrokeDash::new(vec![(3.0 * s) as f32, (2.0 * s) as f32], 0.0);
        pm.stroke_path(&path, &paint(color), &dashed, Transform::identity(), None);
    }
    if record {
        badge(pm, c, s, color);
    }
}

fn stop_glyph(pm: &mut Pixmap, c: Point, s: f64, color: Rgba) {
    if let Some(path) = circle(c, 8.5, s) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
    fill_rect_blended(
        pm,
        &Rect::new(c.x - 3.0, c.y - 3.0, 6.0, 6.0),
        s,
        [0.0, 0.0, 0.0, 1.0],
        BlendMode::Clear,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pixel(pm: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let c = pm.pixel(x, y).expect("inside");
        [c.red(), c.green(), c.blue(), c.alpha()]
    }

    /// Needs the asset directory (`CUPERTINO_ASSET_DIR`); fails with the build instructions when it is missing.
    fn painter() -> Painter {
        let assets = Assets::discover();
        Painter::new(
            Palette {
                text: [0.0; 4],
                secondary: [0.0; 4],
                disabled: [0.0; 4],
                selected: [0.0; 4],
                separator: [0.0; 4],
                highlight: [0.0; 4],
            },
            Typography::default(),
            &assets,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_interactive_overlay_dims_around_a_clear_hole_with_a_border() {
        let mut painter = painter();
        let mut pm = Pixmap::new(400, 300).expect("a pixmap");
        let view = OverlayView {
            bounds: Rect::new(1000.0, 0.0, 400.0, 300.0),
            style: Style::Interactive,
            mode: Mode::Selection,
            selection: Some(Rect::new(1100.0, 100.0, 100.0, 50.0)),
            sizing: false,
            picked: None,
            pointer: None,
            crosshair: false,
        };
        painter.overlay(&mut pm, 1.0, &view);
        assert_eq!(
            pixel(&pm, 10, 10),
            [0, 0, 0, 128],
            "the dim is black at half alpha"
        );
        assert_eq!(
            pixel(&pm, 150, 125),
            [0, 0, 0, 0],
            "the selection is a hole"
        );
        assert_eq!(pixel(&pm, 100, 125)[3], 255, "the border is opaque");
    }

    /// Thin anti-aliased rectangles at sub-pixel positions failed a `tiny-skia` debug assertion and killed the process
    /// mid-drag; the overlay, the crosshair and the readout draw at any pointer position and scale, and so do the
    /// toolbar, the menu and the thumbnail.
    #[test]
    fn every_surface_draws_at_sub_pixel_positions_and_fractional_scales() {
        let mut painter = painter();
        let rows = toolbar::menu(&crate::settings::Settings::default());
        let image = Pixmap::new(30, 20).expect("a pixmap");
        for (scale, offset) in [(1.0, 0.3), (1.5, 0.5), (1.25, 0.77), (2.0, 0.1)] {
            let mut bar =
                Pixmap::new((628.0 * scale) as u32, (104.0 * scale) as u32).expect("a pixmap");
            painter.toolbar(
                &mut bar,
                scale,
                &ToolbarView {
                    mode: Mode::Window,
                    hovered: Some(Item::Options),
                    countdown: Some(3),
                },
            );
            let mut menu =
                Pixmap::new((MENU_WIDTH * scale) as u32, (400.0 * scale) as u32).expect("a pixmap");
            painter.menu(&mut menu, scale, &rows, Some(1));
            let mut thumb =
                Pixmap::new((142.0 * scale) as u32, (132.0 * scale) as u32).expect("a pixmap");
            painter.thumbnail(
                &mut thumb,
                scale,
                &image,
                (30.0, 20.0),
                Look {
                    offset: 7.0 + offset,
                    alpha: 0.5,
                },
            );
            let mut pm =
                Pixmap::new((400.0 * scale) as u32, (300.0 * scale) as u32).expect("a pixmap");
            for style in [Style::Simple, Style::Interactive] {
                let view = OverlayView {
                    bounds: Rect::new(0.0, 0.0, 400.0, 300.0),
                    style,
                    mode: Mode::Selection,
                    selection: Some(Rect::new(10.0 + offset, 20.0, 1.0 + offset, 50.0)),
                    sizing: true,
                    picked: None,
                    pointer: Some(Point::new(200.0 + offset, 150.0 + offset)),
                    crosshair: true,
                };
                painter.overlay(&mut pm, scale, &view);
            }
        }
    }

    #[test]
    fn downscaling_averages_the_covered_pixels() {
        let mut image = Pixmap::new(4, 2).expect("a pixmap");
        image.fill(Color::from_rgba8(0, 0, 0, 255));
        image.data_mut()[..8].copy_from_slice(&[255, 255, 255, 255, 255, 255, 255, 255]);
        let small = downscale(&image, 2, 1).expect("a pixmap");
        assert_eq!(pixel(&small, 0, 0), [128, 128, 128, 255]);
        assert_eq!(pixel(&small, 1, 0), [0, 0, 0, 255]);
    }
}
