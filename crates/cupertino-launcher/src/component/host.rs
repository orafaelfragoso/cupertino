//! What the launcher's model asks of the session, as the component carries it out: every file read, icon, index,
//! search and preview on Kit's task pools, never on the UI thread; programs started through the host, which reaps
//! them; running applications brought forward by the server; timers on the runtime's clock.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cupertino_apps::{MimeDatabase, XdgDirs};
use cupertino_icons::thumbnail::{ThumbSize, Thumbnailer};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::core::{Command, Icons};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::geometry::Size;
use cupertino_kit::task;
use cupertino_kit_shell::{host, session};

use crate::browse::{App, BrowseLayout};
use crate::engine::{Action, ItemKind, ResultItem};
use crate::exec::CommandSpec;
use crate::files::{FileIndex, FileSearchConfig};
use crate::frecency::Frecency;
use crate::kit::{BusCall, Host, Message, SearchRequest};
use crate::panel::Now;
use crate::preview::{self, Preview, PreviewBody, TEXT_SNIFF};
use crate::providers::{Provider, Reply, Update};
use crate::scope::{FileItem, ScopeAnswer, ScopeLayout, ScopeRequest};
use crate::view::{
    DOCUMENT_ICON, FOLDER_ICON, GENERIC_APPLICATION_ICON, ICON_SIZE, PREVIEW_ICON_SIZE,
    PREVIEW_IMAGE, PREVIEW_TEXT_LINES,
};

/// The thumbnail cache's namespace for the thumbnails the launcher makes.
const NAMESPACE: &str = "cupertino-launcher";
/// The scales the icons are drawn at, so they stay sharp on every output.
const SCALES: [u32; 3] = [1, 2, 3];
/// Device pixels a files-scope thumbnail is loaded at: the cache's smallest size, which covers its box at scale 1 and
/// nearly at scale 2.
const FILE_THUMB_EDGE: u32 = 128;
/// The most items of a folder the preview counts.
const MAX_COUNTED: usize = 100_000;
/// How long drawing an icon should take: a few small files of the packs or the host's icon themes.
const ICON_DEADLINE: Duration = Duration::from_secs(2);
/// How long reading what the preview shows of a file should take: its start, its metadata, and a thumbnail it may make.
const PREVIEW_DEADLINE: Duration = Duration::from_secs(5);
/// How long building the index of the home folder should take: the walk stops at its own time budget well before.
const INDEX_DEADLINE: Duration = Duration::from_secs(30);
/// How long writing the launch history should take: one small file.
const HISTORY_DEADLINE: Duration = Duration::from_secs(2);
/// The longest a timer of the model waits, in seconds.
const MAX_WAIT: f64 = 60.0;

/// The MIME database and the icons of file types, made on the I/O pool the first time a file needs them and shared by
/// the tasks after it.
type SharedFileIcons = Arc<Mutex<Option<FileIcons>>>;

fn lock(icons: &Mutex<Option<FileIcons>>) -> MutexGuard<'_, Option<FileIcons>> {
    icons.lock().unwrap_or_else(|e| e.into_inner())
}

/// The file icons, made with the MIME database of the XDG data directories the first time. For the I/O pool.
fn with_file_icons<T>(shared: &SharedFileIcons, work: impl FnOnce(&mut FileIcons) -> T) -> T {
    let mut icons = lock(shared);
    let icons = icons.get_or_insert_with(|| {
        let dirs = XdgDirs::from_env()
            .map(|d| d.all_data_dirs())
            .unwrap_or_default();
        FileIcons::discover(
            MimeDatabase::load(&dirs).unwrap_or_default(),
            HashMap::new(),
        )
    });
    work(icons)
}

