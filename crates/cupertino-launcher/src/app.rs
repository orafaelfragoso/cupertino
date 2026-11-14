//! Glue between the launcher's Kit application ([`cupertino_launcher::kit`]) and the session: the windows get their
//! layer surfaces from [`LauncherRoles`], `Toggle` and `ToggleApplications` arrive from the bus as messages (and the
//! output's size and the Reduce Motion option from the server), and [`AppHost`] starts what the panel activates,
//! builds the file index on a thread, searches it on another, resizes the panel's surface, loads the results' and the
//! applications' icons, reads what the preview shows of the selected file on a thread, and keeps the deadlines.

use cupertino_apps::{MimeDatabase, XdgDirs};
use cupertino_assets::Assets;
use cupertino_defaults::terminal::DEFAULT_TERMINALS;
use cupertino_icons::{SystemIcons, ThemeIcons};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{next_resource_id, Application, Command, Window};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::geometry::Size;
use cupertino_kit::platform::{WindowSpec, WindowSystem as _};
use cupertino_kit::render::{ImageResource, PixelFormat};
use cupertino_launcher::browse::{App as BrowserApp, BrowseLayout};
use cupertino_launcher::control::{self, Call};
use cupertino_launcher::engine::{Action, ItemKind, ResultItem};
use cupertino_launcher::exec::CommandSpec;
use cupertino_launcher::files::{FileIndex, FileSearchConfig};
use cupertino_launcher::kit::{self, Host, Message, Model, Running, SearchRequest};
use cupertino_launcher::panel::Now;
use cupertino_launcher::preview::{self, LatestOnly, Preview, PreviewBody, TEXT_SNIFF};
use cupertino_launcher::providers::{Provider, Reply, Update};
use cupertino_launcher::scope::{FileItem, ScopeAnswer, ScopeLayout, ScopeRequest};
use cupertino_launcher::view::{
    APP_ID, CATCHER_APP_ID, DOCUMENT_ICON, FOLDER_ICON, GENERIC_APPLICATION_ICON, HIDDEN_APP_ID,
    ICON_SIZE, PREVIEW_ICON_SIZE, PREVIEW_IMAGE, PREVIEW_TEXT_LINES, SEARCH_SYMBOL, SHADOW_TOP,
    TOP_FRACTION,
};
use cupertino_launcher::{Launcher, LauncherConfig};
use cupertino_modifiers::Profile;
use cupertino_protocol_app::AppEvent;
use cupertino_shell_client::{
    Anchor, AppActivation, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
    LayerSurfaceConfig, Margin, Session, Shell, ShellEvent,
};
use cupertino_thumbnails::{ThumbSize, Thumbnailer};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles, WaylandWindowSystem};
use cupertino_wayland_client::{Connection, Error as WaylandError, ExtensionEvent, SurfaceId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::error::Error;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Role of the surfaces for the server (`wlr-layer-shell` namespace).
const NAMESPACE: &str = "cupertino-launcher";

/// Device pixels the results' icons are loaded at: their size at scales 1 to 3, so they stay sharp on every output.
const ICON_PX: [u32; 3] = [ICON_SIZE as u32, ICON_SIZE as u32 * 2, ICON_SIZE as u32 * 3];
/// Device pixels the browser's icons are loaded at: the list's and the grid's sizes at scales 1 and 2, the grid's at 3.
const BROWSE_ICON_PX: [u32; 4] = [
    BrowseLayout::LIST_ICON as u32,
    BrowseLayout::ICON as u32,
    BrowseLayout::ICON as u32 * 2,
    BrowseLayout::ICON as u32 * 3,
];
/// Device pixels the preview's application icon is loaded at.
const PREVIEW_ICON_PX: [u32; 3] = [
    PREVIEW_ICON_SIZE as u32,
    PREVIEW_ICON_SIZE as u32 * 2,
    PREVIEW_ICON_SIZE as u32 * 3,
];
/// The most items of a folder the preview counts.
const MAX_COUNTED: usize = 100_000;

/// Command line of the process.
pub struct Options {
    /// Index the home directory for file results.
    pub files: bool,
    /// Terminal emulator that runs entries with `Terminal=true`; the launcher default when absent.
    pub terminal: Option<String>,
}

impl Options {
    /// Parse the arguments; the problem is returned as the error.
    pub fn parse(args: impl Iterator<Item = String>) -> std::result::Result<Self, String> {
        let mut options = Options {
            files: true,
            terminal: None,
        };
        let mut args = args;
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--no-files" => options.files = false,
                "--terminal" => {
                    options.terminal = Some(args.next().ok_or("--terminal needs a program")?)
                }
                _ => return Err(format!("unknown argument {arg}")),
            }
        }
        Ok(options)
    }
}

