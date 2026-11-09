//! The cursor drawn by the client from an XCursor theme, for a compositor without `wp_cursor_shape_v1`. An animated
//! cursor plays its images, each for its own delay, while the pointer stays on the surface it was set over.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use rustix::fs::{memfd_create, MemfdFlags};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::QueueHandle;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;
use xcursor::parser::{parse_xcursor, Image};

use crate::connection::State;
use crate::Error;

/// The size in pixels at scale 1 when `XCURSOR_SIZE` doesn't say.
const DEFAULT_SIZE: u32 = 24;
/// The largest side of a cursor image drawn; themes ship 256 at most.
const MAX_SIDE: u32 = 1024;
/// The type of an image entry of an XCursor file.
const XCURSOR_IMAGE: u32 = 0xfffd_0002;
/// The lengths of an XCursor file's header, of an entry of its table of contents and of an image chunk's header.
const HEADER: usize = 16;
const TOC_ENTRY: usize = 12;
const IMAGE_HEADER: usize = 36;

/// Where the cursors a client draws itself come from: a theme, found by name in the directories of `search_path` with
/// the themes it inherits from (`index.theme`), at a nominal `size` in pixels at scale 1.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CursorTheme {
    pub name: String,
    pub size: u32,
    pub search_path: Vec<PathBuf>,
}

impl CursorTheme {
    /// The user's theme: `XCURSOR_THEME` (else `default`) at `XCURSOR_SIZE` (else 24), searched in `XCURSOR_PATH`, else
    /// in `$XDG_DATA_HOME/icons`, `~/.icons`, the `icons` directory of each of `$XDG_DATA_DIRS` and
    /// `/usr/share/pixmaps`.
    pub fn from_env() -> CursorTheme {
        let var = |name| std::env::var(name).ok().filter(|v| !v.is_empty());
        let home = var("HOME").map(PathBuf::from);
        let expand = |entry: &str| match entry.strip_prefix('~') {
            Some(rest) => home.as_ref().map(|h| h.join(rest.trim_start_matches('/'))),
            None => Some(PathBuf::from(entry)),
        };
        let list = |value: String| -> Vec<PathBuf> {
            value
                .split(':')
                .filter(|e| !e.is_empty())
                .filter_map(&expand)
                .collect()
        };
        let search_path = match var("XCURSOR_PATH") {
            Some(path) => list(path),
            None => {
                let data_home = var("XDG_DATA_HOME")
                    .and_then(|d| expand(&d))
                    .or_else(|| home.as_ref().map(|h| h.join(".local/share")));
                let data_dirs =
                    var("XDG_DATA_DIRS").unwrap_or_else(|| "/usr/local/share:/usr/share".into());
                data_home
                    .map(|d| d.join("icons"))
                    .into_iter()
                    .chain(home.as_ref().map(|h| h.join(".icons")))
                    .chain(list(data_dirs).into_iter().map(|d| d.join("icons")))
                    .chain([PathBuf::from("/usr/share/pixmaps")])
                    .collect()
            }
        };
        CursorTheme {
            name: var("XCURSOR_THEME").unwrap_or_else(|| "default".into()),
            size: var("XCURSOR_SIZE")
                .and_then(|v| v.parse().ok())
                .filter(|s| *s > 0)
                .unwrap_or(DEFAULT_SIZE),
            search_path,
        }
    }

    /// The file of the first of `names` this theme or one it inherits from has.
    fn find(&self, names: &[&str]) -> Option<PathBuf> {
        names
            .iter()
            .find_map(|icon| self.lookup(&self.name, icon, &mut HashSet::new()))
    }

    fn lookup(&self, theme: &str, icon: &str, seen: &mut HashSet<String>) -> Option<PathBuf> {
        if !seen.insert(theme.to_owned()) {
            return None;
        }
        let dirs: Vec<PathBuf> = self
            .search_path
            .iter()
            .map(|p| p.join(theme))
            .filter(|d| d.is_dir())
            .collect();
        if let Some(file) = dirs
            .iter()
            .map(|d| d.join("cursors").join(icon))
            .find(|f| f.is_file())
        {
            return Some(file);
        }
        // a theme that names no parent inherits from `default`
        let parents: Vec<String> = dirs
            .iter()
            .flat_map(|d| {
                inherits(&d.join("index.theme")).unwrap_or_else(|| {
                    Vec::from_iter((theme != "default").then(|| "default".into()))
                })
            })
            .collect();
        parents
            .iter()
            .find_map(|parent| self.lookup(parent, icon, seen))
    }
}

