//! CPU drawing of the overlay's content: the glyph, the 16-segment meter and the label. The window's material is the
//! server's backdrop (see [`MATERIAL`]); nothing here draws a background.

use std::fmt;

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::palette::Rgba;
use cupertino_decorations::text::TextRenderer;
use cupertino_geometry::Rect;
use cupertino_icons::SystemIcons;
use cupertino_theme::{Appearance, SemanticColor, Theme, Typography};
use tiny_skia::{Color, FillRule, Paint, PathBuilder, Pixmap, PixmapPaint, Transform};

use crate::layout::{
    self, GLYPH_SIZE, GLYPH_TOP, LABEL_HEIGHT, LABEL_TOP, LABEL_WIDTH, WINDOW_SIZE,
};
use crate::model::{segments, Fill, Overlay, ICON_NAMES};

/// Material recipe the server draws behind the window: the design's bezel background material, number 26,
/// is an alias of the login-window control material (element number, alias bytes).
pub const MATERIAL: &str = "LoginWindowControl";

/// Label font size and weight (system font, 18 pt).
const LABEL_SIZE: f32 = 18.0;
const LABEL_WEIGHT: u16 = 400;

/// Smallest label size the shrink loop reaches ("floor 6-7 pt").
const LABEL_FLOOR: f32 = 6.0;

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
    /// The track box behind the segments (the 161 x 8 container); empty segments show only this.
    pub track: Rgba,
    /// The filled part of the segments.
    pub segment: Rgba,
    /// The glyph (approximate: the design glyph colour is not known).
    pub glyph: Rgba,
    /// The label (the control text colour).
    pub label: Rgba,
}

/// Track of the dark variant: black at alpha 0.25.
const DARK_TRACK: Rgba = [0.0, 0.0, 0.0, 0.25];
/// Segment of the light variant: opaque white.
const LIGHT_SEGMENT: Rgba = [1.0, 1.0, 1.0, 1.0];
/// Track of the light variant with Increase Contrast: opaque black.
const CONTRAST_LIGHT_TRACK: Rgba = [0.0, 0.0, 0.0, 1.0];
/// Segment of the dark variant with Increase Contrast: opaque white.
const CONTRAST_DARK_SEGMENT: Rgba = [1.0, 1.0, 1.0, 1.0];

impl Palette {
    /// The palette of `appearance`; the design picks the light or dark pair by its menu bar's darkness, which `appearance` stands in for. Light: the secondary label colour as track, white segments. Dark:
    /// black at 0.25 as track, the secondary label colour as segments. A high-contrast appearance makes the light track
    /// opaque black and the dark segments white; Reduce Transparency makes the dark track opaque.
    pub fn from_theme(
        theme: &Theme,
        appearance: Appearance,
        reduce_transparency: bool,
    ) -> Result<Self, MissingColor> {
        let get = |color: SemanticColor, name: &'static str| {
            theme
                .tokens
                .semantic(appearance, color)
                .map(|c| c.to_array())
                .ok_or(MissingColor(name))
        };
        let secondary = get(SemanticColor::SecondaryLabel, "secondary-label")?;
        let contrast = appearance.is_high_contrast();
        let (track, segment) = if appearance.is_dark() {
            let track = if reduce_transparency {
                [DARK_TRACK[0], DARK_TRACK[1], DARK_TRACK[2], 1.0]
            } else {
                DARK_TRACK
            };
            (
                track,
                if contrast {
                    CONTRAST_DARK_SEGMENT
                } else {
                    secondary
                },
            )
        } else {
            (
                if contrast {
                    CONTRAST_LIGHT_TRACK
                } else {
                    secondary
                },
                LIGHT_SEGMENT,
            )
        };
        Ok(Self {
            track,
            segment,
            glyph: get(SemanticColor::Label, "label")?,
            label: get(SemanticColor::ControlText, "control-text")?,
        })
    }
}

fn skia(color: Rgba) -> Color {
    Color::from_rgba(color[0], color[1], color[2], color[3])
        .expect("theme colours are within 0 to 1")
}

