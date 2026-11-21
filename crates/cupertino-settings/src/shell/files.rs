//! The window's file work, off its thread: the Wallpaper pane's pictures, their thumbnails and the chooser's folders;
//! what the panes read from files and programs (the users, the login items, the browsers, the installed applications,
//! the clipboard's allowed programs, About's facts and the graphics memory) and the changes they write that way. Each
//! runs as a task of Kit's I/O pool and comes back to the window as [`super::desk::Done`].

use std::path::{Path, PathBuf};
use std::time::Duration;

use cupertino_icons::thumbnail::{ThumbError, ThumbSize, Thumbnailer};
use cupertino_kit::controls::ImageSource;
use cupertino_kit::file_icons::FileIcons;
use cupertino_kit::filepicker::{Listing, Request};
use cupertino_kit::files::places::{chooser_places, Place};

use super::state::{Choosing, ListingKey, State, VideoMemory};
use crate::chooser::{Chooser, Outcome as ChooserOutcome};
use crate::controls::Unreadable;
use crate::panes::ControlId;
use crate::settings::File;
use crate::PaneId;

/// How long reading a few local files or listing a folder should take.
pub(crate) const FILES_DEADLINE: Duration = Duration::from_secs(2);
/// How long finding where the panel opens, its places and, the first time, the icons of its items should take.
const CHOOSER_DEADLINE: Duration = Duration::from_secs(5);
/// Where the panel that chooses a program opens (approximate).
const PROGRAM_CHOOSER_START: &str = "/usr/bin";
/// How long making one thumbnail should take: a large picture read and scaled down once.
const THUMBNAIL_DEADLINE: Duration = Duration::from_secs(5);
/// How long reading the graphics memory should take: the driver's tool is given
/// [`crate::about::VIDEO_MEMORY_TIMEOUT`] to answer, and is stopped then.
const VIDEO_MEMORY_DEADLINE: Duration = Duration::from_secs(3);

/// The longest edge of a thumbnail the cache keeps, in pixels: the decoder keeps a picture of this size whole.
const THUMBNAIL_EDGE: u32 = 256;
/// The longest edge of the account picture kept for the sidebar's row, in pixels.
const ACCOUNT_PICTURE_EDGE: u32 = 128;
/// The largest account picture file read, in bytes.
const ACCOUNT_PICTURE_BYTES: u64 = 8 * 1024 * 1024;

/// Why the thumbnail of a picture could not be made: its file is gone, or it is no picture that can be read.
fn unreadable(error: &ThumbError) -> Unreadable {
    match error {
        ThumbError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => Unreadable::Missing,
        _ => Unreadable::Damaged,
    }
}

/// The pixels of the image file `path` at most `edge` pixels wide or high, as an image of its own for the window to
/// draw; on the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn decode(path: &Path, edge: u32) -> Option<ImageSource> {
    use cupertino_kit::render::{ImageResource, PixelFormat};
    let data = std::fs::read(path).ok()?;
    let icon = cupertino_icons::decode_png(&data, edge)?;
    Some(ImageSource {
        id: cupertino_kit::core::next_resource_id(),
        image: ImageResource {
            revision: 1,
            width: icon.width,
            height: icon.height,
            stride: icon.width * 4,
            format: PixelFormat::Rgba8Premultiplied,
            pixels: std::sync::Arc::from(icon.rgba),
        },
    })
}

/// The account picture `path`, unless its file is larger than [`ACCOUNT_PICTURE_BYTES`]; on the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn account_picture(path: &Path) -> Option<ImageSource> {
    std::fs::metadata(path)
        .is_ok_and(|m| m.len() <= ACCOUNT_PICTURE_BYTES)
        .then(|| decode(path, ACCOUNT_PICTURE_EDGE))?
}

/// The home folder: `HOME` when it is absolute, else the root.
fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Where the panel choosing for `choosing` opens: pictures in the person's Pictures folder, else their home; a
/// program among the system's; a folder to leave out at home. On the I/O pool.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn chooser_start(choosing: Choosing, home: &Path) -> PathBuf {
    match choosing {
        Choosing::Wallpaper | Choosing::WallpaperFolder => Some(home.join("Pictures"))
            .filter(|p| p.is_dir())
            .unwrap_or_else(|| home.to_path_buf()),
        Choosing::Program => PathBuf::from(PROGRAM_CHOOSER_START),
        Choosing::Exclusion => home.to_path_buf(),
    }
}