/// The layer surfaces of the launcher's windows, by their `app_id`.
#[derive(Default)]
struct Surfaces {
    panel: Option<LayerSurface>,
    /// The panel's top margin as last asked for.
    panel_margin: i32,
    catcher: Option<LayerSurface>,
    hidden: Option<LayerSurface>,
}

impl Surfaces {
    fn all(&self) -> impl Iterator<Item = &LayerSurface> {
        [&self.panel, &self.catcher, &self.hidden]
            .into_iter()
            .flatten()
    }
}

/// Gives the windows their layer surfaces, none of them on an output, so the server puts them on its active output (the
/// one the person works on): the panel on the overlay layer, anchored to the top only (so centred horizontally) at one
/// fifth of the output's height, with the keyboard; the catcher over the whole output on the top layer, below the
/// panel; the hidden window a point that takes no input.
struct LauncherRoles {
    shell: LayerShell,
    surfaces: Rc<RefCell<Surfaces>>,
    /// Where a session lock is told to the model (the window system tells it only to the accessibility adapter).
    signals: async_channel::Sender<Signal>,
}

/// What reaches the model from outside the window system.
#[derive(Clone, Copy, Debug)]
enum Signal {
    /// `org.cupertino.Launcher1.Toggle`.
    Toggle,
    /// `org.cupertino.Launcher1.ToggleApplications`.
    Applications,
    /// The session was locked.
    Locked,
    /// The output the launcher is on is this large (the catcher covers it).
    Output(Size),
    /// The session's Reduce Motion option.
    ReduceMotion(bool),
    /// The server's answer for the application opened as this launch.
    Opened(u64, Running),
}

/// The top margin of the panel's window on an output `height` points tall: the panel at [`TOP_FRACTION`] of the
/// height, less the room above it for its shadow.
fn top_margin(height: f64) -> i32 {
    (height * TOP_FRACTION - SHADOW_TOP).round().max(0.0) as i32
}

impl LauncherRoles {
    fn config(conn: &Connection, spec: &WindowSpec) -> Option<LayerSurfaceConfig> {
        let config = |layer, anchor, size, top, keyboard| LayerSurfaceConfig {
            namespace: NAMESPACE.to_string(),
            output: None,
            layer,
            anchor,
            size,
            margin: Margin {
                top,
                ..Margin::default()
            },
            exclusive_zone: -1,
            keyboard,
        };
        Some(match spec.app_id.as_str() {
            APP_ID => {
                // the output the server picks is not known before the configure: the first output's height guesses
                let height = conn
                    .outputs()
                    .into_iter()
                    .find_map(|o| o.geometry)
                    .map_or(0.0, |g| f64::from(g.height));
                config(
                    Layer::Overlay,
                    Anchor::Top,
                    (
                        spec.size.width.ceil() as u32,
                        spec.size.height.ceil() as u32,
                    ),
                    top_margin(height),
                    KeyboardInteractivity::Exclusive,
                )
            }
            CATCHER_APP_ID => config(
                Layer::Top,
                Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                (0, 0),
                0,
                KeyboardInteractivity::None,
            ),
            HIDDEN_APP_ID => config(
                Layer::Overlay,
                Anchor::Top,
                (1, 1),
                0,
                KeyboardInteractivity::None,
            ),
            _ => return None,
        })
    }
}

