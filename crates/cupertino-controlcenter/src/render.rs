//! CPU drawing of the panel: the shadow, the tiles, the connectivity rows, the sliders and the battery row. The card's
//! material is the server's backdrop (see [`MATERIAL`]); nothing here draws the glass.
//!
//! The surface is the card plus room for its shadow ([`surface_size`]); the card sits at [`card_origin`] in it.

use std::fmt;

use cupertino_decorations::palette::Rgba;
use cupertino_decorations::shadow::{ShadowParams, ShadowRasterizer};
use cupertino_decorations::shape::rounded_path;
use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::{CornerStyle, Point, Radii, Rect, Size};
use cupertino_icons::SystemIcons;
use cupertino_theme::window_shadow::{window_shadow, ShadowOption};
use cupertino_theme::{Appearance, SemanticColor, Theme, Typography};
use tiny_skia::{Color, FillRule, IntSize, Paint, PathBuilder, Pixmap, PixmapPaint, Stroke, Transform};

use crate::layout::{Layout, CARD_RADIUS, ROW_HEIGHT, SHADOW_INSETS, SLIDER_SIDE_INSET, TILE_RADIUS, TRACK_HEIGHT};
use crate::model::{speaker_icon, Battery, Controls, Module, Slider, TileState};

/// Material recipe the server draws behind the card. The design's material has no recipe of its
/// own in the design tokens; the menu recipe is the closest named one (approximate).
pub const MATERIAL: &str = "Menu";

/// Title font: system 12 medium.
const TITLE_SIZE: f32 = 12.0;
const TITLE_WEIGHT: u16 = 500;
/// Caption font: the footnote style, 10 pt (names the style; the size is the design's documented footnote
/// size).
const CAPTION_SIZE: f32 = 10.0;
const CAPTION_WEIGHT: u16 = 400;
/// Gap between a button and its text (approximate).
const TEXT_GAP: f64 = 8.0;
/// Space kept free at the end of a button row's text (approximate; 2 points so that "Focus Mode" in the 17 optical
/// size master, about 8 points wider than in the display master, still fits the 134 point tile).
const TEXT_END_PADDING: f64 = 2.0;
/// Side of a symbolic icon inside a 40 point button (approximate).
const BUTTON_GLYPH: f64 = 18.0;
/// Side of the icon on a compact tile and on the slider (approximate).
const TILE_GLYPH: f64 = 22.0;
const SLIDER_GLYPH: f64 = 14.0;
/// Diameter of the slider knob: about 20 points inside the 22 point track (pixel read).
const KNOB_DIAMETER: f64 = 20.0;
/// Inset of the slider fill from the track (1 pt).
const FILL_INSET: f64 = 1.0;
/// Size and weight of the battery percentage.
const BATTERY_TEXT_SIZE: f32 = 12.0;
/// Space between the battery glyph and its percentage (HStack spacing 4).
const BATTERY_GAP: f64 = 4.0;
/// Battery glyph body, in points: the fill's extent plus its 2 point inset on every side (fill rect
/// (2, 2, w, 8); the body size and the terminal are / approximate).
const BATTERY_BODY: (f64, f64) = (23.0, 12.0);
const BATTERY_FILL_INSET: f64 = 2.0;
const BATTERY_FILL_HEIGHT: f64 = 8.0;
const BATTERY_FILL_RADIUS: f64 = 1.5;
const BATTERY_BODY_RADIUS: f64 = 3.5;
const BATTERY_TERMINAL: (f64, f64) = (1.5, 4.0);

/// A colour token missing from the design tokens of an appearance.
#[derive(Debug)]
pub struct MissingColor(pub &'static str);

impl fmt::Display for MissingColor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the design tokens have no colour {}", self.0)
    }
}

impl std::error::Error for MissingColor {}

