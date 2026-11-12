//! The colours of the bar and its menus: the palette of each appearance, the tone the bar's text takes over the
//! wallpaper under it, and when the bar is solid. Colours not given by the material recipes are approximate.

use cupertino_defaults::accessibility::DisplayOptions;

/// Straight-alpha sRGB components in 0..=1.
pub type Rgba = [f32; 4];

/// Colours of the bar and popup contents. approximate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MenuPalette {
    pub text: Rgba,
    /// What a disabled row's text, mark and arrow keep of the text's opacity: a quarter of full strength in all, half
    /// with Increase Contrast ([`MenuPalette::high_contrast`]).
    pub disabled_alpha: f32,
    /// Fill behind the open title in the bar: a veil over what is under it, dark under dark text and light under light
    /// text.
    pub bar_highlight: Rgba,
    /// Fill of the highlighted popup row and the text on it.
    pub row_highlight: Rgba,
    pub row_highlight_text: Rgba,
    /// The search field's box, and the line between groups of rows unless Increase Contrast strengthens it.
    pub separator: Rgba,
    /// The line between groups of rows.
    pub rule: Rgba,
    /// The screen capture indicator: indigo, (99, 97, 241) dark and (84, 82, 204) light.
    pub screen_capture: Rgba,
}

impl MenuPalette {
    pub fn new(dark: bool) -> Self {
        if dark {
            Self {
                text: [0.97, 0.97, 0.98, 0.95],
                disabled_alpha: DISABLED_ALPHA,
                bar_highlight: [1.0, 1.0, 1.0, 0.1],
                row_highlight: [0.04, 0.45, 1.0, 1.0],
                row_highlight_text: [1.0, 1.0, 1.0, 1.0],
                separator: [1.0, 1.0, 1.0, 0.16],
                rule: [1.0, 1.0, 1.0, 0.125],
                screen_capture: [99.0 / 255.0, 97.0 / 255.0, 241.0 / 255.0, 1.0],
            }
        } else {
            Self {
                text: [0.05, 0.05, 0.06, 0.92],
                disabled_alpha: DISABLED_ALPHA,
                bar_highlight: [0.0, 0.0, 0.0, 0.045],
                row_highlight: [0.04, 0.45, 1.0, 1.0],
                row_highlight_text: [1.0, 1.0, 1.0, 1.0],
                separator: [0.0, 0.0, 0.0, 0.13],
                rule: [0.0, 0.0, 0.0, 0.11],
                screen_capture: [84.0 / 255.0, 82.0 / 255.0, 204.0 / 255.0, 1.0],
            }
        }
    }

    /// The palette with Increase Contrast: disabled rows keep half the text's strength, readable yet apart from the
    /// enabled ones, and the lines between groups of rows are stronger.
    pub fn high_contrast(self) -> Self {
        let mut rule = self.rule;
        rule[3] = HIGH_CONTRAST_RULE_ALPHA;
        Self {
            disabled_alpha: HIGH_CONTRAST_DISABLED_ALPHA,
            rule,
            ..self
        }
    }
}

/// A disabled popup row's share of the text's opacity: with the text at 92–95 %, its text, mark and arrow end at about
/// a quarter, the disabled text of the rest of the desktop.
pub const DISABLED_ALPHA: f32 = 0.26;
/// The same with Increase Contrast: about half.
pub const HIGH_CONTRAST_DISABLED_ALPHA: f32 = 0.53;
/// The line between groups of rows with Increase Contrast.
pub const HIGH_CONTRAST_RULE_ALPHA: f32 = 0.47;

/// Lightness of the wallpaper under the bar (CIE L*/100 in thousandths) at and above which the bar's text is dark.
/// approximate.
pub const DARK_TEXT_FROM: u16 = 550;
/// Lightness of the wallpaper under the bar at and below which the bar's text is light. approximate.
pub const LIGHT_TEXT_UNTIL: u16 = 450;
/// Where a tone chosen afresh splits the band between [`LIGHT_TEXT_UNTIL`] and [`DARK_TEXT_FROM`]. approximate.
const FRESH_SPLIT: u16 = 500;

/// Whether the bar draws a solid strip of its own (the menu bar material's opaque recipe) under its text. Otherwise
/// it draws nothing at all: the wallpaper shows through it as it is, and only its text and symbols follow what is
/// behind them, drawn sharp with no shadow, halo or backing under them over any wallpaper. It is solid with Reduce
/// Transparency only; Increase Contrast keeps it see-through.
pub fn solid_bar(display: DisplayOptions) -> bool {
    display.reduce_transparency
}

