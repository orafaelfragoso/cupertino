//! The overlay model: what is shown, how full its meter is, and the show / hold / fade lifetime.
//!
//! Pure logic; time is injected as seconds on any monotonic clock.

use std::fmt;

/// Segments of the meter (16 segments).
pub const SEGMENTS: usize = 16;

/// Width of one segment in points (9 x 6).
pub const SEGMENT_WIDTH: u32 = 9;

/// Linear fade after the hold, in seconds (alpha `1 - t / 0.7`, 0.7 s).
pub const FADE_SECONDS: f64 = 0.7;

/// Below this level a volume shows the muted glyph.
pub const MUTE_BELOW: f64 = 0.001;

/// The glyph switches from low to medium at a third and from medium to high at two thirds.
const LOW_BELOW: f64 = 1.0 / 3.0;
const MEDIUM_BELOW: f64 = 2.0 / 3.0;

/// Which overlay is shown (graphic names Volume, Mute).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Output volume.
    Volume,
    /// Output muted.
    Mute,
    /// Display brightness.
    Brightness,
    /// Keyboard backlight brightness.
    KeyboardBrightness,
    /// Input level, unmuted.
    Microphone,
    /// Input muted.
    MicrophoneMute,
}

impl Kind {
    /// The name used on the bus (`Show`).
    pub fn name(self) -> &'static str {
        match self {
            Kind::Volume => "volume",
            Kind::Mute => "mute",
            Kind::Brightness => "brightness",
            Kind::KeyboardBrightness => "keyboard-brightness",
            Kind::Microphone => "microphone",
            Kind::MicrophoneMute => "microphone-mute",
        }
    }

    /// The kind of a bus name.
    pub fn from_name(name: &str) -> Option<Kind> {
        [
            Kind::Volume,
            Kind::Mute,
            Kind::Brightness,
            Kind::KeyboardBrightness,
            Kind::Microphone,
            Kind::MicrophoneMute,
        ]
        .into_iter()
        .find(|kind| kind.name() == name)
    }
}

/// A level outside 0 to 1 (or not a number).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InvalidLevel(pub f64);

impl fmt::Display for InvalidLevel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "level {} is not a number from 0 to 1", self.0)
    }
}

impl std::error::Error for InvalidLevel {}

/// What the overlay shows: a glyph and a meter filled to `level`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlay {
    kind: Kind,
    level: f64,
}

impl Overlay {
    /// An overlay of `kind` at `level` (0 to 1).
    pub fn new(kind: Kind, level: f64) -> Result<Self, InvalidLevel> {
        if !(0.0..=1.0).contains(&level) {
            return Err(InvalidLevel(level));
        }
        Ok(Self { kind, level })
    }

    /// Which overlay.
    pub fn kind(&self) -> Kind {
        self.kind
    }

    /// The level of the meter, 0 to 1.
    pub fn level(&self) -> f64 {
        self.level
    }

    /// The text under the glyph. None of the four overlays has one: the design label's content for them was not
    /// (gives its font and frame only).
    pub fn label(&self) -> Option<&'static str> {
        None
    }

    /// The asset-directory name of the glyph's template symbol. The design glyphs `Volume`,
    /// `Mute`, and are named but their artwork is not known, so the symbols
    /// are approximate; the tiers of the volume glyph follow the control panel's speaker (a third and two thirds,
    /// for this overlay) and shows at level 0 (`25 + (level <= 0)).
    pub fn icon_name(&self) -> &'static str {
        match self.kind {
            Kind::Mute => "symbol.speaker.slash",
            Kind::Volume if self.level < MUTE_BELOW => "symbol.speaker.slash",
            Kind::Volume if self.level < LOW_BELOW => "symbol.speaker.low",
            Kind::Volume if self.level < MEDIUM_BELOW => "symbol.speaker.medium",
            Kind::Volume => "symbol.speaker.high",
            Kind::Brightness => "symbol.sun",
            Kind::KeyboardBrightness if self.level <= 0.0 => "symbol.keyboard-brightness.off",
            Kind::KeyboardBrightness => "symbol.keyboard-brightness",
            Kind::Microphone => "symbol.mic",
            Kind::MicrophoneMute => "symbol.mic.slash",
        }
    }
}

/// Every symbol [`Overlay::icon_name`] can return, which the asset directory must provide.
pub const ICON_NAMES: [&str; 9] = [
    "symbol.speaker.slash",
    "symbol.speaker.low",
    "symbol.speaker.medium",
    "symbol.speaker.high",
    "symbol.sun",
    "symbol.keyboard-brightness",
    "symbol.keyboard-brightness.off",
    "symbol.mic",
    "symbol.mic.slash",
];

