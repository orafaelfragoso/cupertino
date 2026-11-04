//! The colours of the bar and its menus: the palette of each appearance, and the tone and scrim the bar's text takes
//! over the wallpaper under it. Colours not given by the material recipes are approximate.

use cupertino_defaults::accessibility::DisplayOptions;

/// Straight-alpha sRGB components in 0..=1.
pub type Rgba = [f32; 4];

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
