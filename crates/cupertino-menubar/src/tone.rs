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
/// behind them. It is solid with Reduce Transparency only; Increase Contrast keeps it see-through.
pub fn solid_bar(display: DisplayOptions) -> bool {
    display.reduce_transparency
}

/// How unevenly light the wallpaper may be under an item of the bar (the standard deviation of its CIE L*, in tenths,
/// as the server measures it) for the item to draw no halo: a calm wallpaper, where the text reads on its own.
pub const CALM_SPREAD: u16 = 60;
/// The spread from which an item's halo is at its strongest: stripes, foliage, text in the picture.
pub const BUSY_SPREAD: u16 = 250;
/// The halo's strength while the spread under the bar is not known (a server older than shell version 21, or before
/// its first measure): the halo every item drew before the spread was measured, black at 0.45 under light text and
/// white at 0.6 under dark text, 2.5 points wide.
pub const UNKNOWN_SPREAD_STRENGTH: f32 = 0.8;
/// The halo at full strength: its opacity under light text and under dark text, and how far it spreads in points,
/// from [`HALO_BLUR_CALM`] at the first trace of it.
pub const HALO_UNDER_LIGHT: f32 = 0.5625;
pub const HALO_UNDER_DARK: f32 = 0.75;
pub const HALO_BLUR_CALM: f64 = 0.5;
pub const HALO_BLUR_BUSY: f64 = 3.0;
/// The halo's dense core over measured wallpaper, hugging the strokes under the soft halo so a thin glyph keeps a
/// solid edge over stripes: its opacity at full strength under light and under dark text, and its blur in points.
/// It is laid [`HALO_CORE_PASSES`] times, each pass under the strokes and the passes before it, so it grows past a
/// thin stroke at close to its full opacity instead of thinning out as one blur of a hairline does: about a point
/// around every stroke reads at 4.5:1 or more against the text even over white (or black) wallpaper.
pub const HALO_CORE_UNDER_LIGHT: f32 = 0.8;
pub const HALO_CORE_UNDER_DARK: f32 = 0.85;
pub const HALO_CORE_BLUR: f64 = 1.0;
pub const HALO_CORE_PASSES: usize = 3;
/// Under the highlight of the open title over measured wallpaper, a backing of the halo's tone at this opacity at full
/// strength, under light and under dark text, so the highlight shows as a shape of its own over a busy picture.
pub const HALO_PILL_UNDER_LIGHT: f32 = 0.4;
pub const HALO_PILL_UNDER_DARK: f32 = 0.5;

/// The soft halo of the opposite tone an item of the bar draws under its text and symbols, so they read over a busy
/// wallpaper.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Halo {
    /// From 0 (none) to 1 (full): the share of [`HALO_UNDER_LIGHT`] or [`HALO_UNDER_DARK`] the halo is drawn at.
    pub strength: f32,
    /// How far it spreads, in points.
    pub blur: f64,
    /// From 0 (none) to 1 (full): the share of [`HALO_CORE_UNDER_LIGHT`] or [`HALO_CORE_UNDER_DARK`] its core is
    /// drawn at; none while the spread is unknown, where the halo is the one the bar always drew.
    pub core: f32,
}

impl Halo {
    /// The halo's colour under text of colour `text`: dark at its strength's share of [`HALO_UNDER_LIGHT`] under light
    /// text, light at its share of [`HALO_UNDER_DARK`] under dark text.
    pub fn color(self, text: Rgba) -> Rgba {
        if light(text) {
            [0.0, 0.0, 0.0, HALO_UNDER_LIGHT * self.strength]
        } else {
            [1.0, 1.0, 1.0, HALO_UNDER_DARK * self.strength]
        }
    }

    /// The colour of the halo's core under text of colour `text`, in the same tone as [`Halo::color`]; `None` when it
    /// has none.
    pub fn core_color(self, text: Rgba) -> Option<Rgba> {
        (self.core > 0.0).then(|| {
            if light(text) {
                [0.0, 0.0, 0.0, HALO_CORE_UNDER_LIGHT * self.core]
            } else {
                [1.0, 1.0, 1.0, HALO_CORE_UNDER_DARK * self.core]
            }
        })
    }

    /// The shadows the halo is drawn as under text of colour `text`, each `(blur in points, colour)` and each laid
    /// under the content and the ones before it: the core's passes, then the soft halo.
    pub fn shadows(self, text: Rgba) -> Vec<(f64, Rgba)> {
        let core = self.core_color(text).map(|c| (HALO_CORE_BLUR, c));
        std::iter::repeat_n(core, HALO_CORE_PASSES)
            .flatten()
            .chain([(self.blur, self.color(text))])
            .collect()
    }

