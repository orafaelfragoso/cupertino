//! The overlay model: what is shown, how full its meter is, and the show / hold / fade lifetime.
//!
//! Pure logic; time is injected as seconds on any monotonic clock.

use std::fmt;

use cupertino_kit::motion::Timing;

/// Segments of the meter (16 segments).
pub const SEGMENTS: usize = 16;

/// Width of one segment in points (9 x 6).
pub const SEGMENT_WIDTH: u32 = 9;

/// The fade after the hold: the catalogue's `Timing::OSD_FADE`, constant speed over 0.7 s (alpha `1 - t / 0.7`).
pub fn fade() -> Timing {
    Timing::OSD_FADE
}

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

/// The lifetime of the overlay: a show gives alpha 1 for the hold time the sender supplies, then the [`fade`],
/// then nothing. A new show replaces the one on screen at any point and restarts the hold
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
            Some(shown) if now < shown.at + shown.hold + fade().duration() => Phase::Fading,
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
                alpha: 1.0 - fade().value(faded).clamp(0.0, 1.0),
            }),
        }
    }

    /// Seconds from `now` until the fade starts; `None` unless holding.
    pub fn until_fade(&self, now: f64) -> Option<f64> {
        let shown = self.shown?;
        (self.phase(now) == Phase::Holding).then_some(shown.at + shown.hold - now)
    }
}
