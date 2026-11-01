//! Text measurement and CPU drawing of the bar and its popups.
//!
//! The bar and the popups sit on material backdrops the server draws; this module draws what is
//! on them (labels, marks, highlights) into transparent pixmaps. Colours and metrics not given
//! by the material recipes are approximate.

use crate::fallback::HELP_SEARCH;
use crate::feed::{Shared, SCREEN_CAPTURE_SLOT};
use crate::geom::Rect;
use crate::item::{ShortcutGlyphs, Toggle};
use crate::layout::{
    BarContent, BarLayout, BarMetrics, MenuMetrics, PopupLayout, SlotKey, TextMeasure,
};
use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::text::TextRenderer;
use cupertino_defaults::accessibility::DisplayOptions;
use cupertino_geometry::{CornerStyle, Radii, Rect as GRect};
use cupertino_theme::Typography;
use cupertino_ui_text::TextBuffer;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke,
    Transform,
};

/// Corner radius of a popup menu in points. approximate.
pub const POPUP_RADIUS: f64 = 8.0;
/// Edge of a status item icon in points. approximate.
pub const STATUS_ICON_PT: f64 = 16.0;
/// Edge of the system menu's mark in points: the grid it is drawn on, so its strokes fall on whole pixels.
const SYSTEM_MARK_PT: f64 = 16.0;
/// Opacity factor of a row's shortcut and of its secondary text. approximate.
const FADED_ALPHA: f32 = 0.65;
/// Inset of the search field's box in its row, vertically, in points. approximate.
const FIELD_INSET_Y: f64 = 2.0;
/// Corner radius of the search field's box in points. approximate.
const FIELD_RADIUS: f64 = 5.0;
/// Space between the search field's box and its text in points. approximate.
const FIELD_TEXT_INSET: f64 = 6.0;
/// Space between the search field's box and the chord after it in points. approximate.
const FIELD_CHORD_GAP: f64 = 8.0;
/// Width of the search field's caret in points. approximate.
const CARET_WIDTH: f64 = 1.0;
/// Opacity factor of the row highlight colour behind the field's selected text. approximate.
const FIELD_SELECTION_ALPHA: f32 = 0.35;

/// Colours of the bar and popup contents. approximate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuPalette {
    pub text: Rgba,
    pub disabled_alpha: f32,
    /// Fill behind the open title in the bar.
    pub bar_highlight: Rgba,
    /// Fill of the highlighted popup row and the text on it.
    pub row_highlight: Rgba,
    pub row_highlight_text: Rgba,
    pub separator: Rgba,
    /// The screen capture indicator: indigo, (99, 97, 241) dark and (84, 82, 204) light.
    pub screen_capture: Rgba,
}

/// Diameter of the privacy indicator dot attached to a menu-bar item (6 pt).
pub const PRIVACY_DOT_PT: f64 = 6.0;

impl MenuPalette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                text: [0.97, 0.97, 0.98, 0.95],
                disabled_alpha: 0.4,
                bar_highlight: [1.0, 1.0, 1.0, 0.2],
                row_highlight: [0.04, 0.45, 1.0, 1.0],
                row_highlight_text: [1.0, 1.0, 1.0, 1.0],
                separator: [1.0, 1.0, 1.0, 0.16],
                screen_capture: [99.0 / 255.0, 97.0 / 255.0, 241.0 / 255.0, 1.0],
            }
        } else {
            Self {
                text: [0.05, 0.05, 0.06, 0.92],
                disabled_alpha: 0.4,
                bar_highlight: [0.0, 0.0, 0.0, 0.13],
                row_highlight: [0.04, 0.45, 1.0, 1.0],
                row_highlight_text: [1.0, 1.0, 1.0, 1.0],
                separator: [0.0, 0.0, 0.0, 0.13],
                screen_capture: [84.0 / 255.0, 82.0 / 255.0, 204.0 / 255.0, 1.0],
            }
        }
    }
}

/// Lightness of the wallpaper under the bar (CIE L*/100 in thousandths) at and above which the bar's text is dark.
/// approximate.
pub const DARK_TEXT_FROM: u16 = 550;
/// Lightness of the wallpaper under the bar at and below which the bar's text is light. approximate.
pub const LIGHT_TEXT_UNTIL: u16 = 450;
/// Where a tone chosen afresh splits the band between [`LIGHT_TEXT_UNTIL`] and [`DARK_TEXT_FROM`]. approximate.
const FRESH_SPLIT: u16 = 500;

/// Contrast the bar's text is brought to over a mid-tone wallpaper by the scrim under it (WCAG 4.5:1 for normal text,
/// with a margin for 8-bit rounding and the strip mean). approximate.
const SCRIM_CONTRAST: f64 = 4.6;
/// The strongest scrim; the band's edges need at most 0.175. approximate.
const SCRIM_MAX_ALPHA: f64 = 0.25;

/// The text of a bar's strip: the appearance's, or dark or light for the wallpaper under the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarTone {
    /// The palette of the appearance: before the server sent the wallpaper's lightness (always below shell version
    /// 14), and while Increase Contrast or Reduce Transparency makes the bar solid.
    #[default]
    Appearance,
    DarkText,
    LightText,
}