/// The themes the `Inherits` key of an `index.theme` names; `None` without the file or the key.
fn inherits(index: &Path) -> Option<Vec<String>> {
    let text = std::fs::read_to_string(index).ok()?;
    let value = text.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key.trim() == "Inherits").then_some(value)
    })?;
    Some(
        value
            .split(|c: char| c.is_whitespace() || c == ',' || c == ';')
            .filter(|t| !t.is_empty())
            .map(String::from)
            .collect(),
    )
}

/// The most images of one animated cursor kept; themes ship a few dozen at most.
const MAX_FRAMES: usize = 64;
/// The shortest an animated cursor shows an image: a frame of a 60 Hz output. A shorter or zero delay in a theme file
/// would redraw the cursor on every wake of the loop.
const MIN_DELAY: Duration = Duration::from_millis(16);

/// A cursor image in a buffer of its own, shown for `delay` when the cursor is animated.
struct Frame {
    buffer: WlBuffer,
    size: (u32, u32),
    hotspot: (u32, u32),
    delay: Duration,
}

/// The cursor on screen: which images, the one shown, and when the next is due.
struct Showing {
    shape: Shape,
    key: (u32, u32),
    frame: usize,
    next: Option<Instant>,
    pointer: WlPointer,
    serial: u32,
    scale: u32,
}

/// The theme (the user's, read when first needed, unless one was set), the images loaded from it for each shape and
/// scale, the cursor shown and the surface it is drawn on.
pub(crate) struct ThemedCursor {
    compositor: WlCompositor,
    shm: WlShm,
    theme: Option<CursorTheme>,
    loaded: HashMap<(u32, u32), Vec<Frame>>,
    showing: Option<Showing>,
    surface: Option<WlSurface>,
}

impl ThemedCursor {
    pub(crate) fn new(compositor: WlCompositor, shm: WlShm) -> ThemedCursor {
        ThemedCursor {
            compositor,
            shm,
            theme: None,
            loaded: HashMap::new(),
            showing: None,
            surface: None,
        }
    }

    /// Draw from `theme` from now on.
    pub(crate) fn set_theme(&mut self, theme: CursorTheme) {
        self.stop();
        for frame in self.loaded.drain().flat_map(|(_, frames)| frames) {
            frame.buffer.destroy();
        }
        self.theme = Some(theme);
    }

    /// No cursor of this client is on screen any more: the pointer left, or another cursor replaced it.
    pub(crate) fn stop(&mut self) {
        self.showing = None;
    }