/// Colours of the content, straight RGBA.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    pub label: Rgba,
    pub secondary: Rgba,
    /// Background of a compact tile (approximate: the design's tile colours were not assigned to roles).
    pub tile: Rgba,
    /// Button fill when the toggle is off (approximate role).
    pub button_off: Rgba,
    /// Button fill when the toggle is on (approximate role).
    pub button_on: Rgba,
    /// Glyph on an `on` button (approximate role: white, a value).
    pub glyph_on: Rgba,
    /// Slider track (grey 0.137 dark, 0.910 light).
    pub track: Rgba,
    /// Slider track outline, light appearance only (black 0.102).
    pub track_outline: Option<Rgba>,
    /// Slider fill and knob (white).
    pub slider_fill: Rgba,
    /// Glyph on the slider fill (approximate).
    pub slider_glyph: Rgba,
    /// Low battery (red).
    pub red: Rgba,
    /// Opacity of a disabled control (0.30 dark, 0.50 otherwise).
    pub disabled_alpha: f32,
}

impl Palette {
    /// The palette of `appearance`; a high-contrast appearance takes the slider asset's contrast track
    /// (grey 0.612 dark, 0.459 light).
    pub fn from_theme(theme: &Theme, appearance: Appearance) -> Result<Self, MissingColor> {
        let get = |color: SemanticColor, name: &'static str| theme.tokens.semantic(appearance, color).map(|c| c.to_array()).ok_or(MissingColor(name));
        let dark = appearance.is_dark();
        let track = match (dark, appearance.is_high_contrast()) {
            (true, false) => 0.137,
            (false, false) => 0.910,
            (true, true) => 0.612,
            (false, true) => 0.459,
        };
        Ok(Self {
            label: get(SemanticColor::Label, "label")?,
            secondary: get(SemanticColor::SecondaryLabel, "secondary-label")?,
            tile: get(SemanticColor::SecondarySystemFill, "secondary-system-fill")?,
            button_off: get(SemanticColor::SystemFill, "system-fill")?,
            button_on: get(SemanticColor::SystemBlue, "system-blue")?,
            glyph_on: [1.0, 1.0, 1.0, 1.0],
            track: [track, track, track, 1.0],
            track_outline: (!dark).then_some([0.0, 0.0, 0.0, 0.102]),
            slider_fill: [1.0, 1.0, 1.0, 1.0],
            slider_glyph: [0.0, 0.0, 0.0, 0.55],
            red: get(SemanticColor::SystemRed, "system-red")?,
            disabled_alpha: if dark { 0.30 } else { 0.50 },
        })
    }
}

/// Size of the layer surface in points: the largest card (with the battery row) and its shadow insets.
pub fn surface_size() -> (u32, u32) {
    let card = Layout::new(true).size();
    ((card.width + SHADOW_INSETS.horizontal()) as u32, (card.height + SHADOW_INSETS.vertical()) as u32)
}

/// Where the card's top-left corner is in the surface, in points.
pub fn card_origin() -> Point {
    Point::new(SHADOW_INSETS.left, SHADOW_INSETS.top)
}

fn with_alpha(color: Rgba, factor: f32) -> Rgba {
    [color[0], color[1], color[2], color[3] * factor]
}

fn skia(color: Rgba) -> Color {
    Color::from_rgba(color[0], color[1], color[2], color[3]).expect("theme colours are within 0 to 1")
}

fn paint(color: Rgba) -> Paint<'static> {
    let mut paint = Paint::default();
    paint.set_color(skia(color));
    paint.anti_alias = true;
    paint
}

/// A pixmap with the card's coordinates: points, relative to the card's top-left corner.
struct Canvas<'a> {
    pixmap: &'a mut Pixmap,
    scale: f32,
    origin: Point,
}