impl BarTone {
    /// The tone after `self` for the wallpaper's `lightness`. Inside the band between [`LIGHT_TEXT_UNTIL`] and
    /// [`DARK_TEXT_FROM`] a dark or light tone holds; from [`BarTone::Appearance`] the band splits at its middle.
    pub fn next(self, lightness: Option<u16>, display: DisplayOptions) -> BarTone {
        let Some(lightness) = lightness else {
            return BarTone::Appearance;
        };
        if display.increase_contrast || display.reduce_transparency {
            return BarTone::Appearance;
        }
        let dark_from = match self {
            BarTone::Appearance => FRESH_SPLIT,
            BarTone::DarkText => LIGHT_TEXT_UNTIL + 1,
            BarTone::LightText => DARK_TEXT_FROM,
        };
        if lightness >= dark_from {
            BarTone::DarkText
        } else {
            BarTone::LightText
        }
    }

    /// The tone chosen with nothing held over: for the first lightness, and after the appearance or the display
    /// options changed.
    pub fn fresh(lightness: Option<u16>, display: DisplayOptions) -> BarTone {
        BarTone::Appearance.next(lightness, display)
    }

    /// The palette of the bar's strip over the palette of the appearance: its text, and the highlight of the open
    /// title with it, dark or light; the screen capture indicator keeps the appearance's indigo.
    pub fn palette(self, appearance: MenuPalette) -> MenuPalette {
        let text = match self {
            BarTone::Appearance => return appearance,
            BarTone::DarkText => MenuPalette::new(false),
            BarTone::LightText => MenuPalette::new(true),
        };
        MenuPalette {
            screen_capture: appearance.screen_capture,
            ..text
        }
    }

    /// Whether the bar's strip has the light palette's text over the palette of the appearance, so it draws the
    /// light symbols of the extras.
    pub fn light_text(self, appearance: MenuPalette) -> bool {
        self.palette(appearance).text == MenuPalette::new(true).text
    }

    /// The fill under the bar's text over a wallpaper of `lightness`: black under light text and white under dark
    /// text, at the smallest alpha that brings the text to [`SCRIM_CONTRAST`]; none where the text reaches it alone,
    /// and none for the appearance's tone (unmeasured, or a solid bar).
    pub fn scrim(self, lightness: Option<u16>) -> Option<Rgba> {
        let (text, scrim) = match self {
            BarTone::Appearance => return None,
            BarTone::DarkText => (MenuPalette::new(false).text, 1.0),
            BarTone::LightText => (MenuPalette::new(true).text, 0.0),
        };
        let strip = srgb_grey(lightness?);
        let contrast = |alpha: f64| {
            let backdrop = alpha * scrim + (1.0 - alpha) * strip;
            text_contrast(text, [backdrop; 3])
        };
        if contrast(0.0) >= SCRIM_CONTRAST {
            return None;
        }
        let (mut low, mut high) = (0.0, SCRIM_MAX_ALPHA);
        for _ in 0..20 {
            let mid = (low + high) / 2.0;
            if contrast(mid) >= SCRIM_CONTRAST {
                high = mid;
            } else {
                low = mid;
            }
        }
        let c = scrim as f32;
        Some([c, c, c, high as f32])
    }
}

/// The sRGB grey of `lightness` (CIE L*/100 in thousandths).
fn srgb_grey(lightness: u16) -> f64 {
    let l = f64::from(lightness.min(1000)) / 10.0;
    let y = if l > 8.0 {
        ((l + 16.0) / 116.0).powi(3)
    } else {
        l / 903.3
    };
    if y <= 0.003_130_8 {
        12.92 * y
    } else {
        1.055 * y.powf(1.0 / 2.4) - 0.055
    }
}

/// The WCAG contrast of `text` drawn source-over in sRGB on the opaque `backdrop`.
fn text_contrast(text: Rgba, backdrop: [f64; 3]) -> f64 {
    let a = f64::from(text[3]);
    let over = [0, 1, 2].map(|i| a * f64::from(text[i]) + (1.0 - a) * backdrop[i]);
    let (t, b) = (relative_luminance(over), relative_luminance(backdrop));
    (t.max(b) + 0.05) / (t.min(b) + 0.05)
}