    /// The backing under the open title's highlight with text of colour `text`: the halo's tone at its core's share of
    /// [`HALO_PILL_UNDER_LIGHT`] or [`HALO_PILL_UNDER_DARK`]; `None` without a core (no measure yet).
    pub fn pill_backing(self, text: Rgba) -> Option<Rgba> {
        (self.core > 0.0).then(|| {
            if light(text) {
                [0.0, 0.0, 0.0, HALO_PILL_UNDER_LIGHT * self.core]
            } else {
                [1.0, 1.0, 1.0, HALO_PILL_UNDER_DARK * self.core]
            }
        })
    }
}

/// Whether text of colour `text` is light (a dark halo goes under it).
fn light(text: Rgba) -> bool {
    0.2126 * text[0] + 0.7152 * text[1] + 0.0722 * text[2] > 0.5
}

/// The halo of an item over wallpaper whose spread under it is `spread` (the most of the bands it covers; `None`
/// while unknown), on a bar that is `solid` or not ([`solid_bar`] of the session's display options). A solid bar
/// draws none, and neither does an item over a calm part of the wallpaper (at most [`CALM_SPREAD`]): the halo is
/// invisible there. Past that its strength, its blur and its core grow smoothly with the spread, to full at
/// [`BUSY_SPREAD`].
pub fn halo(spread: Option<u16>, solid: bool) -> Option<Halo> {
    if solid {
        return None;
    }
    let (strength, core) = match spread {
        None => (UNKNOWN_SPREAD_STRENGTH, 0.0),
        Some(spread) => {
            let t = (f32::from(spread.saturating_sub(CALM_SPREAD))
                / f32::from(BUSY_SPREAD - CALM_SPREAD))
            .clamp(0.0, 1.0);
            let s = t * t * (3.0 - 2.0 * t);
            (s, s)
        }
    };
    (strength > 0.0).then(|| Halo {
        strength,
        blur: HALO_BLUR_CALM + (HALO_BLUR_BUSY - HALO_BLUR_CALM) * f64::from(strength),
        core,
    })
}

