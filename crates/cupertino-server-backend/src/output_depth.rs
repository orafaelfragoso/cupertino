//! The bit depth of a display's primary plane: 10 bits a channel (`Abgr2101010`) by default, 8 when the
//! configuration's `output_depth` is 8. The formats are tried in order when the display is set up, so a plane or
//! driver without a 10-bit format falls back to 8 bits.
//!
//! Captures never read the plane. A screenshot, a screen share and a recording each draw the scene into a target of
//! their own in [`CAPTURE_FORMAT`] and read that back, so their bytes are 8 bits a channel whatever the plane's depth,
//! and a 10-bit plane changes nothing in them.

use smithay::backend::allocator::Fourcc;

/// The format every capture draws into and reads back from, whatever the plane's depth.
pub const CAPTURE_FORMAT: Fourcc = Fourcc::Abgr8888;

/// The formats of a primary plane of 8 bits a channel, in the order they are tried.
const EIGHT_BIT: [Fourcc; 2] = [Fourcc::Abgr8888, Fourcc::Argb8888];

/// The formats of a 10-bit primary plane, in the order they are tried; the 8-bit formats follow, for a plane that has
/// no 10-bit format.
const TEN_BIT: [Fourcc; 4] = [
    Fourcc::Abgr2101010,
    Fourcc::Argb2101010,
    Fourcc::Abgr8888,
    Fourcc::Argb8888,
];

/// The depth of the primary planes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Depth {
    Eight,
    #[default]
    Ten,
}

impl Depth {
    /// The depth `output_depth` asks for: 8 bits for 8, 10 bits for anything else or nothing.
    pub fn from_setting(bits: Option<u8>) -> Depth {
        match bits {
            Some(8) => Depth::Eight,
            _ => Depth::Ten,
        }
    }

    /// The formats a primary plane is set up with, in the order they are tried.
    pub fn primary_formats(self) -> &'static [Fourcc] {
        match self {
            Depth::Eight => &EIGHT_BIT,
            Depth::Ten => &TEN_BIT,
        }
    }

    /// The format a primary plane offering `offered` gets: the first of [`Depth::primary_formats`] it offers, as the
    /// display's setup tries them; `None` when it offers none of them.
    pub fn format_on(self, offered: &[Fourcc]) -> Option<Fourcc> {
        self.primary_formats()
            .iter()
            .copied()
            .find(|f| offered.contains(f))
    }

    /// Bits a channel.
    pub fn bits(self) -> u8 {
        match self {
            Depth::Eight => 8,
            Depth::Ten => 10,
        }
    }
}

/// Bits a colour channel of a primary plane format has, for the log.
pub fn format_bits(format: Fourcc) -> u8 {
    match format {
        Fourcc::Abgr2101010 | Fourcc::Argb2101010 | Fourcc::Xbgr2101010 | Fourcc::Xrgb2101010 => 10,
        _ => 8,
    }
}
