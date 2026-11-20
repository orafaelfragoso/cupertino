//! The desktop's slow work, run on Kit's I/O pool (`cupertino_kit::task::io`) and heard back as messages: the start's
//! reads, what opens an item, the file operations, the copies and moves slice by slice, and the arrangement written.

use std::ffi::OsString;
use std::os::unix::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use cupertino_apps::{Locale, MimeApps, MimeDatabase, Visibility, XdgDirs};
use cupertino_kit::core::{Command, DropPlace, DropTarget, Element};
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::files::open::{
    command_with, default_app_id, open_command, open_with, OpenError,
};
use cupertino_kit::files::operations::{self, SLICE_DEADLINE};
use cupertino_kit::files::places::{desktop_folder, session_places};
use cupertino_kit::platform::DropAction as Offered;
use cupertino_kit::task;
use percent_encoding::percent_decode_str;

use super::{Message, Model, Running};
use crate::arrangement::{self, DesktopPrefs};

pub use cupertino_trash::PendingSize;

/// How long the start's reads should take: the user directories, the file types, the icon packs and the kept
/// arrangement.
const START_DEADLINE: Duration = Duration::from_secs(10);

/// How long one file operation should take: a rename, a new folder, an alias, items moved to the trash, what opens
/// an item, the arrangement written.
const FILE_DEADLINE: Duration = Duration::from_secs(10);

/// How long recording the sizes of trashed folders should take: each folder is walked.
const SIZES_DEADLINE: Duration = Duration::from_secs(60);

/// The file manager, which shows a folder opened on the desktop in a window of its own.
const FILES: &str = "cupertino-files";

/// The program that shows the wallpaper settings, and its pane.
const SETTINGS: &str = "cupertino-settings";
const WALLPAPER_PANE: &str = "wallpaper";

/// The type of a dropped list of files.
const URI_LIST: &str = "text/uri-list";

/// What the start read for the session: the desktop folder, made when missing; the file types; the items' icons with
/// the user directories' folder icons; the kept arrangement and where it is kept; the menu bar's room.
pub struct Session {
    pub folder: PathBuf,
    pub types: Arc<MimeDatabase>,
    pub file_icons: FileIcons,
    pub prefs: DesktopPrefs,
    pub prefs_file: Option<PathBuf>,
    pub menu_bar: f64,
}

/// The start's reads, handed to the UI thread once: the first to [`Started::take`] them has them.
#[derive(Clone)]
pub struct Started(Arc<Mutex<Option<Result<Session, String>>>>);

impl Started {
    /// The reads, or why they failed; `None` once taken.
    pub fn take(&self) -> Option<Result<Session, String>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The start's reads, as a task.
pub fn start() -> Command<Message> {
    task::io(START_DEADLINE, read_session, |read| {
        Message::Started(Started(Arc::new(Mutex::new(Some(read)))))
    })
}

#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn read_session() -> Result<Session, String> {
    let home = PathBuf::from(std::env::var_os("HOME").ok_or("HOME is not set")?);
    let folder = desktop_folder(&home);
    if let Err(e) = std::fs::create_dir_all(&folder) {
        eprintln!(
            "cupertino-shell-desktop: cannot make {}: {e}",
            folder.display()
        );
    }
    let dirs = XdgDirs::from_env().map_err(|e| e.to_string())?;
    let types = MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
    let folder_icons = session_places(&home, &dirs.data_dirs)
        .map(|p| p.folder_icons)
        .unwrap_or_default();
    let file_icons = FileIcons::discover(types.clone(), folder_icons.into_iter().collect());
    let prefs_file = arrangement::location();
    let prefs = prefs_file
        .as_deref()
        .map(DesktopPrefs::load)
        .unwrap_or_default();
    Ok(Session {
        folder,
        types: Arc::new(types),
        file_icons,
        prefs,
        prefs_file,
        menu_bar: cupertino_kit_shell::design::menu_bar().height.ceil(),
    })
}

/// The default-application resolver of the session's directories and desktops, read again at each use.
fn applications() -> Result<MimeApps, String> {
    let dirs = XdgDirs::from_env().map_err(|e| e.to_string())?;
    let db = MimeDatabase::load(&dirs.all_data_dirs()).map_err(|e| e.to_string())?;
    MimeApps::load(&dirs, db, &Locale::from_env(), &Visibility::from_env())
        .map_err(|e| e.to_string())
}