/// How one segment of the meter is filled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fill {
    /// All 9 points.
    Full,
    /// The leftmost 1 to 8 points.
    Partial(u32),
    /// Nothing.
    Empty,
}

/// The fill of the 16 segments at `level`: `v = level * 16`; the first `floor(v) segments are
/// full and the next one is filled `(int)(frac * 9 + 0.5) points wide, so a width of 0 is empty and of 9 is full.
pub fn segments(level: f64) -> [Fill; SEGMENTS] {
    let position = level.clamp(0.0, 1.0) * SEGMENTS as f64;
    let whole = position.floor() as usize;
    let partial = ((position - whole as f64) * f64::from(SEGMENT_WIDTH) + 0.5) as u32;
    std::array::from_fn(|i| {
        if i < whole {
            Fill::Full
        } else if i > whole {
            Fill::Empty
        } else {
            match partial {
                0 => Fill::Empty,
                SEGMENT_WIDTH.. => Fill::Full,
                width => Fill::Partial(width),
            }
        }
    })
}

/// Where the overlay is in its lifetime.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    /// Nothing is shown.
    Hidden,
    /// Shown at full opacity until the hold time passes.
    Holding,
    /// Fading out.
    Fading,
}

/// What to draw now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// The overlay.
    pub overlay: Overlay,
    /// Opacity, 0 to 1.
    pub alpha: f64,
}

#[derive(Clone, Copy, Debug)]
struct Shown {
    overlay: Overlay,
    at: f64,
    hold: f64,
}

/// The lifetime of the overlay: a show gives alpha 1 for the hold time the sender supplies, then a linear
/// fade over [`FADE_SECONDS`], then nothing. A new show replaces the one on screen at any point and restarts the hold
/// (all four overlays share one priority, so the design rule "a lower priority never replaces a visible one"
/// never rejects here).
#[derive(Clone, Copy, Debug, Default)]
pub struct Hud {
    shown: Option<Shown>,
}

impl Hud {
    /// A hidden overlay.
    pub fn new() -> Self {
        Self { shown: None }
    }

    /// Show `overlay` at `now` and hold it `hold` seconds before the fade.
    pub fn show(&mut self, now: f64, overlay: Overlay, hold: f64) {
        self.shown = Some(Shown {
            overlay,
            at: now,
            hold,
        });
    }

    /// The phase at `now`.
    pub fn phase(&self, now: f64) -> Phase {
        match self.shown {
            None => Phase::Hidden,
            Some(shown) if now < shown.at + shown.hold => Phase::Holding,
            Some(shown) if now < shown.at + shown.hold + FADE_SECONDS => Phase::Fading,
            Some(_) => Phase::Hidden,
        }
    }

    /// What to draw at `now`; `None` when hidden.
    pub fn view(&self, now: f64) -> Option<View> {
        let shown = self.shown?;
        let faded = now - (shown.at + shown.hold);
        match self.phase(now) {
            Phase::Hidden => None,
            Phase::Holding => Some(View {
                overlay: shown.overlay,
                alpha: 1.0,
            }),
            Phase::Fading => Some(View {
                overlay: shown.overlay,
                alpha: 1.0 - faded / FADE_SECONDS,
            }),
        }
    }