/// The panel's places with this `home`; on the I/O pool.
fn places(home: &Path) -> Vec<Place> {
    let dirs = cupertino_defaults::user_dirs::read(home).unwrap_or_else(|e| {
        eprintln!("cupertino-settings: cannot read the user directories: {e}");
        Vec::new()
    });
    chooser_places(home, &dirs)
}

/// The icons of the panel's items, typed with the installed MIME database, `places` wearing their own; on the I/O
/// pool.
fn file_icons(places: &[Place]) -> FileIcons {
    let types = cupertino_apps::XdgDirs::from_env()
        .ok()
        .and_then(|dirs| cupertino_apps::MimeDatabase::load(&dirs.all_data_dirs()).ok())
        .unwrap_or_default();
    let folders = places
        .iter()
        .filter_map(|p| Some((p.path.clone(), p.kind.folder_icon()?)))
        .collect();
    FileIcons::discover(types, folders)
}

/// The installed applications with their programs, for the Privacy and Notifications panes; on the I/O pool.
fn installed_apps() -> Vec<crate::clipboard::Installed> {
    let (locale, visibility) = (
        cupertino_apps::entry::Locale::from_env(),
        cupertino_apps::entry::Visibility::from_env(),
    );
    let entries = cupertino_apps::entry::index_dirs(
        &cupertino_apps::entry::xdg_application_dirs(),
        &locale,
        &visibility,
    );
    let path_dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    crate::clipboard::installed(entries, &path_dirs)
}

impl State {
    /// Read the account's picture for the sidebar's row.
    pub(crate) fn read_account_picture(&mut self) {
        let Some(path) = self.account.picture.clone() else {
            return;
        };
        self.desk.off(
            FILES_DEADLINE,
            move || account_picture(&path),
            |state, picture| state.account_picture = picture,
        );
    }

    /// Read what `to` shows from files and programs; it shows each reading as it comes.
    pub(crate) fn read_files(&mut self, to: PaneId) {
        if to == PaneId::Users {
            self.desk
                .off(FILES_DEADLINE, crate::system::users, |state, users| {
                    state.live.users = users
                });
        }
        if matches!(to, PaneId::Account | PaneId::General) {
            self.read_system();
            self.ask_video_memory();
        }
        if to == PaneId::General {
            self.desk.off(
                FILES_DEADLINE,
                crate::system::login_items,
                |state, items| state.live.login_items = items,
            );
        }
        if to == PaneId::DesktopDock {
            self.desk.off(
                FILES_DEADLINE,
                crate::system::browsers,
                |state, browsers| state.live.browsers = browsers,
            );
        }
        if to == PaneId::Privacy {
            self.read_clipboard_states();
        }
        if matches!(to, PaneId::Privacy | PaneId::Notifications) {
            self.desk
                .off(FILES_DEADLINE, installed_apps, |state, apps| {
                    state.live.apps = apps
                });
        }
    }

    /// Read where each program Clipboard Access allows stands now: missing, changed since it was allowed, or allowed.
    pub(crate) fn read_clipboard_states(&mut self) {
        let entries = self.settings.clipboard.conf.allowed.clone();
        self.desk.off(
            FILES_DEADLINE,
            move || crate::clipboard::states(&entries),
            |state, states| state.live.clipboard = states,
        );
    }

    /// Read About's facts with the graphics memory known now; one read without it is read again once it is known.
    fn read_system(&mut self) {
        let root = self.about_root.clone();
        let known = match &self.video_memory {
            Some(VideoMemory::Read(list)) => list.clone(),
            _ => Vec::new(),
        };
        self.desk.off(
            FILES_DEADLINE,
            move || {
                let system = crate::system::SystemInfo::read_with(&root, &known);
                (system, known.is_empty())
            },
            |state, (system, without_memory)| {
                state.live.system = Some(Box::new(system));
                if without_memory {
                    state.read_about();
                }
            },
        );
    }