/// The spread under an item of the bar from `left` to `right` points, on a bar `width` points wide whose wallpaper
/// spreads by `bands` from its left edge to its right: the most of the bands the item covers, or `None` while
/// nothing was measured.
pub fn spread_under(bands: &[u16], width: f64, left: f64, right: f64) -> Option<u16> {
    if bands.is_empty() || width.is_nan() || width <= 0.0 {
        return None;
    }
    let n = bands.len();
    let band = |x: f64| ((x / width * n as f64).floor().max(0.0) as usize).min(n - 1);
    let (first, last) = (band(left), band(right.max(left)));
    bands[first..=last].iter().copied().max()
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

    /// The halo's strength runs from none over a calm wallpaper to full over a busy one, smoothly and never
    /// falling, with its blur growing alongside; a solid bar has none whatever the wallpaper; with no measure the bar
    /// keeps about the halo it used to draw.
    #[test]
    fn the_halo_grows_with_the_spread_from_none_over_calm_wallpaper_to_full_over_busy() {
        for spread in [0, 10, 30, CALM_SPREAD] {
            assert_eq!(halo(Some(spread), false), None, "calm at {spread}");
        }
        let full = halo(Some(BUSY_SPREAD), false).unwrap();
        assert_eq!(full.strength, 1.0);
        assert_eq!(full.blur, HALO_BLUR_BUSY);
        assert_eq!(halo(Some(1000), false), Some(full));
        let mid = halo(Some((CALM_SPREAD + BUSY_SPREAD) / 2), false).unwrap();
        assert!((mid.strength - 0.5).abs() < 0.02, "{mid:?}");
        assert!((mid.blur - (HALO_BLUR_CALM + HALO_BLUR_BUSY) / 2.0).abs() < 0.05);
        let mut last = Halo {
            strength: 0.0,
            blur: HALO_BLUR_CALM,
            core: 0.0,
        };
        for spread in CALM_SPREAD + 1..=BUSY_SPREAD {
            let h = halo(Some(spread), false).unwrap();
            assert!(
                h.strength > last.strength && h.blur > last.blur,
                "at {spread}"
            );
            assert!(h.strength - last.strength < 0.02, "smooth at {spread}");
            last = h;
        }
        // just past calm the halo is still too faint to see: under 2 % of its full opacity
        let faint = halo(Some(CALM_SPREAD + 10), false).unwrap();
        assert!(faint.color([1.0; 4])[3] < 0.02 * HALO_UNDER_DARK);
        for spread in [None, Some(0), Some(BUSY_SPREAD), Some(1000)] {
            assert_eq!(halo(spread, true), None, "a solid bar at {spread:?}");
        }
        let unknown = halo(None, false).unwrap();
        assert_eq!(unknown.strength, UNKNOWN_SPREAD_STRENGTH);
        assert_eq!(
            unknown.core_color([1.0; 4]),
            None,
            "no core without a measure"
        );
        assert!((unknown.blur - 2.5).abs() < 1e-6);
        assert!((unknown.color([0.97, 0.97, 0.98, 0.95])[3] - 0.45).abs() < 1e-6);
        assert!((unknown.color([0.05, 0.05, 0.06, 0.92])[3] - 0.6).abs() < 1e-6);
        // light text gets a dark halo, dark text a light one
        assert_eq!(
            full.color([0.97, 0.97, 0.98, 0.95]),
            [0.0, 0.0, 0.0, HALO_UNDER_LIGHT]
        );
        assert_eq!(
            full.color([0.05, 0.05, 0.06, 0.92]),
            [1.0, 1.0, 1.0, HALO_UNDER_DARK]
        );
        // the core follows the halo's strength in the same tone, from nothing just past calm to full over busy
        assert_eq!(
            full.core_color([0.97, 0.97, 0.98, 0.95]),
            Some([0.0, 0.0, 0.0, HALO_CORE_UNDER_LIGHT])
        );
        assert_eq!(
            full.core_color([0.05, 0.05, 0.06, 0.92]),
            Some([1.0, 1.0, 1.0, HALO_CORE_UNDER_DARK])
        );
        assert!(faint.core_color([1.0; 4]).unwrap()[3] < 0.02 * HALO_CORE_UNDER_DARK);
        const { assert!(HALO_CORE_BLUR < HALO_BLUR_BUSY) };
    }

    /// Over measured wallpaper the halo is drawn as its core's passes, each the core's colour at its blur, then the soft
    /// halo; with no measure it is the soft halo alone, as before the spread was measured. The open title's backing
    /// follows the core in the halo's tone, and there is none with no measure.
    #[test]
    fn the_halo_is_drawn_as_the_core_s_passes_then_the_soft_halo() {
        let (light_text, dark_text) = ([0.97, 0.97, 0.98, 0.95], [0.05, 0.05, 0.06, 0.92]);
        let full = halo(Some(BUSY_SPREAD), false).unwrap();
        for text in [light_text, dark_text] {
            let shadows = full.shadows(text);
            assert_eq!(shadows.len(), HALO_CORE_PASSES + 1);
            for &(blur, color) in &shadows[..HALO_CORE_PASSES] {
                assert_eq!((blur, Some(color)), (HALO_CORE_BLUR, full.core_color(text)));
            }
            assert_eq!(
                shadows[HALO_CORE_PASSES],
                (HALO_BLUR_BUSY, full.color(text))
            );
        }
        const { assert!(HALO_CORE_PASSES >= 2) };
        assert_eq!(
            full.pill_backing(light_text),
            Some([0.0, 0.0, 0.0, HALO_PILL_UNDER_LIGHT])
        );
        assert_eq!(
            full.pill_backing(dark_text),
            Some([1.0, 1.0, 1.0, HALO_PILL_UNDER_DARK])
        );
        let mid = halo(Some((CALM_SPREAD + BUSY_SPREAD) / 2), false).unwrap();
        let backing = mid.pill_backing(light_text).unwrap()[3];
        assert!((backing - HALO_PILL_UNDER_LIGHT * mid.core).abs() < 1e-6);
        let unknown = halo(None, false).unwrap();
        assert_eq!(
            unknown.shadows(light_text),
            vec![(unknown.blur, unknown.color(light_text))]
        );
        assert_eq!(unknown.pill_backing(light_text), None);
    }

    #[test]
    fn an_item_takes_the_most_spread_of_the_bands_it_covers() {
        let bands = [0, 10, 300, 20];
        // a bar 400 points wide: each band is 100 points
        assert_eq!(spread_under(&bands, 400.0, 10.0, 90.0), Some(0));
        assert_eq!(spread_under(&bands, 400.0, 150.0, 250.0), Some(300));
        assert_eq!(spread_under(&bands, 400.0, 380.0, 420.0), Some(20));
        assert_eq!(spread_under(&bands, 400.0, -5.0, 0.0), Some(0));
        assert_eq!(spread_under(&[], 400.0, 10.0, 90.0), None);
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