/// Seconds since the Unix epoch now.
fn unix_now() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Premultiplied RGBA pixels as a Kit image.
fn image(width: u32, height: u32, pixels: &[u8]) -> ImageSource {
    let icon = cupertino_kit::core::IconImage {
        id: cupertino_kit::core::next_resource_id(),
        image: cupertino_kit::render::ImageResource {
            revision: 0,
            width,
            height,
            stride: width * 4,
            format: cupertino_kit::render::PixelFormat::Rgba8Premultiplied,
            pixels: Arc::from(pixels),
        },
    };
    ImageSource {
        id: icon.id,
        image: icon.image,
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

/// The installed application's icon `name` (or the icon of a file it names) at `size` points, one rendition per scale;
/// the generic application icon where nothing has one of that name.
fn named(icons: &dyn Icons, name: &str, size: f64) -> Vec<ImageSource> {
    SCALES
        .iter()
        .filter_map(|scale| icons.named(name, (size as u32) * scale))
        .map(|icon| ImageSource {
            id: icon.id,
            image: icon.image,
        })
        .collect()
}

/// The application's own icon `name` at `size` points; none where it is the generic application icon, which nothing
/// of that name gives.
fn own_icon(icons: &dyn Icons, name: &str, size: f64) -> Vec<ImageSource> {
    let own = named(icons, name, size);
    let generic = named(icons, GENERIC_APPLICATION_ICON, size);
    let same = own.len() == generic.len()
        && own
            .iter()
            .zip(&generic)
            .all(|(a, b)| a.image.pixels == b.image.pixels);
    if same && name != GENERIC_APPLICATION_ICON {
        Vec::new()
    } else {
        own
    }
}

/// The icon a result shows: an application's own (its `Icon`), else the generic application icon; a folder's and a
/// file's the packs'. None for the kinds that show a symbol.
fn result_icon(icons: &dyn Icons, kind: ItemKind, name: Option<&str>) -> Vec<ImageSource> {
    let name = match kind {
        ItemKind::Application | ItemKind::ApplicationAction => {
            name.unwrap_or(GENERIC_APPLICATION_ICON)
        }
        ItemKind::Folder => FOLDER_ICON,
        ItemKind::File => DOCUMENT_ICON,
        ItemKind::WebSearch | ItemKind::Calculator | ItemKind::Conversion => return Vec::new(),
    };
    named(icons, name, ICON_SIZE)
}

/// The browser's icon of an application: its `Icon` at the list's and the grid's sizes, the generic application icon
/// without one.
fn browser_icon(icons: &dyn Icons, name: Option<&str>) -> Vec<ImageSource> {
    let name = name.unwrap_or(GENERIC_APPLICATION_ICON);
    let mut renditions = named(icons, name, BrowseLayout::LIST_ICON);
    renditions.extend(named(icons, name, BrowseLayout::ICON));
    renditions
}

/// The thumbnail of the file at `path` as a Kit image, none when it has none (the type's icon stays). For the I/O
/// pool.
fn file_thumbnail(path: &Path) -> Vec<ImageSource> {
    let thumbnailer = Thumbnailer::from_env(NAMESPACE);
    let Ok(mut pixels) = thumbnailer.pixels(path, ThumbSize::Normal, FILE_THUMB_EDGE) else {
        return Vec::new();
    };
    premultiply(&mut pixels.rgba);
    vec![image(pixels.width, pixels.height, &pixels.rgba)]
}

/// A picture of the image at `path` for the preview, at twice its size in points so it stays sharp at scale 2. For the
/// I/O pool.
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
/// regular. For the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "the preview reads the file on the I/O pool"
)]
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

/// Seconds east of UTC of local time at `time` (Unix seconds).
fn local_offset(time: i64) -> i64 {
    cupertino_kit::time::local(cupertino_kit::time::from_unix(time))
        .map_or(0, |local| i64::from(local.utc_offset))
}

/// What the preview shows of the file or folder at `path` of type `mime`: its picture or text, its size or how many
/// items it holds, when it changed and where it is. For the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "the preview reads the file's metadata on the I/O pool"
)]
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

/// The session as the launcher component reaches it.
pub struct ComponentHost {
    /// The start of the model's clock.
    start: Instant,
    /// The theme's icons, shared by every component of the process.
    icons: Arc<dyn Icons>,
    file_icons: SharedFileIcons,
    /// How many previews were asked for: a file's read that a newer preview replaced before it started is skipped.
    preview_asked: Arc<AtomicU64>,
    /// The newest scope search asked for: an older one that has not started yet answers nothing, as the model drops
    /// its answer.
    scope_asked: Arc<AtomicU64>,
}