    /// Seconds from `now` until the fade starts; `None` unless holding.
    pub fn until_fade(&self, now: f64) -> Option<f64> {
        let shown = self.shown?;
        (self.phase(now) == Phase::Holding).then_some(shown.at + shown.hold - now)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn volume(level: f64) -> Overlay {
        Overlay::new(Kind::Volume, level).unwrap()
    }

    #[test]
    fn meter_is_empty_at_zero_and_full_at_one() {
        assert_eq!(segments(0.0), [Fill::Empty; SEGMENTS]);
        assert_eq!(segments(1.0), [Fill::Full; SEGMENTS]);
    }

    #[test]
    fn half_level_fills_eight_whole_segments() {
        let fill = segments(0.5);
        assert_eq!(fill[..8], [Fill::Full; 8]);
        assert_eq!(fill[8..], [Fill::Empty; 8]);
    }

    #[test]
    fn the_next_segment_is_filled_by_the_rounded_fraction() {
        // 8.5 segments: (int)(0.5 * 9 + 0.5) = 5 points
        assert_eq!(segments(0.53125)[8], Fill::Partial(5));
        // a quarter step has 4 distinct widths: 0.25 -> 2.25 + 0.5 = 2, 0.5 -> 5, 0.75 -> 7, 1.0 -> full
        assert_eq!(
            [0.25, 0.5, 0.75].map(|frac| segments((3.0 + frac) / 16.0)[3]),
            [Fill::Partial(2), Fill::Partial(5), Fill::Partial(7)]
        );
    }

    #[test]
    fn a_fraction_that_rounds_to_nine_points_is_a_full_segment() {
        // 0.95 * 9 + 0.5 = 9.05
        assert_eq!(segments((3.0 + 0.95) / 16.0)[3], Fill::Full);
        assert_eq!(segments((3.0 + 0.04) / 16.0)[3], Fill::Empty);
    }

    #[test]
    fn every_glyph_is_in_the_required_list() {
        let kinds = [
            Kind::Volume,
            Kind::Mute,
            Kind::Brightness,
            Kind::KeyboardBrightness,
            Kind::Microphone,
            Kind::MicrophoneMute,
        ];
        for kind in kinds {
            for level in [0.0, 0.0005, 0.2, 0.5, 0.9, 1.0] {
                let name = Overlay::new(kind, level).unwrap().icon_name();
                assert!(ICON_NAMES.contains(&name), "{name}");
            }
        }
    }

    #[test]
    fn glyph_follows_the_level_tiers() {
        assert_eq!(volume(0.0).icon_name(), "symbol.speaker.slash");
        assert_eq!(volume(0.0005).icon_name(), "symbol.speaker.slash");
        assert_eq!(volume(0.2).icon_name(), "symbol.speaker.low");
        assert_eq!(volume(0.5).icon_name(), "symbol.speaker.medium");
        assert_eq!(volume(0.9).icon_name(), "symbol.speaker.high");
        assert_eq!(
            Overlay::new(Kind::Mute, 0.0).unwrap().icon_name(),
            "symbol.speaker.slash"
        );
        assert_eq!(
            Overlay::new(Kind::Brightness, 0.5).unwrap().icon_name(),
            "symbol.sun"
        );
        assert_eq!(
            Overlay::new(Kind::KeyboardBrightness, 0.5)
                .unwrap()
                .icon_name(),
            "symbol.keyboard-brightness"
        );
        assert_eq!(
            Overlay::new(Kind::KeyboardBrightness, 0.0)
                .unwrap()
                .icon_name(),
            "symbol.keyboard-brightness.off"
        );
    }

    #[test]
    fn levels_outside_the_range_are_refused() {
        assert_eq!(Overlay::new(Kind::Volume, 1.5), Err(InvalidLevel(1.5)));
        assert!(Overlay::new(Kind::Volume, f64::NAN).is_err());
        assert!(Overlay::new(Kind::Volume, -0.1).is_err());
    }

    #[test]
    fn kinds_round_trip_through_their_bus_names() {
        assert_eq!(
            Kind::from_name("keyboard-brightness"),
            Some(Kind::KeyboardBrightness)
        );
        assert_eq!(Kind::from_name("Volume"), None);
    }

    #[test]
    fn lifetime_holds_then_fades_linearly_then_hides() {
        let mut hud = Hud::new();
        assert_eq!(hud.view(0.0), None);
        hud.show(10.0, volume(0.5), 1.0);
        assert_eq!(
            hud.view(10.0),
            Some(View {
                overlay: volume(0.5),
                alpha: 1.0
            })
        );
        assert_eq!(hud.phase(10.999), Phase::Holding);
        assert_eq!(hud.until_fade(10.25), Some(0.75));
        // the fade starts at 11.0; half way through it (0.35 s) alpha is 0.5
        assert_eq!(hud.phase(11.0), Phase::Fading);
        assert_eq!(hud.until_fade(11.0), None);
        let mid = hud.view(11.35).unwrap().alpha;
        assert!((mid - 0.5).abs() < 1e-9, "{mid}");
        // the fade ends at 11.7
        assert_eq!(hud.phase(11.7), Phase::Hidden);
        assert_eq!(hud.view(11.7), None);
    }

    #[test]
    fn a_show_during_the_fade_restarts_the_hold_at_full_alpha() {
        let mut hud = Hud::new();
        hud.show(10.0, volume(0.5), 1.0);
        assert!(hud.view(11.35).unwrap().alpha < 0.6);
        hud.show(11.35, volume(0.5625), 1.0);
        assert_eq!(
            hud.view(11.35),
            Some(View {
                overlay: volume(0.5625),
                alpha: 1.0
            })
        );
        assert_eq!(hud.phase(12.34), Phase::Holding);
        assert_eq!(hud.phase(12.35), Phase::Fading);
    }

    #[test]
    fn a_show_during_the_hold_extends_it() {
        let mut hud = Hud::new();
        hud.show(10.0, volume(0.5), 1.0);
        hud.show(10.8, volume(0.6), 1.0);
        assert_eq!(hud.phase(11.5), Phase::Holding);
        assert_eq!(hud.view(11.5).unwrap().overlay.level(), 0.6);
    }
}
