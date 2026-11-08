//! What a preview of a file shows, without a toolkit: whether it is drawn as a picture, as text or as its icon with its
//! details; the text, read with a bound; the items a preview steps through; the summary and details under it; and the
//! motion of the preview window as it opens and closes.
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
use crate::metrics::timing::WINDOW_RESIZE_CAP;
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
/// The side of the item the preview grows out of, as the icon view draws it.
const SOURCE: f64 = 64.0;
/// How fast a window's frame change moves, in points a second, in the duration rule of [`Motion::duration`].
const VELOCITY: f64 = 1000.0;
/// How long the fade that takes the zoom's place under Reduce Motion lasts, in seconds.
const FADE: f64 = 0.2;
/// The scale the panel starts its zoom from.
const START_SCALE: f64 = SOURCE / WINDOW_SIZE.0;

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
        (w * fit).round().max(SOURCE * 4.0),
        (h * fit).round() + toolbar,
    )
}

/// How the preview window's panel looks at a moment of its opening or closing.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MotionFrame {
    /// The panel's size against its full size, about its centre.
    pub scale: f64,
    /// The panel's opacity.
    pub alpha: f64,
}

/// The opening or closing of the preview window: the panel zooms out of an item-sized square to its size, eased in and
/// out, and back on closing; under Reduce Motion it fades instead, at its full size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub closing: bool,
    pub reduce: bool,
    /// The window's size, in points.
    pub size: (f64, f64),
}

impl Motion {
    /// How long the motion lasts: a frame change takes `181 · ln(d / 100 + 1) / 1000` seconds for its largest change of
    /// side `d`, at most 0.35 s; a fade [`FADE`].
    pub fn duration(&self) -> f64 {
        if self.reduce {
            return FADE;
        }
        let d = (self.size.0 - SOURCE).max(self.size.1 - SOURCE).max(0.0);
        (181.0 * (d / 100.0 + 1.0).ln() / VELOCITY).min(WINDOW_RESIZE_CAP)
    }

    /// The panel `elapsed` seconds into the motion.
    pub fn at(&self, elapsed: f64) -> MotionFrame {
        let duration = self.duration();
        let t = if duration <= 0.0 {
            1.0
        } else {
            (elapsed / duration).clamp(0.0, 1.0)
        };
        let p = ease_in_out(if self.closing { 1.0 - t } else { t });
        if self.reduce {
            return MotionFrame {
                scale: 1.0,
                alpha: p,
            };
        }
        let start = (SOURCE / self.size.0.max(SOURCE)).clamp(START_SCALE.min(1.0), 1.0);
        MotionFrame {
            scale: start + (1.0 - start) * p,
            alpha: p,
        }
    }

    /// Whether the motion is over `elapsed` seconds in.
    pub fn finished(&self, elapsed: f64) -> bool {
        elapsed >= self.duration()
    }
}