    /// Read About's facts again with the graphics memory, once it is read and is not empty.
    fn read_about(&mut self) {
        let Some(VideoMemory::Read(list)) = &self.video_memory else {
            return;
        };
        if list.is_empty() || self.live.system.is_none() {
            return;
        }
        let (root, list) = (self.about_root.clone(), list.clone());
        self.desk.off(
            FILES_DEADLINE,
            move || crate::about::About::read_with(&root, &list),
            |state, about| {
                if let Some(system) = &mut state.live.system {
                    system.about = Some(about);
                }
            },
        );
    }

    /// Read the graphics memory only the driver's tool reports, once; the pane shows at once without it and gains it
    /// when it is read.
    pub(crate) fn ask_video_memory(&mut self) {
        if self.video_memory.is_some() {
            return;
        }
        self.video_memory = Some(VideoMemory::Reading);
        let read = self.read_video_memory.clone();
        self.desk.off(
            VIDEO_MEMORY_DEADLINE,
            move || read(),
            |state, list| {
                state.video_memory = Some(VideoMemory::Read(list));
                state.read_about();
            },
        );
    }

    /// Make `id` a login item or not (`enable`), then read the login items again; a refusal shows under row `row`.
    pub(crate) fn set_login_item(&mut self, row: ControlId, id: String, enable: bool) {
        self.desk.off(
            FILES_DEADLINE,
            move || {
                let done = crate::system::set_login_item(&id, enable)
                    .map_err(|e| format!("The login item could not be changed: {e}"));
                (done, crate::system::login_items())
            },
            move |state, (done, items)| {
                state.live.login_items = items;
                if let Err(e) = done {
                    eprintln!("cupertino-settings: {e}");
                    state.show_error(Some(row), e);
                }
            },
        );
    }

    /// Make `id` the default browser, then read the browsers again; a refusal shows under row `row`.
    pub(crate) fn set_default_browser(&mut self, row: ControlId, id: String) {
        self.desk.off(
            FILES_DEADLINE,
            move || {
                let done = crate::system::set_default_browser(&id)
                    .map_err(|e| format!("The default browser could not be changed: {e}"));
                (done, crate::system::browsers())
            },
            move |state, (done, browsers)| {
                state.live.browsers = browsers;
                if let Err(e) = done {
                    eprintln!("cupertino-settings: {e}");
                    state.show_error(Some(row), e);
                }
            },
        );
    }

    /// Allow `program` to use the clipboard (its entry is read first: a link becomes its file). The applications sheet
    /// stays with the reason it can't be; a program the panel chose shows it under Clipboard Access.
    pub(crate) fn allow_clipboard(&mut self, program: PathBuf) {
        self.desk.off(
            FILES_DEADLINE,
            move || crate::clipboard::entry_for(&program),
            |state, entry| {
                let paths = state.paths.clone();
                let sheet = (state.sheet.as_mut())
                    .filter(|sheet| matches!(sheet.kind, crate::sheet::Kind::Apps { .. }));
                match entry.and_then(|entry| state.settings.allow_clipboard(&paths, entry)) {
                    Ok(()) => {
                        if sheet.is_some() {
                            state.sheet = None;
                        }
                        state.read_clipboard_states();
                    }
                    Err(e) => match sheet {
                        Some(sheet) => sheet.failed(&e),
                        None => state.show_error(Some(ControlId::ClipboardAdd), e),
                    },
                }
            },
        );
    }

    /// Open the panel that chooses for `choosing`, once where it opens and its places are read; the icons of its items
    /// are read with the first.
    pub(crate) fn open_chooser(&mut self, choosing: Choosing) {
        let icons = !std::mem::replace(&mut self.file_icons_asked, true);
        self.desk.off(
            CHOOSER_DEADLINE,
            move || {
                let home = home();
                let places = places(&home);
                let icons = icons.then(|| file_icons(&places));
                (chooser_start(choosing, &home), places, icons)
            },
            move |state, (start, places, icons)| {
                if icons.is_some() {
                    state.file_icons = icons;
                }
                state.chooser = Some((choosing, Chooser::new(&start, choosing.mode(), places)));
                state.drive_picker();
            },
        );
    }

