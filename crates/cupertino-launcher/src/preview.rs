//! What the panel's preview pane shows of the selected result beyond the result itself: for a file or a folder its
//! type in words, its size, when it changed, where it is, and a picture or the start of its text; for an application
//! its icon at the pane's size. The host reads them off the loop and hands them over as a [`Preview`]; the helpers
//! here turn what it read into the words the pane shows.

use cupertino_kit::controls::ImageSource;
use cupertino_kit::geometry::Size;
use std::sync::{Arc, Condvar, Mutex};

/// The picture the pane shows above the result's name.
#[derive(Clone, Debug, PartialEq)]
pub enum PreviewBody {
    /// A picture of the file's content, drawn `size` points large.
    Image {
        sources: Vec<ImageSource>,
        size: Size,
    },
    /// The first lines of a text file.
    Text(String),
    /// The result's icon at the pane's size.
    Icon,
}

/// What the host found out about a result for the pane.
#[derive(Clone, Debug, PartialEq)]
pub struct Preview {
    pub body: PreviewBody,
    /// The result's icon at [`crate::view::PREVIEW_ICON_SIZE`], one rendition per scale; none to draw the list's.
    pub icon: Vec<ImageSource>,
    /// The type in words ("PNG image", "Folder"); empty when it adds nothing.
    pub kind: String,
    /// Lines under the name, each a label and its value: the size, the change date, the folder.
    pub details: Vec<(String, String)>,
}

/// Bytes read from the start of a file to show its text.
pub const TEXT_SNIFF: usize = 4096;
/// The longest line of an excerpt, in characters.
const LINE_CHARS: usize = 160;

/// The type `mime` in words: the kind of content, named by the subtype where that reads well.
pub fn kind_name(mime: &str) -> String {
    let (top, sub) = mime.split_once('/').unwrap_or((mime, ""));
    let sub = sub.split(['+', ';']).next().unwrap_or("");
    let sub = sub
        .trim_start_matches("x-")
        .trim_start_matches("vnd.")
        .to_ascii_uppercase();
    let named = |what: &str| {
        if sub.is_empty() || sub.len() > 6 || sub.contains(['.', '-']) {
            let mut word = what.to_owned();
            if let Some(first) = word.get_mut(..1) {
                first.make_ascii_uppercase();
            }
            word
        } else {
            format!("{sub} {what}")
        }
    };
    match (top, mime) {
        (_, "inode/directory") => "Folder".to_owned(),
        (_, "text/plain") => "Plain text".to_owned(),
        (_, "application/pdf") => "PDF document".to_owned(),
        (_, "application/zip") => "ZIP archive".to_owned(),
        (_, "application/octet-stream") => "Document".to_owned(),
        ("image", _) => named("image"),
        ("audio", _) => named("audio"),
        ("video", _) => named("video"),
        ("font", _) => named("font"),
        ("text", _) => named("text"),
        _ => "Document".to_owned(),
    }
}

/// `bytes` in decimal units: "1 byte", "532 bytes", "12 KB", "1.2 MB".
pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["KB", "MB", "GB", "TB", "PB"];
    match bytes {
        1 => return "1 byte".to_owned(),
        0..1000 => return format!("{bytes} bytes"),
        _ => {}
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    if value < 10.0 {
        format!("{value:.1} {}", UNITS[unit])
    } else {
        format!("{value:.0} {}", UNITS[unit])
    }
}

/// The first `lines` lines of `bytes` when they read as text: UTF-8 (a character cut at the end is dropped) without
/// NUL bytes. Tabs become four spaces, carriage returns go, and long lines are cut. `None` for binary content and for
/// content that is only white space.
pub fn text_excerpt(bytes: &[u8], lines: usize) -> Option<String> {
    if bytes.contains(&0) {
        return None;
    }
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        // a character cut by the end of what was read
        Err(e) if e.error_len().is_none() => std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?,
        Err(_) => return None,
    };
    if text.trim().is_empty() {
        return None;
    }
    let excerpt: Vec<String> = text
        .lines()
        .take(lines)
        .map(|line| {
            line.replace('\t', "    ")
                .replace('\r', "")
                .chars()
                .filter(|c| !c.is_control())
                .take(LINE_CHARS)
                .collect()
        })
        .collect();
    Some(excerpt.join("\n"))
}