/// Cubic ease-in-out.
pub fn ease_in_out(t: f64) -> f64 {
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        4.0 * t * t * t
    } else {
        1.0 - (-2.0 * t + 2.0).powi(3) / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str) -> Entry {
        Entry::synthetic(name, false, 19)
    }

    #[test]
    fn pictures_and_text_draw_themselves_and_the_rest_shows_its_icon() {
        assert_eq!(kind(&entry("a.png")), PreviewKind::Picture);
        assert_eq!(kind(&entry("a.jpg")), PreviewKind::Picture);
        assert_eq!(kind(&entry("notes.txt")), PreviewKind::Text);
        assert_eq!(kind(&entry("main.rs")), PreviewKind::Text);
        assert_eq!(kind(&entry("data.json")), PreviewKind::Text);
        assert_eq!(kind(&entry("report.pdf")), PreviewKind::Icon);
        assert_eq!(kind(&entry("film.mp4")), PreviewKind::Icon);
        assert_eq!(kind(&entry("a.gif")), PreviewKind::Icon, "no decoder");
        assert_eq!(kind(&Entry::synthetic("Notes", true, 0)), PreviewKind::Icon);
    }

    #[test]
    fn text_is_read_with_a_bound_and_binary_files_are_refused() {
        assert_eq!(
            text_of(b"Amber nested file.\n".to_vec()),
            Some(TextPreview {
                text: "Amber nested file.\n".into(),
                truncated: false
            })
        );
        assert_eq!(text_of(b"a\0b".to_vec()), None);
        assert_eq!(text_of(vec![0xff, 0xfe, b'a']), None);
        // a character cut at the bound is dropped, and the text says it goes on
        let mut long = vec![b'a'; TEXT_LIMIT as usize - 1];
        long.extend("é and more".as_bytes());
        let cut = text_of(long).unwrap();
        assert!(cut.truncated);
        assert_eq!(cut.text.len(), TEXT_LIMIT as usize - 1);
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("big.txt");
        std::fs::write(&file, vec![b'x'; TEXT_LIMIT as usize + 50]).unwrap();
        let read = read_text(&file).unwrap().unwrap();
        assert_eq!((read.text.len() as u64, read.truncated), (TEXT_LIMIT, true));
        assert_eq!(read_text(dir.path()).unwrap(), None, "a folder is not text");
    }

    #[test]
    fn a_pipe_named_as_text_is_refused_without_waiting_for_a_writer() {
        let dir = tempfile::tempdir().unwrap();
        let pipe = dir.path().join("notes.txt");
        let name = std::ffi::CString::new(pipe.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: a valid NUL-terminated path
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(
            kind(&Entry::from_path(pipe.clone()).unwrap()),
            PreviewKind::Text
        );
        // with no writer, an open that waited would never return
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || tx.send(read_text(&pipe).map_err(|e| e.kind())));
        let read = rx.recv_timeout(std::time::Duration::from_secs(10));
        assert_eq!(read, Ok(Ok(None)));
    }

    #[test]
    fn the_summary_names_the_kind_and_size() {
        assert_eq!(summary(&entry("Leaf.txt")), "Text - 19 bytes");
        assert_eq!(summary(&Entry::synthetic("Notes", true, 0)), "Folder");
        let mut dated = entry("a.txt");
        dated.modified = Some(SystemTime::UNIX_EPOCH);
        let rows = details(&dated, SystemTime::UNIX_EPOCH);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].0, "Modified");
    }

    #[test]
    fn the_arrows_step_through_the_items_and_stop_at_the_ends() {
        assert_eq!(Browse::new(Vec::new(), 0), None);
        let mut b = Browse::new(vec!["a".into(), "b".into(), "c".into()], 9).unwrap();
        assert_eq!(b.current(), Path::new("a"));
        assert!(!b.step(-1));
        assert!(b.step(1));
        assert!(b.step(5));
        assert_eq!((b.index(), b.current()), (2, Path::new("c")));
    }

    #[test]
    fn the_panel_zooms_out_of_an_item_and_fades_under_reduce_motion() {
        let open = Motion {
            closing: false,
            reduce: false,
            size: WINDOW_SIZE,
        };
        let d = open.duration();
        assert!(d > 0.3 && d <= WINDOW_RESIZE_CAP, "{d}");
        let first = open.at(0.0);
        assert_eq!(first.alpha, 0.0);
        assert!((first.scale - 0.1).abs() < 1e-9, "{first:?}");
        assert_eq!(
            open.at(d),
            MotionFrame {
                scale: 1.0,
                alpha: 1.0
            }
        );
        let mid = open.at(d / 2.0);
        assert!((mid.alpha - 0.5).abs() < 1e-9);
        let close = Motion {
            closing: true,
            ..open
        };
        assert_eq!(
            close.at(0.0),
            MotionFrame {
                scale: 1.0,
                alpha: 1.0
            }
        );
        assert!(close.finished(d));
        let fade = Motion {
            reduce: true,
            ..open
        };
        assert_eq!(fade.duration(), FADE);
        assert_eq!(fade.at(0.0).scale, 1.0);
        assert_eq!(fade.at(FADE / 2.0).alpha, 0.5);
    }

    #[test]
    fn a_picture_window_fits_the_picture_under_the_toolbar() {
        assert_eq!(picture_window((512, 256), 2.0, 36.0), (256.0, 164.0));
        assert_eq!(picture_window((4000, 2000), 1.0, 36.0), (800.0, 436.0));
        assert_eq!(picture_window((0, 10), 1.0, 36.0), WINDOW_SIZE);
    }
}
