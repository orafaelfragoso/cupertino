//! Tags stored in the extended attribute `user.xdg.tags` (comma separated names, the convention of
//! the freedesktop-oriented tools), and the seven label colours of the design.
//!
//! The design stores tags in a binary property list (`..._kMDItemUserTags`, one string
//! `"Name\n<colour index>"` per tag); [`parse_reference_tags`] reads that format so that volumes
//! coming from the design can be shown.

use std::collections::BTreeMap;
use std::io;
use std::path::Path;

pub const XATTR_NAME: &str = "user.xdg.tags";

/// The colour indexes of the design: 1 gray, 2 green, 3 purple, 4 blue, 5 yellow, 6 red, 7 orange.
/// table of the system colour accessors selected by index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum TagColor {
    Gray = 1,
    Green = 2,
    Purple = 3,
    Blue = 4,
    Yellow = 5,
    Red = 6,
    Orange = 7,
}

impl TagColor {
    pub const ALL: [TagColor; 7] = [
        TagColor::Gray,
        TagColor::Green,
        TagColor::Purple,
        TagColor::Blue,
        TagColor::Yellow,
        TagColor::Red,
        TagColor::Orange,
    ];

    pub fn from_index(i: u32) -> Option<Self> {
        Self::ALL.iter().copied().find(|c| *c as u32 == i)
    }

    /// Name of the semantic system colour used to draw the tag (`cupertino_theme::SemanticColor`).
    pub fn system_color_name(self) -> &'static str {
        match self {
            TagColor::Gray => "system-gray",
            TagColor::Green => "system-green",
            TagColor::Purple => "system-purple",
            TagColor::Blue => "system-blue",
            TagColor::Yellow => "system-yellow",
            TagColor::Red => "system-red",
            TagColor::Orange => "system-orange",
        }
    }

    /// English display name, also the name of the default tag of that colour.
    pub fn title(self) -> &'static str {
        match self {
            TagColor::Gray => "Gray",
            TagColor::Green => "Green",
            TagColor::Purple => "Purple",
            TagColor::Blue => "Blue",
            TagColor::Yellow => "Yellow",
            TagColor::Red => "Red",
            TagColor::Orange => "Orange",
        }
    }

    pub fn from_title(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|c| c.title().eq_ignore_ascii_case(name))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Tag {
    pub name: String,
    pub color: Option<TagColor>,
}

/// Colours of user-defined tags (the extended attribute only stores names).
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct TagRegistry {
    pub colors: BTreeMap<String, TagColor>,
}

impl TagRegistry {
    pub fn color_of(&self, name: &str) -> Option<TagColor> {
        self.colors
            .get(name)
            .copied()
            .or_else(|| TagColor::from_title(name))
    }
    pub fn resolve(&self, names: &[String]) -> Vec<Tag> {
        names
            .iter()
            .map(|n| Tag {
                name: n.clone(),
                color: self.color_of(n),
            })
            .collect()
    }
}

pub fn parse_tag_list(value: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(value)
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn format_tag_list(tags: &[String]) -> String {
    tags.iter()
        .map(|t| t.replace(',', " "))
        .collect::<Vec<_>>()
        .join(",")
}

/// Read the tags of `path`; an absent attribute gives an empty list. A symbolic link's tags are its target's: the
/// system keeps no user attributes on a link itself.
pub fn read_tags(path: &Path) -> io::Result<Vec<String>> {
    match xattr::get_deref(path, XATTR_NAME)? {
        Some(v) => Ok(parse_tag_list(&v)),
        None => Ok(Vec::new()),
    }
}

/// Replace the tags; an empty list removes the attribute. Through a symbolic link, its target's tags are replaced.
pub fn write_tags(path: &Path, tags: &[String]) -> io::Result<()> {
    if tags.is_empty() {
        match xattr::remove_deref(path, XATTR_NAME) {
            Ok(()) => Ok(()),
            Err(e) if e.raw_os_error() == Some(libc::ENODATA) => Ok(()),
            Err(e) => Err(e),
        }
    } else {
        xattr::set_deref(path, XATTR_NAME, format_tag_list(tags).as_bytes())
    }
}

pub fn add_tag(path: &Path, tag: &str) -> io::Result<()> {
    let mut t = read_tags(path)?;
    if !t.iter().any(|x| x == tag) {
        t.push(tag.to_string());
    }
    write_tags(path, &t)
}

/// Take the tag `tag` off `path`, in any case, as a tag is matched when the file manager asks whether an item carries
/// it.
pub fn remove_tag(path: &Path, tag: &str) -> io::Result<()> {
    let mut t = read_tags(path)?;
    t.retain(|x| !x.eq_ignore_ascii_case(tag));
    write_tags(path, &t)
}

/// Parse a binary property list value (array of `"Name\nN" strings). Only the subset
/// needed (a flat array of ASCII / UTF-16 strings) is supported.
pub fn parse_reference_tags(data: &[u8]) -> Option<Vec<Tag>> {
    if data.len() < 40 || &data[..8] != b"bplist00" {
        return None;
    }
    let trailer = &data[data.len() - 32..];
    let off_size = trailer[6] as usize;
    let ref_size = trailer[7] as usize;
    let num_objects = u64::from_be_bytes(trailer[8..16].try_into().ok()?) as usize;
    let top = u64::from_be_bytes(trailer[16..24].try_into().ok()?) as usize;
    let table = u64::from_be_bytes(trailer[24..32].try_into().ok()?) as usize;
    let be = |bytes: &[u8]| bytes.iter().fold(0usize, |a, b| (a << 8) | *b as usize);
    let offset_of = |i: usize| -> Option<usize> {
        let s = table + i * off_size;
        Some(be(data.get(s..s + off_size)?))
    };
    let read_len = |pos: usize, low: u8| -> Option<(usize, usize)> {
        if low != 0xF {
            return Some((low as usize, pos + 1));
        }
        let marker = *data.get(pos + 1)?;
        let n = 1usize << (marker & 0xF);
        Some((be(data.get(pos + 2..pos + 2 + n)?), pos + 2 + n))
    };
    let top_pos = offset_of(top)?;
    let m = *data.get(top_pos)?;
    if m >> 4 != 0xA {
        return None;
    }
    let (count, mut p) = read_len(top_pos, m & 0xF)?;
    let mut out = Vec::new();
    for _ in 0..count {
        let r = be(data.get(p..p + ref_size)?);
        p += ref_size;
        if r >= num_objects {
            return None;
        }
        let pos = offset_of(r)?;
        let mk = *data.get(pos)?;
        let text = match mk >> 4 {
            0x5 => {
                let (n, s) = read_len(pos, mk & 0xF)?;
                String::from_utf8_lossy(data.get(s..s + n)?).into_owned()
            }
            0x6 => {
                let (n, s) = read_len(pos, mk & 0xF)?;
                let units: Vec<u16> = data
                    .get(s..s + 2 * n)?
                    .chunks(2)
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                String::from_utf16_lossy(&units)
            }
            _ => return None,
        };
        let (name, color) = match text.rsplit_once('\n') {
            Some((n, c)) => (
                n.to_string(),
                c.parse::<u32>().ok().and_then(TagColor::from_index),
            ),
            None => (text, None),
        };
        out.push(Tag { name, color });
    }
    Some(out)
}