fn relative_luminance(rgb: [f64; 3]) -> f64 {
    let [r, g, b] = rgb.map(|c| {
        if c <= 0.040_45 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    });
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// Measures and draws bar and menu text with the `ui` font of the asset directory. Text is measured at the
/// device scale it is drawn at (glyph advances are pixel-snapped, so widths do not scale
/// linearly) and reported in points. Clones share the font system but each output sets its own
/// scale through [`BarText::for_output`].
#[derive(Clone)]
pub struct BarText {
    renderer: Rc<RefCell<TextRenderer>>,
    scale: Rc<Cell<f64>>,
    font_pt: f32,
    regular_weight: u16,
    bold_weight: u16,
    /// Cap height in em.
    cap_height: f32,
}

impl BarText {
    /// Bar text in the theme's menu bar styles: regular items and the bold application name.
    pub fn new(typography: &Typography, assets: &Assets) -> Result<Self, AssetError> {
        let (regular, bold) = (typography.menu_bar_item(), typography.menu_bar_app_name());
        let renderer = TextRenderer::new(assets, FontRole::Ui)?;
        Ok(Self {
            regular_weight: renderer.effective_weight(regular.weight),
            bold_weight: renderer.effective_weight(bold.weight),
            renderer: Rc::new(RefCell::new(renderer)),
            scale: Rc::new(Cell::new(1.0)),
            font_pt: regular.size as f32,
            cap_height: (typography.metrics.cap_height / typography.metrics.units_per_em) as f32,
        })
    }

    /// The same text with a scale of its own.
    pub fn for_output(&self) -> Self {
        Self {
            scale: Rc::new(Cell::new(1.0)),
            ..self.clone()
        }
    }

    /// Device pixels per point that [`TextMeasure::width`] measures at.
    pub fn set_scale(&self, scale: f64) {
        self.scale.set(scale);
    }

    fn weight(&self, bold: bool) -> u16 {
        if bold {
            self.bold_weight
        } else {
            self.regular_weight
        }
    }

    /// Draw `text` with its left edge at `x_pt`, centred on the cap height in the band that starts
    /// at `band_top` and is `band_h` tall (points), into a pixmap at `scale` device pixels per point.
    #[allow(clippy::too_many_arguments)]
    fn draw(
        &self,
        pm: &mut Pixmap,
        text: &str,
        x_pt: f64,
        band_top: f64,
        band_h: f64,
        scale: f64,
        bold: bool,
        color: Rgba,
    ) {
        let px = self.font_pt * scale as f32;
        let baseline = (((band_top + band_h / 2.0) * scale) as f32) + self.cap_height * px / 2.0;
        let mut renderer = self.renderer.borrow_mut();
        renderer.set_scale(scale as f32);
        renderer.draw(
            pm,
            text,
            (x_pt * scale) as f32,
            baseline,
            px,
            self.weight(bold),
            0.0,
            color,
        );
    }
}

impl TextMeasure for BarText {
    fn width(&self, text: &str, bold: bool) -> f64 {
        let scale = self.scale.get().max(0.5);
        let mut renderer = self.renderer.borrow_mut();
        renderer.set_scale(scale as f32);
        f64::from(renderer.measure(text, self.font_pt * scale as f32, self.weight(bold), 0.0))
            / scale
    }
}

/// Colours and metrics the bar and popups are drawn with.
pub struct Style {
    pub palette: MenuPalette,
    pub bar: BarMetrics,
    pub menu: MenuMetrics,
}

impl Style {
    fn color(&self, alpha: f32) -> Rgba {
        let t = self.palette.text;
        [t[0], t[1], t[2], t[3] * alpha]
    }

    /// Draw the bar into `pm` (the bar surface at `scale` device pixels per point): the `scrim` over the whole
    /// strip, then the highlight of the open or keyboard-focused title, the labels, the status icons, the control
    /// panel glyph and the clock, in the palette of the bar's `tone`.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_bar(
        &self,
        text: &BarText,
        pm: &mut Pixmap,
        layout: &BarLayout,
        content: &BarContent,
        highlight: Option<&SlotKey>,
        tone: BarTone,
        scrim: Option<Rgba>,
        shared: &Shared,
        scale: f64,
    ) {
        fill_scrim(pm, scrim);
        let palette = tone.palette(self.palette);
        for slot in &layout.slots {
            let r = GRect::new(slot.rect.x, slot.rect.y, slot.rect.w, slot.rect.h);
            if highlight == Some(&slot.key) {
                let m = &self.bar;
                let hl = GRect::new(
                    r.x(),
                    r.y() + m.selection_inset_y,
                    r.width(),
                    r.height() - 2.0 * m.selection_inset_y,
                );
                fill_rounded(
                    pm,
                    hl,
                    m.selection_corner_radius,
                    scale,
                    palette.bar_highlight,
                );
            }
            match &slot.key {
                SlotKey::System => draw_system_mark(pm, r, scale, palette.text),
                SlotKey::AppMenu => text.draw(
                    pm,
                    &content.app_name,
                    r.x() + self.bar.title_padding,
                    r.y(),
                    r.height(),
                    scale,
                    true,
                    palette.text,
                ),
                SlotKey::Menu(id) => {
                    if let Some(m) = content.menus.iter().find(|m| m.id == *id) {
                        text.draw(
                            pm,
                            &m.label,
                            r.x() + self.bar.title_padding,
                            r.y(),
                            r.height(),
                            scale,
                            false,
                            palette.text,
                        );
                    }
                }
                SlotKey::Status(id) if id == SCREEN_CAPTURE_SLOT => {
                    let (cx, cy) = (r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
                    if let Some(path) = PathBuilder::from_circle(
                        (cx * scale) as f32,
                        (cy * scale) as f32,
                        (PRIVACY_DOT_PT / 2.0 * scale) as f32,
                    ) {
                        pm.fill_path(
                            &path,
                            &paint(palette.screen_capture),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                SlotKey::Status(id) => {
                    if let Some(icon) = shared.status_icon(id, tone.light_text(self.palette)) {
                        let t = palette.text;
                        self.draw_status(pm, icon, r, scale, [t[0], t[1], t[2], t[3] * 0.7]);
                    }
                }
                SlotKey::ControlPanel => draw_control_panel(pm, r, scale, palette.text),
                SlotKey::Clock => text.draw(
                    pm,
                    &content.clock,
                    r.x() + self.bar.clock_padding,
                    r.y(),
                    r.height(),
                    scale,
                    false,
                    palette.text,
                ),
            }
        }
    }

    /// The icon centred in the slot, scaled down to [`STATUS_ICON_PT`]; a dot of `dot` when the item has none.
    fn draw_status(&self, pm: &mut Pixmap, icon: Option<&Pixmap>, r: GRect, scale: f64, dot: Rgba) {
        let side = STATUS_ICON_PT;
        let (cx, cy) = (r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
        match icon {
            Some(icon) => {
                let k = ((side * scale) as f32 / icon.width().max(icon.height()) as f32)
                    .min(1.0 + 1e-3);
                let (dw, dh) = (icon.width() as f32 * k, icon.height() as f32 * k);
                let (dx, dy) = (
                    (cx * scale) as f32 - dw / 2.0,
                    (cy * scale) as f32 - dh / 2.0,
                );
                let paint = PixmapPaint {
                    quality: FilterQuality::Bilinear,
                    ..PixmapPaint::default()
                };
                pm.draw_pixmap(
                    0,
                    0,
                    icon.as_ref(),
                    &paint,
                    Transform::from_row(k, 0.0, 0.0, k, dx, dy),
                    None,
                );
            }
            None => {
                let radius = (side * 0.3) as f32 * scale as f32;
                if let Some(path) =
                    PathBuilder::from_circle((cx * scale) as f32, (cy * scale) as f32, radius)
                {
                    pm.fill_path(
                        &path,
                        &paint(dot),
                        FillRule::Winding,
                        Transform::identity(),
                        None,
                    );
                }
            }
        }
    }

    /// Draw one popup level into `pm`, which covers the popup surface whose top-left corner is
    /// `origin` in the coordinates of `layout`. `field` is the Help menu's search field, drawn in its row.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_popup(
        &self,
        text: &BarText,
        pm: &mut Pixmap,
        layout: &PopupLayout,
        selected: Option<usize>,
        field: Option<&TextBuffer>,
        origin: (f64, f64),
        scale: f64,
    ) {
        let m = &self.menu;
        let any_mark = layout.rows.iter().any(|r| r.item.toggle != Toggle::None);
        let any_icon = layout
            .rows
            .iter()
            .any(|r| r.item.icon_name.is_some() || r.item.icon_data.is_some());
        for (i, row) in layout.rows.iter().enumerate() {
            let rr = GRect::new(
                row.rect.x - origin.0,
                row.rect.y - origin.1,
                row.rect.w,
                row.rect.h,
            );
            let item = &row.item;
            if let Some(field) = field.filter(|_| item.id == HELP_SEARCH) {
                let chord_left = row.shortcut.as_ref().map(|sc| {
                    self.paint_shortcut(
                        text,
                        pm,
                        rr,
                        sc,
                        layout.shortcut_key_width,
                        self.color(FADED_ALPHA),
                        scale,
                    )
                });
                self.paint_field(text, pm, rr, field, &item.label, chord_left, scale);
                continue;
            }
            if item.separator {
                let y = rr.y() + rr.height() / 2.0;
                let sep = GRect::new(
                    rr.x() + m.padding_x,
                    y - 0.5,
                    rr.width() - 2.0 * m.padding_x,
                    1.0 / scale.max(1.0),
                );
                fill_rounded(pm, sep, 0.0, scale, self.palette.separator);
                continue;
            }
            let hot = selected == Some(i) && item.enabled;
            if hot {
                let hl = GRect::new(rr.x() + 4.0, rr.y(), rr.width() - 8.0, rr.height());
                fill_rounded(pm, hl, 4.0, scale, self.palette.row_highlight);
            }
            let base = if hot {
                self.palette.row_highlight_text
            } else {
                self.palette.text
            };
            let alpha = if item.enabled {
                1.0
            } else {
                self.palette.disabled_alpha
            };
            let color = [base[0], base[1], base[2], base[3] * alpha];
            let mark_x = rr.x() + m.padding_x;
            if item.toggle.is_marked() {
                match item.toggle {
                    Toggle::Radio(_) => draw_dot(
                        pm,
                        mark_x + m.mark_column * 0.35,
                        rr.y() + rr.height() / 2.0,
                        scale,
                        color,
                    ),
                    _ => draw_check(
                        pm,
                        mark_x + m.mark_column * 0.3,
                        rr.y() + rr.height() / 2.0,
                        scale,
                        color,
                    ),
                }
            }
            let text_x = mark_x
                + if any_mark { m.mark_column } else { 0.0 }
                + if any_icon { m.icon_column } else { 0.0 };
            text.draw(
                pm,
                &item.label,
                text_x,
                rr.y(),
                rr.height(),
                scale,
                false,
                color,
            );
            if let Some(detail) = &row.detail {
                text.draw(
                    pm,
                    detail,
                    text_x + text.width(&item.label, false) + m.detail_gap,
                    rr.y(),
                    rr.height(),
                    scale,
                    false,
                    [color[0], color[1], color[2], color[3] * FADED_ALPHA],
                );
            }
            if let Some(sc) = &row.shortcut {
                let rr = if item.has_submenu {
                    GRect::new(rr.x(), rr.y(), rr.width() - m.arrow_width, rr.height())
                } else {
                    rr
                };
                let faded = [color[0], color[1], color[2], color[3] * FADED_ALPHA];
                self.paint_shortcut(text, pm, rr, sc, layout.shortcut_key_width, faded, scale);
            }
            if item.has_submenu {
                draw_chevron(
                    pm,
                    rr.x() + rr.width() - m.padding_x - m.arrow_width * 0.35,
                    rr.y() + rr.height() / 2.0,
                    scale,
                    color,
                );
            }
        }
    }

    /// The shortcut `sc` at the right end of the row `rr`: the keys in a column `key_width` wide, the modifier glyphs
    /// right-aligned just before it. Returns where the modifier glyphs begin.
    #[allow(clippy::too_many_arguments)]
    fn paint_shortcut(
        &self,
        text: &BarText,
        pm: &mut Pixmap,
        rr: GRect,
        sc: &ShortcutGlyphs,
        key_width: f64,
        color: Rgba,
        scale: f64,
    ) -> f64 {
        let key_x = rr.x() + rr.width() - self.menu.padding_x - key_width;
        text.draw(pm, &sc.key, key_x, rr.y(), rr.height(), scale, false, color);
        let modifiers_x = key_x - text.width(&sc.modifiers, false);
        text.draw(
            pm,
            &sc.modifiers,
            modifiers_x,
            rr.y(),
            rr.height(),
            scale,
            false,
            color,
        );
        modifiers_x
    }

    /// The search field in its row `rr`: a box with the text, its selection and a steady caret, or the dimmed
    /// `placeholder` while it is empty. The box ends before `chord_left`, where the row's chord begins.
    #[allow(clippy::too_many_arguments)]
    fn paint_field(
        &self,
        text: &BarText,
        pm: &mut Pixmap,
        rr: GRect,
        field: &TextBuffer,
        placeholder: &str,
        chord_left: Option<f64>,
        scale: f64,
    ) {
        let m = &self.menu;
        let left = rr.x() + m.padding_x;
        let right = chord_left.map_or(rr.x() + rr.width() - m.padding_x, |x| x - FIELD_CHORD_GAP);
        let frame = GRect::new(
            left,
            rr.y() + FIELD_INSET_Y,
            right - left,
            rr.height() - 2.0 * FIELD_INSET_Y,
        );
        fill_rounded(pm, frame, FIELD_RADIUS, scale, self.palette.separator);
        let x = frame.x() + FIELD_TEXT_INSET;
        if field.is_empty() {
            let dimmed = self.color(self.palette.disabled_alpha);
            text.draw(
                pm,
                placeholder,
                x,
                rr.y(),
                rr.height(),
                scale,
                false,
                dimmed,
            );
        }
        let offset = |at: usize| x + text.width(&field.text()[..at], false);
        let selection = field.selection();
        let (y, h) = (
            frame.y() + FIELD_INSET_Y,
            frame.height() - 2.0 * FIELD_INSET_Y,
        );
        if selection.is_empty() {
            let caret = GRect::new(offset(field.caret()), y, CARET_WIDTH, h);
            fill_rounded(pm, caret, 0.0, scale, self.color(1.0));
        } else {
            let (start, end) = (offset(selection.start), offset(selection.end));
            let h_color = self.palette.row_highlight;
            let tint = [
                h_color[0],
                h_color[1],
                h_color[2],
                h_color[3] * FIELD_SELECTION_ALPHA,
            ];
            fill_rounded(pm, GRect::new(start, y, end - start, h), 0.0, scale, tint);
        }
        text.draw(
            pm,
            field.text(),
            x,
            rr.y(),
            rr.height(),
            scale,
            false,
            self.color(1.0),
        );
    }
}

/// The bar's scrim over the whole surface, under everything drawn after it.
fn fill_scrim(pm: &mut Pixmap, scrim: Option<Rgba>) {
    if let Some(c) = scrim {
        pm.fill(
            tiny_skia::Color::from_rgba(c[0], c[1], c[2], c[3]).expect("scrim components in 0..=1"),
        );
    }
}

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    p.set_color(
        tiny_skia::Color::from_rgba(
            c[0].clamp(0.0, 1.0),
            c[1].clamp(0.0, 1.0),
            c[2].clamp(0.0, 1.0),
            c[3].clamp(0.0, 1.0),
        )
        .expect("components clamped to 0..=1"),
    );
    p
}

fn fill_rounded(pm: &mut Pixmap, r: GRect, radius: f64, scale: f64, color: Rgba) {
    let path = if radius > 0.0 {
        cupertino_decorations::shape::rounded_path(
            r,
            Radii::uniform(radius),
            CornerStyle::continuous(),
            scale,
        )
    } else {
        let mut pb = PathBuilder::new();
        let rect = tiny_skia::Rect::from_xywh(
            (r.x() * scale) as f32,
            (r.y() * scale) as f32,
            (r.width() * scale).max(1.0) as f32,
            (r.height() * scale).max(1.0) as f32,
        );
        pb.push_rect(rect.expect("finite rectangle with a positive size"));
        pb.finish()
    };
    if let Some(path) = path {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn stroke(pm: &mut Pixmap, path: &tiny_skia::Path, scale: f64, width_pt: f64, color: Rgba) {
    let s = Stroke {
        width: (width_pt * scale) as f32,
        line_cap: LineCap::Round,
        line_join: LineJoin::Round,
        ..Stroke::default()
    };
    pm.stroke_path(path, &paint(color), &s, Transform::identity(), None);
}

fn draw_check(pm: &mut Pixmap, x: f64, y: f64, scale: f64, color: Rgba) {
    let mut pb = PathBuilder::new();
    let (x, y, s) = ((x * scale) as f32, (y * scale) as f32, scale as f32);
    pb.move_to(x - 4.0 * s, y);
    pb.line_to(x - 1.2 * s, y + 3.2 * s);
    pb.line_to(x + 4.2 * s, y - 3.8 * s);
    if let Some(p) = pb.finish() {
        stroke(pm, &p, scale, 1.6, color);
    }
}

fn draw_dot(pm: &mut Pixmap, x: f64, y: f64, scale: f64, color: Rgba) {
    if let Some(p) =
        PathBuilder::from_circle((x * scale) as f32, (y * scale) as f32, 2.4 * scale as f32)
    {
        pm.fill_path(
            &p,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

fn draw_chevron(pm: &mut Pixmap, x: f64, y: f64, scale: f64, color: Rgba) {
    let mut pb = PathBuilder::new();
    let (x, y, s) = ((x * scale) as f32, (y * scale) as f32, scale as f32);
    pb.move_to(x - 1.8 * s, y - 3.6 * s);
    pb.line_to(x + 1.8 * s, y);
    pb.line_to(x - 1.8 * s, y + 3.6 * s);
    if let Some(p) = pb.finish() {
        stroke(pm, &p, scale, 1.4, color);
    }
}

/// Two small switches, the glyph of the control panel slot.
fn draw_control_panel(pm: &mut Pixmap, r: GRect, scale: f64, color: Rgba) {
    let (cx, cy) = (r.x() + r.width() / 2.0, r.y() + r.height() / 2.0);
    for (dy, knob) in [(-3.2, 1.8), (3.2, -1.8)] {
        let track = GRect::new(cx - 6.0, cy + dy - 2.2, 12.0, 4.4);
        fill_rounded(
            pm,
            track,
            2.2,
            scale,
            [color[0], color[1], color[2], color[3] * 0.45],
        );
        if let Some(p) = PathBuilder::from_circle(
            ((cx + knob * 2.0) * scale) as f32,
            ((cy + dy) * scale) as f32,
            2.6 * scale as f32,
        ) {
            pm.fill_path(
                &p,
                &paint(color),
                FillRule::Winding,
                Transform::identity(),
                None,
            );
        }
    }
}

/// The system menu's mark, centred in the slot on whole device pixels so its strokes stay sharp.
fn draw_system_mark(pm: &mut Pixmap, r: GRect, scale: f64, color: Rgba) {
    let px = (SYSTEM_MARK_PT * scale).round() as u32;
    let mark = cupertino_icons::system_mark(px, color);
    let x = ((r.x() + r.width() / 2.0) * scale - f64::from(mark.width()) / 2.0).round() as i32;
    let y = ((r.y() + r.height() / 2.0) * scale - f64::from(mark.height()) / 2.0).round() as i32;
    pm.draw_pixmap(
        x,
        y,
        mark.as_ref(),
        &PixmapPaint::default(),
        Transform::identity(),
        None,
    );
}

/// `Rect` of the geometry module in the units of the drawing code.
pub fn to_geometry(r: Rect) -> GRect {
    GRect::new(r.x, r.y, r.w, r.h)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSLUCENT: DisplayOptions = DisplayOptions {
        reduce_motion: false,
        reduce_transparency: false,
        increase_contrast: false,
        differentiate_without_color: false,
    };

    /// The tones of a bar told `values` in turn, from the appearance's.
    fn tones(values: &[u16]) -> Vec<BarTone> {
        values
            .iter()
            .scan(BarTone::Appearance, |tone, &l| {
                *tone = tone.next(Some(l), TRANSLUCENT);
                Some(*tone)
            })
            .collect()
    }

    #[test]
    fn the_tone_holds_inside_the_band_and_switches_beyond_either_end() {
        assert_eq!(
            tones(&[600, 451, 549, 450, 451, 549, 550]),
            [
                BarTone::DarkText,
                BarTone::DarkText,
                BarTone::DarkText,
                BarTone::LightText,
                BarTone::LightText,
                BarTone::LightText,
                BarTone::DarkText,
            ]
        );
    }

    #[test]
    fn the_text_follows_the_backdrop_in_either_appearance() {
        for dark in [false, true] {
            let appearance = MenuPalette::new(dark);
            let over_dark = BarTone::fresh(Some(120), TRANSLUCENT).palette(appearance);
            let over_bright = BarTone::fresh(Some(900), TRANSLUCENT).palette(appearance);
            assert_eq!(over_dark.text, MenuPalette::new(true).text, "dark {dark}");
            assert_eq!(
                over_bright.text,
                MenuPalette::new(false).text,
                "dark {dark}"
            );
            assert_eq!(
                over_bright.bar_highlight,
                MenuPalette::new(false).bar_highlight,
                "the open title's pill flips with the text"
            );
            for palette in [over_dark, over_bright] {
                assert_eq!(palette.screen_capture, appearance.screen_capture);
            }
        }
    }

    #[test]
    fn without_a_lightness_or_on_a_solid_bar_the_bar_keeps_the_appearance() {
        assert_eq!(BarTone::fresh(None, TRANSLUCENT), BarTone::Appearance);
        assert_eq!(
            BarTone::DarkText.next(None, TRANSLUCENT),
            BarTone::Appearance
        );
        for display in [
            DisplayOptions {
                increase_contrast: true,
                ..TRANSLUCENT
            },
            DisplayOptions {
                reduce_transparency: true,
                ..TRANSLUCENT
            },
        ] {
            assert_eq!(BarTone::fresh(Some(900), display), BarTone::Appearance);
            assert_eq!(
                BarTone::LightText.next(Some(100), display),
                BarTone::Appearance
            );
        }
        let light = MenuPalette::new(false);
        assert_eq!(BarTone::Appearance.palette(light), light);
    }

    #[test]
    fn the_extras_symbols_follow_the_tone_not_the_appearance() {
        for dark in [false, true] {
            let appearance = MenuPalette::new(dark);
            assert!(BarTone::LightText.light_text(appearance), "dark {dark}");
            assert!(!BarTone::DarkText.light_text(appearance), "dark {dark}");
            assert_eq!(BarTone::Appearance.light_text(appearance), dark);
        }
    }

    #[test]
    fn a_fresh_tone_splits_the_band_at_its_middle_with_nothing_held_over() {
        assert_eq!(tones(&[300, 520]).last(), Some(&BarTone::LightText));
        assert_eq!(BarTone::fresh(Some(520), TRANSLUCENT), BarTone::DarkText);
        assert_eq!(tones(&[700, 480]).last(), Some(&BarTone::DarkText));
        assert_eq!(BarTone::fresh(Some(480), TRANSLUCENT), BarTone::LightText);
        assert_eq!(BarTone::fresh(Some(500), TRANSLUCENT), BarTone::DarkText);
    }

    /// WCAG contrast of `text` source-over in sRGB on `backdrop`, written out here apart from the code under test.
    fn wcag(text: Rgba, backdrop: [f64; 3]) -> f64 {
        let lum = |c: [f64; 3]| {
            let l = |v: f64| {
                if v <= 0.040_45 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * l(c[0]) + 0.7152 * l(c[1]) + 0.0722 * l(c[2])
        };
        let a = f64::from(text[3]);
        let over = [0, 1, 2].map(|i| a * f64::from(text[i]) + (1.0 - a) * backdrop[i]);
        let (t, b) = (lum(over), lum(backdrop));
        (t.max(b) + 0.05) / (t.min(b) + 0.05)
    }

    /// The grey of CIE L* `lightness` thousandths, encoded to sRGB.
    fn grey(lightness: u16) -> f64 {
        let l = f64::from(lightness) / 10.0;
        let y = if l > 8.0 {
            ((l + 16.0) / 116.0).powi(3)
        } else {
            l / 903.3
        };
        if y <= 0.003_130_8 {
            12.92 * y
        } else {
            1.055 * y.powf(1.0 / 2.4) - 0.055
        }
    }

    /// The text of `tone` over its scrim over a strip of `strip` (sRGB).
    fn contrast_over(tone: BarTone, lightness: u16, strip: [f64; 3]) -> f64 {
        let text = tone.palette(MenuPalette::new(false)).text;
        let backdrop = match tone.scrim(Some(lightness)) {
            Some(s) => {
                strip.map(|c| f64::from(s[3]) * f64::from(s[0]) + (1.0 - f64::from(s[3])) * c)
            }
            None => strip,
        };
        wcag(text, backdrop)
    }

    /// Every tone a bar can hold at `lightness`: fresh, and kept from either text.
    fn reachable(lightness: u16) -> Vec<BarTone> {
        [BarTone::Appearance, BarTone::DarkText, BarTone::LightText]
            .map(|held| held.next(Some(lightness), TRANSLUCENT))
            .into_iter()
            .collect()
    }

    #[test]
    fn the_text_reaches_4_5_to_1_over_every_lightness_with_its_scrim() {
        for lightness in 0..=1000 {
            for tone in reachable(lightness) {
                let c = contrast_over(tone, lightness, [grey(lightness); 3]);
                assert!(c >= 4.5, "{tone:?} at {lightness}: {c:.3}");
            }
        }
    }

    #[test]
    fn there_is_no_scrim_where_the_text_reads_alone_or_on_the_appearance() {
        assert_eq!(BarTone::LightText.scrim(Some(300)), None);
        assert_eq!(BarTone::LightText.scrim(Some(450)), None);
        assert_eq!(BarTone::DarkText.scrim(Some(560)), None);
        assert_eq!(BarTone::DarkText.scrim(Some(900)), None);
        assert_eq!(BarTone::Appearance.scrim(Some(500)), None);
        assert_eq!(BarTone::LightText.scrim(None), None);
    }

    #[test]
    fn a_mid_grey_takes_a_light_scrim_under_dark_text_and_a_dark_one_under_light_text() {
        let light = BarTone::LightText.scrim(Some(500)).unwrap();
        assert_eq!(light[..3], [0.0; 3]);
        assert!((light[3] - 0.089).abs() <= 0.005, "{light:?}");
        let dark = BarTone::DarkText.scrim(Some(500)).unwrap();
        assert_eq!(dark[..3], [1.0; 3]);
        assert!((dark[3] - 0.069).abs() <= 0.005, "{dark:?}");
        let edge = BarTone::LightText.scrim(Some(549)).unwrap();
        assert!((edge[3] - 0.175).abs() <= 0.005, "{edge:?}");
    }

    #[test]
    fn the_bar_paints_its_scrim_over_the_whole_surface() {
        let theme = cupertino_theme::Theme::load().unwrap();
        let text = BarText::new(theme.tokens.typography(), &Assets::discover()).unwrap();
        let style = Style {
            palette: MenuPalette::new(false),
            bar: BarMetrics::default(),
            menu: MenuMetrics::default(),
        };
        let layout = BarLayout {
            bar: Rect::new(0.0, 0.0, 40.0, 8.0),
            slots: Vec::new(),
            hidden_menus: Vec::new(),
            hidden_status: Vec::new(),
        };
        let paint = |lightness| {
            let tone = BarTone::LightText;
            let mut pm = Pixmap::new(80, 16).unwrap();
            style.paint_bar(
                &text,
                &mut pm,
                &layout,
                &BarContent::default(),
                None,
                tone,
                tone.scrim(Some(lightness)),
                &Shared::default(),
                2.0,
            );
            pm
        };
        let scrimmed = paint(500);
        for (x, y) in [(0, 0), (79, 15), (40, 8)] {
            let px = scrimmed.pixel(x, y).unwrap();
            assert!((i32::from(px.alpha()) - 23).abs() <= 1, "{px:?}");
            assert_eq!((px.red(), px.green(), px.blue()), (0, 0, 0));
        }
        assert_eq!(paint(300).pixel(40, 8).unwrap().alpha(), 0);
    }

    /// From wallpaper grey to text: the strip through each appearance's MenuBar colour ops, the lightness the server
    /// measures of it, the tone and scrim the bar takes, and the text over scrim over that strip.
    #[test]
    fn through_the_bar_material_the_text_reaches_4_5_to_1_in_both_appearances() {
        use cupertino_theme::{Appearance, MaterialOptions, Theme};
        let theme = Theme::load().unwrap();
        for appearance in [Appearance::Light, Appearance::Dark] {
            let recipe = theme
                .recipe(appearance, "MenuBar", &MaterialOptions::default())
                .unwrap();
            let ops = cupertino_glass::ops::color_ops(&recipe.backdrop.unwrap().ops);
            for g in 0..=255u8 {
                let c = f32::from(g) / 255.0;
                let strip = cupertino_glass::cpu::apply_color_ops([c; 3], &ops)
                    .map(|v| f64::from(v.clamp(0.0, 1.0)));
                let lightness = cie_lightness(strip);
                for tone in reachable(lightness) {
                    let contrast = contrast_over(tone, lightness, strip);
                    assert!(
                        contrast >= 4.5,
                        "{appearance:?} grey {g}: {tone:?} at {lightness}: {contrast:.3}"
                    );
                }
            }
        }
    }

    /// CIE L* of an sRGB colour in thousandths, as the server reports it.
    fn cie_lightness(rgb: [f64; 3]) -> u16 {
        let l = |v: f64| {
            if v <= 0.040_45 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        let y = 0.2126 * l(rgb[0]) + 0.7152 * l(rgb[1]) + 0.0722 * l(rgb[2]);
        let lstar = if y > 216.0 / 24389.0 {
            116.0 * y.cbrt() - 16.0
        } else {
            y * 24389.0 / 27.0
        };
        (lstar * 10.0).round() as u16
    }
}
