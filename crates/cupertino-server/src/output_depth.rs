//! The bit depth of a display's primary plane: 8 bits a channel by default, 10 (`Abgr2101010`) when the
//! configuration's `output_depth` is 10. The formats are tried in order when the display is set up, so a plane or
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
    #[default]
    Eight,
    Ten,
}

impl Depth {
    /// The depth `output_depth` asks for: 10 bits for 10, 8 bits for anything else or nothing.
    pub fn from_setting(bits: Option<u8>) -> Depth {
        match bits {
            Some(10) => Depth::Ten,
            _ => Depth::Eight,
        }
    }

    /// The formats a primary plane is set up with, in the order they are tried.
    pub fn primary_formats(self) -> &'static [Fourcc] {
        match self {
            Depth::Eight => &EIGHT_BIT,
            Depth::Ten => &TEN_BIT,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plane_is_8_bits_unless_the_setting_asks_for_10() {
        assert_eq!(Depth::from_setting(None), Depth::Eight);
        assert_eq!(Depth::from_setting(Some(8)), Depth::Eight);
        assert_eq!(Depth::from_setting(Some(12)), Depth::Eight);
        assert_eq!(Depth::from_setting(Some(10)), Depth::Ten);
        assert_eq!(Depth::default(), Depth::Eight);
    }

    #[test]
    fn a_10_bit_plane_tries_the_10_bit_formats_first_and_falls_back_to_the_8_bit_ones() {
        assert_eq!(
            Depth::Eight.primary_formats(),
            [Fourcc::Abgr8888, Fourcc::Argb8888]
        );
        let ten = Depth::Ten.primary_formats();
        assert_eq!(
            ten[0],
            Fourcc::Abgr2101010,
            "the format the target's plane offers"
        );
        assert!(ten[..2].iter().all(|&f| format_bits(f) == 10));
        assert_eq!(&ten[2..], Depth::Eight.primary_formats(), "then 8 bits");
        assert_eq!(Depth::Ten.bits(), 10);
        assert_eq!(Depth::Eight.bits(), 8);
    }

    /// The server's sources, path and text.
    fn sources() -> Vec<(std::path::PathBuf, String)> {
        let mut found = Vec::new();
        let mut dirs = vec![std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")];
        while let Some(dir) = dirs.pop() {
            for entry in std::fs::read_dir(&dir).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    let text = std::fs::read_to_string(&path).unwrap();
                    found.push((path, text));
                }
            }
        }
        found
    }

    /// The arguments of the call whose name ends at the start of `rest`, up to its closing parenthesis.
    fn arguments(rest: &str) -> &str {
        let mut depth = 1;
        let end = rest
            .char_indices()
            .find(|&(_, c)| {
                depth += match c {
                    '(' => 1,
                    ')' => -1,
                    _ => 0,
                };
                depth == 0
            })
            .map_or(rest.len(), |(i, _)| i);
        &rest[..end]
    }

    #[test]
    fn every_capture_reads_back_8_bits_whatever_the_plane_s_depth() {
        assert_eq!(format_bits(CAPTURE_FORMAT), 8);
        let eight = ["CAPTURE_FORMAT", "MEMORY_FORMAT", "Fourcc::Abgr8888"];
        let sources = sources();
        let mut readbacks = 0;
        for (path, text) in &sources {
            if path.ends_with("output_depth.rs") {
                continue;
            }
            for call in ["copy_framebuffer(", "copy_texture("] {
                for (at, _) in text.match_indices(call) {
                    let args = arguments(&text[at + call.len()..]);
                    assert!(
                        eight.iter().any(|f| args.contains(f)) && !args.contains("2101010"),
                        "{}: a read back in a format of the plane's depth: {args}",
                        path.display()
                    );
                    readbacks += 1;
                }
            }
        }
        assert!(
            readbacks >= 3,
            "the screenshot, window and copy read backs were found"
        );
        let (_, copy) = sources
            .iter()
            .find(|(p, _)| p.ends_with("backend/copy.rs"))
            .unwrap();
        assert!(copy.contains("const MEMORY_FORMAT: Fourcc = Fourcc::Abgr8888;"));
    }
}