impl SurfaceRoles for LauncherRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<std::result::Result<SurfaceId, WaylandError>> {
        let config = Self::config(conn, spec)?;
        let layer = match self.shell.layer_surface(conn, &config) {
            Ok(layer) => layer,
            Err(cupertino_shell_client::Error::Transport(e)) => return Some(Err(e)),
            Err(_) => return Some(Err(WaylandError::Protocol("no output for the launcher"))),
        };
        let surface = layer.surface();
        let mut surfaces = self.surfaces.borrow_mut();
        match spec.app_id.as_str() {
            APP_ID => {
                surfaces.panel_margin = config.margin.top;
                surfaces.panel = Some(layer);
            }
            CATCHER_APP_ID => surfaces.catcher = Some(layer),
            _ => {
                // the hidden window never takes the pointer
                if let Err(e) = conn.set_input_region(surface, Some(&[])) {
                    return Some(Err(e));
                }
                surfaces.hidden = Some(layer);
            }
        }
        Some(Ok(surface))
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        match event.downcast_ref::<AppEvent>() {
            Some(AppEvent::SessionLock { locked: true }) => {
                // not a role event: the window system still tells the accessibility adapter
                let _ = self.signals.try_send(Signal::Locked);
                return None;
            }
            Some(AppEvent::DisplayOptions(options)) => {
                // the window system takes them too, for the theme
                let _ = self
                    .signals
                    .try_send(Signal::ReduceMotion(options.reduce_motion));
                return None;
            }
            _ => {}
        }
        if let Some(&ShellEvent::AppActivation { serial, result }) = event.downcast_ref() {
            let running = match result {
                AppActivation::Activated => Running::Raised,
                AppActivation::Resident => Running::Resident,
                AppActivation::NotRunning => Running::NotRunning,
            };
            let _ = self
                .signals
                .try_send(Signal::Opened(u64::from(serial), running));
            return None;
        }
        let surfaces = &mut *self.surfaces.borrow_mut();
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => {
                let layer = surfaces.all().find(|l| l.surface() == surface)?;
                // a configure without a size closes the window, which the model answers
                let size = match layer.configured(size) {
                    Ok(size) => size,
                    Err(_) => return Some(RoleEvent::Closed(surface)),
                };
                // the catcher covers the output the server put both on: its height places the panel, whose margin
                // guessed from the first output, before the panel's first frame commits it
                if surfaces
                    .catcher
                    .as_ref()
                    .is_some_and(|c| c.surface() == surface)
                {
                    let _ = self.signals.try_send(Signal::Output(Size::new(
                        f64::from(size.0),
                        f64::from(size.1),
                    )));
                    let margin = top_margin(f64::from(size.1));
                    if let Some(panel) = &surfaces.panel {
                        if margin != surfaces.panel_margin {
                            panel.set_margin(Margin {
                                top: margin,
                                ..Margin::default()
                            });
                            surfaces.panel_margin = margin;
                        }
                    }
                }
                Some(RoleEvent::Configure { surface, size })
            }
            // their output went away
            ShellEvent::Closed(surface) if surfaces.all().any(|l| l.surface() == surface) => {
                Some(RoleEvent::Closed(surface))
            }
            _ => None,
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        let surfaces = &mut *self.surfaces.borrow_mut();
        for slot in [
            &mut surfaces.panel,
            &mut surfaces.catcher,
            &mut surfaces.hidden,
        ] {
            if slot.as_ref().is_some_and(|l| l.surface() == surface) {
                *slot = None;
            }
        }
    }
}

fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Run a command detached from the loop; its exit is reaped on a thread.
fn spawn(mut command: Process) {
    let program = command.get_program().to_string_lossy().into_owned();
    match command.spawn() {
        Ok(mut child) => {
            std::thread::spawn(move || {
                if let Err(e) = child.wait() {
                    eprintln!("cupertino-launcher: {program}: {e}");
                }
            });
        }
        Err(e) => eprintln!("cupertino-launcher: cannot run {program}: {e}"),
    }
}

/// A premultiplied pixmap of the icon loaders as a Kit image.
fn image(width: u32, height: u32, pixels: &[u8]) -> ImageSource {
    ImageSource {
        id: next_resource_id(),
        image: ImageResource {
            revision: 0,
            width,
            height,
            stride: width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels),
        },
    }
}

struct AppHost {
    surfaces: Rc<RefCell<Surfaces>>,
    start: Instant,
    /// Applications' own icons (`Icon=`).
    themes: ThemeIcons,
    /// The generic application, folder and document icons.
    system: SystemIcons,
    /// File icons by type, and the type database, for the preview; made at the first preview of a file.
    files: Option<FileIcons>,
    /// Reads what the preview shows of a file, the newest asked for only; started at the first preview of a file.
    previews: Option<LatestOnly<PreviewRead>>,
    /// Searches the file index, the newest query only; started at the first search.
    searches: Option<LatestOnly<FileSearch>>,
    /// Searches the file index for the files scope, the newest request only; started at the first one.
    scopes: Option<LatestOnly<ScopeSearch>>,
    /// Loads and decodes the files scope's thumbnails in the order asked, on a thread of its own; started at the
    /// first one asked. The model keeps at most `kit::MAX_FILE_THUMB_LOADS` waiting.
    thumb_loads: Option<std::sync::mpsc::Sender<ThumbLoad>>,
    /// Loads and decodes the browser's application icons in the order asked, on a thread of its own; started at the
    /// first icon asked. The model keeps at most `kit::MAX_APP_ICON_LOADS` waiting.
    icon_loads: Option<std::sync::mpsc::Sender<IconLoad>>,
    /// The asset packs, for the icon thread's own lookup.
    assets: Assets,
    /// The session object of the shell global, which brings a running application forward; `None` when the server
    /// does not offer the global to the launcher.
    session: Option<Session>,
}

/// A file search the search thread is asked for, and where its answer goes. A search replaced by a newer one before
/// it started drops `reply`, which answers the loop with a stale failure.
struct FileSearch {
    request: SearchRequest,
    reply: async_channel::Sender<Update>,
}

/// A scope search the scope thread is asked for, and where its answer goes. A search replaced by a newer one before it
/// started drops `reply`, which ends the wait without an answer.
struct ScopeSearch {
    request: ScopeRequest,
    reply: async_channel::Sender<ScopeAnswer>,
}