fn fill_rect(pixmap: &mut Pixmap, rect: &Rect, scale: f32, color: Rgba) {
    let rect = tiny_skia::Rect::from_xywh(
        rect.x() as f32 * scale,
        rect.y() as f32 * scale,
        rect.width() as f32 * scale,
        rect.height() as f32 * scale,
    )
    .expect("segment rectangles have a size");
    let mut paint = Paint::default();
    paint.set_color(skia(color));
    pixmap.fill_path(
        &PathBuilder::from_rect(rect),
        &paint,
        FillRule::Winding,
        Transform::identity(),
        None,
    );
}

/// Draw the meter filled to `level`: the track box, then the filled part of each segment.
pub fn draw_meter(pixmap: &mut Pixmap, level: f64, palette: &Palette, scale: f32) {
    fill_rect(pixmap, &layout::meter(), scale, palette.track);
    for (index, fill) in segments(level).into_iter().enumerate() {
        let segment = layout::segment(index);
        let filled_width = match fill {
            Fill::Full => segment.width(),
            Fill::Partial(width) => f64::from(width),
            Fill::Empty => continue,
        };
        fill_rect(
            pixmap,
            &Rect::new(segment.x(), segment.y(), filled_width, segment.height()),
            scale,
            palette.segment,
        );
    }
}

/// The largest size from `start` down to `floor`, in whole steps of 1, at which `measure(size)` fits `max_width`
/// (the label shrinks by 1 pt until it fits); `floor` when none fits.
pub fn shrink_to_fit(
    start: f32,
    floor: f32,
    max_width: f32,
    mut measure: impl FnMut(f32) -> f32,
) -> f32 {
    let mut size = start;
    while size > floor && measure(size) > max_width {
        size -= 1.0;
    }
    size.max(floor)
}

/// Draws overlays.
pub struct Renderer {
    palette: Palette,
    typography: Typography,
    text: TextRenderer,
    icons: SystemIcons,
}

impl Renderer {
    /// A renderer with `palette`; the label uses the theme's typography in the `ui` font of the asset directory.
    /// Fails only when no usable font is left ([`TextRenderer::new`]); a symbol no asset pack provides is reported and drawn as a placeholder ([`ICON_NAMES`]).
    pub fn new(
        palette: Palette,
        typography: Typography,
        assets: &Assets,
    ) -> Result<Self, AssetError> {
        let text = TextRenderer::new(assets, FontRole::Ui)?;
        let icons = SystemIcons::required(assets, &[], &ICON_NAMES);
        Ok(Self {
            palette,
            typography,
            text,
            icons,
        })
    }

    /// The overlay at `scale` device pixels per point, `WINDOW_SIZE * scale` pixels square, premultiplied.
    pub fn render(&mut self, overlay: &Overlay, scale: f32) -> Pixmap {
        self.text.set_scale(scale);
        let side = (f64::from(WINDOW_SIZE) * f64::from(scale)).round() as u32;
        let mut pixmap = Pixmap::new(side, side).expect("the window has a size");
        draw_meter(&mut pixmap, overlay.level(), &self.palette, scale);
        self.draw_glyph(&mut pixmap, overlay.icon_name(), scale);
        if let Some(label) = overlay.label() {
            self.draw_label(&mut pixmap, label, scale);
        }
        pixmap
    }

    /// Draw the glyph centred in its box (a placeholder when no asset pack provides it).
    fn draw_glyph(&mut self, pixmap: &mut Pixmap, name: &str, scale: f32) {
        let px = (GLYPH_SIZE * f64::from(scale)).round() as u32;
        let glyph = self.icons.symbol(name, px, self.palette.glyph);
        let box_left = (f64::from(WINDOW_SIZE) - GLYPH_SIZE) / 2.0 * f64::from(scale);
        let x = box_left + (f64::from(px) - f64::from(glyph.width())) / 2.0;
        let y = GLYPH_TOP * f64::from(scale) + (f64::from(px) - f64::from(glyph.height())) / 2.0;
        pixmap.draw_pixmap(
            x.round() as i32,
            y.round() as i32,
            glyph.as_ref(),
            &PixmapPaint::default(),
            Transform::identity(),
            None,
        );
    }