/// The most details the pane lists, and the longest text of a label, a value, the type or the excerpt it takes.
pub const MAX_DETAILS: usize = 8;
pub const MAX_LABEL_CHARS: usize = 64;
pub const MAX_VALUE_CHARS: usize = 256;
pub const MAX_EXCERPT_CHARS: usize = 4 * TEXT_SNIFF;
/// The largest rendition a preview picture may have, in pixels on a side, and its most renditions.
pub const MAX_IMAGE_EDGE: u32 = 2048;
pub const MAX_RENDITIONS: usize = 4;

/// `text` without control characters (but newlines when `lines`), cut to `limit` characters.
fn clean(text: &str, limit: usize, lines: bool) -> String {
    text.chars()
        .filter(|&c| !c.is_control() || (lines && c == '\n'))
        .take(limit)
        .collect()
}

/// Whether `source` holds the pixels it says it has and is no larger than a preview needs.
fn sound(source: &ImageSource) -> bool {
    let image = &source.image;
    let (w, h, stride) = (image.width, image.height, image.stride);
    w > 0
        && h > 0
        && w <= MAX_IMAGE_EDGE
        && h <= MAX_IMAGE_EDGE
        && u64::from(stride) >= u64::from(w) * 4
        && (image.pixels.len() as u64) >= u64::from(stride) * u64::from(h)
}

/// `preview` as the pane can show it. What a host hands over is never trusted to be well formed: at most
/// [`MAX_DETAILS`] details, every text cut to its limit with control characters dropped, renditions that do not hold
/// their pixels or exceed [`MAX_IMAGE_EDGE`] dropped, a picture left with none (or with a size that is not a finite
/// positive one) shown as the icon, and the picture's size fitted to the pane.
pub fn bounded(preview: Preview, limit: Size) -> Preview {
    let keep = |sources: Vec<ImageSource>| -> Vec<ImageSource> {
        sources
            .into_iter()
            .filter(sound)
            .take(MAX_RENDITIONS)
            .collect()
    };
    let body = match preview.body {
        PreviewBody::Image { sources, size } => {
            let sources = keep(sources);
            let finite = size.width.is_finite()
                && size.height.is_finite()
                && size.width > 0.0
                && size.height > 0.0;
            if sources.is_empty() || !finite {
                PreviewBody::Icon
            } else {
                PreviewBody::Image {
                    sources,
                    size: fit(size, limit),
                }
            }
        }
        PreviewBody::Text(text) => {
            let text = clean(&text, MAX_EXCERPT_CHARS, true);
            if text.trim().is_empty() {
                PreviewBody::Icon
            } else {
                PreviewBody::Text(text)
            }
        }
        PreviewBody::Icon => PreviewBody::Icon,
    };
    Preview {
        body,
        icon: keep(preview.icon),
        kind: clean(&preview.kind, MAX_LABEL_CHARS, false),
        details: preview
            .details
            .into_iter()
            .take(MAX_DETAILS)
            .map(|(label, value)| {
                (
                    clean(&label, MAX_LABEL_CHARS, false),
                    clean(&value, MAX_VALUE_CHARS, false),
                )
            })
            .collect(),
    }
}