impl Canvas<'_> {
    fn px(&self, points: f64) -> f32 {
        (points * f64::from(self.scale)) as f32
    }

    fn rounded(&mut self, rect: Rect, radius: f64, color: Rgba) {
        let rect = rect.translate(self.origin.x, self.origin.y);
        if let Some(path) = rounded_path(rect, Radii::uniform(radius), CornerStyle::continuous(), f64::from(self.scale)) {
            self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
        }
    }

    fn stroke_rounded(&mut self, rect: Rect, radius: f64, width: f64, color: Rgba) {
        let rect = rect.translate(self.origin.x, self.origin.y);
        if let Some(path) = rounded_path(rect, Radii::uniform(radius), CornerStyle::Circular, f64::from(self.scale)) {
            let stroke = Stroke { width: self.px(width), ..Stroke::default() };
            self.pixmap.stroke_path(&path, &paint(color), &stroke, Transform::identity(), None);
        }
    }

    fn circle(&mut self, center: Point, diameter: f64, color: Rgba) {
        let radius = self.px(diameter / 2.0);
        let (x, y) = (self.px(center.x + self.origin.x), self.px(center.y + self.origin.y));
        if let Some(path) = PathBuilder::from_circle(x, y, radius) {
            self.pixmap.fill_path(&path, &paint(color), FillRule::Winding, Transform::identity(), None);
        }
    }

    /// Draw a glyph centred on `center`.
    fn glyph(&mut self, glyph: &Pixmap, center: Point) {
        let x = self.px(center.x + self.origin.x) - glyph.width() as f32 / 2.0;
        let y = self.px(center.y + self.origin.y) - glyph.height() as f32 / 2.0;
        self.pixmap.draw_pixmap(x.round() as i32, y.round() as i32, glyph.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }
}

/// Every symbol the panel can draw, which the renderer requires of the asset directory at startup.
pub const ICON_NAMES: [&str; 12] = [
    "symbol.wifi",
    "symbol.bluetooth",
    "symbol.file-sharing",
    "symbol.moon",
    "symbol.stage",
    "symbol.screen-mirroring",
    "symbol.sun",
    "symbol.music",
    "symbol.speaker.slash",
    "symbol.speaker.low",
    "symbol.speaker",
    "symbol.speaker.high",
];

/// Draws the panel.
pub struct Renderer {
    palette: Palette,
    typography: Typography,
    text: TextRenderer,
    icons: SystemIcons,
    shadows: ShadowRasterizer,
}

impl Renderer {
    /// A renderer with `palette`; text uses the theme's typography in the `ui` font of the asset directory.
    /// Fails only when no usable font is left ([`TextRenderer::new`]); a symbol no asset pack provides is reported and drawn as a placeholder ([`ICON_NAMES`]).
    pub fn new(palette: Palette, typography: Typography, assets: &Assets) -> Result<Self, AssetError> {
        let text = TextRenderer::new(assets, FontRole::Ui)?;
        let icons = SystemIcons::required(assets, &[], &ICON_NAMES);
        Ok(Self { palette, typography, text, icons, shadows: ShadowRasterizer::new() })
    }

    /// The panel for `controls` at `scale` device pixels per point.
    pub fn render(&mut self, controls: &Controls, scale: f32) -> Pixmap {
        self.text.set_scale(scale);
        let (width, height) = surface_size();
        let side = |points: u32| (f64::from(points) * f64::from(scale)).round() as u32;
        let mut pixmap = Pixmap::new(side(width), side(height)).expect("the surface has a size");
        let layout = Layout::new(controls.battery.is_some());
        self.draw_shadow(&mut pixmap, layout.size(), scale);
        let mut canvas = Canvas { pixmap: &mut pixmap, scale, origin: card_origin() };
        for tile in layout.tiles() {
            canvas.rounded(tile, TILE_RADIUS, self.palette.tile);
        }
        if let (Some(row), Some(battery)) = (layout.battery(), controls.battery) {
            self.draw_battery(&mut canvas, row, battery);
        }
        for module in Module::ALL {
            let frame = layout.frame(module);
            let state = controls.tile(module);
            let disabled = matches!(state, TileState::Disabled(_));
            let factor = if disabled { self.palette.disabled_alpha } else { 1.0 };
            match module {
                Module::WiFi | Module::Bluetooth | Module::FileSharing | Module::FocusMode => self.draw_button_row(&mut canvas, frame, module, controls, &state, factor),
                Module::Stage | Module::ScreenMirroring => self.draw_compact_tile(&mut canvas, frame, module, factor),
                Module::Display | Module::Sound => {
                    let slider = if module == Module::Display { Slider::Display } else { Slider::Sound };
                    self.draw_slider_tile(&mut canvas, &layout, frame, slider, controls, factor);
                }
                Module::NowPlaying => self.draw_button_row(&mut canvas, frame, module, controls, &state, factor),
            }
        }
        pixmap
    }