/// The text of a bar's strip: the appearance's, or dark or light for the wallpaper under the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarTone {
    /// The palette of the appearance: before the server sent the wallpaper's lightness (always below shell version
    /// 14), and while Reduce Transparency makes the bar solid ([`solid_bar`]).
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
        if solid_bar(display) {
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
        let solid = DisplayOptions {
            reduce_transparency: true,
            ..TRANSLUCENT
        };
        assert_eq!(BarTone::fresh(Some(900), solid), BarTone::Appearance);
        assert_eq!(
            BarTone::LightText.next(Some(100), solid),
            BarTone::Appearance
        );
        let light = MenuPalette::new(false);
        assert_eq!(BarTone::Appearance.palette(light), light);
    }

    /// A disabled popup row (its text, mark and arrow) shows about a quarter of the text's strength, as disabled text
    /// does elsewhere, and about half with Increase Contrast; both stay well apart from the enabled rows, and Increase
    /// Contrast strengthens the lines between groups.
    #[test]
    fn disabled_rows_recede_and_high_contrast_keeps_them_readable() {
        for dark in [false, true] {
            let normal = MenuPalette::new(dark);
            let contrast = normal.high_contrast();
            let shown = |p: MenuPalette| p.text[3] * p.disabled_alpha;
            assert!((shown(normal) - 0.25).abs() < 0.02, "dark {dark}");
            assert!((shown(contrast) - 0.5).abs() < 0.02, "dark {dark}");
            assert!(contrast.rule[3] > normal.rule[3] * 2.0, "dark {dark}");
            assert_eq!(
                contrast.separator, normal.separator,
                "the field's box stays"
            );
            assert_eq!(contrast.text, normal.text);
        }
    }

    #[test]
    fn only_reduce_transparency_makes_the_bar_solid() {
        let contrast = DisplayOptions {
            increase_contrast: true,
            ..TRANSLUCENT
        };
        let reduced = DisplayOptions {
            reduce_transparency: true,
            ..TRANSLUCENT
        };
        assert!(!solid_bar(TRANSLUCENT));
        assert!(
            !solid_bar(contrast),
            "Increase Contrast keeps it see-through"
        );
        assert!(solid_bar(reduced));
        assert!(solid_bar(DisplayOptions {
            increase_contrast: true,
            ..reduced
        }));
        // over a see-through bar the text still follows the wallpaper
        assert_eq!(BarTone::fresh(Some(120), contrast), BarTone::LightText);
        assert_eq!(BarTone::fresh(Some(900), contrast), BarTone::DarkText);
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

    /// The text of `tone` straight over a strip of `strip` (sRGB): the bar draws nothing under it.
    fn contrast_over(tone: BarTone, strip: [f64; 3]) -> f64 {
        wcag(tone.palette(MenuPalette::new(false)).text, strip)
    }

    /// Every tone a bar can hold at `lightness`: fresh, and kept from either text.
    fn reachable(lightness: u16) -> Vec<BarTone> {
        [BarTone::Appearance, BarTone::DarkText, BarTone::LightText]
            .map(|held| held.next(Some(lightness), TRANSLUCENT))
            .into_iter()
            .collect()
    }

    #[test]
    fn outside_the_band_the_text_is_the_one_that_reads_better_and_clears_4_5_to_1_away_from_it() {
        for lightness in (0..=LIGHT_TEXT_UNTIL).chain(DARK_TEXT_FROM..=1000) {
            let strip = [grey(lightness); 3];
            let (light, dark) = (
                contrast_over(BarTone::LightText, strip),
                contrast_over(BarTone::DarkText, strip),
            );
            for tone in reachable(lightness) {
                let c = contrast_over(tone, strip);
                assert_eq!(c, light.max(dark), "{tone:?} at {lightness}");
                if !(400..=600).contains(&lightness) {
                    assert!(c >= 4.5, "{tone:?} at {lightness}: {c:.3}");
                }
            }
        }
    }

    #[test]
    fn the_text_is_white_over_a_dark_blue_wallpaper_and_dark_over_a_pale_one() {
        let navy = cie_lightness([14.0 / 255.0, 8.0 / 255.0, 134.0 / 255.0]);
        let pale = cie_lightness([0.9, 0.9, 0.92]);
        for dark in [false, true] {
            let appearance = MenuPalette::new(dark);
            let over_navy = BarTone::fresh(Some(navy), TRANSLUCENT).palette(appearance);
            let over_pale = BarTone::fresh(Some(pale), TRANSLUCENT).palette(appearance);
            assert_eq!(over_navy.text, MenuPalette::new(true).text, "{navy}");
            assert_eq!(over_pale.text, MenuPalette::new(false).text, "{pale}");
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
