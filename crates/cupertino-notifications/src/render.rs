//! Banner layout and CPU rendering.
//!
//! Layout is computed in points (independent of the output scale) from the notification and a
//! [`BannerStyle`]; [`BannerRenderer::render`] rasterises it to a premultiplied RGBA [`Pixmap`]
//! at a device scale. Rendering has three layers that the host can take separately
//! ([`Layers`]): the drop shadow, the background material and the content (icon, text, buttons,
//! rim). A compositor with a GPU material pass draws the first two itself — the recipe is in
//! [`BannerStyle::material`] — and only uploads the content layer.
//!
//! Provenance of the look: colours (labels, fills, rim) are tokens of `cupertino-theme`; the
//! material recipe's fill layers come from the theme's material presets and its backdrop from the
//! verified platter recipe ([`crate::platter`]); the shadow is the platter's shadow layer; the
//! system UI font is substituted by the open font through `cupertino-decorations`' text renderer.
//! The corner radius, the outline width and the action-button metrics are as designed; every other
//! metric (padding, icon size, line counts) is approximate and configurable. Italic markup
//! is parsed but rendered upright (the text renderer has no italic face selection); links are
//! underlined.

use crate::center::BannerMeasure;
use crate::markup::Span;
use crate::notification::Notification;
use crate::platter::{PlatterMaterial, PlatterShadow, OUTLINE_WIDTH};
use crate::surface::{
    CentreHeader, ClearMenu, CENTRE_TITLE, CENTRE_TITLE_SIZE, CLEAR_BUTTON_PADDING, CLEAR_LABEL,
    MENU_LABEL_PADDING, MENU_RADIUS,
};
use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::shadow::{ShadowParams, ShadowRasterizer};
use cupertino_decorations::shape::rounded_path;
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::{CornerStyle, Point, Radii, Rect, Size};
use cupertino_icons::SystemIcons;
use cupertino_theme::recipe::{MaterialOptions, Recipe};
use cupertino_theme::{Appearance, SemanticColor, Theme, Typography};
use std::collections::HashMap;
use tiny_skia::{FillRule, Mask, Paint, Pixmap, PixmapPaint, Stroke, Transform};

pub type Rgba = [f32; 4];

/// Weight of the centre title: bold.
const CENTRE_TITLE_WEIGHT: u16 = 700;
/// Clear button label: 13 pt medium (approximate).
const CLEAR_LABEL_SIZE: f64 = 13.0;
const CLEAR_LABEL_WEIGHT: u16 = 500;
/// Clear menu labels: 13 pt regular (approximate; the design menu font is the system menu font size).
const MENU_LABEL_SIZE: f64 = 13.0;
const MENU_LABEL_WEIGHT: u16 = 400;
/// Highlight radius of a menu row: 4 pt (section 1).
const MENU_SELECTION_RADIUS: f64 = 4.0;

/// Colours resolved from the design tokens (straight RGBA).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BannerPalette {
    pub label: Rgba,
    pub secondary_label: Rgba,
    pub fill: Rgba,
    pub fill_secondary: Rgba,
    pub rim: Rgba,
    pub accent: Rgba,
    pub link: Rgba,
    /// Used instead of the material when no backdrop is available.
    pub fallback_background: Rgba,
}

impl BannerPalette {
    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Self {
        let get = |c: SemanticColor, fallback: Rgba| {
            theme
                .tokens
                .semantic(appearance, c)
                .map(|c| c.to_array())
                .unwrap_or(fallback)
        };
        let dark = appearance.is_dark();
        let window = get(
            SemanticColor::WindowBackground,
            if dark {
                [0.2, 0.2, 0.2, 1.0]
            } else {
                [0.93, 0.93, 0.93, 1.0]
            },
        );
        let sys = |c: SemanticColor| get(c, [0.5, 0.5, 0.5, 1.0]);
        Self {
            label: get(
                SemanticColor::Label,
                if dark {
                    [1.0; 4]
                } else {
                    [0.0, 0.0, 0.0, 0.85]
                },
            ),
            secondary_label: get(
                SemanticColor::SecondaryLabel,
                if dark {
                    [1.0, 1.0, 1.0, 0.55]
                } else {
                    [0.0, 0.0, 0.0, 0.5]
                },
            ),
            fill: get(SemanticColor::SystemFill, [0.5, 0.5, 0.5, 0.2]),
            fill_secondary: get(SemanticColor::SecondarySystemFill, [0.5, 0.5, 0.5, 0.16]),
            rim: get(SemanticColor::WindowRim, [1.0, 1.0, 1.0, 0.35]),
            accent: sys(SemanticColor::SystemBlue),
            link: get(SemanticColor::Link, [0.0, 0.4, 0.9, 1.0]),
            fallback_background: [window[0], window[1], window[2], 0.92],
        }
    }
}

/// Banner metrics in points. approximate unless a field says otherwise.
#[derive(Clone, Debug)]
pub struct BannerStyle {
    pub appearance: Appearance,
    /// Banner card corner radius: 16 pt, continuous (role).
    pub corner_radius: f64,
    pub corner_style: CornerStyle,
    pub padding: f64,
    pub icon_size: f64,
    pub icon_gap: f64,
    pub title_size: f64,
    pub title_weight: u16,
    pub body_size: f64,
    pub body_weight: u16,
    pub body_bold_weight: u16,
    pub title_max_lines: usize,
    pub body_max_lines: usize,
    pub line_gap: f64,
    /// Action button height: 24 pt.
    pub button_height: f64,
    /// Action button corner radius: 12 pt, continuous.
    pub button_radius: f64,
    /// Horizontal padding around an action button's label: 12 pt.
    pub button_padding: f64,
    pub button_gap: f64,
    pub button_top_gap: f64,
    pub max_buttons: usize,
    pub close_diameter: f64,
    /// Platter outline width: 0.5 pt.
    pub rim_width: f64,
    /// The platter shadow layer (see [`PlatterShadow::params`]).
    pub shadow: ShadowParams,
    pub palette: BannerPalette,
    /// Material recipe name looked up in the material presets for the fill layers (approximate choice).
    pub material_name: String,
    /// Platter material whose backdrop replaces the recipe's: `.medium` (role).
    pub platter_material: PlatterMaterial,
    pub material: Option<Recipe>,
    pub typography: Typography,
}

impl BannerStyle {
    pub const DEFAULT_MATERIAL: &'static str = "Menu";
    pub const DEFAULT_PLATTER_MATERIAL: PlatterMaterial = PlatterMaterial::Medium;