/// A file thumbnail the thumbnail thread is asked for, and where its picture goes (none when the file has none).
struct ThumbLoad {
    path: PathBuf,
    reply: async_channel::Sender<Vec<ImageSource>>,
}

/// Device pixels a files-scope thumbnail is loaded at: the cache's smallest size, which covers its box at scale 1 and
/// nearly at scale 2.
const FILE_THUMB_EDGE: u32 = 128;

/// The thumbnail of the file at `path` as a Kit image, none when it has none (the type's icon stays).
fn file_thumbnail(thumbnailer: &Thumbnailer, path: &Path) -> Vec<ImageSource> {
    let Ok(mut pixels) = thumbnailer.pixels(path, ThumbSize::Normal, FILE_THUMB_EDGE) else {
        return Vec::new();
    };
    premultiply(&mut pixels.rgba);
    vec![image(pixels.width, pixels.height, &pixels.rgba)]
}

/// An application icon the icon thread is asked for (the entry's `Icon`), and where its renditions go.
struct IconLoad {
    icon: Option<String>,
    reply: async_channel::Sender<Vec<ImageSource>>,
}

/// The renditions of the browser's icon `name` at its sizes, from the icon theme, or the generic application's.
fn browser_icon(
    themes: &mut ThemeIcons,
    system: &mut SystemIcons,
    name: Option<&str>,
) -> Vec<ImageSource> {
    let own: Vec<ImageSource> = name
        .map(|name| {
            BROWSE_ICON_PX
                .iter()
                .filter_map(|&px| themes.load(name, px))
                .map(|pixmap| image(pixmap.width(), pixmap.height(), pixmap.data()))
                .collect()
        })
        .unwrap_or_default();
    if !own.is_empty() {
        return own;
    }
    BROWSE_ICON_PX
        .iter()
        .map(|&px| {
            let pixmap = system.raster(GENERIC_APPLICATION_ICON, px);
            image(pixmap.width(), pixmap.height(), pixmap.data())
        })
        .collect()
}

/// A file the preview reader is asked about, and where its answer goes.
struct PreviewRead {
    path: PathBuf,
    mime: String,
    is_dir: bool,
    icon: Vec<ImageSource>,
    reply: async_channel::Sender<Preview>,
}

/// The MIME database of the XDG data directories, empty when it cannot be read.
fn mime_database() -> MimeDatabase {
    let dirs = XdgDirs::from_env()
        .map(|d| d.all_data_dirs())
        .unwrap_or_default();
    MimeDatabase::load(&dirs).unwrap_or_default()
}

/// Seconds east of UTC of local time at `time` (Unix seconds).
fn local_offset(time: i64) -> i64 {
    let tt = time as libc::time_t;
    // SAFETY: a zeroed tm is a valid out value for localtime_r, which reads `tt` and writes only `tm`
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let converted = unsafe { !libc::localtime_r(&tt, &mut tm).is_null() };
    if converted {
        tm.tm_gmtoff
    } else {
        0
    }
}

/// Straight RGBA as premultiplied, in place.
fn premultiply(rgba: &mut [u8]) {
    for pixel in rgba.as_chunks_mut::<4>().0 {
        let a = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * a + 127) / 255) as u8;
        }
    }
}

/// A picture of the image at `path` for the preview, at twice its size in points so it stays sharp at scale 2.
fn picture(path: &Path) -> Option<PreviewBody> {
    let edge = (2.0 * PREVIEW_IMAGE.width.max(PREVIEW_IMAGE.height)) as u32;
    let mut pixels = Thumbnailer::from_env(NAMESPACE)
        .pixels(path, ThumbSize::XLarge, edge)
        .ok()?;
    premultiply(&mut pixels.rgba);
    let size = preview::fit(
        Size::new(
            f64::from(pixels.width) / 2.0,
            f64::from(pixels.height) / 2.0,
        ),
        PREVIEW_IMAGE,
    );
    Some(PreviewBody::Image {
        sources: vec![image(pixels.width, pixels.height, &pixels.rgba)],
        size,
    })
}

/// At most `limit` bytes from the start of `path`, when it is a regular file (a link is followed). It is opened without
/// waiting, so a pipe or a device put in its place never holds the reader, and refused unless the opened file is
/// regular.
fn read_start(path: &Path, limit: usize) -> Option<Vec<u8>> {
    use std::os::unix::fs::OpenOptionsExt;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_NOCTTY)
        .open(path)
        .ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut bytes = Vec::with_capacity(limit);
    file.take(limit as u64).read_to_end(&mut bytes).ok()?;
    Some(bytes)
}

/// The start of the file at `path` as text, when it reads as text.
fn excerpt(path: &Path) -> Option<PreviewBody> {
    let bytes = read_start(path, TEXT_SNIFF)?;
    preview::text_excerpt(&bytes, PREVIEW_TEXT_LINES).map(PreviewBody::Text)
}