/// What a launch starts once its activation token comes.
#[derive(Clone, Debug)]
pub enum Launch {
    /// A folder, in a window of the file manager.
    Folder(PathBuf),
    /// An application on a file.
    Command(cupertino_apps::CommandSpec),
    /// The settings' wallpaper pane.
    Wallpaper,
}

impl Launch {
    /// The settings' wallpaper pane, as the application it is.
    pub fn wallpaper() -> (Option<String>, Launch) {
        (Some(SETTINGS.to_string()), Launch::Wallpaper)
    }

    /// The process to start, with the activation `token` in its environment.
    pub fn command(self, token: Option<&str>) -> std::process::Command {
        let mut command = match self {
            Launch::Folder(path) => {
                let mut command = std::process::Command::new(FILES);
                command.arg(path);
                command
            }
            Launch::Command(spec) => spec.to_command(),
            Launch::Wallpaper => {
                let mut command = std::process::Command::new(SETTINGS);
                command.args(["--pane", WALLPAPER_PANE]);
                command
            }
        };
        if let Some(token) = token {
            command.env("XDG_ACTIVATION_TOKEN", token);
            command.env("DESKTOP_STARTUP_ID", token);
        }
        command
    }
}

/// An item to open: the application that opens it, for its activation token, and what starts, or why nothing can.
#[derive(Clone, Debug)]
pub struct Opening {
    pub path: PathBuf,
    pub app_id: Option<String>,
    pub launch: Result<Launch, String>,
}

/// What opens each of `items` (a path, and whether the listing says it is a folder): the application of desktop
/// entry `with`, else a folder's file manager or a file's default application; as a task.
pub fn resolve(items: Vec<(PathBuf, bool)>, with: Option<String>) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        move || {
            let apps = applications();
            items
                .into_iter()
                .map(|(path, is_dir)| opening(apps.as_ref(), path, is_dir, with.as_deref()))
                .collect()
        },
        Message::Resolved,
    )
}

fn opening(
    apps: Result<&MimeApps, &String>,
    path: PathBuf,
    is_dir: bool,
    with: Option<&str>,
) -> Opening {
    if is_dir && with.is_none() {
        return Opening {
            path: path.clone(),
            app_id: Some(FILES.to_string()),
            launch: Ok(Launch::Folder(path)),
        };
    }
    let apps = match apps {
        Ok(apps) => apps,
        Err(e) => {
            return Opening {
                path,
                app_id: None,
                launch: Err(e.clone()),
            }
        }
    };
    let (app_id, launch) = match with {
        Some(id) => {
            let launch = open_with(apps, &path, is_dir)
                .into_iter()
                .find(|item| item.entry.id == id)
                .ok_or_else(|| format!("{id} no longer opens it"))
                .and_then(|item| command_with(item.entry, &path, None).map_err(|e| e.to_string()));
            (Some(id.to_string()), launch)
        }
        None => {
            let launch = open_command(apps, &path, is_dir, None).map_err(|e| match e {
                OpenError::NoApplication { .. } => "No application is set to open it".to_string(),
                e => e.to_string(),
            });
            (default_app_id(apps, &path, is_dir), launch)
        }
    };
    Opening {
        path,
        app_id,
        launch: launch.map(Launch::Command),
    }
}

/// The applications that open `path` (a folder when `is_dir`), by desktop entry id and name, for the Open With of
/// menu `menu`; as a task.
pub fn choices(menu: u64, path: PathBuf, is_dir: bool) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        move || match applications() {
            Ok(apps) => open_with(&apps, &path, is_dir)
                .into_iter()
                .map(|i| (i.entry.id.clone(), i.entry.name.clone()))
                .collect(),
            Err(e) => {
                eprintln!("cupertino-shell-desktop: cannot read the default applications: {e}");
                Vec::new()
            }
        },
        move |choices| Message::Choices(menu, choices),
    )
}

/// Rename the item at `from` to `name` in its folder, as a task.
pub fn rename(from: PathBuf, name: String) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        {
            let from = from.clone();
            move || operations::rename_item(&from, &name).map_err(|e| e.to_string())
        },
        move |to| Message::Renamed { from, to },
    )
}

/// Make a folder in `folder`, to sit in the free cell nearest to `at`, as a task.
pub fn new_folder(folder: PathBuf, at: Option<cupertino_kit::geometry::Point>) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        move || operations::new_folder(&folder).map_err(|e| e.to_string()),
        move |made| Message::Made(made, at),
    )
}