    /// The card's drop shadow (style 2, light: opacity 0.25, radius 8, offset 4; the selection of that style is
    /// and the radius-to-sigma conversion, sigma = radius / 2, is approximate), with the card's own shape
    /// knocked out so the material is not darkened.
    fn draw_shadow(&mut self, pixmap: &mut Pixmap, card: Size, scale: f32) {
        let mut entry = *window_shadow(ShadowOption::Panel, true, false, false);
        entry.density = 0.25;
        entry.sigma = 4.0;
        entry.vertical_offset = 4.0;
        entry.clip_top = 0.0;
        let style = CornerStyle::continuous();
        let mut image = self.shadows.render(&ShadowParams::new(entry), card, CARD_RADIUS, style, f64::from(scale));
        let frame_px = ((card.width * f64::from(scale)).round() as u32, (card.height * f64::from(scale)).round() as u32);
        image.knock_out(frame_px, CARD_RADIUS * f64::from(scale), style);
        let size = IntSize::from_wh(image.width, image.height).expect("the shadow image has a size");
        let shadow = Pixmap::from_vec(image.data, size).expect("the shadow image holds width * height pixels");
        let origin = card_origin();
        let x = (origin.x * f64::from(scale)).round() as i32 - image.margins.left as i32;
        let y = (origin.y * f64::from(scale)).round() as i32 - image.margins.top as i32;
        pixmap.draw_pixmap(x, y, shadow.as_ref(), &PixmapPaint::default(), Transform::identity(), None);
    }

    fn draw_glyph(&mut self, canvas: &mut Canvas, name: &'static str, size: f64, center: Point, color: Rgba) {
        let px = (size * f64::from(canvas.scale)).round() as u32;
        let glyph = self.icons.symbol(name, px, color);
        canvas.glyph(glyph, center);
    }

    /// Baseline for a line of `size` points centred in a band starting at `top` of height `height`.
    fn baseline(&self, top: f64, height: f64, size: f32) -> f64 {
        let metrics = &self.typography.metrics;
        let (ascent, descent) = (metrics.ascender / metrics.units_per_em * f64::from(size), metrics.descender.abs() / metrics.units_per_em * f64::from(size));
        top + (height - (ascent + descent)) / 2.0 + ascent
    }

    /// Draw `text` with its left edge at `x`, shortened with an ellipsis to fit `max_width`.
    #[allow(clippy::too_many_arguments)]
    fn draw_text(&mut self, canvas: &mut Canvas, text: &str, x: f64, baseline: f64, size: f32, weight: u16, max_width: f64, color: Rgba) {
        let scale = canvas.scale;
        let weight = self.text.effective_weight(weight);
        let (fitted, _) = self.text.fit(text, size * scale, weight, 0.0, canvas.px(max_width));
        self.text.draw(canvas.pixmap, &fitted, canvas.px(x + canvas.origin.x), canvas.px(baseline + canvas.origin.y), size * scale, weight, 0.0, color);
    }

    /// A 40 point button with a title and a caption beside it: the connectivity rows, the focus mode tile and the
    /// now playing row.
    #[allow(clippy::too_many_arguments)]
    fn draw_button_row(&mut self, canvas: &mut Canvas, frame: Rect, module: Module, controls: &Controls, state: &TileState, factor: f32) {
        let palette = self.palette;
        // the compact tiles inset their button; the connectivity rows sit flush on the card
        let inset = if frame.height() > ROW_HEIGHT { (frame.height() - ROW_HEIGHT) / 2.0 } else { 0.0 };
        let button = Rect::new(frame.x() + inset, frame.y() + inset, ROW_HEIGHT, ROW_HEIGHT);
        let on = matches!(state, TileState::On);
        canvas.circle(button.center(), ROW_HEIGHT, with_alpha(if on { palette.button_on } else { palette.button_off }, factor));
        let glyph_color = with_alpha(if on { palette.glyph_on } else { palette.label }, factor);
        self.draw_glyph(canvas, module_icon(module), BUTTON_GLYPH, button.center(), glyph_color);
        let text_x = button.max_x() + TEXT_GAP;
        let width = frame.max_x() - text_x - TEXT_END_PADDING;
        let title_height = f64::from(TITLE_SIZE) * 1.25;
        let caption_height = f64::from(CAPTION_SIZE) * 1.25;
        let top = frame.y() + (frame.height() - title_height - caption_height) / 2.0;
        let title_baseline = self.baseline(top, title_height, TITLE_SIZE);
        let caption_baseline = self.baseline(top + title_height, caption_height, CAPTION_SIZE);
        self.draw_text(canvas, module.title(), text_x, title_baseline, TITLE_SIZE, TITLE_WEIGHT, width, with_alpha(palette.label, factor));
        self.draw_text(canvas, controls.subtitle(module), text_x, caption_baseline, CAPTION_SIZE, CAPTION_WEIGHT, width, with_alpha(palette.secondary, factor));
    }