/// What the preview shows of the file or folder at `path` of type `mime`: its picture or text, its size or how many
/// items it holds, when it changed and where it is.
fn read_preview(path: &Path, mime: &str, is_dir: bool, icon: Vec<ImageSource>) -> Preview {
    let meta = std::fs::metadata(path).ok();
    let mut details = Vec::new();
    if is_dir {
        if let Ok(entries) = std::fs::read_dir(path) {
            let count = entries.take(MAX_COUNTED).count();
            let items = if count == 1 { "item" } else { "items" };
            details.push(("Contains".to_owned(), format!("{count} {items}")));
        }
    } else if let Some(meta) = &meta {
        details.push(("Size".to_owned(), preview::human_size(meta.len())));
    }
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64);
    if let Some(time) = modified {
        let now = unix_now() as i64;
        details.push((
            "Modified".to_owned(),
            preview::format_date(time, local_offset(time), now),
        ));
    }
    let home = std::env::var_os("HOME").map(PathBuf::from);
    details.push(("Where".to_owned(), preview::location(path, home.as_deref())));
    let body = if is_dir {
        None
    } else if mime.starts_with("image/") {
        picture(path)
    } else if meta.as_ref().is_some_and(|m| m.len() > 0) {
        excerpt(path)
    } else {
        None
    };
    Preview {
        body: body.unwrap_or(PreviewBody::Icon),
        icon,
        kind: preview::kind_name(mime),
        details,
    }
}

impl Host for AppHost {
    fn run(&mut self, spec: CommandSpec, token: Option<String>) {
        let spec = match token {
            Some(token) => spec.with_activation_token(&token),
            None => spec,
        };
        spawn(spec.to_command());
    }

    fn open(&mut self, target: &str) {
        let mut command = Process::new("xdg-open");
        command.arg(target);
        spawn(command);
    }

    fn index(&mut self, config: FileSearchConfig) -> Vec<Command<Message>> {
        let (sender, built) = async_channel::bounded(1);
        std::thread::spawn(move || {
            let _ = sender.send_blocking(Arc::new(FileIndex::build(config)));
        });
        vec![Command::Perform(Box::pin(async move {
            built
                .recv()
                .await
                .map_or(Message::IndexFailed, Message::Indexed)
        }))]
    }

    fn resize(&mut self, size: Size) {
        if let Some(panel) = self.surfaces.borrow_mut().panel.as_mut() {
            panel.set_placement(
                Anchor::Top,
                (size.width.ceil() as u32, size.height.ceil() as u32),
            );
        }
    }

    fn icon(&mut self, item: &ResultItem) -> Vec<ImageSource> {
        let system = match item.kind {
            ItemKind::Application | ItemKind::ApplicationAction => {
                if let Some(name) = item.icon.as_deref() {
                    let own: Vec<ImageSource> = ICON_PX
                        .iter()
                        .filter_map(|&px| self.themes.load(name, px))
                        .map(|pixmap| image(pixmap.width(), pixmap.height(), pixmap.data()))
                        .collect();
                    if !own.is_empty() {
                        return own;
                    }
                }
                GENERIC_APPLICATION_ICON
            }
            ItemKind::Folder => FOLDER_ICON,
            ItemKind::File => DOCUMENT_ICON,
            ItemKind::WebSearch | ItemKind::Calculator | ItemKind::Conversion => return Vec::new(),
        };
        ICON_PX
            .iter()
            .map(|&px| {
                let pixmap = self.system.raster(system, px);
                image(pixmap.width(), pixmap.height(), pixmap.data())
            })
            .collect()
    }