    /// When the next image of an animated cursor is due.
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.showing.as_ref()?.next
    }

    /// Show the next image of an animated cursor when it is due at `now`; whether one was shown.
    pub(crate) fn advance(&mut self, now: Instant) -> bool {
        let Some(showing) = self
            .showing
            .as_mut()
            .filter(|s| s.next.is_some_and(|at| at <= now))
        else {
            return false;
        };
        let frames = &self.loaded[&showing.key];
        showing.frame = (showing.frame + 1) % frames.len();
        let frame = &frames[showing.frame];
        showing.next = Some(now + frame.delay);
        if let Some(surface) = &self.surface {
            draw(
                surface,
                &showing.pointer,
                showing.serial,
                frame,
                showing.scale,
            );
        }
        true
    }

    /// The surface under the cursor now draws at `scale`: a cursor showing at another scale is drawn again from the
    /// images for this one, so it is never left magnified or blurred from the old scale. Whether it was.
    pub(crate) fn rescale(&mut self, qh: &QueueHandle<State>, scale: u32) -> Result<bool, Error> {
        let scale = scale.max(1);
        let Some(showing) = self.showing.as_ref().filter(|s| s.scale != scale) else {
            return Ok(false);
        };
        let (pointer, serial, shape) = (showing.pointer.clone(), showing.serial, showing.shape);
        self.show(qh, &pointer, serial, shape, scale)?;
        Ok(true)
    }

    /// Show `shape` drawn for `scale` as the cursor of the pointer entered with `serial`.
    pub(crate) fn show(
        &mut self,
        qh: &QueueHandle<State>,
        pointer: &WlPointer,
        serial: u32,
        shape: Shape,
        scale: u32,
    ) -> Result<(), Error> {
        let scale = scale.max(1);
        let key = (shape as u32, scale);
        if !self.loaded.contains_key(&key) {
            let frames = self.load(qh, shape, scale)?;
            self.loaded.insert(key, frames);
        }
        let frames = &self.loaded[&key];
        let surface = self
            .surface
            .get_or_insert_with(|| self.compositor.create_surface(qh, ()));
        draw(surface, pointer, serial, &frames[0], scale);
        let animated = frames.len() > 1;
        self.showing = Some(Showing {
            shape,
            key,
            frame: 0,
            next: animated.then(|| Instant::now() + frames[0].delay),
            pointer: pointer.clone(),
            serial,
            scale,
        });
        Ok(())
    }

    /// The images of the theme's file for `shape`, or else its default cursor, at the nominal size nearest to the
    /// theme's size at `scale`.
    fn load(
        &mut self,
        qh: &QueueHandle<State>,
        shape: Shape,
        scale: u32,
    ) -> Result<Vec<Frame>, Error> {
        let theme = self.theme.get_or_insert_with(CursorTheme::from_env);
        let names: Vec<&str> = names(shape)
            .iter()
            .chain(&["default", "left_ptr"])
            .copied()
            .collect();
        let file = theme
            .find(&names)
            .ok_or(Error::Protocol("the cursor theme has no default cursor"))?;
        let images = std::fs::read(&file)
            .ok()
            .and_then(|bytes| images(&bytes))
            .ok_or(Error::Protocol("a cursor file that is not an XCursor file"))?;
        let images = frames(&images, theme.size.saturating_mul(scale));
        if images.is_empty() {
            return Err(Error::Protocol("a cursor file without an image"));
        }
        images
            .into_iter()
            .map(|image| {
                Ok(Frame {
                    buffer: upload(&self.shm, qh, image)?,
                    size: (image.width, image.height),
                    hotspot: (image.xhot.min(image.width), image.yhot.min(image.height)),
                    delay: delay(image.delay),
                })
            })
            .collect()
    }
}

/// Show `frame` drawn for `scale` as the cursor of the pointer entered with `serial`.
fn draw(surface: &WlSurface, pointer: &WlPointer, serial: u32, frame: &Frame, scale: u32) {
    let (width, height) = frame.size;
    // a buffer scale must divide the buffer's size; a theme without a size for this scale draws it at 1
    let buffer_scale = if width.is_multiple_of(scale) && height.is_multiple_of(scale) {
        scale
    } else {
        1
    };
    surface.set_buffer_scale(buffer_scale as i32);
    surface.attach(Some(&frame.buffer), 0, 0);
    surface.damage_buffer(0, 0, width as i32, height as i32);
    surface.commit();
    pointer.set_cursor(
        serial,
        Some(surface),
        (frame.hotspot.0 / buffer_scale) as i32,
        (frame.hotspot.1 / buffer_scale) as i32,
    );
}

/// Whether the table of contents and every image an XCursor file declares lie within the file. The parser sizes an
/// image's allocation from its declared sides before it reads the pixels, so a short file could ask for gigabytes.
fn holds_its_images(bytes: &[u8]) -> bool {
    let word = |at: usize| -> Option<usize> {
        let b = bytes.get(at..at.checked_add(4)?)?;
        usize::try_from(u32::from_le_bytes(b.try_into().ok()?)).ok()
    };
    let (Some(header), Some(count)) = (word(4), word(12)) else {
        return false;
    };
    if !bytes.starts_with(b"Xcur") || header < HEADER {
        return false;
    }
    let fits =
        |start: usize, len: usize| start.checked_add(len).is_some_and(|end| end <= bytes.len());
    (0..count).all(|i| {
        let entry = header + i * TOC_ENTRY;
        let (Some(kind), Some(at)) = (word(entry), word(entry + 8)) else {
            return false;
        };
        if kind != XCURSOR_IMAGE as usize {
            return true;
        }
        let pixels = word(at + 16)
            .zip(word(at + 20))
            .and_then(|(w, h)| w.checked_mul(h)?.checked_mul(4));
        pixels.is_some_and(|len| fits(at + IMAGE_HEADER, len))
    })
}

/// The images of an XCursor file; `None` for a file that is not one or does not hold what it declares.
fn images(bytes: &[u8]) -> Option<Vec<Image>> {
    holds_its_images(bytes)
        .then(|| parse_xcursor(bytes))
        .flatten()
}