    /// A square tile with an icon over its title.
    fn draw_compact_tile(&mut self, canvas: &mut Canvas, frame: Rect, module: Module, factor: f32) {
        let palette = self.palette;
        let icon_center = Point::new(frame.center().x, frame.y() + 22.0);
        self.draw_glyph(canvas, module_icon(module), TILE_GLYPH, icon_center, with_alpha(palette.label, factor));
        let text_height = f64::from(CAPTION_SIZE) * 1.25;
        let baseline = self.baseline(frame.max_y() - 8.0 - text_height, text_height, CAPTION_SIZE);
        let width = self.text.measure(module.title(), CAPTION_SIZE * canvas.scale, self.text.effective_weight(TITLE_WEIGHT), 0.0) / canvas.scale;
        let x = frame.center().x - f64::from(width.min(frame.width() as f32 - 8.0)) / 2.0;
        self.draw_text(canvas, module.title(), x, baseline, CAPTION_SIZE, TITLE_WEIGHT, frame.width() - 8.0, with_alpha(palette.label, factor));
    }

    /// A tile with its title over a slider.
    #[allow(clippy::too_many_arguments)]
    fn draw_slider_tile(&mut self, canvas: &mut Canvas, layout: &Layout, frame: Rect, slider: Slider, controls: &Controls, factor: f32) {
        let palette = self.palette;
        let module = slider.module();
        let title_height = f64::from(TITLE_SIZE) * 1.25;
        let baseline = self.baseline(frame.y() + 8.0, title_height, TITLE_SIZE);
        self.draw_text(canvas, module.title(), frame.x() + SLIDER_SIDE_INSET, baseline, TITLE_SIZE, TITLE_WEIGHT, frame.width() - 2.0 * SLIDER_SIDE_INSET, with_alpha(palette.label, factor));
        let track = layout.track(slider);
        canvas.rounded(track, TRACK_HEIGHT / 2.0, with_alpha(palette.track, factor));
        if let Some(outline) = palette.track_outline {
            canvas.stroke_rounded(track.outset(-0.25), TRACK_HEIGHT / 2.0, 0.5, with_alpha(outline, factor));
        }
        // the fill runs from the left end to the knob's right edge, so an empty slider still shows the knob
        let fraction = controls.fraction(slider).unwrap_or(0.0);
        let travel = track.width() - TRACK_HEIGHT;
        let knob_center = Point::new(track.x() + TRACK_HEIGHT / 2.0 + fraction * travel, track.center().y);
        let fill = Rect::from_ltrb(track.x(), track.y(), knob_center.x + TRACK_HEIGHT / 2.0, track.max_y()).outset(-FILL_INSET);
        canvas.rounded(fill, fill.height() / 2.0, with_alpha(palette.slider_fill, factor));
        canvas.circle(Point::new(knob_center.x, knob_center.y + 0.5), KNOB_DIAMETER + 1.0, with_alpha([0.0, 0.0, 0.0, 0.12], factor));
        canvas.circle(knob_center, KNOB_DIAMETER, with_alpha(palette.slider_fill, factor));
        let icon = match slider {
            Slider::Display => "symbol.sun",
            Slider::Sound => controls.volume.ready().map_or("symbol.speaker.slash", speaker_icon),
        };
        let glyph_center = Point::new(track.x() + TRACK_HEIGHT / 2.0, track.center().y);
        self.draw_glyph(canvas, icon, SLIDER_GLYPH, glyph_center, with_alpha(palette.slider_glyph, factor));
    }