/// The folder holding `path`, with the home folder written as `~`.
pub fn location(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    let Some(parent) = path.parent() else {
        return "/".to_owned();
    };
    if let Some(rest) = home.and_then(|home| parent.strip_prefix(home).ok()) {
        return if rest.as_os_str().is_empty() {
            "~".to_owned()
        } else {
            format!("~/{}", rest.display())
        };
    }
    parent.display().to_string()
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The civil date of `days` since 1970-01-01: (year, month 1–12, day 1–31).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let year = yoe + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

/// When `time` (Unix seconds) was, in local time `offset` seconds east of UTC, seen from `now`: "Today at 14:02",
/// "Yesterday at 09:15", else "3 November 2026 at 14:02".
pub fn format_date(time: i64, offset: i64, now: i64) -> String {
    let local = time + offset;
    let day = local.div_euclid(86_400);
    let today = (now + offset).div_euclid(86_400);
    let seconds = local.rem_euclid(86_400);
    let clock = format!("{:02}:{:02}", seconds / 3600, seconds % 3600 / 60);
    match today - day {
        0 => format!("Today at {clock}"),
        1 => format!("Yesterday at {clock}"),
        _ => {
            let (year, month, date) = civil(day);
            format!("{date} {} {year} at {clock}", MONTHS[month as usize - 1])
        }
    }
}

/// `size` shrunk to fit inside `limit` with its aspect ratio kept; never enlarged.
pub fn fit(size: Size, limit: Size) -> Size {
    if size.width <= 0.0 || size.height <= 0.0 {
        return Size::ZERO;
    }
    let scale = (limit.width / size.width)
        .min(limit.height / size.height)
        .min(1.0);
    Size::new(
        (size.width * scale).round().max(1.0),
        (size.height * scale).round().max(1.0),
    )
}

/// Runs jobs one at a time on a thread of its own, only the newest: a job handed over while another runs replaces the
/// one still waiting, which is dropped unrun. Moving through the results asks for a preview at every step; this keeps
/// the reads (and the decoding of large pictures) from piling up on threads of their own. The thread ends when the
/// reader is dropped, after the job it runs.
pub struct LatestOnly<J: Send + 'static> {
    shared: Arc<(Mutex<Slot<J>>, Condvar)>,
}

struct Slot<J> {
    job: Option<J>,
    closed: bool,
}

impl<J: Send + 'static> LatestOnly<J> {
    /// A reader whose thread, named `name`, hands each job to `run`.
    pub fn new(name: &str, mut run: impl FnMut(J) + Send + 'static) -> std::io::Result<Self> {
        let shared = Arc::new((
            Mutex::new(Slot {
                job: None,
                closed: false,
            }),
            Condvar::new(),
        ));
        let theirs = Arc::clone(&shared);
        std::thread::Builder::new()
            .name(name.to_owned())
            .spawn(move || loop {
                let (lock, ready) = &*theirs;
                let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
                while slot.job.is_none() && !slot.closed {
                    slot = ready.wait(slot).unwrap_or_else(|e| e.into_inner());
                }
                if slot.closed {
                    return;
                }
                let job = slot.job.take();
                drop(slot);
                if let Some(job) = job {
                    run(job);
                }
            })?;
        Ok(Self { shared })
    }

    /// Run `job` next, in place of any job still waiting.
    pub fn submit(&self, job: J) {
        let (lock, ready) = &*self.shared;
        lock.lock().unwrap_or_else(|e| e.into_inner()).job = Some(job);
        ready.notify_one();
    }
}