/// The image of `images` no wider or taller than [`MAX_SIDE`] whose nominal size is nearest to `wanted`.
fn nearest(images: &[Image], wanted: u32) -> Option<&Image> {
    images
        .iter()
        .filter(|i| drawable(i))
        .min_by_key(|i| i.size.abs_diff(wanted))
}

/// The images of the cursor at the nominal size nearest to `wanted`, in the file's order, at most [`MAX_FRAMES`]: one
/// for a still cursor, each animation step for an animated one.
fn frames(images: &[Image], wanted: u32) -> Vec<&Image> {
    let Some(size) = nearest(images, wanted).map(|i| i.size) else {
        return Vec::new();
    };
    images
        .iter()
        .filter(|i| i.size == size && drawable(i))
        .take(MAX_FRAMES)
        .collect()
}

/// How long an image whose file says `millis` is shown, at least [`MIN_DELAY`].
fn delay(millis: u32) -> Duration {
    Duration::from_millis(millis.into()).max(MIN_DELAY)
}

fn drawable(image: &Image) -> bool {
    (1..=MAX_SIDE).contains(&image.width) && (1..=MAX_SIDE).contains(&image.height)
}

/// A buffer holding `image`. XCursor pixels are little-endian ARGB, the byte order of `wl_shm`'s `argb8888`.
fn upload(shm: &WlShm, qh: &QueueHandle<State>, image: &Image) -> Result<WlBuffer, Error> {
    let len = (image.width * image.height * 4) as usize;
    let pool_len =
        i32::try_from(len).map_err(|_| Error::Protocol("a cursor image too large to share"))?;
    let pixels = image
        .pixels_rgba
        .get(..len)
        .ok_or(Error::Protocol("a cursor image shorter than its size"))?;
    let mut file = File::from(
        memfd_create("cupertino-cursor", MemfdFlags::CLOEXEC).map_err(|e| Error::Io(e.into()))?,
    );
    file.write_all(pixels).map_err(Error::Io)?;
    let pool = shm.create_pool(file.as_fd(), pool_len, qh, ());
    let buffer = pool.create_buffer(
        0,
        image.width as i32,
        image.height as i32,
        (image.width * 4) as i32,
        wl_shm::Format::Argb8888,
        qh,
        (),
    );
    pool.destroy();
    Ok(buffer)
}