    /// Style for `appearance`. With `reduce_transparency` the opaque recipe variant is used and
    /// keeps its own (absent) backdrop.
    pub fn from_theme(theme: &Theme, appearance: Appearance, reduce_transparency: bool) -> Self {
        let opts = MaterialOptions {
            reduce_transparency,
            ..Default::default()
        };
        let dark = appearance.is_dark();
        let platter_material = Self::DEFAULT_PLATTER_MATERIAL;
        let material = theme
            .recipe(appearance, Self::DEFAULT_MATERIAL, &opts)
            .ok()
            .map(|mut r| {
                if r.backdrop.is_some() {
                    r.backdrop = Some(platter_material.backdrop(dark));
                }
                r
            });
        Self {
            appearance,
            corner_radius: 16.0,
            corner_style: CornerStyle::continuous(),
            padding: 12.0,
            icon_size: 36.0,
            icon_gap: 10.0,
            title_size: theme.tokens.typography().sizes.regular,
            title_weight: 600,
            body_size: theme.tokens.typography().sizes.regular,
            body_weight: 400,
            body_bold_weight: 700,
            title_max_lines: 1,
            body_max_lines: 3,
            line_gap: 1.0,
            button_height: 24.0,
            button_radius: 12.0,
            button_padding: 12.0,
            button_gap: 8.0,
            button_top_gap: 10.0,
            max_buttons: 3,
            close_diameter: 18.0,
            rim_width: OUTLINE_WIDTH,
            shadow: PlatterShadow::new(dark).params(),
            palette: BannerPalette::from_theme(theme, appearance),
            material_name: Self::DEFAULT_MATERIAL.into(),
            platter_material,
            material,
            typography: theme.tokens.typography().clone(),
        }
    }

    fn px(&self, size: f64) -> f32 {
        self.typography.substitute_size(size) as f32
    }

    fn tracking(&self, size: f64) -> f32 {
        self.typography.letter_spacing(size) as f32
    }

    fn line_height(&self, size: f64) -> f64 {
        self.typography
            .line_height(self.typography.substitute_size(size))
            + self.line_gap
    }

    fn baseline_in_line(&self, size: f64) -> f64 {
        let m = &self.typography.metrics;
        let px = self.typography.substitute_size(size);
        let (asc, desc) = (
            m.ascender / m.units_per_em * px,
            m.descender.abs() / m.units_per_em * px,
        );
        let lh = self.line_height(size);
        (lh - (asc + desc)) / 2.0 + asc
    }
}

/// One styled run of text on a line (x relative to the banner's left edge, points).
#[derive(Clone, Debug, PartialEq)]
pub struct Run {
    pub x: f64,
    pub text: String,
    pub bold: bool,
    pub underline: bool,
    pub link: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub baseline: f64,
    pub runs: Vec<Run>,
    pub width: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ButtonLayout {
    pub rect: Rect,
    pub key: String,
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct BannerLayout {
    pub size: Size,
    pub icon: Rect,
    pub title: Vec<Line>,
    pub body: Vec<Line>,
    pub buttons: Vec<ButtonLayout>,
    /// Close button shown while the banner is hovered (centred on the top-left corner).
    pub close: Rect,
}

#[derive(Clone, Debug, PartialEq)]
pub enum BannerHit {
    None,
    Body,
    Close,
    Button { index: usize, key: String },
    Link(String),
}

impl BannerLayout {
    /// Hit test in banner coordinates (points, origin at the banner's top-left corner).
    /// `close_visible`: the close button only exists while the banner is hovered.
    pub fn hit(&self, p: Point, close_visible: bool) -> BannerHit {
        if close_visible && self.close.contains(p) {
            return BannerHit::Close;
        }
        if !Rect::new(0.0, 0.0, self.size.width, self.size.height).contains(p) {
            return BannerHit::None;
        }
        if let Some((index, b)) = self
            .buttons
            .iter()
            .enumerate()
            .find(|(_, b)| b.rect.contains(p))
        {
            return BannerHit::Button {
                index,
                key: b.key.clone(),
            };
        }
        for line in &self.body {
            let (top, bottom) = (line.baseline - 14.0, line.baseline + 5.0);
            if p.y >= top && p.y <= bottom {
                for (i, r) in line.runs.iter().enumerate() {
                    let end = line.runs.get(i + 1).map_or(line.x_end(), |n| n.x);
                    if let (Some(href), true) = (&r.link, p.x >= r.x && p.x <= end) {
                        return BannerHit::Link(href.clone());
                    }
                }
            }
        }
        BannerHit::Body
    }
}

impl Line {
    fn x_end(&self) -> f64 {
        self.runs.first().map_or(0.0, |r| r.x) + self.width
    }
}

/// Source of the posting application's own icon pixmaps.
pub trait IconSource {
    /// The icon `name` (an `app_icon` name or a path or `file://` URI).
    fn load(&mut self, name: &str, size_px: u32) -> Option<Pixmap>;
    /// The `Icon=` of the installed desktop entry `entry` (the `desktop-entry` hint).
    fn load_for_entry(&mut self, entry: &str, size_px: u32) -> Option<Pixmap>;
}

impl IconSource for cupertino_icons::ThemeIcons {
    fn load(&mut self, name: &str, size_px: u32) -> Option<Pixmap> {
        cupertino_icons::ThemeIcons::load(self, name, size_px)
    }

    fn load_for_entry(&mut self, entry: &str, size_px: u32) -> Option<Pixmap> {
        cupertino_icons::ThemeIcons::load_for_entry(self, entry, size_px)
    }
}

/// Layers of a rendered banner.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layers {
    pub shadow: bool,
    pub background: bool,
    pub content: bool,
}

impl Layers {
    pub const ALL: Layers = Layers {
        shadow: true,
        background: true,
        content: true,
    };
    /// Icon, text, buttons and rim only, for a compositor that draws shadow and material itself.
    pub const CONTENT_ONLY: Layers = Layers {
        shadow: false,
        background: false,
        content: true,
    };
}

/// The wallpaper (or whatever is below) around a banner, in device pixels, for the CPU material.
pub struct Backdrop<'a> {
    pub image: &'a Pixmap,
    /// Position of the banner's top-left corner inside `image`.
    pub banner_origin: (u32, u32),
}

pub struct RenderOptions<'a> {
    pub scale: f32,
    pub hovered: bool,
    pub backdrop: Option<Backdrop<'a>>,
    pub layers: Layers,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            scale: 1.0,
            hovered: false,
            backdrop: None,
            layers: Layers::ALL,
        }
    }
}