    /// What an event of the panel decided: it stays, closes, or closes with what it chose, which the pane takes.
    pub(crate) fn chooser_outcome(&mut self, outcome: ChooserOutcome) {
        let ChooserOutcome::Chosen(path) = outcome else {
            if outcome == ChooserOutcome::Cancel {
                self.chooser = None;
            }
            return;
        };
        let Some((choosing, _)) = self.chooser.take() else {
            return;
        };
        match choosing {
            Choosing::Program => self.allow_clipboard(path),
            Choosing::Exclusion => {
                let text = path.to_string_lossy().into_owned();
                if !self.settings.launcher.excluded.contains(&text) {
                    self.settings.launcher.excluded.push(text);
                    if let Err(e) = self.settings.store(&self.paths, File::Launcher) {
                        eprintln!("cupertino-settings: {e}");
                    }
                }
            }
            Choosing::Wallpaper | Choosing::WallpaperFolder => {
                let added = if choosing == Choosing::WallpaperFolder {
                    self.settings.add_wallpaper_folder(&self.paths, &path)
                } else {
                    self.settings.set_wallpaper(&self.paths, &path)
                };
                match added {
                    Ok(()) => self.load_pictures(),
                    Err(e) => eprintln!("cupertino-settings: {e}"),
                }
            }
        }
    }

    /// List the pictures of the packs and of the added folders, and start the thumbnails of those without one.
    pub(crate) fn load_pictures(&mut self) {
        let folders: Vec<PathBuf> = self
            .settings
            .compositor
            .wallpaper_folders
            .iter()
            .flatten()
            .map(PathBuf::from)
            .collect();
        self.live.pictures = crate::live::Pictures {
            bundled: Vec::new(),
            folders: folders.iter().map(|f| (f.clone(), Vec::new())).collect(),
            default: self.assets.wallpaper_path(false),
            loading: (0..=folders.len()).collect(),
            // what could not be made stays known until it is asked for again and answered
            unreadable: std::mem::take(&mut self.live.pictures.unreadable),
        };
        let current: Vec<PathBuf> = self
            .settings
            .compositor
            .wallpaper
            .iter()
            .map(PathBuf::from)
            .collect();
        self.ask_again(&current);
        self.ask_thumbnails(current);
        // each listing a task of its own: a folder on a hung mount shows "Loading…" and holds nothing else up
        self.start_listing(ListingKey::Packs, None);
        for folder in folders {
            self.start_listing(ListingKey::Pictures(folder), None);
        }
    }

    /// Start the read of `key` unless one is running (a folder on a dead mount keeps one task, however often it is
    /// visited); it is read once more when the running one is answered if it was asked for meanwhile.
    pub(crate) fn start_listing(&mut self, key: ListingKey, request: Option<Request>) {
        if let Some(request) = self.listings.ask(key.clone(), request) {
            self.run_listing(key, request);
        }
    }

    /// Run the read of `key` as a task; its answer comes back to [`State::pictures_listed`] or [`State::listed`].
    fn run_listing(&mut self, key: ListingKey, request: Option<Request>) {
        match key.clone() {
            ListingKey::Packs => {
                let assets = self.assets.clone();
                self.desk.off(
                    FILES_DEADLINE,
                    move || assets.wallpaper_files(),
                    move |state, files| state.pictures_listed(key, files),
                );
            }
            ListingKey::Pictures(folder) => self.desk.off(
                FILES_DEADLINE,
                move || cupertino_assets::pictures_in(&folder),
                move |state, files| state.pictures_listed(key, files),
            ),
            ListingKey::Chooser(..) => match request {
                Some(request) => self.desk.off(
                    FILES_DEADLINE,
                    move || request.run(),
                    |state, listing| state.listed(listing),
                ),
                None => self.listings.abandon(&key),
            },
        }
    }