    fn preview(&mut self, item: &ResultItem) -> Vec<Command<Message>> {
        let id = item.id.clone();
        let ready = |id: String, preview: Preview| -> Vec<Command<Message>> {
            vec![Command::Perform(Box::pin(async move {
                Message::Previewed(id, Arc::new(preview))
            }))]
        };
        match (&item.kind, &item.action) {
            (ItemKind::Application | ItemKind::ApplicationAction, _) => {
                let icon: Vec<ImageSource> = item
                    .icon
                    .as_deref()
                    .map(|name| {
                        PREVIEW_ICON_PX
                            .iter()
                            .filter_map(|&px| self.themes.load(name, px))
                            .map(|pixmap| image(pixmap.width(), pixmap.height(), pixmap.data()))
                            .collect()
                    })
                    .unwrap_or_default();
                if icon.is_empty() {
                    return Vec::new();
                }
                ready(
                    id,
                    Preview {
                        body: PreviewBody::Icon,
                        icon,
                        kind: String::new(),
                        details: Vec::new(),
                    },
                )
            }
            (ItemKind::File | ItemKind::Folder, Action::OpenPath(path)) => {
                let path = path.clone();
                // the index knows a folder from a file: the loop asks the file system nothing
                let is_dir = item.kind == ItemKind::Folder;
                let files = self
                    .files
                    .get_or_insert_with(|| FileIcons::discover(mime_database(), HashMap::new()));
                let mime = files.database().file_type(&path, is_dir).to_owned();
                let icon = files.images(&path, &mime, false, PREVIEW_ICON_SIZE);
                // what shows if the reader is lost; a read replaced by a newer one also ends here, for a result no
                // longer selected, and the model drops it
                let plain = Preview {
                    body: PreviewBody::Icon,
                    icon: icon.clone(),
                    kind: preview::kind_name(&mime),
                    details: Vec::new(),
                };
                if self.previews.is_none() {
                    self.previews = LatestOnly::new("launcher-preview", |job: PreviewRead| {
                        let preview = read_preview(&job.path, &job.mime, job.is_dir, job.icon);
                        let _ = job.reply.send_blocking(preview);
                    })
                    .map_err(|e| eprintln!("cupertino-launcher: no preview reader: {e}"))
                    .ok();
                }
                let (reply, read) = async_channel::bounded(1);
                if let Some(previews) = &self.previews {
                    previews.submit(PreviewRead {
                        path,
                        mime,
                        is_dir,
                        icon,
                        reply,
                    });
                }
                vec![Command::Perform(Box::pin(async move {
                    let preview = read.recv().await.unwrap_or(plain);
                    Message::Previewed(id, Arc::new(preview))
                }))]
            }
            _ => Vec::new(),
        }
    }

    fn app_icon(&mut self, app: &BrowserApp) -> Vec<Command<Message>> {
        if self.icon_loads.is_none() {
            let assets = self.assets.clone();
            let (jobs, queue) = std::sync::mpsc::channel::<IconLoad>();
            let started = std::thread::Builder::new()
                .name("launcher-icons".to_owned())
                .spawn(move || {
                    // the thread's own lookup and rasters: nothing it reads or decodes is shared with the loop
                    let mut themes = ThemeIcons::new(&assets);
                    let mut system = SystemIcons::new(&assets);
                    for job in queue {
                        let images = browser_icon(&mut themes, &mut system, job.icon.as_deref());
                        let _ = job.reply.send_blocking(images);
                    }
                });
            match started {
                Ok(_) => self.icon_loads = Some(jobs),
                Err(e) => eprintln!("cupertino-launcher: no icon thread: {e}"),
            }
        }
        let id = app.id.clone();
        let (reply, loaded) = async_channel::bounded(1);
        if let Some(loads) = &self.icon_loads {
            let _ = loads.send(IconLoad {
                icon: app.icon.clone(),
                reply,
            });
        }
        // without the thread, or once it is gone, the browser draws a placeholder
        vec![Command::Perform(Box::pin(async move {
            Message::AppIcon(id, loaded.recv().await.unwrap_or_default())
        }))]
    }

    fn search(&mut self, request: SearchRequest) -> Vec<Command<Message>> {
        if self.searches.is_none() {
            self.searches = LatestOnly::new("launcher-search", |job: FileSearch| {
                if job.request.cancel.is_cancelled() {
                    return;
                }
                let update = job.request.run();
                if !job.request.cancel.is_cancelled() {
                    let _ = job.reply.send_blocking(update);
                }
            })
            .map_err(|e| eprintln!("cupertino-launcher: no search thread: {e}"))
            .ok();
        }
        let generation = request.generation;
        let (reply, answer) = async_channel::bounded(1);
        match &self.searches {
            Some(searches) => searches.submit(FileSearch { request, reply }),
            // no thread: the search says it failed rather than nothing found
            None => drop(reply),
        }
        vec![Command::Perform(Box::pin(async move {
            Message::Provided(answer.recv().await.unwrap_or(Update {
                generation,
                provider: Provider::Files,
                reply: Reply::Failed("the search did not finish".to_owned()),
            }))
        }))]
    }

    fn scope_search(&mut self, request: ScopeRequest) -> Vec<Command<Message>> {
        if self.scopes.is_none() {
            self.scopes = LatestOnly::new("launcher-scope", |job: ScopeSearch| {
                let answer = job.request.run();
                let _ = job.reply.send_blocking(answer);
            })
            .map_err(|e| eprintln!("cupertino-launcher: no scope search thread: {e}"))
            .ok();
        }
        let (reply, answer) = async_channel::bounded(1);
        let generation = request.generation;
        match &self.scopes {
            Some(scopes) => scopes.submit(ScopeSearch { request, reply }),
            None => drop(reply),
        }
        vec![Command::Perform(Box::pin(async move {
            // without the thread, or for a search a newer one replaced, the answer is empty: the grid shows nothing
            // and never says it is searching
            Message::Scoped(answer.recv().await.unwrap_or(ScopeAnswer {
                generation,
                suggestions: Vec::new(),
                items: Vec::new(),
            }))
        }))]
    }

