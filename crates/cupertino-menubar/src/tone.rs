//! The colours of the bar and its menus: the palette of each appearance, the tone the bar's text takes over the
//! wallpaper under it, and when the bar is solid. Colours not given by the material recipes are approximate.

use cupertino_defaults::accessibility::DisplayOptions;

use crate::geom::Rect;
use crate::layout::{Slot, SlotKey};

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

/// Relative luminance of the wallpaper under the bar below which the bar's text is light and at and above which it is
/// dark. The luminance is that of the mean encoded sRGB colour of the strip under the bar, made linear and weighed as
/// Rec. 709 weighs red, green and blue ([`Backdrop::luminance`]).
pub const LIGHT_TEXT_BELOW: f64 = 0.334;

/// The text and symbols of the bar over a light wallpaper: black at 85 %.
pub const BAR_DARK_TEXT: Rgba = [0.0, 0.0, 0.0, 0.85];
/// The text and symbols of the bar over a dark wallpaper: white at 90 %.
pub const BAR_LIGHT_TEXT: Rgba = [1.0, 1.0, 1.0, 0.90];
/// The opacity of the bar's text and symbols, either colour, with Increase Contrast: opaque.
pub const BAR_HIGH_CONTRAST_TEXT_ALPHA: f32 = 1.0;

/// The bar's text and symbols: light or dark, opaque with Increase Contrast.
pub fn bar_text(light: bool, increase_contrast: bool) -> Rgba {
    let mut text = if light { BAR_LIGHT_TEXT } else { BAR_DARK_TEXT };
    if increase_contrast {
        text[3] = BAR_HIGH_CONTRAST_TEXT_ALPHA;
    }
    text
}

/// The faint shadow under a status item's symbol drawn light over the wallpaper: black at this opacity, blurred by
/// [`STATUS_SHADOW_BLUR`] points and dropped [`STATUS_SHADOW_DROP`] points below the symbol, in one pass. Dark symbols
/// and every text of the bar draw none.
pub const STATUS_SHADOW_ALPHA: f32 = 0.3;
pub const STATUS_SHADOW_BLUR: f64 = 2.5;
pub const STATUS_SHADOW_DROP: f64 = 1.75;

/// The colour of the shadow under a status item's symbol in a bar of `tone` over the appearance `dark`, if it has one:
/// only under light symbols.
pub fn status_shadow(tone: BarTone, dark: bool) -> Option<Rgba> {
    tone.light_text(dark)
        .then_some([0.0, 0.0, 0.0, STATUS_SHADOW_ALPHA])
}

/// Whether the bar draws a solid strip of its own (the menu bar material's opaque recipe) under its text. Otherwise
/// it draws nothing at all: the wallpaper shows through it as it is, and only its text and symbols follow what is
/// behind them, drawn sharp over any wallpaper (a status symbol drawn light keeps one faint shadow,
/// [`status_shadow`]). It is solid with Reduce Transparency only; Increase Contrast keeps it see-through.
pub fn solid_bar(display: DisplayOptions) -> bool {
    display.reduce_transparency
}

/// What the server says of the wallpaper under the bar.
#[derive(Clone, Debug, PartialEq)]
pub enum Backdrop {
    /// Its lightness, CIE L*/100 in thousandths, from a server older than the bands.
    Lightness(u16),
    /// Its mean encoded sRGB colour in bands of equal width from the output's left edge to its right, channels from 0
    /// to 65535.
    Bands(Vec<[u16; 3]>),
}

impl Backdrop {
    /// The relative luminance of the whole strip: that of the mean of its bands' colours.
    pub fn luminance(&self) -> f64 {
        match self {
            Backdrop::Lightness(l) => luminance_of_lightness(*l),
            Backdrop::Bands(bands) => {
                let weights = vec![1.0; bands.len()];
                luminance_of(bands, &weights)
            }
        }
    }