impl<J: Send + 'static> Drop for LatestOnly<J> {
    fn drop(&mut self) {
        let (lock, ready) = &*self.shared;
        let mut slot = lock.lock().unwrap_or_else(|e| e.into_inner());
        slot.closed = true;
        slot.job = None;
        ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(width: u32, height: u32, pixels: usize) -> ImageSource {
        use cupertino_kit::render::{ImageResource, PixelFormat};
        ImageSource {
            id: cupertino_kit::core::next_resource_id(),
            image: ImageResource {
                revision: 1,
                width,
                height,
                stride: width * 4,
                format: PixelFormat::Rgba8Premultiplied,
                pixels: vec![0u8; pixels].into(),
            },
        }
    }

    #[test]
    fn an_oversized_or_corrupt_preview_is_cut_down_to_what_the_pane_shows() {
        let limit = Size::new(300.0, 136.0);
        let hostile = Preview {
            body: PreviewBody::Image {
                // a rendition that says it is larger than its pixels, one past the edge limit, one sound
                sources: vec![
                    source(64, 64, 10),
                    source(MAX_IMAGE_EDGE + 1, 4, (MAX_IMAGE_EDGE as usize + 1) * 16),
                    source(8, 8, 256),
                ],
                size: Size::new(6000.0, 1000.0),
            },
            icon: vec![source(4, 4, 3)],
            kind: format!("bad\u{7}{}", "k".repeat(500)),
            details: (0..100)
                .map(|i| (format!("label{i}\u{1b}[31m"), "v".repeat(10_000)))
                .collect(),
        };
        let p = bounded(hostile, limit);
        match &p.body {
            PreviewBody::Image { sources, size } => {
                assert_eq!(sources.len(), 1, "only the sound rendition is kept");
                assert!(size.width <= limit.width && size.height <= limit.height);
            }
            other => panic!("{other:?}"),
        }
        assert!(p.icon.is_empty(), "an icon without its pixels is dropped");
        assert_eq!(p.kind.chars().count(), MAX_LABEL_CHARS);
        assert!(!p.kind.contains('\u{7}'));
        assert_eq!(p.details.len(), MAX_DETAILS);
        assert!(p
            .details
            .iter()
            .all(|(l, v)| !l.contains('\u{1b}') && v.chars().count() == MAX_VALUE_CHARS));
    }

    #[test]
    fn a_picture_without_usable_renditions_or_size_shows_the_icon_and_text_keeps_its_lines() {
        let limit = Size::new(300.0, 136.0);
        let picture = |sources, size| Preview {
            body: PreviewBody::Image { sources, size },
            icon: Vec::new(),
            kind: String::new(),
            details: Vec::new(),
        };
        let p = bounded(picture(vec![source(0, 0, 0)], Size::new(10.0, 10.0)), limit);
        assert_eq!(p.body, PreviewBody::Icon);
        let p = bounded(
            picture(vec![source(2, 2, 16)], Size::new(f64::NAN, 1.0)),
            limit,
        );
        assert_eq!(p.body, PreviewBody::Icon);
        let text = Preview {
            body: PreviewBody::Text(format!("one\ntwo\u{0}\n{}", "x".repeat(MAX_EXCERPT_CHARS))),
            ..picture(Vec::new(), Size::ZERO)
        };
        match bounded(text, limit).body {
            PreviewBody::Text(t) => {
                assert!(t.starts_with("one\ntwo\n"));
                assert_eq!(t.chars().count(), MAX_EXCERPT_CHARS);
            }
            other => panic!("{other:?}"),
        }
        let blank = Preview {
            body: PreviewBody::Text("\n\u{1}\n".into()),
            ..picture(Vec::new(), Size::ZERO)
        };
        assert_eq!(bounded(blank, limit).body, PreviewBody::Icon);
    }
    use std::path::Path;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn only_the_newest_waiting_job_runs() {
        let (started, starts) = mpsc::channel::<u32>();
        let (release, gate) = mpsc::channel::<()>();
        let (finished, ends) = mpsc::channel::<u32>();
        let reader = LatestOnly::new("preview-test", move |job: (u32, mpsc::Sender<()>)| {
            started.send(job.0).unwrap();
            gate.recv().unwrap();
            finished.send(job.0).unwrap();
        })
        .unwrap();
        // each job holds a sender, so a job dropped unrun closes its receiver
        let job = |n: u32| {
            let (tx, rx) = mpsc::channel::<()>();
            ((n, tx), rx)
        };
        let (first, _first_rx) = job(1);
        reader.submit(first);
        assert_eq!(starts.recv_timeout(Duration::from_secs(10)), Ok(1));
        // while the first runs, three more are asked for: the second and third are replaced
        let (second, second_rx) = job(2);
        let (third, third_rx) = job(3);
        let (fourth, _fourth_rx) = job(4);
        reader.submit(second);
        reader.submit(third);
        reader.submit(fourth);
        assert_eq!(second_rx.recv(), Err(mpsc::RecvError), "dropped unrun");
        assert_eq!(third_rx.recv(), Err(mpsc::RecvError), "dropped unrun");
        release.send(()).unwrap();
        assert_eq!(ends.recv_timeout(Duration::from_secs(10)), Ok(1));
        assert_eq!(starts.recv_timeout(Duration::from_secs(10)), Ok(4));
        release.send(()).unwrap();
        assert_eq!(ends.recv_timeout(Duration::from_secs(10)), Ok(4));
        drop(reader);
        // the thread ended with the reader: nothing else ever starts
        assert_eq!(starts.recv(), Err(mpsc::RecvError));
    }

    #[test]
    fn types_read_as_words() {
        assert_eq!(kind_name("image/png"), "PNG image");
        assert_eq!(kind_name("image/svg+xml"), "SVG image");
        assert_eq!(kind_name("image/x-portable-pixmap"), "Image");
        assert_eq!(kind_name("text/plain"), "Plain text");
        assert_eq!(kind_name("text/x-rust"), "RUST text");
        assert_eq!(kind_name("text/markdown"), "Text");
        assert_eq!(kind_name("inode/directory"), "Folder");
        assert_eq!(kind_name("application/pdf"), "PDF document");
        assert_eq!(kind_name("application/x-whatever"), "Document");
        assert_eq!(kind_name("video/mp4"), "MP4 video");
        assert_eq!(kind_name("nonsense"), "Document");
    }

    #[test]
    fn sizes_use_decimal_units() {
        assert_eq!(human_size(0), "0 bytes");
        assert_eq!(human_size(1), "1 byte");
        assert_eq!(human_size(999), "999 bytes");
        assert_eq!(human_size(1000), "1.0 KB");
        assert_eq!(human_size(12_345), "12 KB");
        assert_eq!(human_size(1_234_567), "1.2 MB");
        assert_eq!(human_size(999_999), "1.0 MB");
        assert_eq!(human_size(u64::MAX), "18447 PB");
    }

    #[test]
    fn text_shows_its_first_lines_and_binary_shows_none() {
        assert_eq!(
            text_excerpt(b"one\r\n\ttwo\nthree\nfour", 3).as_deref(),
            Some("one\n    two\nthree")
        );
        assert_eq!(text_excerpt(b"\x89PNG\r\n\x1a\n\0\0", 3), None);
        assert_eq!(text_excerpt(b"\xff\xfe junk", 3), None);
        assert_eq!(text_excerpt(b"  \n\n ", 3), None);
        // a character cut by the end of the read
        assert_eq!(
            text_excerpt("caf\u{e9}".as_bytes().split_last().unwrap().1, 3).as_deref(),
            Some("caf")
        );
        let long = "x".repeat(500);
        assert_eq!(
            text_excerpt(long.as_bytes(), 1).unwrap().chars().count(),
            LINE_CHARS
        );
    }

    #[test]
    fn the_location_writes_home_as_a_tilde() {
        let home = Path::new("/home/me");
        assert_eq!(location(Path::new("/home/me/a.txt"), Some(home)), "~");
        assert_eq!(
            location(Path::new("/home/me/Documents/a.txt"), Some(home)),
            "~/Documents"
        );
        assert_eq!(location(Path::new("/etc/hosts"), Some(home)), "/etc");
        assert_eq!(location(Path::new("/"), Some(home)), "/");
    }

    #[test]
    fn dates_name_today_and_yesterday_and_else_the_day() {
        // 2026-11-08 12:00 UTC
        let now = 1_794_139_200;
        assert_eq!(format_date(now - 3600, 0, now), "Today at 11:00");
        assert_eq!(format_date(now - 86_400, 0, now), "Yesterday at 12:00");
        assert_eq!(
            format_date(now - 5 * 86_400, 0, now),
            "3 November 2026 at 12:00"
        );
        // an hour east of UTC moves the clock and, near midnight, the day
        assert_eq!(
            format_date(now + 11 * 3600 + 1800, 3600, now),
            "9 November 2026 at 00:30"
        );
        assert_eq!(format_date(0, 0, now), "1 January 1970 at 00:00");
        assert_eq!(
            format_date(951_782_400, 0, now),
            "29 February 2000 at 00:00"
        );
    }

    #[test]
    fn pictures_shrink_to_fit_and_never_grow() {
        let limit = Size::new(280.0, 140.0);
        assert_eq!(
            fit(Size::new(1000.0, 500.0), limit),
            Size::new(280.0, 140.0)
        );
        assert_eq!(fit(Size::new(100.0, 1000.0), limit), Size::new(14.0, 140.0));
        assert_eq!(fit(Size::new(50.0, 40.0), limit), Size::new(50.0, 40.0));
        assert_eq!(fit(Size::new(0.0, 40.0), limit), Size::ZERO);
    }
}
