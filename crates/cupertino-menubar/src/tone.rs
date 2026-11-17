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
