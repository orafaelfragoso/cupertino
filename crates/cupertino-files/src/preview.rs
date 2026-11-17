//! What a preview of a file shows, without a toolkit: whether it is drawn as a picture, as text or as its icon with its
//! details; the text, read with a bound; the items a preview steps through; the summary and details under it; and the
//! size of the preview window.
//!
//! Only what the file manager can already read is drawn: pictures the thumbnailer decodes (PNG, JPEG, WebP) and text
//! files. Any other file (a PDF, a film, an archive) and a folder show their icon, their name and their details. A
//! text file is read up to [`TEXT_LIMIT`] bytes; one holding a NUL byte or bytes that are not UTF-8 shows its icon, so a
//! preview never draws a binary file as text.

use std::io::{self, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::entry::Entry;
use crate::format::{human_size, relative_date};
use crate::mime::{self, KindGroup};

/// The most of a text file a preview reads.
pub const TEXT_LIMIT: u64 = 64 * 1024;
/// The largest picture edge a preview decodes, in pixels.
pub const PICTURE_EDGE: u32 = 1024;

/// How a preview draws an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PreviewKind {
    /// A picture the thumbnailer decodes.
    Picture,
    /// A text file.
    Text,
    /// Its icon and details.
    Icon,
}

/// The picture types the thumbnailer decodes.
const PICTURES: [&str; 3] = ["image/png", "image/jpeg", "image/webp"];

/// Text types that are not under `text/`.
const TEXT_TYPES: [&str; 6] = [
    "application/json",
    "application/xml",
    "application/javascript",
    "application/x-shellscript",
    "application/toml",
    "application/x-yaml",
];

/// How a preview draws `entry`.
pub fn kind(entry: &Entry) -> PreviewKind {
    if entry.is_dir {
        return PreviewKind::Icon;
    }
    let mime = mime::entry_mime(entry);
    if PICTURES.contains(&mime.as_str()) {
        return PreviewKind::Picture;
    }
    let text = matches!(mime::kind_group(&mime), KindGroup::Text | KindGroup::Source)
        && mime != "application/rtf"
        && mime != "text/rtf"
        && mime != "application/msword"
        && mime != "application/vnd.oasis.opendocument.text";
    if text || mime.starts_with("text/") || TEXT_TYPES.contains(&mime.as_str()) {
        return PreviewKind::Text;
    }
    PreviewKind::Icon
}

/// The start of a text file: at most [`TEXT_LIMIT`] bytes, cut at a character boundary, and whether there was more.
/// `None` when the file is not text: a NUL byte, or bytes that are not UTF-8 before the cut.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextPreview {
    pub text: String,
    pub truncated: bool,
}

/// Read the start of the text file at `path` ([`TextPreview`]); `Ok(None)` when it is not text, or not a plain file:
/// a pipe or a device, even one put in the file's place since it was listed, is never opened to wait on it or read.
pub fn read_text(path: &Path) -> io::Result<Option<TextPreview>> {
    if !std::fs::metadata(path)?.is_file() {
        return Ok(None);
    }
    // a pipe swapped in after the check opens without waiting for a writer, and is refused below
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)?;
    if !file.metadata()?.is_file() {
        return Ok(None);
    }
    let mut bytes = Vec::new();
    file.take(TEXT_LIMIT + 1).read_to_end(&mut bytes)?;
    Ok(text_of(bytes))
}

/// The text preview of `bytes`, the first [`TEXT_LIMIT`] of a file, plus one when there was more.
pub fn text_of(mut bytes: Vec<u8>) -> Option<TextPreview> {
    let truncated = bytes.len() as u64 > TEXT_LIMIT;
    bytes.truncate(TEXT_LIMIT as usize);
    if bytes.contains(&0) {
        return None;
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) => {
            let error = e.utf8_error();
            // a character cut at the limit is no reason to refuse the file
            if !truncated || error.error_len().is_some() {
                return None;
            }
            let valid = error.valid_up_to();
            let mut bytes = e.into_bytes();
            bytes.truncate(valid);
            String::from_utf8(bytes).ok()?
        }
    };
    Some(TextPreview { text, truncated })
}

/// The line under the name: "Plain text - 19 bytes", "Folder".
pub fn summary(entry: &Entry) -> String {
    let kind = mime::kind_description(&mime::entry_mime(entry));
    if entry.is_dir {
        kind
    } else {
        format!("{kind} - {}", human_size(entry.size))
    }
}

/// The details under the summary, each a label and its value, dates worded relative to `now`: when the item was made,
/// changed and last opened, those the file system knows.
pub fn details(entry: &Entry, now: SystemTime) -> Vec<(&'static str, String)> {
    [
        ("Created", entry.created),
        ("Modified", entry.modified),
        ("Last opened", entry.accessed),
    ]
    .into_iter()
    .filter_map(|(label, time)| Some((label, relative_date(time?, now))))
    .collect()
}

/// The items a preview steps through with the arrows: the selection when it holds several, else the one item.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Browse {
    items: Vec<PathBuf>,
    index: usize,
}

impl Browse {
    /// A preview of `items` starting at `index` (the first when out of range); `None` without items.
    pub fn new(items: Vec<PathBuf>, index: usize) -> Option<Self> {
        if items.is_empty() {
            return None;
        }
        let index = if index < items.len() { index } else { 0 };
        Some(Browse { items, index })
    }

    pub fn current(&self) -> &Path {
        &self.items[self.index]
    }

    pub fn index(&self) -> usize {
        self.index
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Step `by` items, stopping at either end; whether the item changed.
    pub fn step(&mut self, by: isize) -> bool {
        let last = self.items.len() - 1;
        let next = self.index.saturating_add_signed(by).min(last);
        let changed = next != self.index;
        self.index = next;
        changed
    }

    /// The items stepped through.
    pub fn items(&self) -> &[PathBuf] {
        &self.items
    }
}

/// The preview window's side when it shows text or an icon, in points.
pub const WINDOW_SIZE: (f64, f64) = (640.0, 680.0);
/// The most the preview window takes when it shows a picture; a smaller picture shows at its own size, with room for the
/// toolbar.
pub const PICTURE_MAX: (f64, f64) = (800.0, 680.0);
/// The narrowest a picture's preview window is: four times the side of an item as the icon view draws it.
const NARROWEST: f64 = 4.0 * 64.0;
/// How long a preview waits for what it shows (a picture to decode, a text to be read) before it says it is loading,
/// in seconds: one that comes sooner shows at once, without a loading state in between.
pub const LOADING_DELAY: f64 = 0.3;

/// The size the preview window takes to show a picture of `pixels` at `scale` pixels a point below a toolbar `toolbar`
/// points tall: its own size within [`PICTURE_MAX`], its sides kept in proportion.
pub fn picture_window(pixels: (u32, u32), scale: f64, toolbar: f64) -> (f64, f64) {
    let (w, h) = (
        f64::from(pixels.0) / scale.max(1.0),
        f64::from(pixels.1) / scale.max(1.0),
    );
    if w <= 0.0 || h <= 0.0 {
        return WINDOW_SIZE;
    }
    let room = (PICTURE_MAX.0, PICTURE_MAX.1 - toolbar);
    let fit = (room.0 / w).min(room.1 / h).min(1.0);
    (
        (w * fit).round().max(NARROWEST),
        (h * fit).round() + toolbar,
    )
}