    /// Draw `label` centred in its frame at the largest size that fits it.
    fn draw_label(&mut self, pixmap: &mut Pixmap, label: &str, scale: f32) {
        let s = scale;
        let size = shrink_to_fit(LABEL_SIZE, LABEL_FLOOR, LABEL_WIDTH as f32, |px| {
            self.text.measure(label, px * s, LABEL_WEIGHT, 0.0) / s
        });
        let width = self.text.measure(label, size * s, LABEL_WEIGHT, 0.0);
        let metrics = &self.typography.metrics;
        let (ascent, descent) = (
            metrics.ascender / metrics.units_per_em * f64::from(size),
            metrics.descender.abs() / metrics.units_per_em * f64::from(size),
        );
        let baseline = LABEL_TOP + (LABEL_HEIGHT - (ascent + descent)) / 2.0 + ascent;
        let x = (f64::from(WINDOW_SIZE) * f64::from(s) - f64::from(width)) / 2.0;
        self.text.draw(
            pixmap,
            label,
            x as f32,
            (baseline * f64::from(s)) as f32,
            size * s,
            LABEL_WEIGHT,
            0.0,
            self.palette.label,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Kind;

    const PALETTE: Palette = Palette {
        segment: [1.0, 1.0, 1.0, 1.0],
        track: [0.0, 0.0, 0.0, 1.0],
        glyph: [1.0; 4],
        label: [1.0; 4],
    };

    /// RGBA of the pixel at (x, y).
    fn pixel(pixmap: &Pixmap, x: u32, y: u32) -> [u8; 4] {
        let p = pixmap.pixel(x, y).unwrap();
        [p.red(), p.green(), p.blue(), p.alpha()]
    }

    fn meter_at(level: f64, scale: f32) -> Pixmap {
        let side = (200.0 * scale) as u32;
        let mut pixmap = Pixmap::new(side, side).unwrap();
        draw_meter(&mut pixmap, level, &PALETTE, scale);
        pixmap
    }

    #[test]
    fn full_segments_are_filled_and_the_rest_show_the_track() {
        // 4 whole segments: x = 21..61 filled, segment 4 (x = 61..70) track only
        let pixmap = meter_at(0.25, 1.0);
        assert_eq!(pixel(&pixmap, 25, 176), [255, 255, 255, 255]);
        assert_eq!(pixel(&pixmap, 55, 176), [255, 255, 255, 255]);
        assert_eq!(pixel(&pixmap, 65, 176), [0, 0, 0, 255]);
        assert_eq!(pixel(&pixmap, 175, 176), [0, 0, 0, 255]);
    }

    #[test]
    fn the_track_shows_between_and_around_the_segments_and_nothing_outside_the_container() {
        let pixmap = meter_at(1.0, 1.0);
        // the pixel between segment 0 (x = 21..30) and segment 1 (x = 31..40), and the rows above and below a segment
        assert_eq!(pixel(&pixmap, 30, 176), [0, 0, 0, 255]);
        assert_eq!(pixel(&pixmap, 25, 172), [0, 0, 0, 255]);
        assert_eq!(pixel(&pixmap, 25, 179), [0, 0, 0, 255]);
        assert_eq!(pixel(&pixmap, 25, 173), [255, 255, 255, 255]);
        // the container is x = 20..181, y = 172..180
        assert_eq!(
            (
                pixel(&pixmap, 19, 176)[3],
                pixel(&pixmap, 181, 176)[3],
                pixel(&pixmap, 25, 171)[3],
                pixel(&pixmap, 25, 180)[3]
            ),
            (0, 0, 0, 0)
        );
    }

    #[test]
    fn the_light_palette_has_a_secondary_label_track_and_white_segments() {
        let theme = Theme::load().unwrap();
        let palette = Palette::from_theme(&theme, Appearance::Light, false).unwrap();
        assert!(
            (palette.track[3] - 0.498).abs() < 0.001 && palette.track[..3] == [0.0, 0.0, 0.0],
            "{:?}",
            palette.track
        );
        assert_eq!(palette.segment, [1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn increase_contrast_and_reduce_transparency_make_the_meter_opaque() {
        let theme = Theme::load().unwrap();
        let light = Palette::from_theme(&theme, Appearance::LightHighContrast, false).unwrap();
        assert_eq!(
            (light.track, light.segment),
            ([0.0, 0.0, 0.0, 1.0], [1.0, 1.0, 1.0, 1.0])
        );
        let dark = Palette::from_theme(&theme, Appearance::DarkHighContrast, false).unwrap();
        assert_eq!(
            (dark.track, dark.segment),
            ([0.0, 0.0, 0.0, 0.25], [1.0, 1.0, 1.0, 1.0])
        );
        assert_eq!(
            Palette::from_theme(&theme, Appearance::Dark, true)
                .unwrap()
                .track,
            [0.0, 0.0, 0.0, 1.0]
        );
        assert_eq!(
            Palette::from_theme(&theme, Appearance::Light, true).unwrap(),
            Palette::from_theme(&theme, Appearance::Light, false).unwrap(),
            "the light meter has no transparency rule"
        );
    }

    #[test]
    fn the_dark_palette_has_a_quarter_black_track_and_secondary_label_segments() {
        let theme = Theme::load().unwrap();
        let palette = Palette::from_theme(&theme, Appearance::Dark, false).unwrap();
        assert_eq!(palette.track, [0.0, 0.0, 0.0, 0.25]);
        assert!(
            (palette.segment[3] - 0.549).abs() < 0.001 && palette.segment[..3] == [1.0, 1.0, 1.0],
            "{:?}",
            palette.segment
        );
    }

    #[test]
    fn a_partial_segment_is_filled_from_the_left_by_its_rounded_width() {
        // 8.5 segments: segment 8 spans x = 101..110 and is filled 5 points wide
        let pixmap = meter_at(8.5 / 16.0, 1.0);
        assert_eq!(pixel(&pixmap, 101, 176), [255, 255, 255, 255]);
        assert_eq!(pixel(&pixmap, 105, 176), [255, 255, 255, 255]);
        assert_eq!(pixel(&pixmap, 106, 176), [0, 0, 0, 255]);
        assert_eq!(pixel(&pixmap, 109, 176), [0, 0, 0, 255]);
    }

    #[test]
    fn the_meter_scales_with_the_output() {
        let pixmap = meter_at(1.0, 2.0);
        assert_eq!(
            (pixmap.width(), pixel(&pixmap, 42, 352)),
            (400, [255, 255, 255, 255])
        );
        assert_eq!(pixel(&pixmap, 38, 352)[3], 0);
    }

    #[test]
    fn the_label_shrinks_one_point_at_a_time_until_it_fits() {
        // 0.6 em per glyph: "100" is 1.8 em wide, so 14 pt (25.2) fits a 26 pt frame and 15 pt (27) does not
        assert_eq!(shrink_to_fit(18.0, 6.0, 26.0, |px| 1.8 * px), 14.0);
        assert_eq!(shrink_to_fit(18.0, 6.0, 26.0, |px| 0.5 * px), 18.0);
        assert_eq!(shrink_to_fit(18.0, 6.0, 26.0, |px| 100.0 * px), 6.0);
    }

    /// Needs the asset directory (`CUPERTINO_ASSET_DIR`); fails with the build instructions when it is missing.
    fn renderer() -> Renderer {
        let theme = Theme::load().unwrap();
        let assets = Assets::discover();
        Renderer::new(PALETTE, theme.tokens.typography().clone(), &assets)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn the_glyph_is_drawn_from_the_assets_inside_its_box_at_both_scales() {
        let mut renderer = renderer();
        for scale in [1.0f32, 2.0] {
            let pixmap = renderer.render(&Overlay::new(Kind::Volume, 0.9).unwrap(), scale);
            // rows above the meter's container (y = 172) hold only the glyph
            let meter_top = (172.0 * scale) as u32;
            let ink: Vec<u32> = (0..meter_top)
                .filter(|&y| (0..pixmap.width()).any(|x| pixel(&pixmap, x, y)[3] > 0))
                .collect();
            let (top, bottom) = (
                (GLYPH_TOP * f64::from(scale)) as u32,
                ((GLYPH_TOP + GLYPH_SIZE) * f64::from(scale)).ceil() as u32,
            );
            assert!(
                ink.first().is_some_and(|&y| y >= top) && ink.last().is_some_and(|&y| y <= bottom),
                "ink rows {:?}..{:?} outside {top}..{bottom}",
                ink.first(),
                ink.last()
            );
            assert!(
                ink.len() as f64 > 0.5 * GLYPH_SIZE * f64::from(scale),
                "the glyph is {} rows tall",
                ink.len()
            );
        }
    }
}