    /// A listing of the Wallpaper pane's pictures: group 0 the packs', then one per added folder.
    fn pictures_listed(&mut self, key: ListingKey, files: Vec<cupertino_assets::WallpaperFile>) {
        if let Some(request) = self.listings.answered(&key) {
            self.run_listing(key.clone(), request);
        }
        let images: Vec<PathBuf> = files.iter().map(|f| f.light.clone()).collect();
        let pictures = &mut self.live.pictures;
        // the pane may list other folders by now: the answer goes to the one it is for, if still listed
        let group = match &key {
            ListingKey::Packs => Some(0),
            ListingKey::Pictures(folder) => pictures
                .folders
                .iter()
                .position(|(f, _)| f == folder)
                .map(|i| i + 1),
            ListingKey::Chooser(..) => None,
        };
        if let Some(group) = group {
            pictures.loading.remove(&group);
            match group {
                0 => pictures.bundled = files,
                n => pictures.folders[n - 1].1 = files,
            }
            self.ask_again(&images);
            self.ask_thumbnails(images);
        }
    }

    /// A folder the chooser asked for was read.
    fn listed(&mut self, listing: Listing) {
        let (dir, hidden, mode) = listing.key();
        let key = ListingKey::Chooser(dir, hidden, mode);
        if let Some(request) = self.listings.answered(&key) {
            self.run_listing(key, request);
        }
        if let Some((_, chooser)) = self.chooser.as_mut() {
            chooser.apply(listing);
        }
    }

    /// The pictures among `images` whose thumbnail could not be made are asked for again by the next
    /// [`Self::ask_thumbnails`], as they are listed again: a file that came back, or was mended, shows; one still
    /// missing or unreadable says so again. Each keeps what it says until it is answered.
    pub(crate) fn ask_again(&mut self, images: &[PathBuf]) {
        for image in images {
            if self.live.pictures.unreadable.contains_key(image) {
                self.thumbnails_asked.remove(image);
                self.thumbnails.remove(image);
            }
        }
    }

    /// Make the thumbnails of `images` that have none and were not asked for yet, a task each.
    pub(crate) fn ask_thumbnails(&mut self, images: Vec<PathBuf>) {
        for image in images {
            if self.thumbnails.contains_key(&image) || !self.thumbnails_asked.insert(image.clone())
            {
                continue;
            }
            let cache_home = self.thumbnail_cache.clone();
            self.desk.off(
                THUMBNAIL_DEADLINE,
                {
                    let image = image.clone();
                    move || {
                        let cache = match cache_home {
                            Some(home) => Thumbnailer::new(home, "cupertino-settings"),
                            None => Thumbnailer::from_env("cupertino-settings"),
                        };
                        cache
                            .get_or_create(&image, ThumbSize::Large)
                            .map(|thumbnail| {
                                let pixels = decode(&thumbnail, THUMBNAIL_EDGE);
                                (thumbnail, pixels)
                            })
                            .map_err(|e| {
                                eprintln!(
                                    "cupertino-settings: no thumbnail of {}: {e}",
                                    image.display()
                                );
                                unreadable(&e)
                            })
                    }
                },
                move |state, made| state.thumbnail_made(image, made),
            );
        }
    }

    /// The thumbnail of `image` is made, with its pixels when they could be decoded, or could not be made.
    fn thumbnail_made(
        &mut self,
        image: PathBuf,
        made: Result<(PathBuf, Option<ImageSource>), Unreadable>,
    ) {
        match made {
            Ok((thumbnail, Some(pixels))) => {
                self.live.pictures.unreadable.remove(&image);
                self.thumbnail_images.insert(image.clone(), pixels);
                self.thumbnails.insert(image, thumbnail);
            }
            // a thumbnail that cannot be decoded shows as a picture that cannot be read
            Ok((thumbnail, None)) => {
                self.live
                    .pictures
                    .unreadable
                    .insert(image.clone(), Unreadable::Damaged);
                self.thumbnails.insert(image, thumbnail);
            }
            Err(why) => {
                self.live.pictures.unreadable.insert(image, why);
            }
        }
    }

    /// Hand the chooser's next listing to a task.
    pub(crate) fn drive_picker(&mut self) {
        let Some(request) = self.chooser.as_mut().and_then(|(_, c)| c.take_request()) else {
            return;
        };
        let (dir, hidden, mode) = request.key();
        let key = ListingKey::Chooser(dir, hidden, mode);
        self.start_listing(key, Some(request));
    }
}