pub struct BannerImage {
    pub pixmap: Pixmap,
    /// Device-pixel distance from the image edges to the banner rectangle (shadow margins).
    pub margins: [u32; 4],
}

impl BannerImage {
    /// Top-left corner of the banner rectangle inside `pixmap`.
    pub fn banner_origin(&self) -> (u32, u32) {
        (self.margins[0], self.margins[1])
    }
}

pub struct BannerRenderer {
    pub style: BannerStyle,
    text: TextRenderer,
    shadows: ShadowRasterizer,
    pub icons: Box<dyn IconSource>,
    /// The system's own icons: the generic application icon of a banner whose application has none.
    system: SystemIcons,
    cache: HashMap<(u32, u32, u64), BannerLayout>,
}

fn skia_color(c: Rgba, opacity: f32) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba(
        c[0].clamp(0.0, 1.0),
        c[1].clamp(0.0, 1.0),
        c[2].clamp(0.0, 1.0),
        (c[3] * opacity).clamp(0.0, 1.0),
    )
    .unwrap_or(tiny_skia::Color::TRANSPARENT)
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    p.set_color(skia_color(c, 1.0));
    p
}

/// The platter outline inside `rect` (device pixels): the cards, the centre header and the Clear menu share it, so
/// each panel keeps an edge against a like panel below it.
fn stroke_rim(
    pm: &mut Pixmap,
    rect: Rect,
    radius_px: f64,
    corner_style: CornerStyle,
    style: &BannerStyle,
    scale: f32,
) {
    if style.rim_width <= 0.0 {
        return;
    }
    let half = 0.5;
    let inner = Rect::new(
        rect.x() + half,
        rect.y() + half,
        rect.width() - 2.0 * half,
        rect.height() - 2.0 * half,
    );
    let Some(path) = rounded_path(inner, Radii::uniform(radius_px - half), corner_style, 1.0)
    else {
        return;
    };
    let stroke = Stroke {
        width: (style.rim_width as f32 * scale.max(1.0)).max(1.0),
        ..Stroke::default()
    };
    pm.stroke_path(
        &path,
        &paint(style.palette.rim),
        &stroke,
        Transform::identity(),
        None,
    );
}