/// Make an alias of each of `paths`, as a task.
pub fn aliases(paths: Vec<PathBuf>) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        move || {
            paths
                .into_iter()
                .map(|path| {
                    let made = operations::make_alias(&path).map_err(|e| e.to_string());
                    (path, made)
                })
                .collect()
        },
        Message::Aliased,
    )
}

/// Move each of `paths` to the trash, as a task; the sizes of the folders trashed are recorded by [`record_sizes`].
pub fn trash(paths: Vec<PathBuf>) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        move || {
            let trash = cupertino_trash::Trash::new();
            let mut sizes = Vec::new();
            let moved = paths
                .into_iter()
                .map(|path| {
                    let moved = trash.trash_unsized(&path).map(|(_, size)| {
                        sizes.extend(size);
                    });
                    (path, moved.map_err(|e| e.to_string()))
                })
                .collect();
            (moved, sizes)
        },
        |(moved, sizes)| Message::Trashed(moved, sizes),
    )
}

/// Record the sizes of trashed folders in their trash, as a task.
pub fn record_sizes(sizes: Vec<PendingSize>) -> Command<Message> {
    task::io(
        SIZES_DEADLINE,
        move || {
            for size in sizes {
                if let Err(e) = size.record() {
                    eprintln!(
                        "cupertino-shell-desktop: cannot record a trashed folder's size: {e}"
                    );
                }
            }
        },
        |()| Message::Worked,
    )
}

/// The next slice of transfer `running`, as a task; none while one runs or once it has ended.
pub fn slice(running: &mut Running) -> Option<Command<Message>> {
    let slice = running.transfer.take_slice()?;
    let number = running.number;
    Some(task::io(
        SLICE_DEADLINE,
        move || slice.run(),
        move |()| Message::Sliced(number),
    ))
}

/// Write `prefs` to `file`, as a task.
pub fn write_prefs(prefs: DesktopPrefs, file: PathBuf) -> Command<Message> {
    task::io(
        FILE_DEADLINE,
        move || prefs.save(&file).err().map(|e| e.to_string()),
        Message::Saved,
    )
}

impl Model {
    /// The arrangement as it is now, written when it changed and no write is under way.
    pub(super) fn write_prefs(&mut self) -> Vec<Command<Message>> {
        if !self.unsaved || self.saving {
            return Vec::new();
        }
        let Some(file) = self.prefs_file.clone() else {
            return Vec::new();
        };
        self.unsaved = false;
        self.saving = true;
        vec![write_prefs(self.prefs.clone(), file)]
    }
}

/// The desktop's `content` taking a `text/uri-list` dropped from another application ([`Message::DroppedIn`]).
pub fn takes_drops(content: Element<Message>) -> Element<Message> {
    Element::from(DropTarget {
        content,
        mimes: vec![URI_LIST.into()],
        actions: [Offered::Copy].into(),
        place: Box::new(|over| {
            over.actions.contains(&Offered::Copy).then_some(DropPlace {
                key: None,
                action: Offered::Copy,
            })
        }),
        on_hover: None,
        on_drop: Box::new(Message::DroppedIn),
        on_leave: None,
    })
}

/// The local files a `text/uri-list` dropped on the desktop names, to copy into `folder`: none when the list is not of
/// local files, is empty, or holds an item already in `folder` (the desktop's own items move by its own drag).
pub fn dropped_sources(bytes: &[u8], folder: &Path) -> Option<Vec<PathBuf>> {
    uri_paths(bytes)
        .filter(|paths| !paths.is_empty())
        .filter(|paths| paths.iter().all(|p| p.parent() != Some(folder)))
}

/// The local files of a `text/uri-list`: one URI a line, `#` lines being comments. The list comes from any
/// application, so it is taken whole or not at all: `None` when it is not text, or a URI is of another scheme or host,
/// or its path is not absolute, steps through `.` or `..`, or holds a NUL.
fn uri_paths(bytes: &[u8]) -> Option<Vec<PathBuf>> {
    std::str::from_utf8(bytes)
        .ok()?
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(|uri| {
            let rest = uri.strip_prefix("file://")?;
            let path = rest.strip_prefix("localhost").unwrap_or(rest);
            // the query and fragment are not part of the path
            let path = path.split(['?', '#']).next()?;
            if !path.starts_with('/') {
                return None;
            }
            let bytes: Vec<u8> = percent_decode_str(path).collect();
            let plain = bytes
                .split(|&b| b == b'/')
                .all(|step| step != b"." && step != b"..");
            (plain && !bytes.contains(&0)).then(|| PathBuf::from(OsString::from_vec(bytes)))
        })
        .collect()
}
