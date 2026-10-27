//! CPU rendering of sample icon and list views (tiny-skia, cosmic-text through the decorations
//! crate). Items are drawn with the file icons of the asset packs; an icon no pack provides falls back to a more generic
//! one, and with none at all to a placeholder.

use crate::entry::{Entry, Kind};
use crate::geom::{Rect, Size};
use crate::group::Group;
use crate::layout::icon::{self, FontMetrics, IconParams};
use crate::layout::list::{self, ListParams, RowSpec};
use crate::metrics::{icon_view as im, list_view as lm, selection as sel};
use crate::mime;
use crate::settings::{ColumnId, IconViewSettings, ListViewSettings};
use crate::sort::SortSpec;
use crate::tags::TagColor;
use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::palette::{Palette as DecorationPalette, Rgba};
use cupertino_decorations::text::TextRenderer;
use cupertino_decorations::TitlebarStyle;
use cupertino_icons::document::{badge_rect, Badge, FileIcons};
use cupertino_icons::SystemIcons;
use cupertino_theme::{Appearance, SemanticColor, Theme};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, Rect as SkRect, Stroke, Transform};

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub dark: bool,
    pub background: Rgba,
    /// Fill of the toolbar strip, of an active and of an inactive window: the unified title bar fills of
    /// `cupertino-decorations`.
    pub toolbar: Rgba,
    pub toolbar_inactive: Rgba,
    pub text: Rgba,
    pub secondary_text: Rgba,
    pub separator: Rgba,
    pub accent: Rgba,
    pub accent_text: Rgba,
    /// The background of selected text (the name editor).
    pub text_selection: Rgba,
    pub inactive_selection: Rgba,
    /// and: the rubber band's border and fill, the icon bubble's fill.
    pub tertiary_label: Rgba,
    pub quaternary_label: Rgba,
    pub row_alternate: Rgba,
    /// Fill of the list's column header row: the content background, its resting colour; a window
    /// draws it with its toolbar strip's fill, being part of the strip.
    pub header: Rgba,
    /// The system colours of the seven tag colours, indexed like [`TagColor::ALL`].
    pub tag_colors: [Rgba; 7],
}

/// The `selected-text-background` token of both appearances, for [`Palette::builtin`].
const TEXT_SELECTION: Rgba = [0.0, 122.0 / 255.0, 1.0, 89.0 / 255.0];

fn c8(r: u8, g: u8, b: u8) -> Rgba {
    [r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0, 1.0]
}

/// The system colours of the tag colours when the specification files are missing, in the order of [`TagColor::ALL`]:
/// gray, green, purple, blue, yellow, red, orange. approximate (the design tokens give the real ones, which
/// [`Palette::from_theme`] reads).
const TAG_COLORS_LIGHT: [Rgba; 7] = [
    [0.557, 0.557, 0.576, 1.0],
    [0.204, 0.78, 0.349, 1.0],
    [0.686, 0.322, 0.871, 1.0],
    [0.0, 0.478, 1.0, 1.0],
    [1.0, 0.8, 0.0, 1.0],
    [1.0, 0.231, 0.188, 1.0],
    [1.0, 0.584, 0.0, 1.0],
];
const TAG_COLORS_DARK: [Rgba; 7] = [
    [0.596, 0.596, 0.616, 1.0],
    [0.188, 0.82, 0.345, 1.0],
    [0.749, 0.353, 0.949, 1.0],
    [0.039, 0.518, 1.0, 1.0],
    [1.0, 0.839, 0.039, 1.0],
    [1.0, 0.271, 0.227, 1.0],
    [1.0, 0.624, 0.039, 1.0],
];

impl Palette {
    /// The colour of tag colour `color`.
    pub fn tag_color(&self, color: TagColor) -> Rgba {
        self.tag_colors[TagColor::ALL
            .iter()
            .position(|c| *c == color)
            .expect("every tag colour is in ALL")]
    }

    /// Colours used when the specification files are not available (the same numbers as the
    /// design tokens: window background, label colours, system blue).
    pub fn builtin(dark: bool) -> Self {
        let decorations = DecorationPalette::builtin(dark);
        if dark {
            Palette {
                dark,
                background: c8(0x1E, 0x1E, 0x1E),
                toolbar: decorations.titlebar(TitlebarStyle::Unified, true),
                toolbar_inactive: decorations.titlebar(TitlebarStyle::Unified, false),
                text: [1.0, 1.0, 1.0, 0.85],
                secondary_text: [1.0, 1.0, 1.0, 0.55],
                separator: [1.0, 1.0, 1.0, 0.1],
                accent: c8(0x0A, 0x84, 0xFF),
                accent_text: [1.0, 1.0, 1.0, 1.0],
                text_selection: TEXT_SELECTION,
                inactive_selection: [1.0, 1.0, 1.0, 0.2],
                tertiary_label: [1.0, 1.0, 1.0, 0.2471],
                quaternary_label: [1.0, 1.0, 1.0, 0.098],
                row_alternate: [1.0, 1.0, 1.0, 0.04],
                header: c8(0x1E, 0x1E, 0x1E),
                tag_colors: TAG_COLORS_DARK,
            }
        } else {
            Palette {
                dark,
                background: c8(0xFF, 0xFF, 0xFF),
                toolbar: decorations.titlebar(TitlebarStyle::Unified, true),
                toolbar_inactive: decorations.titlebar(TitlebarStyle::Unified, false),
                text: [0.0, 0.0, 0.0, 0.85],
                secondary_text: [0.0, 0.0, 0.0, 0.5],
                separator: [0.0, 0.0, 0.0, 0.1],
                accent: c8(0x00, 0x7A, 0xFF),
                accent_text: [1.0, 1.0, 1.0, 1.0],
                text_selection: TEXT_SELECTION,
                inactive_selection: [0.0, 0.0, 0.0, 0.12],
                tertiary_label: [0.0, 0.0, 0.0, 0.2588],
                quaternary_label: [0.0, 0.0, 0.0, 0.098],
                row_alternate: [0.0, 0.0, 0.0, 0.035],
                header: c8(0xFF, 0xFF, 0xFF),
                tag_colors: TAG_COLORS_LIGHT,
            }
        }
    }

    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Self {
        let mut p = Self::builtin(appearance.is_dark());
        let sem = |c: SemanticColor| theme.tokens.semantic(appearance, c).map(|c| c.to_array());
        let decorations = DecorationPalette::from_theme(theme, appearance);
        p.toolbar = decorations.titlebar(TitlebarStyle::Unified, true);
        p.toolbar_inactive = decorations.titlebar(TitlebarStyle::Unified, false);
        p.background = sem(SemanticColor::ControlBackground).unwrap_or(p.background);
        p.header = p.background;
        p.text = sem(SemanticColor::Label).unwrap_or(p.text);
        p.secondary_text = sem(SemanticColor::SecondaryLabel).unwrap_or(p.secondary_text);
        p.separator = sem(SemanticColor::Grid).unwrap_or(p.separator);
        p.accent = sem(SemanticColor::SystemBlue).unwrap_or(p.accent);
        p.text_selection = sem(SemanticColor::SelectedTextBackground).unwrap_or(p.text_selection);
        for (slot, color) in p.tag_colors.iter_mut().zip(TagColor::ALL) {
            *slot = theme
                .tokens
                .color(appearance, color.system_color_name())
                .map_or(*slot, |c| c.to_array());
        }
        p.tertiary_label = sem(SemanticColor::TertiaryLabel).unwrap_or(p.tertiary_label);
        p.quaternary_label = sem(SemanticColor::QuaternaryLabel).unwrap_or(p.quaternary_label);
        p
    }
}