    /// The relative luminance of the strip between `x0` and `x1` of a bar `width` wide: that of the mean colour of the
    /// bands there, each counted by how much of it lies inside. A lightness alone is that of the whole strip.
    pub fn luminance_under(&self, x0: f64, x1: f64, width: f64) -> f64 {
        let Backdrop::Bands(bands) = self else {
            return self.luminance();
        };
        if bands.is_empty() || width <= 0.0 || x1 <= x0 {
            return self.luminance();
        }
        let band = width / bands.len() as f64;
        let weights: Vec<f64> = (0..bands.len())
            .map(|i| {
                let (b0, b1) = (i as f64 * band, (i + 1) as f64 * band);
                (x1.min(b1) - x0.max(b0)).max(0.0)
            })
            .collect();
        if weights.iter().sum::<f64>() <= 0.0 {
            return self.luminance();
        }
        luminance_of(bands, &weights)
    }
}

/// The relative luminance of the mean of `bands`, each weighed by `weights`: the mean of the encoded colour, made
/// linear, weighed as Rec. 709 weighs red, green and blue.
fn luminance_of(bands: &[[u16; 3]], weights: &[f64]) -> f64 {
    let total: f64 = weights.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let mean = [0, 1, 2].map(|c| {
        bands
            .iter()
            .zip(weights)
            .map(|(b, w)| f64::from(b[c]) * w)
            .sum::<f64>()
            / total
            / f64::from(u16::MAX)
    });
    let [r, g, b] = mean.map(linear);
    0.2126 * r + 0.7152 * g + 0.0722 * b
}

/// An encoded sRGB channel from 0 to 1, made linear.
fn linear(c: f64) -> f64 {
    let c = c.clamp(0.0, 1.0);
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// The relative luminance of CIE L* `lightness` thousandths.
fn luminance_of_lightness(lightness: u16) -> f64 {
    let l = f64::from(lightness.min(1000)) / 10.0;
    if l > 8.0 {
        ((l + 16.0) / 116.0).powi(3)
    } else {
        l / 903.3
    }
}

/// The text of a bar's strip: the appearance's, or dark or light for the wallpaper under the bar.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum BarTone {
    /// The palette of the appearance: before the server sent the wallpaper's measure (always below shell version 14),
    /// and while the bar is solid ([`solid_bar`], or the bar's own backdrop).
    #[default]
    Appearance,
    DarkText,
    LightText,
}

impl BarTone {
    /// The tone for a wallpaper of relative luminance `luminance` under the bar: light text below
    /// [`LIGHT_TEXT_BELOW`], dark text from it; the appearance's with no measure or on a solid bar.
    pub fn fresh(luminance: Option<f64>, display: DisplayOptions) -> BarTone {
        match luminance {
            None => BarTone::Appearance,
            Some(_) if solid_bar(display) => BarTone::Appearance,
            Some(y) if y < LIGHT_TEXT_BELOW => BarTone::LightText,
            Some(_) => BarTone::DarkText,
        }
    }

    /// The palette of the bar's strip in the appearance `dark`: the popup palette of the tone (the appearance's for
    /// [`BarTone::Appearance`]) with the bar's own text ([`bar_text`]), opaque with `increase_contrast`; the screen
    /// capture indicator keeps the appearance's indigo.
    pub fn palette(self, dark: bool, increase_contrast: bool) -> MenuPalette {
        let appearance = MenuPalette::new(dark);
        let light = self.light_text(dark);
        MenuPalette {
            text: bar_text(light, increase_contrast),
            screen_capture: appearance.screen_capture,
            ..MenuPalette::new(light)
        }
    }

    /// Whether the bar's strip has light text in the appearance `dark`, so it draws the light symbols of the extras.
    pub fn light_text(self, dark: bool) -> bool {
        match self {
            BarTone::Appearance => dark,
            BarTone::DarkText => false,
            BarTone::LightText => true,
        }
    }
}