impl ComponentHost {
    pub fn new(icons: Arc<dyn Icons>) -> Self {
        Self {
            start: Instant::now(),
            icons,
            file_icons: Arc::new(Mutex::new(None)),
            scope_asked: Arc::new(AtomicU64::new(0)),
            preview_asked: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Start `command` as a program of its own, which the host reaps.
    fn spawn(command: std::process::Command) {
        let program = command.get_program().to_string_lossy().into_owned();
        if !host::spawn(command) {
            eprintln!("cupertino-launcher: cannot run {program}");
        }
    }

    /// What the preview shows of the file or folder `path` of the result `id`, read on the I/O pool.
    fn file_preview(&self, id: String, path: PathBuf, is_dir: bool) -> Command<Message> {
        let shared = Arc::clone(&self.file_icons);
        let asked = self.preview_asked.fetch_add(1, Ordering::AcqRel) + 1;
        let newest = Arc::clone(&self.preview_asked);
        task::io(
            PREVIEW_DEADLINE,
            move || {
                // moving through the results asks at every step: only the newest is read
                if newest.load(Ordering::Acquire) != asked {
                    return None;
                }
                let (mime, icon) = with_file_icons(&shared, |files| {
                    let mime = files.database().file_type(&path, is_dir).to_owned();
                    let icon = files.images(&path, &mime, false, PREVIEW_ICON_SIZE);
                    (mime, icon)
                });
                Some(read_preview(&path, &mime, is_dir, icon))
            },
            move |preview| match preview {
                Some(preview) => Message::Previewed(id, Arc::new(preview)),
                None => Message::Shell(super::Shell::Replaced),
            },
        )
    }
}

impl Host for ComponentHost {
    fn run(&mut self, spec: CommandSpec, token: Option<String>) {
        let spec = match token {
            Some(token) => spec.with_activation_token(&token),
            None => spec,
        };
        Self::spawn(spec.to_command());
    }

    fn open(&mut self, target: &str) {
        let mut command = std::process::Command::new("xdg-open");
        command.arg(target);
        Self::spawn(command);
    }

    fn index(&mut self, config: FileSearchConfig) -> Vec<Command<Message>> {
        vec![task::io(
            INDEX_DEADLINE,
            move || Arc::new(FileIndex::build(config)),
            Message::Indexed,
        )]
    }

    // the panel's window takes the size the model gives with its next frame (`Program::surface`)
    fn resize(&mut self, _: Size) {}

    fn icon(&mut self, item: &ResultItem) -> Vec<Command<Message>> {
        if matches!(
            item.kind,
            ItemKind::WebSearch | ItemKind::Calculator | ItemKind::Conversion
        ) {
            return Vec::new();
        }
        let (id, kind, name) = (item.id.clone(), item.kind, item.icon.clone());
        let icons = Arc::clone(&self.icons);
        vec![task::io(
            ICON_DEADLINE,
            move || result_icon(&*icons, kind, name.as_deref()),
            move |sources| Message::ResultIcon(id, sources),
        )]
    }

    fn app_icon(&mut self, app: &App) -> Vec<Command<Message>> {
        let (id, name) = (app.id.clone(), app.icon.clone());
        let icons = Arc::clone(&self.icons);
        vec![task::io(
            ICON_DEADLINE,
            move || browser_icon(&*icons, name.as_deref()),
            move |sources| Message::AppIcon(id, sources),
        )]
    }

    fn preview(&mut self, item: &ResultItem) -> Vec<Command<Message>> {
        let id = item.id.clone();
        // a file's read under way that has not started gives way to this one
        self.preview_asked.fetch_add(1, Ordering::AcqRel);
        match (&item.kind, &item.action) {
            (ItemKind::Application | ItemKind::ApplicationAction, _) => {
                let Some(name) = item.icon.clone() else {
                    return Vec::new();
                };
                // without an icon of its own the preview shows only the result
                let icons = Arc::clone(&self.icons);
                vec![task::io(
                    ICON_DEADLINE,
                    move || own_icon(&*icons, &name, PREVIEW_ICON_SIZE),
                    move |icon| {
                        if icon.is_empty() {
                            return Message::NoPreview(id);
                        }
                        Message::Previewed(
                            id,
                            Arc::new(Preview {
                                body: PreviewBody::Icon,
                                icon,
                                kind: String::new(),
                                details: Vec::new(),
                            }),
                        )
                    },
                )]
            }
            (ItemKind::File | ItemKind::Folder, Action::OpenPath(path)) => {
                // the index knows a folder from a file: nothing asks the file system before the pool
                let is_dir = item.kind == ItemKind::Folder;
                vec![self.file_preview(id, path.clone(), is_dir)]
            }
            _ => Vec::new(),
        }
    }

    fn search(&mut self, request: SearchRequest) -> Vec<Command<Message>> {
        let generation = request.generation;
        vec![task::cpu(
            move || {
                // a newer search took its place: it says nothing, and the model drops its answer
                (!request.cancel.is_cancelled()).then(|| request.run())
            },
            move |update| {
                Message::Provided(update.unwrap_or(Update {
                    generation,
                    provider: Provider::Files,
                    reply: Reply::Failed("a newer search replaced it".to_owned()),
                }))
            },
        )]
    }

    fn scope_search(&mut self, request: ScopeRequest) -> Vec<Command<Message>> {
        let generation = request.generation;
        self.scope_asked.store(generation, Ordering::Release);
        let newest = Arc::clone(&self.scope_asked);
        vec![task::cpu(
            move || {
                if newest.load(Ordering::Acquire) != generation {
                    return ScopeAnswer {
                        generation,
                        suggestions: Vec::new(),
                        items: Vec::new(),
                    };
                }
                request.run()
            },
            Message::Scoped,
        )]
    }

    fn file_icon(&mut self, item: &FileItem) -> Vec<Command<Message>> {
        let (path, is_dir) = (item.path.clone(), item.is_dir);
        let shared = Arc::clone(&self.file_icons);
        vec![task::io(
            ICON_DEADLINE,
            {
                let path = path.clone();
                move || {
                    with_file_icons(&shared, |files| {
                        let mime = files.database().file_type(&path, is_dir).to_owned();
                        files.images(&path, &mime, false, ScopeLayout::THUMB)
                    })
                }
            },
            move |sources| Message::FileIcon(path, sources),
        )]
    }

    fn file_thumb(&mut self, item: &FileItem) -> Vec<Command<Message>> {
        let path = item.path.clone();
        vec![task::io(
            PREVIEW_DEADLINE,
            {
                let path = path.clone();
                move || file_thumbnail(&path)
            },
            move |sources| Message::FileThumb(path, sources),
        )]
    }

    fn save_history(&mut self, mut history: Frecency) -> Vec<Command<Message>> {
        vec![task::io(
            HISTORY_DEADLINE,
            move || {
                if let Err(e) = history.save() {
                    eprintln!("cupertino-launcher: launch history not saved: {e}");
                }
            },
            |()| Message::Shell(super::Shell::Saved),
        )]
    }

    fn after(&mut self, seconds: f64, message: Message) -> Vec<Command<Message>> {
        vec![Command::After {
            delay: Duration::from_secs_f64(seconds.clamp(0.0, MAX_WAIT)),
            message,
        }]
    }

    fn now(&self) -> Now {
        Now {
            mono: self.start.elapsed().as_secs_f64(),
            unix: unix_now(),
        }
    }

    fn bring_forward(&mut self, desktop_id: &str, launch: u64) -> bool {
        // the answer carries the launch back as its serial
        let Ok(serial) = u32::try_from(launch) else {
            return false;
        };
        session::activate_app(serial, desktop_id);
        true
    }

    fn ask_for_window(
        &mut self,
        bus_name: &str,
        source: Option<&Path>,
        token: Option<String>,
    ) -> Option<BusCall> {
        use cupertino_apps::activate::{activate_on_session, ACTIVATE_DEADLINE};
        // whether its entry allows it is read with the call, on the I/O pool: one that does not is started again
        let source = source?.to_owned();
        let id = bus_name.to_owned();
        let asked = task::io(
            ACTIVATE_DEADLINE,
            move || {
                cupertino_apps::entry::dbus_activatable_file(&source)
                    && activate_on_session("cupertino-launcher", &id, token.as_deref())
            },
            |ok| ok,
        );
        match asked {
            Command::Perform(call) => Some(call),
            _ => None,
        }
    }
}