pub(crate) fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color(Color::from_rgba(c[0], c[1], c[2], c[3]).unwrap_or(Color::BLACK));
    p.anti_alias = true;
    p
}

fn rounded(r: &Rect, radius: f64, s: f32) -> Option<tiny_skia::Path> {
    rounded_px(
        r.x as f32 * s,
        r.y as f32 * s,
        r.w as f32 * s,
        r.h as f32 * s,
        radius as f32 * s,
    )
}

/// A rounded rectangle in device pixels.
fn rounded_px(x: f32, y: f32, w: f32, h: f32, radius: f32) -> Option<tiny_skia::Path> {
    let rad = radius.min(w / 2.0).min(h / 2.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + rad, y);
    pb.line_to(x + w - rad, y);
    pb.quad_to(x + w, y, x + w, y + rad);
    pb.line_to(x + w, y + h - rad);
    pb.quad_to(x + w, y + h, x + w - rad, y + h);
    pb.line_to(x + rad, y + h);
    pb.quad_to(x, y + h, x, y + h - rad);
    pb.line_to(x, y + rad);
    pb.quad_to(x, y, x + rad, y);
    pb.close();
    pb.finish()
}

pub(crate) fn fill_rounded(pm: &mut Pixmap, r: &Rect, radius: f64, s: f32, c: Rgba) {
    if let Some(p) = rounded(r, radius, s) {
        pm.fill_path(
            &p,
            &paint(c),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

/// [`fill_rounded`] for the content views, drawn `up` whole device pixels higher: the same pixels wherever the view is
/// drawn from ([`in_layer`]).
pub(crate) fn fill_rounded_up(pm: &mut Pixmap, r: &Rect, radius: f64, s: f32, up: f32, c: Rgba) {
    in_layer(pm, r, s, up, 0.0, |layer, x, y| {
        if let Some(p) = rounded_px(x, y, r.w as f32 * s, r.h as f32 * s, radius as f32 * s) {
            layer.fill_path(
                &p,
                &paint(c),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    });
}

/// Draw a shape covering `r` (points) through a layer placed on whole device pixels, `up` pixels higher: `draw` gets
/// the layer and the position of `r` in it, which keeps only the fraction of the position. Curves are flattened in
/// floating point, so a shape drawn in place would change its edge pixels with its distance from the origin; this way a
/// view drawn in bands or scrolled by whole pixels has the same pixels. `reach` is how far the shape goes past `r`
/// (half a stroke).
fn in_layer(
    pm: &mut Pixmap,
    r: &Rect,
    s: f32,
    up: f32,
    reach: f32,
    draw: impl FnOnce(&mut Pixmap, f32, f32),
) {
    let margin = reach.ceil() + 1.0;
    let (x, y) = (r.x as f32 * s, r.y as f32 * s);
    let (x0, y0) = (x.floor() - margin, y.floor() - margin);
    let (w, h) = (
        (x + r.w as f32 * s + margin).ceil() - x0,
        (y + r.h as f32 * s + margin).ceil() - y0,
    );
    let Some(mut layer) = Pixmap::new(w.max(1.0) as u32, h.max(1.0) as u32) else {
        return;
    };
    draw(&mut layer, x - x0, y - y0);
    pm.draw_pixmap(
        x0 as i32,
        (y0 - up) as i32,
        layer.as_ref(),
        &tiny_skia::PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// The bubble behind a selected icon: the icon enlarged by its padding, corner radius equal to the padding, a
/// quaternary label fill and a white 50 % border (padding = radius = longest side / 16, border
/// `min(2, trunc(padding)); no shadow). The same in an active and an inactive window.
pub(crate) fn paint_icon_bubble(pm: &mut Pixmap, icon: &Rect, scale: f32, up: f32, pal: &Palette) {
    let pad = icon.w.max(icon.h) / sel::BUBBLE_DIVISOR;
    let bubble = icon.inset(-pad, -pad);
    fill_rounded_up(pm, &bubble, pad, scale, up, pal.quaternary_label);
    let border = pad.trunc().min(sel::BUBBLE_BORDER_MAX);
    if border >= 1.0 {
        stroke_rounded_up(
            pm,
            &bubble.inset(border / 2.0, border / 2.0),
            (pad - border / 2.0).max(0.0),
            border,
            scale,
            up,
            [1.0, 1.0, 1.0, sel::BUBBLE_BORDER_ALPHA],
        );
    }
}

/// Stroke the outline of `r` (the stroke is centred on it).
pub(crate) fn stroke_rounded(pm: &mut Pixmap, r: &Rect, radius: f64, width: f64, s: f32, c: Rgba) {
    if let Some(p) = rounded(r, radius, s) {
        pm.stroke_path(
            &p,
            &paint(c),
            &Stroke {
                width: width as f32 * s,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}

/// [`stroke_rounded`] for the content views, drawn `up` whole device pixels higher ([`in_layer`]).
fn stroke_rounded_up(pm: &mut Pixmap, r: &Rect, radius: f64, width: f64, s: f32, up: f32, c: Rgba) {
    let stroke = width as f32 * s;
    in_layer(pm, r, s, up, stroke / 2.0, |layer, x, y| {
        if let Some(p) = rounded_px(x, y, r.w as f32 * s, r.h as f32 * s, radius as f32 * s) {
            layer.stroke_path(
                &p,
                &paint(c),
                &Stroke {
                    width: stroke,
                    ..Default::default()
                },
                Transform::identity(),
                None,
            );
        }
    });
}

pub(crate) fn fill_rect(pm: &mut Pixmap, r: &Rect, s: f32, c: Rgba) {
    fill_rect_up(pm, r, s, 0.0, c);
}

/// [`fill_rect`] drawn `up` whole device pixels higher: the pixels of drawing it in place, moved (the position is
/// scaled first and moved by whole pixels after, so the fraction that decides the edges is the same).
pub(crate) fn fill_rect_up(pm: &mut Pixmap, r: &Rect, s: f32, up: f32, c: Rgba) {
    if let Some(rr) = SkRect::from_xywh(
        r.x as f32 * s,
        r.y as f32 * s - up,
        r.w as f32 * s,
        r.h as f32 * s,
    ) {
        pm.fill_rect(rr, &paint(c), Transform::identity(), None);
    }
}

/// Split `text` into at most `max_lines` lines no wider than `max_w`; the last line is middle-truncated.
pub fn wrap_label(
    tr: &mut TextRenderer,
    text: &str,
    px: f32,
    max_w: f32,
    max_lines: usize,
) -> Vec<String> {
    if tr.measure(text, px, 400, 0.0) <= max_w || max_lines <= 1 {
        return vec![truncate_middle(tr, text, px, max_w)];
    }
    let chars: Vec<char> = text.chars().collect();
    let mut lines = Vec::new();
    let mut start = 0;
    while start < chars.len() && lines.len() + 1 < max_lines {
        let mut end = start;
        let mut last_break = None;
        while end < chars.len() {
            let s: String = chars[start..=end].iter().collect();
            if tr.measure(&s, px, 400, 0.0) > max_w {
                break;
            }
            if matches!(chars[end], ' ' | '-' | '_' | '.') {
                last_break = Some(end + 1);
            }
            end += 1;
        }
        if end >= chars.len() {
            start = chars.len();
            lines.push(chars[start.min(chars.len())..].iter().collect::<String>());
            break;
        }
        let cut = last_break
            .filter(|b| *b > start)
            .unwrap_or(end.max(start + 1));
        lines.push(
            chars[start..cut]
                .iter()
                .collect::<String>()
                .trim_end()
                .to_string(),
        );
        start = cut;
    }
    if start < chars.len() {
        let rest: String = chars[start..].iter().collect();
        lines.push(truncate_middle(tr, &rest, px, max_w));
    }
    lines.retain(|l| !l.is_empty());
    lines
}

/// Draw `icon` scaled to fit a `px` square with its top-left corner at (`x`, `y`) device pixels.
fn draw_pixmap_fit(pm: &mut Pixmap, icon: &Pixmap, x: f32, y: f32, px: u32) {
    let k = px as f32 / icon.width().max(icon.height()) as f32;
    let (dx, dy) = (
        (px as f32 - icon.width() as f32 * k) / 2.0,
        (px as f32 - icon.height() as f32 * k) / 2.0,
    );
    let paint = tiny_skia::PixmapPaint {
        quality: tiny_skia::FilterQuality::Bilinear,
        ..Default::default()
    };
    pm.draw_pixmap(
        0,
        0,
        icon.as_ref(),
        &paint,
        Transform::from_row(k, 0.0, 0.0, k, x + dx, y + dy),
        None,
    );
}

/// Middle truncation: keep the start and the end (with the extension) and put an ellipsis between.
pub fn truncate_middle(tr: &mut TextRenderer, text: &str, px: f32, max_w: f32) -> String {
    if tr.measure(text, px, 400, 0.0) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    let n = chars.len();
    for keep in (2..n).rev() {
        let tail = keep / 2;
        let head = keep - tail;
        let s: String = chars[..head].iter().collect::<String>()
            + "…"
            + &chars[n - tail..].iter().collect::<String>();
        if tr.measure(&s, px, 400, 0.0) <= max_w {
            return s;
        }
    }
    "…".to_string()
}

/// Tail truncation: keep the start and end with an ellipsis, the sidebar's names (a screenshot shows the
/// computer's name cut this way; that every sidebar row truncates at the tail is ).
pub fn truncate_tail(tr: &mut TextRenderer, text: &str, px: f32, max_w: f32) -> String {
    if tr.measure(text, px, 400, 0.0) <= max_w {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    for keep in (1..chars.len()).rev() {
        let s: String = chars[..keep]
            .iter()
            .collect::<String>()
            .trim_end()
            .to_string()
            + "…";
        if tr.measure(&s, px, 400, 0.0) <= max_w {
            return s;
        }
    }
    "…".to_string()
}

/// Space left on a file system for the status bar: decimal units like [`human_size`], with two decimals from a
/// gigabyte up (approximate, a screenshot's "227.27 GB"; its formatter is not known).
pub fn available_size(bytes: u64) -> String {
    match bytes {
        b if b >= 1_000_000_000_000 => format!("{:.2} TB", b as f64 / 1e12),
        b if b >= 1_000_000_000 => format!("{:.2} GB", b as f64 / 1e9),
        b => human_size(b),
    }
}

/// Every template symbol the window draws (toolbar items, sidebar places and the path bar), which the renderer
/// requires of the asset directory when it is made.
pub const SYMBOL_NAMES: [&str; 21] = [
    "symbol.columns",
    "symbol.gallery",
    "symbol.chevron.left",
    "symbol.chevron.right",
    "symbol.grid",
    "symbol.list",
    "symbol.house",
    "symbol.desktop",
    "symbol.document",
    "symbol.download",
    "symbol.music",
    "symbol.photo",
    "symbol.film",
    "symbol.computer",
    "symbol.trash",
    "symbol.recents",
    "symbol.applications",
    "symbol.chevron.up.down",
    "symbol.tag",
    "symbol.ellipsis.circle",
    "symbol.magnifyingglass",
];

/// `scroll` (points) as the whole device pixels a view moves by at `scale`.
pub(crate) fn scroll_pixels(scroll: f64, scale: f32) -> f32 {
    (scroll * f64::from(scale)).round() as f32
}

/// Points around a band that items are still drawn in: their anti-aliased edges reach into it.
const CULL_MARGIN: f64 = 2.0;

/// A slice of a content view in device pixels: `height` rows from row `top` of the whole view. A view drawn as a band
/// has the pixels the whole view has there: positions move by whole pixels only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Band {
    pub top: u32,
    pub height: u32,
}

impl Band {
    /// The band's top and height in points at `scale`.
    fn points(self, scale: f32) -> (f64, f64) {
        let s = f64::from(scale);
        (f64::from(self.top) / s, f64::from(self.height) / s)
    }
}

pub struct Renderer {
    pub text: TextRenderer,
    /// Icons of files from the asset directory.
    pub icons: FileIcons,
    /// The template symbols of the sidebar and the toolbar.
    symbols: SystemIcons,
    /// The content view of the last frame, given back by [`Renderer::recycle`]: the next view of its size reuses it.
    spare: Option<Pixmap>,
}

impl Renderer {
    /// A renderer with the `ui` font and the symbols of the asset packs, drawing items with `icons`. Fails only when no
    /// usable font is left ([`TextRenderer::new`]); a symbol of [`SYMBOL_NAMES`] no pack provides is reported and drawn as a placeholder.
    pub fn new(icons: FileIcons, assets: &Assets) -> Result<Self, AssetError> {
        Ok(Renderer {
            text: TextRenderer::new(assets, FontRole::Ui)?,
            icons,
            symbols: SystemIcons::required(assets, &[], &SYMBOL_NAMES),
            spare: None,
        })
    }

    /// A pixmap for a content view of `w` x `h` device pixels, filled with the opaque background: the spare one when it
    /// has that size. Every view starts opaque, so it covers what is under it ([`copy_opaque`]).
    fn view_pixmap(&mut self, w: u32, h: u32, pal: &Palette) -> Pixmap {
        let mut pm = match self.spare.take() {
            Some(pm) if (pm.width(), pm.height()) == (w, h) => pm,
            _ => Pixmap::new(w, h).unwrap(),
        };
        pm.fill(
            Color::from_rgba(pal.background[0], pal.background[1], pal.background[2], 1.0).unwrap(),
        );
        pm
    }

    /// Give back a content view once it is drawn, for the next frame's view.
    pub fn recycle(&mut self, view: Pixmap) {
        self.spare = Some(view);
    }

    /// Draw the template symbol `name` of the asset packs in `color`, fitted to `fit` points and centred in `r` (a
    /// placeholder when no pack provides it).
    pub fn draw_symbol(
        &mut self,
        pm: &mut Pixmap,
        r: &Rect,
        name: &str,
        fit: f64,
        s: f32,
        color: Rgba,
    ) {
        let px = (fit * f64::from(s)).round().max(1.0) as u32;
        let symbol = self.symbols.symbol(name, px, color);
        let c = r.center();
        let (x, y) = (
            c.x as f32 * s - symbol.width() as f32 / 2.0,
            c.y as f32 * s - symbol.height() as f32 / 2.0,
        );
        pm.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            symbol.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draw the icon of `e` into `r`: its icon from the asset directory with the alias badge on a symbolic link.
    /// Draw the icon of `e` centred in `r`, `up` whole device pixels higher. It lands on whole device pixels, so a
    /// view drawn in bands or scrolled by whole pixels samples it the same way.
    pub(crate) fn draw_item_icon(&mut self, pm: &mut Pixmap, r: &Rect, e: &Entry, s: f32, up: f32) {
        let icons = &mut self.icons;
        let side = r.w.min(r.h);
        let px = (side * s as f64).round().max(1.0) as u32;
        let mime = crate::open::file_type(icons.database(), &e.path, e.is_dir).to_string();
        let (x, y) = (
            ((r.x + (r.w - side) / 2.0) as f32 * s).round(),
            ((r.y + (r.h - side) / 2.0) as f32 * s).round() - up,
        );
        draw_pixmap_fit(pm, icons.document(&e.path, &mime, px), x, y, px);
        if e.kind == Kind::Symlink {
            let (bx, by, bside) = badge_rect(side);
            let bpx = (bside * s as f64).round().max(1.0) as u32;
            draw_pixmap_fit(
                pm,
                icons.badge(Badge::Alias, bpx),
                x + bx as f32 * s,
                y + by as f32 * s,
                bpx,
            );
        }
    }

    /// Font measures at `text_size` points from the shaper.
    pub fn font_metrics(&mut self, text_size: f64, scale: f32) -> FontMetrics {
        let px = text_size as f32 * scale;
        let cw = (self.text.measure("0", px, 400, 0.0) / scale).ceil() as f64;
        let est = FontMetrics::estimate(text_size);
        FontMetrics {
            char_width: if cw > 0.0 { cw } else { est.char_width },
            ..est
        }
    }

    /// Parameters and flow layout of an icon view `width` points wide, measured with the shaper at `scale`. The
    /// drawing of [`Renderer::icon_view`] and the hit testing of a window share it.
    pub fn icon_layout(
        &mut self,
        settings: &IconViewSettings,
        sections: &[icon::SectionSpec],
        width: f64,
        scale: f32,
    ) -> (IconParams, icon::IconLayout) {
        let font = self.font_metrics(settings.text_size as f64, scale);
        let params = IconParams::new(
            settings.icon_size,
            settings.grid_spacing,
            settings.text_size,
            settings.label_on_bottom,
            settings.show_item_info,
            font,
        );
        let layout = icon::layout(&params, width, sections, icon::Flow::Leading);
        (params, layout)
    }

    /// Layout of a list view of the rows `outline` (one per displayed item: its level and disclosure state) `width`
    /// points wide, measured with the shaper at `scale`. Shared by [`Renderer::list_view`] and hit testing.
    pub fn list_layout(
        &mut self,
        outline: &[RowSpec],
        settings: &ListViewSettings,
        width: f64,
        scale: f32,
    ) -> list::ListLayout {
        let font_line = self
            .font_metrics(settings.text_size as f64, scale)
            .line_height;
        list::layout(
            &ListParams {
                icon_size: settings.icon_size,
                font_line,
                hierarchical: true,
                columns: &settings.columns,
                viewport_width: width,
            },
            outline,
        )
    }

    /// Render an icon view of `entries` (already in display order) into a pixmap of `size` points, with the top
    /// `scroll` points of the content scrolled out of view.
    #[allow(clippy::too_many_arguments)]
    pub fn icon_view(
        &mut self,
        entries: &[Entry],
        groups: Option<&[Group]>,
        selected: &[usize],
        active: bool,
        settings: &IconViewSettings,
        size: Size,
        scroll: f64,
        band: Option<Band>,
        scale: f32,
        pal: &Palette,
    ) -> Pixmap {
        let (rows, height) = match band {
            Some(b) => (b.height, b.points(scale).1),
            None => ((size.h as f32 * scale) as u32, size.h),
        };
        let mut pm = self.view_pixmap((size.w as f32 * scale) as u32, rows, pal);
        // the view moves by whole device pixels: the scroll's and the band's
        let up = scroll_pixels(scroll, scale) + band.map_or(0, |b| b.top) as f32;
        // sections follow the group order; the display order of items is the concatenation of the groups
        let (order, sections): (Vec<usize>, Vec<icon::SectionSpec>) = match groups {
            Some(gs) => (
                gs.iter().flat_map(|g| g.items.iter().copied()).collect(),
                gs.iter()
                    .map(|g| icon::SectionSpec {
                        title: Some(g.title.clone()),
                        count: g.items.len(),
                    })
                    .collect(),
            ),
            None => (
                (0..entries.len()).collect(),
                vec![icon::SectionSpec {
                    title: None,
                    count: entries.len(),
                }],
            ),
        };
        let (params, l) = self.icon_layout(settings, &sections, size.w, scale);
        let font = params.font;
        let px = settings.text_size as f32 * scale;
        let viewport = Rect::new(
            0.0,
            f64::from(up) / f64::from(scale) - CULL_MARGIN,
            size.w,
            height + 2.0 * CULL_MARGIN,
        );
        for (title, r) in l.headers.iter().filter(|(_, r)| r.intersects(&viewport)) {
            self.text.draw(
                &mut pm,
                title,
                (r.x as f32 + 12.0) * scale,
                (r.y as f32 + 21.0) * scale - up,
                13.0 * scale,
                600,
                0.0,
                pal.text,
            );
            fill_rect_up(
                &mut pm,
                &Rect::new(12.0, r.bottom() - 1.0, r.w - 24.0, 1.0),
                scale,
                up,
                pal.separator,
            );
        }
        for cell in l.cells.iter().filter(|c| c.frame.intersects(&viewport)) {
            let e = &entries[order[cell.index]];
            let is_sel = selected.contains(&order[cell.index]);
            if is_sel {
                paint_icon_bubble(&mut pm, &cell.icon_frame, scale, up, pal);
            }
            self.draw_item_icon(&mut pm, &cell.icon_frame, e, scale, up);
            let max_w = cell.label_frame.w as f32 * scale - 6.0 * scale;
            let max_lines = icon::max_title_lines(&params).ceil() as usize;
            let lines = if settings.label_on_bottom {
                wrap_label(&mut self.text, &e.name, px, max_w, max_lines)
            } else {
                vec![truncate_middle(&mut self.text, &e.name, px, max_w)]
            };
            let lh = font.line_height as f32 * scale;
            let total = lh * lines.len() as f32;
            let top = cell.label_frame.y as f32 * scale
                + (cell.label_frame.h as f32 * scale - total) / 2.0;
            for (i, line) in lines.iter().enumerate() {
                let w = self.text.measure(line, px, 400, 0.0);
                let (x, color) = if settings.label_on_bottom {
                    (
                        cell.label_frame.x as f32 * scale
                            + (cell.label_frame.w as f32 * scale - w) / 2.0,
                        pal.text,
                    )
                } else {
                    (cell.label_frame.x as f32 * scale, pal.text)
                };
                if is_sel {
                    let r = Rect::new(
                        (x - 3.0 * scale) as f64 / scale as f64,
                        (top + i as f32 * lh) as f64 / scale as f64,
                        (w + 6.0 * scale) as f64 / scale as f64,
                        font.line_height,
                    );
                    fill_rounded_up(
                        &mut pm,
                        &r,
                        font.line_height / 4.0,
                        scale,
                        up,
                        if active {
                            pal.accent
                        } else {
                            pal.inactive_selection
                        },
                    );
                }
                let col = if is_sel && active {
                    pal.accent_text
                } else {
                    color
                };
                self.text.draw(
                    &mut pm,
                    line,
                    x,
                    top + i as f32 * lh + lh * 0.78 - up,
                    px,
                    400,
                    0.0,
                    col,
                );
            }
        }
        pm
    }

    /// Render a list view (header, alternating rows, name / date / size / kind columns) with the rows below the
    /// header scrolled up by `scroll` points; `outline` as for [`Renderer::list_layout`], one row per entry. The column of
    /// `sort` carries the sort caret.
    #[allow(clippy::too_many_arguments)]
    pub fn list_view(
        &mut self,
        entries: &[Entry],
        outline: &[RowSpec],
        selected: &[usize],
        active: bool,
        settings: &ListViewSettings,
        sort: SortSpec,
        size: Size,
        scroll: f64,
        band: Option<Band>,
        scale: f32,
        pal: &Palette,
        now: std::time::SystemTime,
    ) -> Pixmap {
        assert_eq!(entries.len(), outline.len(), "one outline row per entry");
        let (rows, (top, height)) = match band {
            Some(b) => (b.height, b.points(scale)),
            None => ((size.h as f32 * scale) as u32, (0.0, size.h)),
        };
        let mut pm = self.view_pixmap((size.w as f32 * scale) as u32, rows, pal);
        let px = settings.text_size as f32 * scale;
        let l = self.list_layout(outline, settings, size.w, scale);
        // the rows scroll under the header by whole device pixels; the header stays at the view's top
        let header_up = band.map_or(0, |b| b.top) as f32;
        let up = scroll_pixels(scroll, scale) + header_up;
        let first = f64::from(up) / f64::from(scale);
        // the rows under the header are drawn too: the header covers them
        for i in l.visible_rows(first - CULL_MARGIN, first + height + CULL_MARGIN) {
            let e = &entries[i];
            let row = &l.rows[i];
            let is_sel = selected.contains(&i);
            let band = Rect::new(0.0, row.frame.y, size.w, row.frame.h);
            if is_sel {
                fill_rect_up(
                    &mut pm,
                    &band,
                    scale,
                    up,
                    if active {
                        pal.accent
                    } else {
                        pal.inactive_selection
                    },
                );
            } else if list::ListLayout::is_alternate(i) {
                fill_rect_up(&mut pm, &band, scale, up, pal.row_alternate);
            }
            let fg = if is_sel && active {
                pal.accent_text
            } else {
                pal.text
            };
            let fg2 = if is_sel && active {
                [1.0, 1.0, 1.0, 0.85]
            } else {
                pal.secondary_text
            };
            if let Some(d) = row.disclosure {
                // a chevron pointing right, turned down when the folder is expanded (approximate shape)
                let mut pb = PathBuilder::new();
                let (cx, cy) = (
                    d.center().x as f32 * scale,
                    d.center().y as f32 * scale - up,
                );
                let point = |along: f32, across: f32| {
                    if row.expanded {
                        (cx - across * scale, cy + along * scale)
                    } else {
                        (cx + along * scale, cy + across * scale)
                    }
                };
                for (i, (x, y)) in [point(-2.0, -3.5), point(2.5, 0.0), point(-2.0, 3.5)]
                    .into_iter()
                    .enumerate()
                {
                    if i == 0 {
                        pb.move_to(x, y);
                    } else {
                        pb.line_to(x, y);
                    }
                }
                if let Some(p) = pb.finish() {
                    pm.stroke_path(
                        &p,
                        &paint(fg2),
                        &Stroke {
                            width: 1.4 * scale,
                            ..Default::default()
                        },
                        Transform::identity(),
                        None,
                    );
                }
            }
            self.draw_item_icon(&mut pm, &row.icon, e, scale, up);
            let base = (row.frame.y + row.frame.h / 2.0) as f32 * scale + px * 0.36 - up;
            let mut col_x = 0.0;
            for c in l.columns.iter() {
                let text = match c.id {
                    ColumnId::Name => truncate_middle(
                        &mut self.text,
                        &e.name,
                        px,
                        (c.frame.w as f32 - row.text_x as f32 - 4.0) * scale,
                    ),
                    ColumnId::DateModified => e
                        .modified
                        .map(|t| relative_date(t, now))
                        .unwrap_or_else(|| "--".into()),
                    ColumnId::Size => {
                        if e.is_dir {
                            "--".into()
                        } else {
                            human_size(e.size)
                        }
                    }
                    ColumnId::Kind => mime::kind_description(&mime::entry_mime(e)),
                    _ => String::new(),
                };
                let col = if c.id == ColumnId::Name { fg } else { fg2 };
                let room = (c.frame.w as f32 - CELL_LEADING - CELL_TRAILING) * scale;
                // kinds are cut at the tail, other cells in the middle (approximate, a screenshot)
                let text = match c.id {
                    ColumnId::Name => text,
                    ColumnId::Kind => truncate_tail(&mut self.text, &text, px, room),
                    _ => truncate_middle(&mut self.text, &text, px, room),
                };
                // sizes are right-aligned in their column (approximate, a screenshot)
                let x = match c.id {
                    ColumnId::Name => row.text_x as f32 * scale,
                    ColumnId::Size => {
                        (c.frame.right() as f32 - CELL_TRAILING) * scale
                            - self.text.measure(&text, px, 400, 0.0)
                    }
                    _ => (c.frame.x as f32 + CELL_LEADING) * scale,
                };
                self.text.draw(&mut pm, &text, x, base, px, 400, 0.0, col);
                col_x = c.frame.right();
            }
            let _ = col_x;
        }
        // header
        if top < lm::HEADER_HEIGHT {
            self.paint_list_header(&mut pm, &l, sort, size.w, header_up, scale, pal);
        }
        let _ = im::ICON_SIZE_DEFAULT;
        pm
    }

    /// The list view's header of `width` points at the top of `pm`, `up` whole device pixels higher.
    #[allow(clippy::too_many_arguments)]
    fn paint_list_header(
        &mut self,
        pm: &mut Pixmap,
        l: &list::ListLayout,
        sort: SortSpec,
        width: f64,
        up: f32,
        scale: f32,
        pal: &Palette,
    ) {
        fill_rect_up(
            pm,
            &Rect::new(0.0, 0.0, width, lm::HEADER_HEIGHT),
            scale,
            up,
            pal.header,
        );
        fill_rect_up(
            pm,
            &Rect::new(0.0, lm::HEADER_HEIGHT - 1.0, width, 1.0),
            scale,
            up,
            pal.separator,
        );
        for c in &l.columns {
            let title = match c.id {
                ColumnId::Name => "Name",
                ColumnId::DateModified => "Date Modified",
                ColumnId::DateCreated => "Date Created",
                ColumnId::Size => "Size",
                ColumnId::Kind => "Kind",
                ColumnId::Label => "Tags",
                ColumnId::Version => "Version",
                ColumnId::Comments => "Comments",
            };
            let x = if c.id == ColumnId::Name {
                12.0
            } else {
                c.frame.x + f64::from(CELL_LEADING)
            };
            self.text.draw(
                pm,
                title,
                x as f32 * scale,
                (lm::HEADER_HEIGHT as f32 / 2.0) * scale + 4.5 * scale - up,
                12.0 * scale,
                600,
                0.0,
                pal.text,
            );
            if c.id != ColumnId::Name {
                fill_rect_up(
                    pm,
                    &Rect::new(c.frame.x, 6.0, 1.0, lm::HEADER_HEIGHT - 12.0),
                    scale,
                    up,
                    pal.separator,
                );
            }
        }
        if let Some(c) = l.columns.iter().find(|c| c.id.sort_key() == sort.key) {
            paint_sort_caret(pm, &c.frame, up, sort.ascending, scale, pal.secondary_text);
        }
    }
}

/// Space between a list cell's text and the start and end of its column (columns abut). approximate: the leading
/// inset keeps the text where the template's 5 pt column spacing put it.
const CELL_LEADING: f32 = 5.0;
const CELL_TRAILING: f32 = 4.0;
/// The sort caret: a chevron `SORT_CARET.0` wide and `.1` high, its centre `SORT_CARET_INSET` from the right edge of the
/// sorted column's header, pointing up for ascending order. approximate (screenshot; the header cell's indicator
/// glyph and its geometry are unknown).
const SORT_CARET: (f32, f32) = (8.0, 4.0);
const SORT_CARET_INSET: f32 = 14.0;

/// Draw the sort caret into the header cell `column` (header coordinates, points), `up` whole device pixels higher.
fn paint_sort_caret(
    pm: &mut Pixmap,
    column: &Rect,
    up: f32,
    ascending: bool,
    scale: f32,
    color: Rgba,
) {
    let (cx, cy) = (
        (column.right() as f32 - SORT_CARET_INSET) * scale,
        (lm::HEADER_HEIGHT as f32 / 2.0) * scale - up,
    );
    let (hw, hh) = (SORT_CARET.0 / 2.0 * scale, SORT_CARET.1 / 2.0 * scale);
    let tip = if ascending { -hh } else { hh };
    let mut pb = PathBuilder::new();
    pb.move_to(cx - hw, cy - tip);
    pb.line_to(cx, cy + tip);
    pb.line_to(cx + hw, cy - tip);
    if let Some(p) = pb.finish() {
        pm.stroke_path(
            &p,
            &paint(color),
            &Stroke {
                width: 1.4 * scale,
                ..Default::default()
            },
            Transform::identity(),
            None,
        );
    }
}

pub fn human_size(bytes: u64) -> String {
    const U: [&str; 5] = ["bytes", "KB", "MB", "GB", "TB"];
    if bytes == 1 {
        return "1 byte".to_string();
    }
    if bytes < 1000 {
        return format!("{bytes} bytes");
    }
    let mut v = bytes as f64;
    let mut i = 0;
    while v >= 1000.0 && i < 4 {
        v /= 1000.0;
        i += 1;
    }
    if v >= 100.0 {
        format!("{:.0} {}", v, U[i])
    } else {
        format!("{:.1} {}", v, U[i])
    }
}

/// Month abbreviations of the list's dates (English; the design follows the locale, approximate).
const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// "Today at 09:41", "Yesterday at 18:02", else "22 Nov 2024 at 08:30" in local time: relative dates are on by default;
/// the medium date with " at " and the time is a screenshot's (approximate, the
/// format string is unknown), the same " at " after the relative words is.
pub fn relative_date(t: std::time::SystemTime, now: std::time::SystemTime) -> String {
    let (bucket, _) = crate::group::date_bucket(Some(t), now);
    let secs = match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_secs() as i64,
        Err(before) => -(before.duration().as_secs() as i64),
    };
    let tt = secs as libc::time_t;
    // SAFETY: a zeroed tm is a valid out value for localtime_r
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    unsafe {
        libc::localtime_r(&tt, &mut tm);
    }
    let hm = format!("{:02}:{:02}", tm.tm_hour, tm.tm_min);
    match bucket.as_str() {
        "Today" | "Yesterday" => format!("{bucket} at {hm}"),
        _ => format!(
            "{} {} {} at {hm}",
            tm.tm_mday,
            MONTH_ABBREVIATIONS[tm.tm_mon as usize],
            tm.tm_year + 1900
        ),
    }
}

/// Put the opaque `src` into `dst` with its top left corner at device pixel (`x`, `y`), clipped to `dst`: row copies,
/// the same pixels as drawing it over, at a fraction of the cost.
pub(crate) fn copy_opaque(dst: &mut Pixmap, src: &Pixmap, x: i32, y: i32) {
    let (dw, dh) = (dst.width() as i64, dst.height() as i64);
    let (sw, sh) = (src.width() as i64, src.height() as i64);
    let (x, y) = (i64::from(x), i64::from(y));
    let (x0, x1) = (x.max(0), (x + sw).min(dw));
    let (y0, y1) = (y.max(0), (y + sh).min(dh));
    if x0 >= x1 || y0 >= y1 {
        return;
    }
    let len = ((x1 - x0) * 4) as usize;
    let (dst_stride, src_stride) = (dw as usize * 4, sw as usize * 4);
    let (src_data, dst_data) = (src.data(), dst.data_mut());
    for row in y0..y1 {
        let d = row as usize * dst_stride + x0 as usize * 4;
        let s = (row - y) as usize * src_stride + (x0 - x) as usize * 4;
        dst_data[d..d + len].copy_from_slice(&src_data[s..s + len]);
    }
}

/// A renderer for tests. Uses the built-in assets only, so the result does not depend on the packs installed on the machine.
#[cfg(test)]
pub(crate) fn renderer() -> Renderer {
    let assets = Assets::builtin();
    let icons = FileIcons::new(
        &assets,
        cupertino_apps::MimeDatabase::load(&[]).unwrap(),
        std::collections::HashMap::new(),
    );
    Renderer::new(icons, &assets).unwrap_or_else(|e| panic!("{e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An opaque pixmap with a different colour in every pixel.
    fn opaque(w: u32, h: u32) -> Pixmap {
        let mut pm = Pixmap::new(w, h).unwrap();
        for (i, p) in pm.pixels_mut().iter_mut().enumerate() {
            let i = i as u32;
            *p = tiny_skia::PremultipliedColorU8::from_rgba(
                i as u8,
                (i >> 8) as u8,
                (i >> 16) as u8,
                255,
            )
            .unwrap();
        }
        pm
    }

    fn drawn_over(dst: &Pixmap, src: &Pixmap, x: i32, y: i32) -> Pixmap {
        let mut over = dst.clone();
        over.draw_pixmap(
            x,
            y,
            src.as_ref(),
            &tiny_skia::PixmapPaint::default(),
            Transform::identity(),
            None,
        );
        over
    }

    #[test]
    fn an_opaque_view_copied_in_by_rows_gives_the_pixels_of_drawing_it_over() {
        let mut dst = Pixmap::new(64, 48).unwrap();
        dst.fill(Color::from_rgba8(10, 20, 30, 128));
        let src = opaque(40, 30);
        // the window puts its view at a positive position; clipped at the right and bottom, or wholly outside
        for (x, y) in [(0, 0), (11, 7), (50, 40), (30, 20), (64, 0), (0, 48)] {
            let mut copied = dst.clone();
            copy_opaque(&mut copied, &src, x, y);
            let over = drawn_over(&dst, &src, x, y);
            let diff = copied
                .data()
                .iter()
                .zip(over.data())
                .enumerate()
                .find(|(_, (a, b))| a != b);
            assert!(diff.is_none(), "at ({x}, {y}): first difference {diff:?}");
        }
        // above and left of the window, the part inside is copied and nothing else
        let mut copied = dst.clone();
        copy_opaque(&mut copied, &src, -9, -4);
        let pixel = |pm: &Pixmap, x: u32, y: u32| pm.pixel(x, y).unwrap();
        assert_eq!(pixel(&copied, 0, 0), pixel(&src, 9, 4));
        assert_eq!(pixel(&copied, 30, 25), pixel(&src, 39, 29));
        assert_eq!(pixel(&copied, 31, 0), pixel(&dst, 31, 0));
        assert_eq!(pixel(&copied, 0, 26), pixel(&dst, 0, 26));
    }

    #[test]
    fn copying_a_view_in_costs_a_fraction_of_drawing_it_over_at_every_size() {
        for (w, h) in [(600, 400), (1920, 1080), (2560, 1440)] {
            let src = opaque(w, h);
            let mut dst = Pixmap::new(w, h).unwrap();
            let start = std::time::Instant::now();
            copy_opaque(&mut dst, &src, 0, 0);
            let copied = start.elapsed();
            let start = std::time::Instant::now();
            let over = drawn_over(&dst, &src, 0, 0);
            let drawn = start.elapsed();
            eprintln!("{w} x {h}: copied {copied:?}, drawn over {drawn:?}");
            assert!(
                copied * 3 < drawn,
                "{w} x {h}: copied {copied:?}, drawn over {drawn:?}"
            );
            assert!(over.data() == dst.data());
        }
    }

    /// Forty entries with names of every length, folders among them, a few selected.
    fn many_entries() -> (Vec<Entry>, Vec<RowSpec>, Vec<usize>) {
        let entries: Vec<Entry> = (0..40)
            .map(|i| {
                let name = format!("item {i:02} {}.txt", "x".repeat(i % 13));
                Entry::synthetic(&name, i % 7 == 0, 1000 + i as u64)
            })
            .collect();
        let outline = entries
            .iter()
            .map(|e| RowSpec {
                level: 0,
                expandable: e.is_dir,
                expanded: false,
            })
            .collect();
        (entries, outline, vec![3, 4, 17])
    }

    /// Rows `top..top + rows.height()` of `whole`.
    fn rows_of(whole: &Pixmap, top: u32, height: u32) -> Vec<u8> {
        let stride = whole.width() as usize * 4;
        whole.data()[top as usize * stride..(top + height) as usize * stride].to_vec()
    }

    #[test]
    fn a_band_of_a_view_has_the_pixels_of_those_rows_of_the_whole_view() {
        let (entries, outline, selected) = many_entries();
        let mut r = renderer();
        let pal = Palette::builtin(false);
        let size = Size { w: 640.0, h: 300.0 };
        for (scale, scroll) in [1.0_f32, 1.5, 2.0]
            .into_iter()
            .flat_map(|s| [0.0, 137.0, 61.3, 233.7].map(|scroll| (s, scroll)))
        {
            let rows = (size.h as f32 * scale) as u32;
            for list in [true, false] {
                let draw = |r: &mut Renderer, band: Option<Band>| {
                    if list {
                        r.list_view(
                            &entries,
                            &outline,
                            &selected,
                            true,
                            &ListViewSettings::default(),
                            SortSpec::default(),
                            size,
                            scroll,
                            band,
                            scale,
                            &pal,
                            std::time::UNIX_EPOCH,
                        )
                    } else {
                        r.icon_view(
                            &entries,
                            None,
                            &selected,
                            true,
                            &IconViewSettings::default(),
                            size,
                            scroll,
                            band,
                            scale,
                            &pal,
                        )
                    }
                };
                let whole = draw(&mut r, None);
                // the header and the rows under it, a band through the middle, the bottom rows
                for (top, height) in [
                    (0, 40),
                    (3, 17),
                    (41, 9),
                    (rows / 3, 29),
                    (rows / 2, 61),
                    (rows - 45, 45),
                    (rows - 1, 1),
                ] {
                    let band = draw(&mut r, Some(Band { top, height }));
                    assert_eq!((band.width(), band.height()), (whole.width(), height));
                    let expected = rows_of(&whole, top, height);
                    if let Some(i) = band.data().iter().zip(&expected).position(|(a, b)| a != b) {
                        let px = i / 4;
                        eprintln!(
                            "first difference at x {}, band row {}: {:?} against {:?}",
                            px % whole.width() as usize,
                            px / whole.width() as usize,
                            &band.data()[px * 4..px * 4 + 4],
                            &expected[px * 4..px * 4 + 4]
                        );
                    }
                    assert!(
                        band.data() == expected.as_slice(),
                        "{} view at {scale}x scrolled {scroll}, rows {top}..{}",
                        if list { "list" } else { "icon" },
                        top + height
                    );
                }
            }
        }
    }

    #[test]
    fn a_band_costs_its_rows_not_the_view_at_every_height() {
        let (entries, outline, selected) = many_entries();
        let mut r = renderer();
        let pal = Palette::builtin(false);
        for h in [300.0, 1080.0, 2160.0] {
            let size = Size { w: 1600.0, h };
            let draw = |r: &mut Renderer, band: Option<Band>| {
                (0..3)
                    .map(|_| {
                        let start = std::time::Instant::now();
                        let pm = r.list_view(
                            &entries,
                            &outline,
                            &selected,
                            true,
                            &ListViewSettings::default(),
                            SortSpec::default(),
                            size,
                            0.0,
                            band,
                            1.0,
                            &pal,
                            std::time::UNIX_EPOCH,
                        );
                        let took = start.elapsed();
                        r.recycle(pm);
                        took
                    })
                    .min()
                    .unwrap()
            };
            let whole = draw(&mut r, None);
            let band = draw(
                &mut r,
                Some(Band {
                    top: (h / 2.0) as u32,
                    height: 40,
                }),
            );
            eprintln!("{h} pt high: whole view {whole:?}, a 40 px band {band:?}");
            assert!(
                band * 4 < whole,
                "{h}: band {band:?} against whole {whole:?}"
            );
        }
    }

    #[test]
    fn sizes_are_decimal() {
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(12), "12 bytes");
        assert_eq!(human_size(1500), "1.5 KB");
        assert_eq!(human_size(250_000_000), "250 MB");
    }

    #[test]
    fn renders_without_panicking_and_paints_content() {
        let entries: Vec<Entry> = [
            "Documents",
            "photo.png",
            "a-very-long-name-that-has-to-wrap-and-truncate-in-the-middle.txt",
        ]
        .iter()
        .map(|n| Entry::synthetic(n, *n == "Documents", 1234))
        .collect();
        let mut r = renderer();
        let pal = Palette::builtin(false);
        let pm = r.icon_view(
            &entries,
            None,
            &[1],
            true,
            &IconViewSettings::default(),
            Size { w: 420.0, h: 240.0 },
            0.0,
            None,
            1.0,
            &pal,
        );
        assert!(pm
            .pixels()
            .iter()
            .any(|p| p.red() != 255 || p.green() != 255));
        let outline: Vec<RowSpec> = entries
            .iter()
            .map(|e| RowSpec {
                level: 0,
                expandable: e.is_dir,
                expanded: false,
            })
            .collect();
        let pm = r.list_view(
            &entries,
            &outline,
            &[0],
            true,
            &ListViewSettings::default(),
            SortSpec::default(),
            Size { w: 640.0, h: 160.0 },
            0.0,
            None,
            1.0,
            &pal,
            std::time::SystemTime::now(),
        );
        assert!(pm.pixels().iter().any(|p| p.blue() > p.red()));
    }

    #[test]
    fn middle_truncation_keeps_the_extension() {
        let mut r = renderer();
        let s = truncate_middle(
            &mut r.text,
            "some-long-document-name.final.pdf",
            12.0,
            100.0,
        );
        assert!(s.contains('…'));
        assert!(s.ends_with("pdf"), "{s}");
    }

    /// The instant of a local date and time.
    fn local(year: i32, month: i32, day: i32, hour: i32, minute: i32) -> std::time::SystemTime {
        // SAFETY: a zeroed tm is valid input once its fields are set; mktime only reads and normalises it
        let mut tm: libc::tm = unsafe { std::mem::zeroed() };
        (
            tm.tm_year,
            tm.tm_mon,
            tm.tm_mday,
            tm.tm_hour,
            tm.tm_min,
            tm.tm_isdst,
        ) = (year - 1900, month - 1, day, hour, minute, -1);
        let t = unsafe { libc::mktime(&mut tm) };
        std::time::UNIX_EPOCH + std::time::Duration::from_secs(t as u64)
    }

    #[test]
    fn dates_are_relative_words_for_the_last_two_days_and_a_medium_date_before() {
        let now = local(2026, 10, 25, 18, 0);
        assert_eq!(
            relative_date(local(2026, 10, 25, 9, 41), now),
            "Today at 09:41"
        );
        assert_eq!(
            relative_date(local(2026, 10, 24, 18, 2), now),
            "Yesterday at 18:02"
        );
        assert_eq!(
            relative_date(local(2024, 11, 22, 8, 30), now),
            "22 Nov 2024 at 08:30"
        );
        assert_eq!(
            relative_date(local(2026, 1, 5, 23, 59), now),
            "5 Jan 2026 at 23:59"
        );
    }

    #[test]
    fn the_sorted_column_has_the_caret_and_sizes_end_at_the_column_edge() {
        use crate::sort::SortKey;
        let entries = vec![Entry::synthetic("report.txt", false, 1500)];
        let outline = [RowSpec {
            level: 0,
            expandable: false,
            expanded: false,
        }];
        let mut r = renderer();
        let pal = Palette::builtin(false);
        let settings = ListViewSettings::default();
        let size = Size { w: 760.0, h: 120.0 };
        let draw = |r: &mut Renderer, key: SortKey| {
            r.list_view(
                &entries,
                &outline,
                &[],
                true,
                &settings,
                SortSpec {
                    key,
                    ascending: true,
                    ..SortSpec::default()
                },
                size,
                0.0,
                None,
                1.0,
                &pal,
                std::time::SystemTime::now(),
            )
        };
        let l = r.list_layout(&outline, &settings, size.w, 1.0);
        let column = |id: ColumnId| l.columns.iter().find(|c| c.id == id).unwrap().frame;
        // ink in a box around a point (header and row coordinates, points)
        let inked = |pm: &Pixmap, x: f64, y: f64, half: f64| {
            (((x - half) as u32)..((x + half) as u32)).any(|px| {
                (((y - half) as u32)..((y + half) as u32))
                    .any(|py| pm.pixel(px, py).unwrap().red() < 200)
            })
        };
        let (name, size_column) = (column(ColumnId::Name), column(ColumnId::Size));
        let caret_y = lm::HEADER_HEIGHT / 2.0;
        let by_name = draw(&mut r, SortKey::Name);
        assert!(
            inked(&by_name, name.right() - 14.0, caret_y, 4.0),
            "caret 14 pt inside the Name column's right edge"
        );
        assert!(!inked(&by_name, size_column.right() - 14.0, caret_y, 4.0));
        let by_size = draw(&mut r, SortKey::Size);
        assert!(inked(&by_size, size_column.right() - 14.0, caret_y, 4.0));
        // "1.5 KB" ends 4 pt before the Size column's edge and leaves the column's start blank
        let row_y = l.rows[0].frame.center().y;
        assert!(inked(&by_size, size_column.right() - 8.0, row_y, 4.0));
        assert!(!inked(&by_size, size_column.x + 4.0, row_y, 4.0));
    }

    #[test]
    fn tail_truncation_keeps_the_start() {
        let mut r = renderer();
        let s = truncate_tail(&mut r.text, "Ada's Laptop Workstation", 12.0, 80.0);
        assert!(s.starts_with("Ada") && s.ends_with('…'), "{s}");
        assert_eq!(truncate_tail(&mut r.text, "box", 12.0, 80.0), "box");
    }

    #[test]
    fn available_space_has_two_decimals_from_a_gigabyte() {
        assert_eq!(available_size(227_270_000_000), "227.27 GB");
        assert_eq!(available_size(1_500_000_000_000), "1.50 TB");
        assert_eq!(available_size(250_000_000), "250 MB");
    }

    #[test]
    fn every_symbol_a_place_shows_is_required() {
        use crate::places::PlaceKind::*;
        for kind in [
            Home,
            Desktop,
            Documents,
            Downloads,
            Music,
            Pictures,
            Videos,
            Recents,
            Applications,
            Computer,
            Trash,
        ] {
            assert!(SYMBOL_NAMES.contains(&kind.symbol()), "{}", kind.symbol());
        }
    }
}
