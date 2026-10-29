//! The cursor drawn by the client from an XCursor theme, for a compositor without `wp_cursor_shape_v1`. An animated
//! cursor shows its first image.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::path::{Path, PathBuf};

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
/// The largest side the XCursor format allows.
const MAX_SIDE: u32 = 0x7fff;

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

/// A cursor image in a buffer of its own.
struct Loaded {
    buffer: WlBuffer,
    size: (u32, u32),
    hotspot: (u32, u32),
}

/// The theme (the user's, read when first needed, unless one was set), the images loaded from it for each shape and
/// scale, and the surface the cursor is drawn on.
pub(crate) struct ThemedCursor {
    compositor: WlCompositor,
    shm: WlShm,
    theme: Option<CursorTheme>,
    loaded: HashMap<(u32, u32), Loaded>,
    surface: Option<WlSurface>,
}

impl ThemedCursor {
    pub(crate) fn new(compositor: WlCompositor, shm: WlShm) -> ThemedCursor {
        ThemedCursor {
            compositor,
            shm,
            theme: None,
            loaded: HashMap::new(),
            surface: None,
        }
    }

    /// Draw from `theme` from now on.
    pub(crate) fn set_theme(&mut self, theme: CursorTheme) {
        for (_, loaded) in self.loaded.drain() {
            loaded.buffer.destroy();
        }
        self.theme = Some(theme);
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
            let loaded = self.load(qh, shape, scale)?;
            self.loaded.insert(key, loaded);
        }
        let loaded = &self.loaded[&key];
        let (width, height) = loaded.size;
        // a buffer scale must divide the buffer's size; a theme without a size for this scale draws it at 1
        let buffer_scale = if width.is_multiple_of(scale) && height.is_multiple_of(scale) {
            scale
        } else {
            1
        };
        let surface = self
            .surface
            .get_or_insert_with(|| self.compositor.create_surface(qh, ()));
        surface.set_buffer_scale(buffer_scale as i32);
        surface.attach(Some(&loaded.buffer), 0, 0);
        surface.damage_buffer(0, 0, width as i32, height as i32);
        surface.commit();
        pointer.set_cursor(
            serial,
            Some(surface),
            (loaded.hotspot.0 / buffer_scale) as i32,
            (loaded.hotspot.1 / buffer_scale) as i32,
        );
        Ok(())
    }

    /// The image of the theme's file for `shape`, or else its default cursor, whose nominal size is nearest to the
    /// theme's size at `scale`.
    fn load(&mut self, qh: &QueueHandle<State>, shape: Shape, scale: u32) -> Result<Loaded, Error> {
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
            .and_then(|bytes| parse_xcursor(&bytes))
            .ok_or(Error::Protocol("a cursor file that is not an XCursor file"))?;
        let wanted = theme.size.saturating_mul(scale);
        let image = images
            .iter()
            .filter(|i| (1..=MAX_SIDE).contains(&i.width) && (1..=MAX_SIDE).contains(&i.height))
            .min_by_key(|i| i.size.abs_diff(wanted))
            .ok_or(Error::Protocol("a cursor file without an image"))?;
        Ok(Loaded {
            buffer: upload(&self.shm, qh, image)?,
            size: (image.width, image.height),
            hotspot: (image.xhot.min(image.width), image.yhot.min(image.height)),
        })
    }
}

/// A buffer holding `image`. XCursor pixels are little-endian ARGB, the byte order of `wl_shm`'s `argb8888`.
fn upload(shm: &WlShm, qh: &QueueHandle<State>, image: &Image) -> Result<WlBuffer, Error> {
    let len = (image.width * image.height * 4) as usize;
    let pixels = image
        .pixels_rgba
        .get(..len)
        .ok_or(Error::Protocol("a cursor image shorter than its size"))?;
    let mut file = File::from(
        memfd_create("cupertino-cursor", MemfdFlags::CLOEXEC).map_err(|e| Error::Io(e.into()))?,
    );
    file.write_all(pixels).map_err(Error::Io)?;
    let pool = shm.create_pool(file.as_fd(), len as i32, qh, ());
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