    /// The optional first row: the battery glyph and its percentage.
    fn draw_battery(&mut self, canvas: &mut Canvas, row: Rect, battery: Battery) {
        let palette = self.palette;
        let body = Rect::new(row.x(), row.center().y - BATTERY_BODY.1 / 2.0, BATTERY_BODY.0, BATTERY_BODY.1);
        canvas.stroke_rounded(body.outset(-0.5), BATTERY_BODY_RADIUS, 1.0, with_alpha(palette.secondary, 1.0));
        let terminal = Rect::new(body.max_x() + 1.0, row.center().y - BATTERY_TERMINAL.1 / 2.0, BATTERY_TERMINAL.0, BATTERY_TERMINAL.1);
        canvas.rounded(terminal, BATTERY_TERMINAL.0 / 2.0, palette.secondary);
        let fill = Rect::new(body.x() + BATTERY_FILL_INSET, body.y() + BATTERY_FILL_INSET, battery.fill_width(), BATTERY_FILL_HEIGHT);
        canvas.rounded(fill, BATTERY_FILL_RADIUS, if battery.is_low() { palette.red } else { palette.label });
        let text_x = terminal.max_x() + BATTERY_GAP;
        let line = f64::from(BATTERY_TEXT_SIZE) * 1.25;
        let baseline = self.baseline(row.center().y - line / 2.0, line, BATTERY_TEXT_SIZE);
        let percent = format!("{}%", battery.percent);
        self.draw_text(canvas, &percent, text_x, baseline, BATTERY_TEXT_SIZE, TITLE_WEIGHT, row.max_x() - text_x, palette.label);
        if battery.charging {
            let caption = "Charging";
            let width = f64::from(self.text.measure(caption, CAPTION_SIZE * canvas.scale, self.text.effective_weight(CAPTION_WEIGHT), 0.0) / canvas.scale);
            let caption_line = f64::from(CAPTION_SIZE) * 1.25;
            let caption_baseline = self.baseline(row.center().y - caption_line / 2.0, caption_line, CAPTION_SIZE);
            self.draw_text(canvas, caption, row.max_x() - width, caption_baseline, CAPTION_SIZE, CAPTION_WEIGHT, width + 1.0, palette.secondary);
        }
    }
}