/// The theme's names for `shape`: the CSS name of the cursor specification, then older X11 names themes still use.
fn names(shape: Shape) -> &'static [&'static str] {
    match shape {
        Shape::Default => &["default", "left_ptr"],
        Shape::ContextMenu => &["context-menu"],
        Shape::Help => &["help", "question_arrow"],
        Shape::Pointer => &["pointer", "hand2"],
        Shape::Progress => &["progress", "left_ptr_watch"],
        Shape::Wait => &["wait", "watch"],
        Shape::Cell => &["cell", "plus"],
        Shape::Crosshair => &["crosshair", "cross"],
        Shape::Text => &["text", "xterm"],
        Shape::VerticalText => &["vertical-text"],
        Shape::Alias => &["alias", "dnd-link"],
        Shape::Copy => &["copy", "dnd-copy"],
        Shape::Move => &["move", "dnd-move"],
        Shape::NoDrop => &["no-drop", "dnd-none"],
        Shape::NotAllowed => &["not-allowed", "crossed_circle"],
        Shape::Grab => &["grab", "openhand"],
        Shape::Grabbing => &["grabbing", "closedhand"],
        Shape::EResize => &["e-resize", "right_side"],
        Shape::NResize => &["n-resize", "top_side"],
        Shape::NeResize => &["ne-resize", "top_right_corner"],
        Shape::NwResize => &["nw-resize", "top_left_corner"],
        Shape::SResize => &["s-resize", "bottom_side"],
        Shape::SeResize => &["se-resize", "bottom_right_corner"],
        Shape::SwResize => &["sw-resize", "bottom_left_corner"],
        Shape::WResize => &["w-resize", "left_side"],
        Shape::EwResize => &["ew-resize", "sb_h_double_arrow"],
        Shape::NsResize => &["ns-resize", "sb_v_double_arrow"],
        Shape::NeswResize => &["nesw-resize", "fd_double_arrow"],
        Shape::NwseResize => &["nwse-resize", "bd_double_arrow"],
        Shape::ColResize => &["col-resize", "sb_h_double_arrow"],
        Shape::RowResize => &["row-resize", "sb_v_double_arrow"],
        Shape::AllScroll => &["all-scroll", "fleur"],
        Shape::ZoomIn => &["zoom-in"],
        Shape::ZoomOut => &["zoom-out"],
        Shape::DndAsk => &["dnd-ask", "copy"],
        Shape::AllResize => &["all-resize", "fleur"],
        _ => &[],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An XCursor file whose images, `(nominal size, width, height)`, declare their sides but carry `pixels` bytes
    /// each.
    fn file(images: &[(u32, u32, u32)], pixels: usize) -> Vec<u8> {
        let mut out = b"Xcur".to_vec();
        let words =
            |out: &mut Vec<u8>, ws: &[u32]| ws.iter().for_each(|w| out.extend(w.to_le_bytes()));
        words(&mut out, &[16, 0x1_0000, images.len() as u32]);
        let chunk = IMAGE_HEADER + pixels;
        for (i, (size, _, _)) in images.iter().enumerate() {
            let at = HEADER + TOC_ENTRY * images.len() + chunk * i;
            words(&mut out, &[XCURSOR_IMAGE, *size, at as u32]);
        }
        for (size, width, height) in images {
            words(
                &mut out,
                &[36, XCURSOR_IMAGE, *size, 1, *width, *height, 0, 0, 0],
            );
            out.extend(std::iter::repeat_n(0xff, pixels));
        }
        out
    }

    #[test]
    fn an_image_shows_for_its_delay_but_never_less_than_a_frame() {
        assert_eq!(delay(0), MIN_DELAY);
        assert_eq!(delay(1), MIN_DELAY);
        assert_eq!(delay(30), Duration::from_millis(30));
    }

    #[test]
    fn an_animated_cursor_keeps_each_image_of_the_nearest_size_in_order() {
        let bytes = file(
            &[(24, 4, 4), (48, 4, 4), (24, 4, 4), (48, 4, 4), (24, 4, 4)],
            64,
        );
        let images = parse_xcursor(&bytes).unwrap();
        assert_eq!(frames(&images, 24).len(), 3);
        assert!(frames(&images, 40).iter().all(|i| i.size == 48));
        assert_eq!(frames(&images, 40).len(), 2);
        let many = file(&vec![(24, 4, 4); MAX_FRAMES + 6], 64);
        assert_eq!(frames(&parse_xcursor(&many).unwrap(), 24).len(), MAX_FRAMES);
    }

    #[test]
    fn a_file_holding_what_it_declares_is_read() {
        let bytes = file(&[(24, 24, 24), (24, 24, 24)], 24 * 24 * 4);
        assert!(holds_its_images(&bytes));
        assert_eq!(parse_xcursor(&bytes).map(|i| i.len()), Some(2));
    }

    #[test]
    fn an_image_larger_than_the_file_is_refused_before_parsing() {
        // 30000 × 30000 declared, 16 pixels present: the parser alone would ask for 3.6 GB before finding the file short
        assert!(images(&file(&[(24, 30_000, 30_000)], 64)).is_none());
        assert!(!holds_its_images(&file(&[(24, 4096, 4096)], 64)));
        assert!(!holds_its_images(&file(&[(24, 4, 5)], 4 * 4 * 4)));
    }

    #[test]
    fn a_table_of_contents_longer_than_the_file_is_refused() {
        let mut bytes = file(&[(24, 4, 4)], 64);
        bytes[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(images(&bytes).is_none());
        assert!(!holds_its_images(b"Xcur"));
        assert!(!holds_its_images(b"Xcuz\x10\0\0\0\0\0\x01\0\0\0\0\0"));
    }

    #[test]
    fn an_image_wider_or_taller_than_the_largest_cursor_is_never_drawn() {
        let side = MAX_SIDE + 1;
        let bytes = file(&[(48, side, 4), (24, 24, 24)], (side * 4 * 4) as usize);
        let images = parse_xcursor(&bytes).expect("a well-formed file");
        let chosen = nearest(&images, 48).expect("the small image");
        assert_eq!((chosen.width, chosen.height), (24, 24));
        let images = parse_xcursor(&file(&[(48, 4, side)], (side * 4 * 4) as usize)).unwrap();
        assert!(nearest(&images, 48).is_none());
    }
}