    fn file_icon(&mut self, item: &FileItem) -> Vec<ImageSource> {
        let files = self
            .files
            .get_or_insert_with(|| FileIcons::discover(mime_database(), HashMap::new()));
        let mime = files
            .database()
            .file_type(&item.path, item.is_dir)
            .to_owned();
        files.images(&item.path, &mime, false, ScopeLayout::THUMB)
    }

    fn file_thumb(&mut self, item: &FileItem) -> Vec<Command<Message>> {
        if self.thumb_loads.is_none() {
            let (jobs, queue) = std::sync::mpsc::channel::<ThumbLoad>();
            let started = std::thread::Builder::new()
                .name("launcher-thumbs".to_owned())
                .spawn(move || {
                    let thumbnailer = Thumbnailer::from_env(NAMESPACE);
                    for job in queue {
                        let images = file_thumbnail(&thumbnailer, &job.path);
                        let _ = job.reply.send_blocking(images);
                    }
                });
            match started {
                Ok(_) => self.thumb_loads = Some(jobs),
                Err(e) => eprintln!("cupertino-launcher: no thumbnail thread: {e}"),
            }
        }
        let path = item.path.clone();
        let (reply, loaded) = async_channel::bounded(1);
        if let Some(loads) = &self.thumb_loads {
            let _ = loads.send(ThumbLoad {
                path: path.clone(),
                reply,
            });
        }
        vec![Command::Perform(Box::pin(async move {
            Message::FileThumb(path, loaded.recv().await.unwrap_or_default())
        }))]
    }

    fn after(&mut self, seconds: f64, message: Message) -> Vec<Command<Message>> {
        let wait = Duration::from_secs_f64(seconds.clamp(0.0, 60.0));
        vec![Command::Perform(Box::pin(async move {
            async_io::Timer::after(wait).await;
            message
        }))]
    }

    fn now(&self) -> Now {
        Now {
            mono: self.start.elapsed().as_secs_f64(),
            unix: unix_now(),
        }
    }

    fn bring_forward(&mut self, desktop_id: &str, launch: u64) -> bool {
        // the answer carries the launch back as its serial
        let (Some(session), Ok(serial)) = (&self.session, u32::try_from(launch)) else {
            return false;
        };
        session.activate_app(serial, desktop_id)
    }

    fn ask_for_window(
        &mut self,
        bus_name: &str,
        source: Option<&Path>,
        token: Option<String>,
    ) -> Option<kit::BusCall> {
        if !source.is_some_and(cupertino_apps::entry::dbus_activatable_file) {
            return None;
        }
        Some(Box::pin(cupertino_apps::activate::activate_in_thread(
            "cupertino-launcher",
            bus_name.to_owned(),
            token,
        )))
    }
}

/// The model and the channel `Toggle` and the session lock arrive on.
pub struct App {
    model: Model,
    signals: async_channel::Receiver<Signal>,
}

fn listen(signals: &async_channel::Receiver<Signal>) -> Command<Message> {
    let signals = signals.clone();
    Command::Perform(Box::pin(async move {
        signals
            .recv()
            .await
            .map_or(Message::Unheard, |signal| match signal {
                Signal::Toggle => Message::Toggle,
                Signal::Applications => Message::Browse,
                Signal::Locked => Message::Locked,
                Signal::Output(size) => Message::Output(size),
                Signal::ReduceMotion(on) => Message::ReduceMotion(on),
                Signal::Opened(launch, running) => Message::Opened(launch, running),
            })
    }))
}

impl Application for App {
    type Model = App;
    type Message = Message;
    const RESIDENT: bool = true;

    /// Not used: the model needs the host and the bus [`run`] sets up, and starts through `run_on`.
    fn init() -> (App, Vec<Command<Message>>) {
        unreachable!("the launcher starts only through `run`, which hands the model to `run_on`")
    }

    fn update(app: &mut App, message: Message) -> Vec<Command<Message>> {
        let signalled = matches!(
            message,
            Message::Toggle
                | Message::Browse
                | Message::Locked
                | Message::Output(_)
                | Message::ReduceMotion(_)
                | Message::Opened(..)
        );
        let mut commands = kit::update(&mut app.model, message);
        if signalled {
            commands.push(listen(&app.signals));
        }
        commands
    }

    fn view(app: &App) -> Vec<Window<Message>> {
        kit::view(&app.model)
    }
}

