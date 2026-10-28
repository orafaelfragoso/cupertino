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
use cupertino_geometry::{CornerStyle, Radii, Rect as GRect};
use cupertino_text_field::TextField;
use cupertino_theme::Typography;
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

    /// Draw the bar into `pm` (the bar surface at `scale` device pixels per point): the highlight
    /// of the open or keyboard-focused title, the labels, the status icons, the control panel
    /// glyph and the clock.
    #[allow(clippy::too_many_arguments)]
    pub fn paint_bar(
        &self,
        text: &BarText,
        pm: &mut Pixmap,
        layout: &BarLayout,
        content: &BarContent,
        highlight: Option<&SlotKey>,
        shared: &Shared,
        scale: f64,
    ) {
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
                    self.palette.bar_highlight,
                );
            }
            match &slot.key {
                SlotKey::System => draw_system_mark(pm, r, scale, self.color(1.0)),
                SlotKey::AppMenu => text.draw(
                    pm,
                    &content.app_name,
                    r.x() + self.bar.title_padding,
                    r.y(),
                    r.height(),
                    scale,
                    true,
                    self.color(1.0),
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
                            self.color(1.0),
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
                            &paint(self.palette.screen_capture),
                            FillRule::Winding,
                            Transform::identity(),
                            None,
                        );
                    }
                }
                SlotKey::Status(id) => {
                    if let Some(icon) = shared.status_icon(id) {
                        self.draw_status(pm, icon, r, scale);
                    }
                }
                SlotKey::ControlPanel => draw_control_panel(pm, r, scale, self.color(1.0)),
                SlotKey::Clock => text.draw(
                    pm,
                    &content.clock,
                    r.x() + self.bar.clock_padding,
                    r.y(),
                    r.height(),
                    scale,
                    false,
                    self.color(1.0),
                ),
            }
        }
    }

    /// The icon centred in the slot, scaled down to [`STATUS_ICON_PT`]; a dot when the item has none.
    fn draw_status(&self, pm: &mut Pixmap, icon: Option<&Pixmap>, r: GRect, scale: f64) {
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
                        &paint(self.color(0.7)),
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
        field: Option<&TextField>,
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
        field: &TextField,
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