/// Half the width of the band of luminance around [`LIGHT_TEXT_BELOW`] inside which a status item keeps the tone it
/// has, so an item over a wallpaper near the cut, or moved a little by its neighbours, does not change back and forth.
pub const SLOT_TONE_HOLD: f64 = 0.02;
/// How long, in seconds, the wallpaper under a status item must stay past the band before the item changes tone.
pub const SLOT_TONE_SETTLE: f64 = 0.25;

/// The tone of one status item over the wallpaper under it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SlotTone {
    pub tone: BarTone,
    /// The other tone the wallpaper under the item asks for, and since when.
    pending: Option<(BarTone, f64)>,
}

impl SlotTone {
    /// The tone of an item first seen over a wallpaper of relative luminance `luminance`: the bar's cut, with nothing
    /// held.
    pub fn fresh(luminance: f64) -> Self {
        Self {
            tone: if luminance < LIGHT_TEXT_BELOW {
                BarTone::LightText
            } else {
                BarTone::DarkText
            },
            pending: None,
        }
    }

    /// The tone after `self` over a wallpaper of `luminance` at `now` (seconds): inside the band the tone holds; past
    /// it the other tone waits [`SLOT_TONE_SETTLE`] and takes over only if the wallpaper still asks for it then.
    pub fn next(self, luminance: f64, now: f64) -> Self {
        let wanted = if luminance < LIGHT_TEXT_BELOW - SLOT_TONE_HOLD {
            BarTone::LightText
        } else if luminance >= LIGHT_TEXT_BELOW + SLOT_TONE_HOLD {
            BarTone::DarkText
        } else {
            self.tone
        };
        if wanted == self.tone {
            return Self {
                tone: self.tone,
                pending: None,
            };
        }
        match self.pending {
            Some((tone, since)) if tone == wanted && now - since >= SLOT_TONE_SETTLE => Self {
                tone: wanted,
                pending: None,
            },
            Some((tone, since)) if tone == wanted => Self {
                tone: self.tone,
                pending: Some((tone, since)),
            },
            _ => Self {
                tone: self.tone,
                pending: Some((wanted, now)),
            },
        }
    }

    /// When the tone waiting to take over may do so, if one waits.
    pub fn deadline(&self) -> Option<f64> {
        self.pending.map(|(_, since)| since + SLOT_TONE_SETTLE)
    }
}

/// Whether the slot of `key` is in the bar's status area, whose items may each follow the wallpaper under them.
pub fn in_status_area(key: &SlotKey) -> bool {
    matches!(
        key,
        SlotKey::Status(_) | SlotKey::ControlPanel | SlotKey::Clock
    )
}

/// The tone of each item of a bar's status area over the wallpaper under it, for a wallpaper that is not even across
/// the bar: an item over a light part has dark text while the rest of the bar has light text, and the other way round.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SlotTones {
    tones: Vec<(SlotKey, SlotTone)>,
}

impl SlotTones {
    /// Follow the wallpaper `backdrop` under the status-area slots of `slots`, in a bar laid out at `bar`, at `now`
    /// (seconds): an item seen before keeps its tone through [`SlotTone::next`], a new one takes [`SlotTone::fresh`],
    /// and an item gone is forgotten.
    pub fn update(&mut self, slots: &[Slot], bar: Rect, backdrop: &Backdrop, now: f64) {
        let old = std::mem::take(&mut self.tones);
        self.tones = slots
            .iter()
            .filter(|slot| in_status_area(&slot.key))
            .map(|slot| {
                let x0 = slot.rect.x - bar.x;
                let y = backdrop.luminance_under(x0, x0 + slot.rect.w, bar.w);
                let tone = match old.iter().find(|(key, _)| *key == slot.key) {
                    Some((_, held)) => held.next(y, now),
                    None => SlotTone::fresh(y),
                };
                (slot.key.clone(), tone)
            })
            .collect();
    }

    /// Forget every item's tone: the bar's own tone stands for all of them.
    pub fn clear(&mut self) {
        self.tones.clear();
    }