/// Connect, claim the bus name and serve until the connection or the bus fails.
pub fn run(options: &Options) -> Result<std::convert::Infallible> {
    let mut system = WaylandWindowSystem::connect(Profile::cupertino())
        .map_err(|e| format!("cannot connect to the Wayland display: {e:?}"))?;
    let shell =
        LayerShell::bind(system.connection()).ok_or("the server lacks zwlr_layer_shell_v1")?;
    // the session object brings a running application forward; without it every application is started
    let session = Shell::bind(system.connection()).map(|shell| shell.session(system.connection()));
    let mut config = LauncherConfig::default();
    let terminal = options.terminal.clone().or_else(|| {
        let entries = DEFAULT_TERMINALS.map(String::from);
        cupertino_defaults::terminal::resolve_installed(&entries)
    });
    if let Some(terminal) = terminal {
        config.terminal_command = cupertino_defaults::terminal::command_prefix(&terminal);
    }
    let files = if options.files {
        Some(
            FileSearchConfig::home()
                .ok_or("HOME is not set (start with --no-files to skip file results)")?,
        )
    } else {
        None
    };
    let (sender, signals) = async_channel::unbounded();
    // the first output's size guesses the browser's height until the catcher is configured on the output the server
    // picked
    if let Some(g) = system
        .connection()
        .outputs()
        .into_iter()
        .find_map(|o| o.geometry)
    {
        let _ = sender.try_send(Signal::Output(Size::new(
            f64::from(g.width),
            f64::from(g.height),
        )));
    }
    let toggles = sender.clone();
    let _control = control::serve(zbus::connection::Builder::session()?, move |call| {
        let _ = toggles.send_blocking(match call {
            Call::Toggle => Signal::Toggle,
            Call::Applications => Signal::Applications,
        });
    })
    .map_err(|e| match e {
        zbus::Error::NameTaken => "another launcher owns org.cupertino.Launcher1".into(),
        e => Box::new(e) as Box<dyn Error>,
    })?;
    let surfaces = Rc::new(RefCell::new(Surfaces::default()));
    let system = system.with_roles(LauncherRoles {
        shell,
        surfaces: Rc::clone(&surfaces),
        signals: sender,
    });
    let assets = Assets::discover();
    let host = AppHost {
        surfaces,
        start: Instant::now(),
        themes: ThemeIcons::new(&assets),
        system: SystemIcons::required(
            &assets,
            &[GENERIC_APPLICATION_ICON, FOLDER_ICON, DOCUMENT_ICON],
            &[SEARCH_SYMBOL],
        ),
        files: None,
        previews: None,
        searches: None,
        scopes: None,
        thumb_loads: None,
        icon_loads: None,
        assets,
        session,
    };
    let mut model = Model::new(Launcher::from_system(config), Box::new(host));
    model.files = files;
    model.settled = cupertino_shell_client::test_clock::motion_settled();
    // the compositor runs the panel's close when it shows a window's parts as layers of their own, with glass
    let capabilities = system.capabilities();
    model.compositor_layers = capabilities.surface_layers && capabilities.app_effects;
    let app = App { model, signals };
    let first = listen(&app.signals);
    cupertino_kit::app::run_on::<App>(system, (app, vec![first]))?;
    Err("the launcher's surfaces were closed".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_preview_reads_only_the_start_of_regular_files_and_never_waits() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("notes.txt");
        std::fs::write(&text, "x".repeat(3 * TEXT_SNIFF)).unwrap();
        assert_eq!(
            read_start(&text, TEXT_SNIFF).map(|b| b.len()),
            Some(TEXT_SNIFF)
        );
        // a link is followed
        let link = dir.path().join("link.txt");
        std::os::unix::fs::symlink(&text, &link).unwrap();
        assert_eq!(read_start(&link, 10).map(|b| b.len()), Some(10));
        // a pipe with no writer would hold a blocking open forever; a device reads without end
        let fifo = dir.path().join("pipe.txt");
        let name = std::ffi::CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: `name` is a NUL-terminated path that outlives the call
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert_eq!(read_start(&fifo, TEXT_SNIFF), None);
        assert_eq!(read_start(Path::new("/dev/zero"), TEXT_SNIFF), None);
        assert_eq!(read_start(dir.path(), TEXT_SNIFF), None, "a folder");
        assert_eq!(read_start(&dir.path().join("gone"), TEXT_SNIFF), None);
        let fifo_link = dir.path().join("pipe-link.txt");
        std::os::unix::fs::symlink(&fifo, &fifo_link).unwrap();
        assert_eq!(read_start(&fifo_link, TEXT_SNIFF), None);
        // an unreadable file shows no text and no error
        let secret = dir.path().join("secret.txt");
        std::fs::write(&secret, "hidden").unwrap();
        std::fs::set_permissions(&secret, std::os::unix::fs::PermissionsExt::from_mode(0o000))
            .unwrap();
        if std::fs::File::open(&secret).is_err() {
            assert_eq!(excerpt(&secret), None);
        }
        // the whole preview of a pipe comes back with its details and the icon as its picture
        let preview = read_preview(&fifo, "text/plain", false, Vec::new());
        assert_eq!(preview.body, PreviewBody::Icon);
    }
}