/// The symbol of a module's button (approximate except Wi-Fi and Bluetooth, which the design names; the state shows
/// in the button's fill and glyph colour, as in the menu bar). The design's other tile symbols is not known.
fn module_icon(module: Module) -> &'static str {
    match module {
        Module::WiFi => "symbol.wifi",
        Module::Bluetooth => "symbol.bluetooth",
        Module::FileSharing => "symbol.file-sharing",
        Module::FocusMode => "symbol.moon",
        Module::Stage => "symbol.stage",
        Module::ScreenMirroring => "symbol.screen-mirroring",
        Module::Display => "symbol.sun",
        Module::Sound => "symbol.speaker.high",
        Module::NowPlaying => "symbol.music",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Reading, VolumeState, WifiStatus};

    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pixmap.pixel(x, y).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    fn controls() -> Controls {
        Controls {
            wifi: Reading::Ready(WifiStatus { enabled: true, network: None }),
            bluetooth: Reading::Ready(false),
            focus_mode: Reading::Unavailable("none".into()),
            brightness: Reading::Ready(1.0),
            volume: Reading::Ready(VolumeState { level: 0.0, muted: false }),
            battery: None,
        }
    }

    /// Needs the asset directory (`CUPERTINO_ASSET_DIR`); fails with the build instructions when it is missing.
    fn renderer(appearance: Appearance) -> Renderer {
        let theme = Theme::load().unwrap();
        let assets = Assets::discover();
        Renderer::new(Palette::from_theme(&theme, appearance).unwrap(), theme.tokens.typography().clone(), &assets).unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_surface_holds_the_tallest_card_and_its_shadow_room() {
        assert_eq!(surface_size(), (330, 430));
        assert_eq!(card_origin(), Point::new(16.0, 6.0));
    }

    #[test]
    fn the_light_palette_has_the_slider_asset_colours() {
        let theme = Theme::load().unwrap();
        let light = Palette::from_theme(&theme, Appearance::Light).unwrap();
        assert_eq!((light.track, light.track_outline, light.disabled_alpha), ([0.910, 0.910, 0.910, 1.0], Some([0.0, 0.0, 0.0, 0.102]), 0.50));
        let dark = Palette::from_theme(&theme, Appearance::Dark).unwrap();
        assert_eq!((dark.track, dark.track_outline, dark.disabled_alpha), ([0.137, 0.137, 0.137, 1.0], None, 0.30));
    }

    #[test]
    fn increase_contrast_takes_the_contrast_slider_track() {
        let theme = Theme::load().unwrap();
        assert_eq!(Palette::from_theme(&theme, Appearance::LightHighContrast).unwrap().track, [0.459, 0.459, 0.459, 1.0]);
        assert_eq!(Palette::from_theme(&theme, Appearance::DarkHighContrast).unwrap().track, [0.612, 0.612, 0.612, 1.0]);
    }

    #[test]
    fn a_full_slider_is_filled_to_the_end_and_an_empty_one_shows_only_the_knob() {
        let mut renderer = renderer(Appearance::Light);
        let rendered = renderer.render(&controls(), 1.0);
        let layout = Layout::new(false);
        let display = layout.track(Slider::Display);
        let sound = layout.track(Slider::Sound);
        let origin = card_origin();
        let at = |track: Rect, dx: f64| ((origin.x + track.x() + dx) as u32, (origin.y + track.center().y) as u32);
        // brightness 1.0: white 12 points before the right end of the 254 point track
        let (x, y) = at(display, 242.0);
        assert_eq!(pixel(&rendered, x, y), [255, 255, 255, 255]);
        // volume 0.0: the knob fills the left 22 points (sampled above the speaker symbol that sits on it), the rest is
        // track grey 0.910 (232)
        let (x, y) = at(sound, 11.0);
        assert_eq!(pixel(&rendered, x, y - 8), [255, 255, 255, 255]);
        let (x, y) = at(sound, 100.0);
        assert_eq!(pixel(&rendered, x, y), [232, 232, 232, 255]);
    }

    #[test]
    fn a_disabled_slider_is_drawn_at_half_opacity_in_the_light_appearance() {
        let mut renderer = renderer(Appearance::Light);
        let mut controls = controls();
        controls.volume = Reading::Unavailable("no card".into());
        let rendered = renderer.render(&controls, 1.0);
        let track = Layout::new(false).track(Slider::Sound);
        let origin = card_origin();
        let p = pixel(&rendered, (origin.x + track.x() + 100.0) as u32, (origin.y + track.center().y) as u32);
        // grey 0.910 at alpha 0.5 (232 * 0.5 = 116, premultiplied) over the tile's black at alpha 20: 128 + 20 / 2 = 138
        assert_eq!(p, [116, 116, 116, 138]);
    }

    #[test]
    fn the_battery_row_draws_a_red_fill_at_ten_percent() {
        let mut renderer = renderer(Appearance::Light);
        let mut controls = controls();
        controls.battery = Some(Battery { percent: 10, charging: false });
        let rendered = renderer.render(&controls, 1.0);
        // the fill is 2 points wide at the lowest charge and starts 2 points inside the body, which starts at the row's left edge
        let layout = Layout::new(true);
        let row = layout.battery().unwrap();
        let origin = card_origin();
        let p = pixel(&rendered, (origin.x + row.x() + 3.0) as u32, (origin.y + row.center().y) as u32);
        assert_eq!(p, [255, 59, 48, 255]);
    }

    #[test]
    fn every_symbol_the_panel_draws_is_in_the_required_list() {
        let speakers = [(0.0, false), (0.2, false), (0.5, false), (0.9, false)].map(|(level, muted)| speaker_icon(&VolumeState { level, muted }));
        for name in Module::ALL.map(module_icon).into_iter().chain(speakers).chain(["symbol.sun", "symbol.speaker.slash"]) {
            assert!(ICON_NAMES.contains(&name), "{name}");
        }
    }

    #[test]
    fn the_surface_scales_with_the_output() {
        let mut renderer = renderer(Appearance::Light);
        let rendered = renderer.render(&controls(), 2.0);
        assert_eq!((rendered.width(), rendered.height()), (660, 860));
    }
}