    /// The tone of the item of `key`, if it follows the wallpaper under it.
    pub fn tone(&self, key: &SlotKey) -> Option<BarTone> {
        self.tones
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, t)| t.tone)
    }

    /// The earliest time a tone waiting to take over may do so.
    pub fn deadline(&self) -> Option<f64> {
        self.tones
            .iter()
            .filter_map(|(_, t)| t.deadline())
            .reduce(f64::min)
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

    /// The encoded sRGB channel whose linear value is `y`.
    fn encode(y: f64) -> f64 {
        if y <= 0.003_130_8 {
            12.92 * y
        } else {
            1.055 * y.powf(1.0 / 2.4) - 0.055
        }
    }

    /// A band of encoded sRGB grey `v` (0 to 1).
    fn grey(v: f64) -> [u16; 3] {
        [(v * f64::from(u16::MAX)).round() as u16; 3]
    }

    /// `n` bands of encoded grey `v`.
    fn flat(v: f64, n: usize) -> Backdrop {
        Backdrop::Bands(vec![grey(v); n])
    }

    #[test]
    fn the_cut_is_at_luminance_0_334_of_the_mean_colour_with_no_band_around_it() {
        let cut = encode(LIGHT_TEXT_BELOW);
        let just_under = flat(cut - 0.001, 64).luminance();
        let just_over = flat(cut + 0.001, 64).luminance();
        assert!(just_under < LIGHT_TEXT_BELOW && just_over > LIGHT_TEXT_BELOW);
        assert_eq!(
            BarTone::fresh(Some(just_under), TRANSLUCENT),
            BarTone::LightText
        );
        assert_eq!(
            BarTone::fresh(Some(just_over), TRANSLUCENT),
            BarTone::DarkText
        );
        assert_eq!(
            BarTone::fresh(Some(LIGHT_TEXT_BELOW), TRANSLUCENT),
            BarTone::DarkText
        );
        // sRGB 151 is luminance 0.309: light text; sRGB 162 is 0.361: dark text
        let g151 = flat(151.0 / 255.0, 64).luminance();
        let g162 = flat(162.0 / 255.0, 64).luminance();
        assert!((g151 - 0.3095).abs() < 0.001, "{g151}");
        assert_eq!(BarTone::fresh(Some(g151), TRANSLUCENT), BarTone::LightText);
        assert_eq!(BarTone::fresh(Some(g162), TRANSLUCENT), BarTone::DarkText);
    }

    /// A strip half black and half white averages to encoded 0.5 (luminance 0.214, light text), not to the 0.5 the mean
    /// of each half's luminance gives; a gradient from black to white reads as its middle grey.
    #[test]
    fn a_split_or_graded_strip_is_measured_by_its_mean_encoded_colour() {
        let mut split = vec![grey(0.0); 32];
        split.extend(vec![grey(1.0); 32]);
        let y = Backdrop::Bands(split).luminance();
        assert!((y - 0.2140).abs() < 0.001, "{y}");
        assert_eq!(BarTone::fresh(Some(y), TRANSLUCENT), BarTone::LightText);
        let ramp: Vec<[u16; 3]> = (0..=64).map(|i| grey(f64::from(i) / 64.0)).collect();
        let y = Backdrop::Bands(ramp).luminance();
        assert!((y - 0.2140).abs() < 0.001, "{y}");
    }

    #[test]
    fn an_older_servers_lightness_reads_as_its_luminance() {
        assert_eq!(Backdrop::Lightness(0).luminance(), 0.0);
        assert!((Backdrop::Lightness(1000).luminance() - 1.0).abs() < 1e-9);
        // L* 50 is luminance 0.184; L* 70 is 0.408
        assert!((Backdrop::Lightness(500).luminance() - 0.1842).abs() < 0.001);
        assert!((Backdrop::Lightness(700).luminance() - 0.4075).abs() < 0.001);
        assert_eq!(
            Backdrop::Lightness(500).luminance_under(0.0, 10.0, 100.0),
            Backdrop::Lightness(500).luminance()
        );
    }

    #[test]
    fn the_luminance_under_an_item_weighs_the_bands_it_covers() {
        // four bands of 25 points: black, black, white, white
        let bands = Backdrop::Bands(vec![grey(0.0), grey(0.0), grey(1.0), grey(1.0)]);
        assert_eq!(bands.luminance_under(0.0, 50.0, 100.0), 0.0);
        assert!((bands.luminance_under(50.0, 100.0, 100.0) - 1.0).abs() < 1e-9);
        // half over black and half over white: encoded 0.5
        let y = bands.luminance_under(40.0, 60.0, 100.0);
        assert!((y - 0.2140).abs() < 0.001, "{y}");
        // nothing covered: the whole strip
        assert_eq!(
            bands.luminance_under(200.0, 220.0, 100.0),
            bands.luminance()
        );
    }

    #[test]
    fn without_a_measure_or_on_a_solid_bar_the_bar_keeps_the_appearance() {
        assert_eq!(BarTone::fresh(None, TRANSLUCENT), BarTone::Appearance);
        let solid = DisplayOptions {
            reduce_transparency: true,
            ..TRANSLUCENT
        };
        assert_eq!(BarTone::fresh(Some(0.9), solid), BarTone::Appearance);
        assert_eq!(BarTone::fresh(Some(0.01), solid), BarTone::Appearance);
        for dark in [false, true] {
            let palette = BarTone::Appearance.palette(dark, false);
            assert_eq!(palette.text, bar_text(dark, false), "dark {dark}");
            assert_eq!(palette.bar_highlight, MenuPalette::new(dark).bar_highlight);
        }
    }

    #[test]
    fn the_bar_text_is_black_at_85_or_white_at_90_and_opaque_with_increase_contrast() {
        assert_eq!(bar_text(false, false), [0.0, 0.0, 0.0, 0.85]);
        assert_eq!(bar_text(true, false), [1.0, 1.0, 1.0, 0.90]);
        assert_eq!(bar_text(false, true), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(bar_text(true, true), [1.0, 1.0, 1.0, 1.0]);
        for dark in [false, true] {
            for (tone, light) in [(BarTone::DarkText, false), (BarTone::LightText, true)] {
                assert_eq!(tone.palette(dark, false).text, bar_text(light, false));
                assert_eq!(tone.palette(dark, true).text, bar_text(light, true));
            }
            // the popups keep their own text: only the bar's changes
            let popup = MenuPalette::new(dark);
            assert_eq!(popup.high_contrast().text, popup.text);
        }
    }

    #[test]
    fn the_text_follows_the_backdrop_in_either_appearance() {
        for dark in [false, true] {
            let over_dark = BarTone::fresh(Some(0.05), TRANSLUCENT).palette(dark, false);
            let over_bright = BarTone::fresh(Some(0.8), TRANSLUCENT).palette(dark, false);
            assert_eq!(over_dark.text, BAR_LIGHT_TEXT, "dark {dark}");
            assert_eq!(over_bright.text, BAR_DARK_TEXT, "dark {dark}");
            assert_eq!(
                over_bright.bar_highlight,
                MenuPalette::new(false).bar_highlight,
                "the open title's pill flips with the text"
            );
            for palette in [over_dark, over_bright] {
                assert_eq!(
                    palette.screen_capture,
                    MenuPalette::new(dark).screen_capture
                );
            }
        }
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
        assert_eq!(BarTone::fresh(Some(0.05), contrast), BarTone::LightText);
        assert_eq!(BarTone::fresh(Some(0.8), contrast), BarTone::DarkText);
    }

    #[test]
    fn the_extras_symbols_follow_the_tone_not_the_appearance() {
        for dark in [false, true] {
            assert!(BarTone::LightText.light_text(dark), "dark {dark}");
            assert!(!BarTone::DarkText.light_text(dark), "dark {dark}");
            assert_eq!(BarTone::Appearance.light_text(dark), dark);
        }
    }

    #[test]
    fn only_light_status_symbols_have_a_faint_shadow_below_them() {
        assert_eq!(
            status_shadow(BarTone::LightText, false),
            Some([0.0, 0.0, 0.0, 0.3])
        );
        assert_eq!(status_shadow(BarTone::DarkText, true), None);
        assert_eq!(status_shadow(BarTone::Appearance, false), None);
        assert_eq!(
            status_shadow(BarTone::Appearance, true),
            Some([0.0, 0.0, 0.0, 0.3])
        );
        assert_eq!((STATUS_SHADOW_BLUR, STATUS_SHADOW_DROP), (2.5, 1.75));
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

    /// Over an even grey well under the cut (luminance below 0.15) or well over it (above 0.434) the text the cut picks
    /// reads better than the other one and clears 4.5:1. Between them the cut, not the better contrast, decides.
    #[test]
    fn the_text_the_cut_picks_reads_better_away_from_the_cut() {
        for i in 0..=255 {
            let v = f64::from(i) / 255.0;
            let y = flat(v, 8).luminance();
            let tone = BarTone::fresh(Some(y), TRANSLUCENT);
            let c = |t: BarTone| wcag(t.palette(false, false).text, [v; 3]);
            let (picked, other) = match tone {
                BarTone::LightText => (c(BarTone::LightText), c(BarTone::DarkText)),
                _ => (c(BarTone::DarkText), c(BarTone::LightText)),
            };
            if !(0.15..=LIGHT_TEXT_BELOW + 0.1).contains(&y) {
                assert!(picked > other, "grey {i}: {picked:.2} {other:.2}");
                assert!(picked >= 4.5, "grey {i}: {picked:.2}");
            }
        }
    }

    #[test]
    fn a_status_items_tone_holds_inside_a_narrow_band_and_changes_once_settled() {
        let (lo, hi) = (
            LIGHT_TEXT_BELOW - SLOT_TONE_HOLD,
            LIGHT_TEXT_BELOW + SLOT_TONE_HOLD,
        );
        assert!(SLOT_TONE_HOLD <= 0.025, "the band stays narrow");
        let light = SlotTone::fresh(LIGHT_TEXT_BELOW - 0.001);
        assert_eq!(light.tone, BarTone::LightText);
        assert_eq!(
            SlotTone::fresh(LIGHT_TEXT_BELOW).tone,
            BarTone::DarkText,
            "a new item takes the bar's cut"
        );
        // inside the band it holds, with nothing waiting
        let held = light.next(hi - 0.001, 0.0);
        assert_eq!((held.tone, held.deadline()), (BarTone::LightText, None));
        // past it the dark tone waits, and takes over only once settled
        let waiting = light.next(hi + 0.01, 1.0);
        assert_eq!(waiting.tone, BarTone::LightText);
        assert_eq!(waiting.deadline(), Some(1.0 + SLOT_TONE_SETTLE));
        let early = waiting.next(hi + 0.01, 1.0 + SLOT_TONE_SETTLE / 2.0);
        assert_eq!(early.tone, BarTone::LightText);
        let settled = early.next(hi + 0.01, 1.0 + SLOT_TONE_SETTLE);
        assert_eq!(
            (settled.tone, settled.deadline()),
            (BarTone::DarkText, None)
        );
        // a reading that comes back before then cancels the wait
        let back = waiting.next(lo + 0.001, 1.1);
        assert_eq!((back.tone, back.deadline()), (BarTone::LightText, None));
        let again = back.next(hi + 0.01, 1.0 + SLOT_TONE_SETTLE);
        assert_eq!(again.tone, BarTone::LightText, "the wait starts over");
    }

    /// A reading wobbling across the cut, but inside the band, never changes the tone however long it goes on.
    #[test]
    fn a_wobbling_reading_does_not_flicker() {
        let mut tone = SlotTone::fresh(0.2);
        let mut changes = 0;
        for i in 0..1000 {
            let y = LIGHT_TEXT_BELOW + if i % 2 == 0 { 0.015 } else { -0.015 };
            let next = tone.next(y, f64::from(i) * 0.016);
            changes += usize::from(next.tone != tone.tone);
            tone = next;
        }
        assert_eq!(changes, 0);
        // past the band in both directions every frame: the wait never settles
        let mut tone = SlotTone::fresh(0.2);
        for i in 0..1000 {
            let y = if i % 2 == 0 { 0.5 } else { 0.1 };
            tone = tone.next(y, f64::from(i) * 0.016);
            assert_eq!(tone.tone, BarTone::LightText, "frame {i}");
        }
    }

    fn slot(key: SlotKey, x: f64, w: f64) -> Slot {
        Slot {
            key,
            rect: Rect::new(x, 0.0, w, 24.0),
            bold: false,
        }
    }

    /// Over a wallpaper dark on the left and light on the right, the status items over the light half take dark text
    /// and those over the dark half light text; the menus are not followed; an item gone is forgotten.
    #[test]
    fn each_status_item_takes_the_tone_of_the_wallpaper_under_it() {
        let mut bands = vec![grey(0.1); 50];
        bands.extend(vec![grey(0.95); 50]);
        let backdrop = Backdrop::Bands(bands);
        let bar = Rect::new(0.0, 0.0, 1000.0, 24.0);
        let slots = [
            slot(SlotKey::AppMenu, 10.0, 60.0),
            slot(SlotKey::Status("dark".into()), 400.0, 30.0),
            slot(SlotKey::Status("light".into()), 700.0, 30.0),
            slot(SlotKey::ControlPanel, 900.0, 32.0),
            slot(SlotKey::Clock, 932.0, 60.0),
        ];
        let mut tones = SlotTones::default();
        tones.update(&slots, bar, &backdrop, 0.0);
        assert_eq!(tones.tone(&SlotKey::AppMenu), None);
        assert_eq!(
            tones.tone(&SlotKey::Status("dark".into())),
            Some(BarTone::LightText)
        );
        assert_eq!(
            tones.tone(&SlotKey::Status("light".into())),
            Some(BarTone::DarkText)
        );
        assert_eq!(tones.tone(&SlotKey::ControlPanel), Some(BarTone::DarkText));
        assert_eq!(tones.tone(&SlotKey::Clock), Some(BarTone::DarkText));
        assert_eq!(tones.deadline(), None);
        // the dark item moves over the light half: it waits, then follows
        let moved = [slot(SlotKey::Status("dark".into()), 600.0, 30.0)];
        tones.update(&moved, bar, &backdrop, 1.0);
        assert_eq!(
            tones.tone(&SlotKey::Status("dark".into())),
            Some(BarTone::LightText)
        );
        assert_eq!(tones.tone(&SlotKey::ControlPanel), None, "gone");
        let due = tones.deadline().unwrap();
        tones.update(&moved, bar, &backdrop, due);
        assert_eq!(
            tones.tone(&SlotKey::Status("dark".into())),
            Some(BarTone::DarkText)
        );
        tones.clear();
        assert_eq!(tones.tone(&SlotKey::Status("dark".into())), None);
    }

    /// Over an even wallpaper every status item has the bar's own tone.
    #[test]
    fn over_an_even_wallpaper_the_items_keep_the_bars_tone() {
        for v in [0.1, 0.5, 0.6, 0.62, 0.9] {
            let backdrop = flat(v, 128);
            let bar_tone = BarTone::fresh(Some(backdrop.luminance()), TRANSLUCENT);
            let mut tones = SlotTones::default();
            let slots: Vec<Slot> = (0..10)
                .map(|i| {
                    slot(
                        SlotKey::Status(i.to_string()),
                        1000.0 + f64::from(i) * 37.0,
                        31.0,
                    )
                })
                .collect();
            tones.update(&slots, Rect::new(0.0, 0.0, 1512.0, 24.0), &backdrop, 0.0);
            for s in &slots {
                assert_eq!(tones.tone(&s.key), Some(bar_tone), "{v}");
            }
        }
    }
}