fn fill_rounded(
    pm: &mut Pixmap,
    rect: Rect,
    radius: f64,
    style: CornerStyle,
    color: Rgba,
    scale: f64,
) {
    if let Some(path) = rounded_path(rect, Radii::uniform(radius), style, scale) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

impl BannerRenderer {
    /// Fails only when no usable font is left ([`TextRenderer::new`]); without `application.generic` in any asset pack, a
    /// banner of an application without an icon shows the placeholder.
    pub fn new(style: BannerStyle, assets: &Assets) -> Result<Self, AssetError> {
        let system = SystemIcons::required(assets, &["application.generic"], &[]);
        Ok(Self {
            style,
            text: TextRenderer::new(assets, FontRole::Ui)?,
            shadows: ShadowRasterizer::new(),
            icons: Box::new(cupertino_icons::ThemeIcons::new(assets)),
            system,
            cache: HashMap::new(),
        })
    }

    /// Name of the resolved text family.
    pub fn font_family(&self) -> Option<&str> {
        self.text.family()
    }

    /// Width in points. Layout is in points, so the optical size is that of the point size (the scale-1 request),
    /// the one `draw_text` selects at any output scale.
    fn measure(&mut self, text: &str, size: f64, weight: u16) -> f64 {
        self.text.set_scale(1.0);
        let (px, ls) = (self.style.px(size), self.style.tracking(size));
        self.text.measure(text, px, weight, ls) as f64
    }

    fn text_fit(&mut self, text: &str, size: f64, weight: u16, max: f64) -> (String, f64) {
        let (px, ls) = (self.style.px(size), self.style.tracking(size));
        self.text.set_scale(1.0);
        let (s, w) = self.text.fit(text, px, weight, ls, max as f32);
        (s, w as f64)
    }

    /// Greedy word wrap of styled spans into at most `max_lines` lines of `width`; the last line is
    /// ellipsised when text remains. Returns lines of `(runs, width)` with x relative to `x0`.
    #[allow(clippy::too_many_arguments)]
    fn wrap(
        &mut self,
        spans: &[Span],
        size: f64,
        weight: u16,
        bold_weight: u16,
        x0: f64,
        width: f64,
        max_lines: usize,
    ) -> Vec<(Vec<Run>, f64)> {
        struct Word {
            text: String,
            bold: bool,
            underline: bool,
            link: Option<String>,
            space_before: bool,
            break_before: bool,
        }
        let mut words: Vec<Word> = Vec::new();
        let mut carry_space = false;
        for s in spans {
            for (li, line) in s.text.split('\n').enumerate() {
                let mut first = true;
                let leading_space =
                    line.starts_with(char::is_whitespace) || (li == 0 && carry_space);
                for w in line.split_whitespace() {
                    words.push(Word {
                        text: w.to_string(),
                        bold: s.bold,
                        underline: s.underline,
                        link: s.link.clone(),
                        space_before: !first || leading_space,
                        break_before: li > 0 && first,
                    });
                    first = false;
                }
                if li > 0 && line.trim().is_empty() {
                    words.push(Word {
                        text: String::new(),
                        bold: false,
                        underline: false,
                        link: None,
                        space_before: false,
                        break_before: true,
                    });
                }
            }
            carry_space = s.text.ends_with(char::is_whitespace);
        }
        let space_w = self.measure("a a", size, weight) - self.measure("aa", size, weight);
        let mut lines: Vec<(Vec<Run>, f64)> = vec![(Vec::new(), 0.0)];
        let mut truncated = false;
        'outer: for w in words {
            if w.text.is_empty() {
                if lines.len() >= max_lines {
                    truncated = true;
                    break;
                }
                lines.push((Vec::new(), 0.0));
                continue;
            }
            let wt = if w.bold { bold_weight } else { weight };
            let ww = self.measure(&w.text, size, wt);
            loop {
                let (runs, used) = lines.last().expect("a line");
                let gap = if !runs.is_empty() && w.space_before {
                    space_w
                } else {
                    0.0
                };
                let force_break = w.break_before && !runs.is_empty();
                if !force_break && used + gap + ww <= width + 0.01 {
                    let line = lines.last_mut().expect("a line");
                    let x = x0 + line.1 + gap;
                    line.0.push(Run {
                        x,
                        text: w.text.clone(),
                        bold: w.bold,
                        underline: w.underline,
                        link: w.link.clone(),
                    });
                    line.1 += gap + ww;
                    break;
                }
                if runs.is_empty() {
                    let (t, tw) = self.text_fit(&w.text, size, wt, width);
                    let line = lines.last_mut().expect("a line");
                    line.0.push(Run {
                        x: x0,
                        text: t,
                        bold: w.bold,
                        underline: w.underline,
                        link: w.link.clone(),
                    });
                    line.1 = tw;
                    truncated = true;
                    continue 'outer;
                }
                if lines.len() >= max_lines {
                    truncated = true;
                    break 'outer;
                }
                lines.push((Vec::new(), 0.0));
            }
        }
        if truncated {
            if let Some((runs, used)) = lines.last_mut() {
                while let Some(last) = runs.last_mut() {
                    let wt = if last.bold { bold_weight } else { weight };
                    let avail = width - (last.x - x0);
                    let candidate = format!("{}…", last.text.trim_end_matches('…'));
                    let (fit, fw) = self.text_fit(&candidate, size, wt, avail);
                    if !fit.is_empty() {
                        last.text = fit;
                        *used = last.x - x0 + fw;
                        break;
                    }
                    runs.pop();
                }
                if runs.is_empty() {
                    *used = 0.0;
                }
            }
        }
        lines.retain(|(r, _)| !r.is_empty());
        lines
    }

    /// Compute the layout of a banner `width` points wide.
    pub fn layout(&mut self, n: &Notification, width: f64) -> BannerLayout {
        let s = self.style.clone();
        let text_x = s.padding + s.icon_size + s.icon_gap;
        let text_w = (width - text_x - s.padding).max(20.0);
        let mut y = s.padding;
        let mut title = Vec::new();
        let mut body = Vec::new();
        let title_spans = [Span {
            text: n.summary.replace('\n', " "),
            ..Default::default()
        }];
        let title_lines = if n.summary.trim().is_empty() {
            Vec::new()
        } else {
            self.wrap(
                &title_spans,
                s.title_size,
                s.title_weight,
                s.title_weight,
                text_x,
                text_w,
                s.title_max_lines,
            )
        };
        for (runs, w) in title_lines {
            let runs: Vec<Run> = runs.into_iter().map(|r| Run { bold: true, ..r }).collect();
            title.push(Line {
                baseline: y + s.baseline_in_line(s.title_size),
                runs,
                width: w,
            });
            y += s.line_height(s.title_size);
        }
        let body_lines = self.wrap(
            &n.spans,
            s.body_size,
            s.body_weight,
            s.body_bold_weight,
            text_x,
            text_w,
            s.body_max_lines,
        );
        for (runs, w) in body_lines {
            body.push(Line {
                baseline: y + s.baseline_in_line(s.body_size),
                runs,
                width: w,
            });
            y += s.line_height(s.body_size);
        }
        let text_h = y - s.padding;
        let mut bottom = s.padding + text_h.max(s.icon_size);
        let mut buttons = Vec::new();
        let actions: Vec<_> = n.button_actions().take(s.max_buttons).collect();
        if !actions.is_empty() {
            let top = bottom + s.button_top_gap;
            let k = actions.len() as f64;
            let bw = (width - 2.0 * s.padding - (k - 1.0) * s.button_gap) / k;
            for (i, a) in actions.iter().enumerate() {
                let (label, _) =
                    self.text_fit(&a.label, s.body_size, 500, bw - 2.0 * s.button_padding);
                buttons.push(ButtonLayout {
                    rect: Rect::new(
                        s.padding + i as f64 * (bw + s.button_gap),
                        top,
                        bw,
                        s.button_height,
                    ),
                    key: a.key.clone(),
                    label,
                });
            }
            bottom = top + s.button_height;
        }
        let height = bottom + s.padding;
        let d = s.close_diameter;
        BannerLayout {
            size: Size::new(width, height),
            icon: Rect::new(s.padding, s.padding, s.icon_size, s.icon_size),
            title,
            body,
            buttons,
            close: Rect::new(-d / 3.0, -d / 3.0, d, d),
        }
    }

    pub fn layout_cached(&mut self, n: &Notification, width: f64) -> &BannerLayout {
        if self.cache.len() > 256 {
            self.cache.clear();
        }
        let key = (n.id, n.revision, width.to_bits());
        if !self.cache.contains_key(&key) {
            let l = self.layout(n, width);
            self.cache.insert(key, l);
        }
        &self.cache[&key]
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_text(
        &mut self,
        pm: &mut Pixmap,
        text: &str,
        x: f64,
        baseline: f64,
        size: f64,
        weight: u16,
        color: Rgba,
        scale: f32,
    ) {
        let px = self.style.px(size) * scale;
        let ls = self.style.tracking(size) * scale;
        self.text.set_scale(scale);
        self.text.draw(
            pm,
            text,
            x as f32 * scale,
            baseline as f32 * scale,
            px,
            weight,
            ls,
            color,
        );
    }

    /// Baseline that centres a line of `size` points in a box `height` points tall (cap height
    /// about 0.72 of the size; approximate).
    fn centred_baseline(&self, size: f64, height: f64) -> f64 {
        height / 2.0 + 0.36 * self.style.px(size) as f64
    }

    /// Width of the Clear button: label plus the padding of the banner action buttons.
    pub fn clear_button_width(&mut self) -> f64 {
        self.measure(CLEAR_LABEL, CLEAR_LABEL_SIZE, CLEAR_LABEL_WEIGHT) + 2.0 * CLEAR_BUTTON_PADDING
    }

    /// Width of the widest label of the Clear menu.
    pub fn widest_label(&mut self, labels: &[&str]) -> f64 {
        labels
            .iter()
            .map(|l| self.measure(l, MENU_LABEL_SIZE, MENU_LABEL_WEIGHT))
            .fold(0.0, f64::max)
    }

    /// Header row of the open list at `scale`, on its material panel with the card rim: the title (bold 22) at the
    /// card padding from the left edge and the
    /// Clear button at the right edge. The pixmap covers `header.row`.
    pub fn render_header(
        &mut self,
        header: &CentreHeader,
        button_hovered: bool,
        scale: f32,
    ) -> Pixmap {
        let sc = scale as f64;
        let row = header.row;
        let mut pm = Pixmap::new(
            (row.width() * sc).ceil() as u32,
            (row.height() * sc).ceil() as u32,
        )
        .expect("non-empty header");
        let label = self.style.palette.label;
        let baseline = self.centred_baseline(CENTRE_TITLE_SIZE, row.height());
        let edge = Rect::new(0.0, 0.0, f64::from(pm.width()), f64::from(pm.height()));
        stroke_rim(
            &mut pm,
            edge,
            self.style.corner_radius * sc,
            self.style.corner_style,
            &self.style,
            scale,
        );
        self.draw_text(
            &mut pm,
            CENTRE_TITLE,
            self.style.padding,
            baseline,
            CENTRE_TITLE_SIZE,
            CENTRE_TITLE_WEIGHT,
            label,
            scale,
        );
        let button = Rect::new(
            header.clear.x() - row.x(),
            header.clear.y() - row.y(),
            header.clear.width(),
            header.clear.height(),
        );
        let fill = if button_hovered {
            self.style.palette.fill
        } else {
            self.style.palette.fill_secondary
        };
        fill_rounded(
            &mut pm,
            button,
            button.height() / 2.0,
            CornerStyle::continuous(),
            fill,
            sc,
        );
        let baseline = button.y() + self.centred_baseline(CLEAR_LABEL_SIZE, button.height());
        self.draw_text(
            &mut pm,
            CLEAR_LABEL,
            button.x() + CLEAR_BUTTON_PADDING,
            baseline,
            CLEAR_LABEL_SIZE,
            CLEAR_LABEL_WEIGHT,
            label,
            scale,
        );
        pm
    }

    /// The Clear menu at `scale`: one label per row, the row under the pointer highlighted with a
    /// 4 pt radius (section 1). The material comes from a backdrop
    /// the host places under `menu.panel`; the pixmap covers the panel.
    pub fn render_clear_menu(
        &mut self,
        menu: &ClearMenu,
        labels: &[&str],
        hovered: Option<usize>,
        scale: f32,
    ) -> Pixmap {
        let sc = scale as f64;
        let panel = menu.panel;
        let mut pm = Pixmap::new(
            (panel.width() * sc).ceil() as u32,
            (panel.height() * sc).ceil() as u32,
        )
        .expect("non-empty menu");
        let (fill, label) = (self.style.palette.fill, self.style.palette.label);
        // opaque enough that the cards under the panel do not show through the backdrop's blur
        fill_rounded(
            &mut pm,
            Rect::new(0.0, 0.0, panel.width(), panel.height()),
            MENU_RADIUS,
            CornerStyle::continuous(),
            self.style.palette.fallback_background,
            sc,
        );
        let edge = Rect::new(0.0, 0.0, f64::from(pm.width()), f64::from(pm.height()));
        stroke_rim(
            &mut pm,
            edge,
            MENU_RADIUS * sc,
            CornerStyle::continuous(),
            &self.style,
            scale,
        );
        for (i, (row, text)) in menu.rows.iter().zip(labels).enumerate() {
            let local = Rect::new(
                row.x() - panel.x(),
                row.y() - panel.y(),
                row.width(),
                row.height(),
            );
            if hovered == Some(i) {
                fill_rounded(
                    &mut pm,
                    local,
                    MENU_SELECTION_RADIUS,
                    CornerStyle::continuous(),
                    fill,
                    sc,
                );
            }
            let baseline = local.y() + self.centred_baseline(MENU_LABEL_SIZE, local.height());
            self.draw_text(
                &mut pm,
                text,
                local.x() + MENU_LABEL_PADDING,
                baseline,
                MENU_LABEL_SIZE,
                MENU_LABEL_WEIGHT,
                label,
                scale,
            );
        }
        pm
    }

    /// The banner's icon in a rounded square: the posted image data, then `image-path`, then the application's own
    /// icon (`app_icon`, else the `Icon=` of its `desktop-entry`), else the generic application icon of the system.
    fn draw_icon(&mut self, pm: &mut Pixmap, n: &Notification, rect: Rect, scale: f32) {
        let size_px = (rect.width() as f32 * scale).round().max(1.0) as u32;
        let own = n
            .image
            .as_ref()
            .and_then(|i| i.to_pixmap())
            .or_else(|| {
                n.image_path
                    .as_deref()
                    .and_then(|p| self.icons.load(p, size_px))
            })
            .or_else(|| self.icons.load(&n.icon, size_px))
            .or_else(|| {
                n.desktop_entry
                    .as_deref()
                    .and_then(|e| self.icons.load_for_entry(e, size_px))
            });
        let src = match own {
            Some(own) => own,
            None => self.system.raster("application.generic", size_px).clone(),
        };
        let r = rect.scale(scale as f64);
        let radius = r.width() * 0.225;
        let Some(path) = rounded_path(r, Radii::uniform(radius), CornerStyle::continuous(), 1.0)
        else {
            return;
        };
        let Some(mut mask) = Mask::new(pm.width(), pm.height()) else {
            return;
        };
        mask.fill_path(&path, FillRule::Winding, true, Transform::identity());
        let sx = r.width() as f32 / src.width() as f32;
        let sy = r.height() as f32 / src.height() as f32;
        let ts = Transform::from_row(sx, 0.0, 0.0, sy, r.x() as f32, r.y() as f32);
        let pp = PixmapPaint {
            quality: tiny_skia::FilterQuality::Bicubic,
            ..PixmapPaint::default()
        };
        pm.draw_pixmap(0, 0, src.as_ref(), &pp, ts, Some(&mask));
    }

    fn background(
        &self,
        pm: &mut Pixmap,
        origin: (u32, u32),
        size_px: (u32, u32),
        radius_px: f64,
        backdrop: Option<&Backdrop>,
        scale: f32,
    ) {
        let rect = Rect::new(
            origin.0 as f64,
            origin.1 as f64,
            size_px.0 as f64,
            size_px.1 as f64,
        );
        let Some(shape) = rounded_path(
            rect,
            Radii::uniform(radius_px),
            self.style.corner_style,
            1.0,
        ) else {
            return;
        };
        #[cfg(feature = "material")]
        if let (Some(b), Some(recipe)) = (backdrop, self.style.material.as_ref()) {
            use cupertino_glass::{cpu::render_regions, MaterialRegion};
            let (w, h) = (b.image.width(), b.image.height());
            let mut wall = image::RgbaImage::new(w, h);
            for (dst, src) in wall.pixels_mut().zip(b.image.pixels()) {
                let c = src.demultiply();
                *dst = image::Rgba([c.red(), c.green(), c.blue(), 255]);
            }
            let region = MaterialRegion::new(
                [
                    b.banner_origin.0 as f32,
                    b.banner_origin.1 as f32,
                    size_px.0 as f32,
                    size_px.1 as f32,
                ],
                recipe,
            );
            let out = render_regions(&wall, &[region], scale, [0.5, 0.5, 0.5], false, false);
            let Some(mut mask) = Mask::new(pm.width(), pm.height()) else {
                return;
            };
            mask.fill_path(&shape, FillRule::Winding, true, Transform::identity());
            let Some(mut layer) = Pixmap::new(pm.width(), pm.height()) else {
                return;
            };
            let dx = origin.0 as i64 - b.banner_origin.0 as i64;
            let dy = origin.1 as i64 - b.banner_origin.1 as i64;
            for y in origin.1..origin.1 + size_px.1 {
                for x in origin.0..origin.0 + size_px.0 {
                    let (sx, sy) = ((x as i64 - dx) as u32, (y as i64 - dy) as u32);
                    if x >= pm.width() || y >= pm.height() || sx >= w || sy >= h {
                        continue;
                    }
                    let p = out.get_pixel(sx, sy).0;
                    layer.pixels_mut()[(y * pm.width() + x) as usize] =
                        tiny_skia::ColorU8::from_rgba(p[0], p[1], p[2], 255).premultiply();
                }
            }
            pm.draw_pixmap(
                0,
                0,
                layer.as_ref(),
                &PixmapPaint::default(),
                Transform::identity(),
                Some(&mask),
            );
            return;
        }
        let _ = (backdrop, scale);
        pm.fill_path(
            &shape,
            &paint(self.style.palette.fallback_background),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }

    /// Rasterise a banner at `opts.scale` device pixels per point.
    pub fn render(
        &mut self,
        n: &Notification,
        layout: &BannerLayout,
        opts: &RenderOptions,
    ) -> BannerImage {
        let s = self.style.clone();
        let scale = opts.scale;
        let sc = scale as f64;
        let params = s.shadow;
        let m = params.margins_px(sc);
        let margins = [m.left, m.top, m.right, m.bottom];
        let bw = (layout.size.width * sc).round() as u32;
        let bh = (layout.size.height * sc).round() as u32;
        let (w, h) = (bw + m.left + m.right, bh + m.top + m.bottom);
        let mut pm = Pixmap::new(w.max(1), h.max(1)).expect("non-empty banner");
        let origin = (m.left, m.top);
        let radius_px = s.corner_radius * sc;

        if opts.layers.shadow {
            let img =
                self.shadows
                    .render(&params, layout.size, s.corner_radius, s.corner_style, sc);
            for (dst, a) in pm.pixels_mut().iter_mut().zip(img.data.chunks_exact(4)) {
                *dst = tiny_skia::PremultipliedColorU8::from_rgba(0, 0, 0, a[3])
                    .unwrap_or(tiny_skia::PremultipliedColorU8::TRANSPARENT);
            }
            // the shadow image has the same dimensions as the pixmap by construction
            debug_assert_eq!((img.width, img.height), (w, h));
            if let Some(path) = rounded_path(
                Rect::new(origin.0 as f64, origin.1 as f64, bw as f64, bh as f64),
                Radii::uniform(radius_px),
                s.corner_style,
                1.0,
            ) {
                // keep the shadow out from under translucent materials
                let mut p = Paint {
                    blend_mode: tiny_skia::BlendMode::Clear,
                    anti_alias: true,
                    ..Paint::default()
                };
                p.set_color(tiny_skia::Color::BLACK);
                pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
            }
        }
        if opts.layers.background {
            self.background(
                &mut pm,
                origin,
                (bw, bh),
                radius_px,
                opts.backdrop.as_ref(),
                scale,
            );
        }
        if !opts.layers.content {
            return BannerImage {
                pixmap: pm,
                margins,
            };
        }

        let (ox, oy) = (origin.0 as f64 / sc, origin.1 as f64 / sc);
        let at = |p: f64, o: f64| p + o;
        let icon = layout.icon.translate(ox, oy);
        self.draw_icon(&mut pm, n, icon, scale);
        for line in layout.title.iter() {
            for r in &line.runs {
                self.draw_text(
                    &mut pm,
                    &r.text,
                    at(r.x, ox),
                    at(line.baseline, oy),
                    s.title_size,
                    s.title_weight,
                    s.palette.label,
                    scale,
                );
            }
        }
        for line in &layout.body {
            for (i, r) in line.runs.iter().enumerate() {
                let wt = if r.bold {
                    s.body_bold_weight
                } else {
                    s.body_weight
                };
                let color = if r.link.is_some() {
                    s.palette.link
                } else {
                    s.palette.label
                };
                let (x, base) = (at(r.x, ox), at(line.baseline, oy));
                self.draw_text(&mut pm, &r.text, x, base, s.body_size, wt, color, scale);
                if r.underline {
                    let end = line.runs.get(i + 1).map_or(line.x_end(), |n| n.x);
                    let wpx = self
                        .measure(&r.text, s.body_size, wt)
                        .min((end - r.x).max(1.0))
                        .max(1.0);
                    let rect = Rect::new(x * sc, (base + 1.5) * sc, wpx * sc, sc.max(1.0));
                    fill_rounded(&mut pm, rect, 0.0, CornerStyle::Circular, color, 1.0);
                }
            }
        }
        for b in &layout.buttons {
            let rect = b.rect.translate(ox, oy);
            fill_rounded(
                &mut pm,
                rect,
                s.button_radius,
                CornerStyle::continuous(),
                s.palette.fill,
                sc,
            );
            let label_w = self.measure(&b.label, s.body_size, 500);
            let x = rect.x() + (rect.width() - label_w) / 2.0;
            let base = rect.y()
                + s.baseline_in_line(s.body_size)
                + (rect.height() - s.line_height(s.body_size)) / 2.0;
            self.draw_text(
                &mut pm,
                &b.label,
                x,
                base,
                s.body_size,
                500,
                s.palette.label,
                scale,
            );
        }
        stroke_rim(
            &mut pm,
            Rect::new(origin.0 as f64, origin.1 as f64, bw as f64, bh as f64),
            radius_px,
            s.corner_style,
            &s,
            scale,
        );
        if opts.hovered {
            let c = layout.close.translate(ox, oy);
            fill_rounded(
                &mut pm,
                c,
                c.width() / 2.0,
                CornerStyle::Circular,
                [0.35, 0.35, 0.35, 0.92],
                sc,
            );
            let mut pb = tiny_skia::PathBuilder::new();
            let (cx, cy, k) = (c.center().x * sc, c.center().y * sc, c.width() * sc * 0.18);
            pb.move_to((cx - k) as f32, (cy - k) as f32);
            pb.line_to((cx + k) as f32, (cy + k) as f32);
            pb.move_to((cx + k) as f32, (cy - k) as f32);
            pb.line_to((cx - k) as f32, (cy + k) as f32);
            if let Some(path) = pb.finish() {
                let stroke = Stroke {
                    width: (1.4 * scale).max(1.0),
                    line_cap: tiny_skia::LineCap::Round,
                    ..Stroke::default()
                };
                pm.stroke_path(
                    &path,
                    &paint([1.0, 1.0, 1.0, 0.95]),
                    &stroke,
                    Transform::identity(),
                    None,
                );
            }
        }
        BannerImage {
            pixmap: pm,
            margins,
        }
    }
}

impl BannerMeasure for BannerRenderer {
    fn banner_height(&mut self, n: &Notification, width: f64) -> f64 {
        self.layout_cached(n, width).size.height
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::markup;
    use crate::notification::NotifyInput;

    /// Needs the asset directory (`CUPERTINO_ASSET_DIR`); fails with the build instructions when it is missing.
    fn renderer() -> BannerRenderer {
        let theme = Theme::load().unwrap();
        let assets = Assets::builtin();
        BannerRenderer::new(
            BannerStyle::from_theme(&theme, Appearance::Light, false),
            &assets,
        )
        .unwrap_or_else(|e| panic!("{e}"))
    }

    fn note(summary: &str, body: &str) -> Notification {
        Notification::from_input(1, NotifyInput::new("Mail", summary, body), 0.0)
    }

    fn ink_columns_in(pm: &Pixmap, x0: u32, x1: u32, rows: std::ops::Range<u32>) -> usize {
        (x0..x1)
            .filter(|&x| {
                rows.clone()
                    .any(|y| pm.pixels()[(y * pm.width() + x) as usize].alpha() > 0)
            })
            .count()
    }

    #[test]
    fn header_insets_the_title_by_the_card_padding_and_the_clear_button_by_6_pt() {
        let mut r = renderer();
        let w = r.clear_button_width();
        let header = crate::surface::centre_header(
            &crate::center::StackLayout::default(),
            Size::new(376.0, 800.0),
            w,
        );
        let pm = r.render_header(&header, false, 2.0);
        assert_eq!((pm.width(), pm.height()), (688, 72));
        // at 2x, between the 32 px corners, where the rim is only the panel's left and right edges
        let (rows, button) = (32..40, (w * 2.0) as u32);
        let ink = |x0: u32, x1: u32| ink_columns_in(&pm, x0, x1, rows.clone());
        assert_eq!(ink(2, 24), 0, "12 pt free left of the title");
        assert!(ink(24, 300) > 100, "title");
        assert_eq!(ink(480, 676 - button - 2), 0, "gap");
        assert!(
            ink(676 - button, 676) as f64 > f64::from(button) * 0.9,
            "button"
        );
        assert_eq!(ink(678, 686), 0, "6 pt free right of the button");
        assert!(
            ink(0, 2) > 0 && ink(686, 688) > 0,
            "the rim edges the panel"
        );
    }

    #[test]
    fn the_clear_menu_has_the_card_rim() {
        let mut r = renderer();
        let header = crate::surface::centre_header(
            &crate::center::StackLayout::default(),
            Size::new(376.0, 800.0),
            60.0,
        );
        let menu = crate::surface::clear_menu(header.clear, 1, 120.0);
        let pm = r.render_clear_menu(&menu, &["All Notifications"], None, 1.0);
        let (mid, w) = (pm.height() / 2, pm.width());
        let at = |x: u32| pm.pixels()[(mid * w + x) as usize];
        assert_ne!(at(0), at(3), "the edge differs from the panel inside it");
    }

    #[test]
    fn only_the_hovered_menu_row_is_highlighted() {
        let mut r = renderer();
        let header = crate::surface::centre_header(
            &crate::center::StackLayout::default(),
            Size::new(376.0, 800.0),
            60.0,
        );
        let menu = crate::surface::clear_menu(
            header.clear,
            2,
            r.widest_label(&["Banners Only", "All Notifications"]),
        );
        let labels = ["Banners Only", "All Notifications"];
        let pm = r.render_clear_menu(&menu, &labels, Some(1), 1.0);
        let at = |row: usize| {
            let y = (menu.rows[row].center().y - menu.panel.y()) as u32;
            // the 5 pt edge padding, left of the label: highlight fill only
            pm.pixels()[(y * pm.width() + 8) as usize].alpha()
        };
        let (plain, lit) = (at(0), at(1));
        assert!(lit > plain, "hovered row {lit} vs plain {plain}");
    }

    #[test]
    fn palette_matches_tokens() {
        let theme = Theme::load().unwrap();
        let p = BannerPalette::from_theme(&theme, Appearance::Dark);
        let label = theme
            .tokens
            .semantic(Appearance::Dark, SemanticColor::Label)
            .unwrap();
        assert_eq!(p.label, label.to_array());
        let p = BannerPalette::from_theme(&theme, Appearance::Light);
        assert_eq!(
            p.link,
            theme
                .tokens
                .semantic(Appearance::Light, SemanticColor::Link)
                .unwrap()
                .to_array()
        );
    }

    #[test]
    fn style_resolves_a_material_recipe() {
        let theme = Theme::load().unwrap();
        let s = BannerStyle::from_theme(&theme, Appearance::Light, false);
        let r = s.material.expect("recipe for the Menu material");
        assert_eq!(r.backdrop, Some(PlatterMaterial::Medium.backdrop(false)));
        assert!(
            !r.fills.is_empty(),
            "fill layers of the Menu recipe are kept"
        );
        assert_eq!(
            (s.rim_width, s.shadow.density(), s.shadow.vertical_offset()),
            (0.5, 0.20, 0.0)
        );
        assert_eq!(s.corner_radius, 16.0);
        let opaque = BannerStyle::from_theme(&theme, Appearance::Light, true)
            .material
            .unwrap();
        assert!(opaque.opaque);
    }

    #[test]
    fn layout_wraps_and_limits_lines() {
        let mut r = renderer();
        let long = "word ".repeat(200);
        let l = r.layout(&note("Title", &long), 360.0);
        assert_eq!(l.body.len(), 3);
        assert_eq!(l.title.len(), 1);
        let last = l.body.last().unwrap();
        assert!(
            last.runs.last().unwrap().text.ends_with('…'),
            "ellipsis on the last line: {:?}",
            last.runs.last()
        );
        let text_x = 12.0 + 36.0 + 10.0;
        for line in &l.body {
            assert!(
                line.width <= 360.0 - text_x - 12.0 + 0.5,
                "line {} fits",
                line.width
            );
        }
        let short = r.layout(&note("Title", "one line"), 360.0);
        assert_eq!(short.body.len(), 1);
        assert!(short.size.height < l.size.height);
        assert!(
            short.size.height >= 36.0 + 24.0,
            "at least icon plus padding"
        );
    }

    #[test]
    fn explicit_newlines_break_lines() {
        let mut r = renderer();
        let l = r.layout(&note("T", "first\nsecond"), 360.0);
        assert_eq!(l.body.len(), 2);
        assert!(l.body[1].baseline > l.body[0].baseline);
    }

    #[test]
    fn buttons_are_laid_out_in_one_row_and_hit_testable() {
        let mut r = renderer();
        let n = Notification::from_input(
            1,
            NotifyInput::new("Mail", "T", "b")
                .with_action("default", "Open")
                .with_action("reply", "Reply")
                .with_action("later", "Later"),
            0.0,
        );
        let l = r.layout(&n, 360.0);
        assert_eq!(l.buttons.len(), 2, "default action has no button");
        assert_eq!(l.buttons[0].rect.y(), l.buttons[1].rect.y());
        assert_eq!(l.buttons[0].rect.height(), 24.0);
        assert!(l.buttons[1].rect.x() > l.buttons[0].rect.max_x());
        assert!(l.buttons[1].rect.max_x() <= 360.0 - 12.0 + 0.01);
        assert_eq!(
            l.hit(l.buttons[1].rect.center(), false),
            BannerHit::Button {
                index: 1,
                key: "later".into()
            }
        );
        assert_eq!(
            l.hit(Point::new(5.0, l.size.height / 2.0), false),
            BannerHit::Body
        );
        assert_eq!(l.hit(Point::new(-50.0, -50.0), false), BannerHit::None);
        assert_eq!(l.hit(l.close.center(), true), BannerHit::Close);
        assert_ne!(l.hit(l.close.center(), false), BannerHit::Close);
    }

    #[test]
    fn space_between_styled_spans_is_kept() {
        let mut r = renderer();
        let l = r.layout(
            &note("T", "see <b>bold</b> and <a href=\"x\">link</a>"),
            360.0,
        );
        let runs = &l.body[0].runs;
        assert_eq!(runs.len(), 4);
        let w = |r: &mut BannerRenderer, t: &str, b: bool| {
            r.measure(t, 13.0, if b { 700 } else { 400 })
        };
        let gap = runs[1].x - (runs[0].x + w(&mut r, "see", false));
        assert!(gap > 1.5, "gap {gap}");
        let gap2 = runs[3].x - (runs[2].x + w(&mut r, "and", false));
        assert!(gap2 > 1.5, "gap {gap2}");
    }

    #[test]
    fn links_are_hit_testable() {
        let mut r = renderer();
        let l = r.layout(
            &note("T", "go <a href=\"https://example.org\">there</a> now"),
            360.0,
        );
        let run = l.body[0]
            .runs
            .iter()
            .find(|r| r.link.is_some())
            .expect("link run");
        let p = Point::new(run.x + 3.0, l.body[0].baseline - 4.0);
        assert_eq!(
            l.hit(p, false),
            BannerHit::Link("https://example.org".into())
        );
        assert_eq!(
            markup::strip("go <a href=\"https://example.org\">there</a> now"),
            "go there now"
        );
    }

    #[test]
    fn render_draws_shadow_background_and_text() {
        let mut r = renderer();
        let n = note(
            "Quarterly report",
            "The numbers are in and they look better than expected.",
        );
        let l = r.layout(&n, 360.0);
        let img = r.render(
            &n,
            &l,
            &RenderOptions {
                scale: 2.0,
                ..Default::default()
            },
        );
        let (ox, oy) = img.banner_origin();
        assert_eq!(
            img.pixmap.width(),
            (360.0 * 2.0) as u32 + img.margins[0] + img.margins[2]
        );
        let px = |x: u32, y: u32| img.pixmap.pixels()[(y * img.pixmap.width() + x) as usize];
        let inside = px(ox + 400, oy + 6);
        assert!(
            inside.alpha() > 200,
            "background is nearly opaque: {}",
            inside.alpha()
        );
        assert_eq!(px(0, 0).alpha(), 0, "far corner has no shadow");
        let below = px(ox + 300, oy + (l.size.height * 2.0) as u32 + 4);
        assert!(below.alpha() > 0, "shadow below the banner");
        let text_pixels = img
            .pixmap
            .pixels()
            .iter()
            .filter(|p| p.alpha() > 0 && p.red() < 90 && p.green() < 90)
            .count();
        assert!(text_pixels > 200, "dark text pixels drawn: {text_pixels}");
    }

    #[test]
    fn content_only_layer_has_no_background() {
        let mut r = renderer();
        let n = note("T", "b");
        let l = r.layout(&n, 360.0);
        let img = r.render(
            &n,
            &l,
            &RenderOptions {
                layers: Layers::CONTENT_ONLY,
                ..Default::default()
            },
        );
        let (ox, oy) = img.banner_origin();
        assert_eq!(
            img.pixmap.pixels()[((oy + 40) * img.pixmap.width() + ox + 300) as usize].alpha(),
            0
        );
        assert!(
            img.pixmap.pixels().iter().any(|p| p.alpha() > 0),
            "icon and text exist"
        );
    }

    #[test]
    fn measure_trait_caches_layouts() {
        let mut r = renderer();
        let n = note("T", "body");
        let h1 = r.banner_height(&n, 360.0);
        let h2 = r.banner_height(&n, 360.0);
        assert_eq!(h1, h2);
        assert!(h1 > 40.0);
    }

    #[test]
    fn an_application_without_an_icon_shows_the_same_generic_icon_whatever_its_name() {
        let mut r = renderer();
        let (mut a, mut b) = (note("T", "b"), note("T", "b"));
        (a.app_name, b.app_name) = ("Mail".into(), "Terminal".into());
        let layout = r.layout(&a, 360.0);
        let (first, second) = (
            r.render(&a, &layout, &RenderOptions::default()),
            r.render(&b, &layout, &RenderOptions::default()),
        );
        assert_eq!(first.pixmap.data(), second.pixmap.data());
        let icon = layout.icon;
        let centre = first
            .pixmap
            .pixel(
                (icon.x() + icon.width() / 2.0) as u32,
                (icon.y() + icon.height() / 2.0) as u32,
            )
            .unwrap();
        assert!(centre.alpha() > 0, "the generic application icon is drawn");
    }
}
