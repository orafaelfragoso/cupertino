//! The standard places of the sidebar: the home folder (under its own name), Applications, Desktop, Documents and
//! Downloads under "Favorites", the design's default Favorites in its order without its nearby-sharing item, which has
//! no counterpart here; the computer (the file system root, under the machine's name), the network (the session's mounted network locations, only while the session has a GVFS folder) and the trash under "Locations".
//! The other user directories are not in the sidebar but keep their folder icons ([`SessionPlaces`]). Recents, the
//! virtual folder of [`crate::recents`], is not a row: the Go menu reaches it ([`recents_place`]).
//!
//! * Applications is the virtual folder of [`crate::applications`]: the installed applications under their names and
//!   icons, merged from the `applications` folders of every data folder, shown while the session has one.
//! * The computer's name is the pretty host name of `/etc/machine-info` (machine-info(5),
//!   the name the design shows is the user-facing computer name), else the host name.
//!
//! The user directories come from `user-dirs.dirs` ([`cupertino_defaults::user_dirs`]; a directory set to `$HOME`
//! itself is disabled; the desktop defaults to `$HOME/Desktop` when the file does not name it). The group titles are
//! the design sidebar's; the order of the user directories is approximate (the design lists
//! Desktop, Documents and Downloads and has no Linux counterpart for the others). A file chooser's sidebar lists the
//! home folder and every user directory ([`chooser_places`]).

use crate::{applications, recents};
use cupertino_defaults::user_dirs;
use std::io;
use std::path::{Path, PathBuf};

/// Sidebar section a place is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceGroup {
    Favorites,
    Locations,
}

impl PlaceGroup {
    pub fn title(self) -> &'static str {
        match self {
            PlaceGroup::Favorites => "Favorites",
            PlaceGroup::Locations => "Locations",
        }
    }
}

/// What a place is, which picks its sidebar symbol and, for a folder, its folder icon.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlaceKind {
    Home,
    Desktop,
    Documents,
    Downloads,
    Music,
    Pictures,
    Videos,
    Recents,
    Applications,
    Computer,
    /// The network locations the session has mounted ([`network_root`]).
    Network,
    Trash,
    /// A file system of a removable drive the program lists, in this state, on this kind of medium.
    Removable(Volume, Medium),
}

/// The kind of medium a removable file system is on, which picks its symbol.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Medium {
    /// An external disk, or a drive of no known kind.
    #[default]
    Disk,
    /// A flash drive: a stick that plugs straight in.
    Stick,
    /// A memory card in a card reader.
    Card,
}

/// The state of a removable file system's row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Volume {
    /// Not mounted: a click mounts it.
    Unmounted,
    /// Mounted: the row goes to the mount point and has the eject button.
    Mounted,
    /// A mount or an eject is running: the row takes no other.
    Busy,
}

impl PlaceKind {
    /// The semantic name of the place's template symbol, the outline the path bar shows.
    pub fn symbol(self) -> &'static str {
        match self {
            PlaceKind::Home => "symbol.house",
            PlaceKind::Desktop => "symbol.desktop",
            PlaceKind::Documents => "symbol.document",
            PlaceKind::Downloads => "symbol.download",
            PlaceKind::Music => "symbol.music",
            PlaceKind::Pictures => "symbol.photo",
            PlaceKind::Videos => "symbol.film",
            PlaceKind::Recents => "symbol.recents",
            PlaceKind::Applications => "symbol.applications",
            PlaceKind::Computer => "symbol.computer",
            PlaceKind::Network => "symbol.network",
            PlaceKind::Trash => "symbol.trash",
            PlaceKind::Removable(_, Medium::Disk) => "symbol.externaldrive",
            PlaceKind::Removable(_, Medium::Stick) => "symbol.flash-drive",
            PlaceKind::Removable(_, Medium::Card) => "symbol.memory-card",
        }
    }

    /// The semantic name of the filled symbol the place's sidebar row wears: the template symbol's filled edition,
    /// which reads at a sidebar's small size and takes the row's tint whole.
    pub fn sidebar_symbol(self) -> &'static str {
        match self {
            PlaceKind::Home => "symbol.house.fill",
            PlaceKind::Desktop => "symbol.desktop.fill",
            PlaceKind::Documents => "symbol.document.fill",
            PlaceKind::Downloads => "symbol.download.fill",
            PlaceKind::Music => "symbol.music.fill",
            PlaceKind::Pictures => "symbol.photo.fill",
            PlaceKind::Videos => "symbol.film.fill",
            PlaceKind::Recents => "symbol.recents.fill",
            PlaceKind::Applications => "symbol.applications.fill",
            PlaceKind::Computer => "symbol.computer.fill",
            PlaceKind::Network => "symbol.network.fill",
            PlaceKind::Trash => "symbol.trash.fill",
            PlaceKind::Removable(_, Medium::Disk) => "symbol.externaldrive.fill",
            PlaceKind::Removable(_, Medium::Stick) => "symbol.flash-drive.fill",
            PlaceKind::Removable(_, Medium::Card) => "symbol.memory-card.fill",
        }
    }

    /// The semantic name of the icon the folder itself wears in a listing and in the path bar; the file system root
    /// wears the computer's. Recents (not a folder) and the trash folder have none of their own.
    pub fn folder_icon(self) -> Option<&'static str> {
        match self {
            PlaceKind::Home => Some("folder.home"),
            PlaceKind::Desktop => Some("folder.desktop"),
            PlaceKind::Documents => Some("folder.documents"),
            PlaceKind::Downloads => Some("folder.downloads"),
            PlaceKind::Music => Some("folder.music"),
            PlaceKind::Pictures => Some("folder.pictures"),
            PlaceKind::Videos => Some("folder.movies"),
            PlaceKind::Applications => Some("folder.applications"),
            PlaceKind::Computer => Some("computer"),
            PlaceKind::Recents
            | PlaceKind::Network
            | PlaceKind::Trash
            | PlaceKind::Removable(..) => None,
        }
    }
}

/// One sidebar row: a folder to go to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub name: String,
    /// The folder; empty for a removable file system that is not mounted.
    pub path: PathBuf,
    pub group: PlaceGroup,
    pub kind: PlaceKind,
}

impl Place {
    /// The row of a removable file system named `name` on `medium`, mounted at `mount`.
    pub fn removable(name: &str, mount: Option<&Path>, busy: bool, medium: Medium) -> Place {
        let volume = match (busy, mount) {
            (true, _) => Volume::Busy,
            (false, Some(_)) => Volume::Mounted,
            (false, None) => Volume::Unmounted,
        };
        Place {
            name: name.into(),
            path: mount.map(Path::to_path_buf).unwrap_or_default(),
            group: PlaceGroup::Locations,
            kind: PlaceKind::Removable(volume, medium),
        }
    }

    /// The folder a drop or a click goes to: none for a removable file system that is not mounted.
    pub fn folder(&self) -> Option<&Path> {
        (!self.path.as_os_str().is_empty()).then_some(self.path.as_path())
    }

    /// Whether the row shows the eject button: a mounted removable file system.
    pub fn ejectable(&self) -> bool {
        matches!(self.kind, PlaceKind::Removable(Volume::Mounted, _))
    }
}

/// What a window showing `dir` is titled: the place's name for the trash, Recents and Applications, else the folder's
/// name.
pub fn window_title(dir: &Path, places: &[Place]) -> String {
    if dir == recents::location() {
        return recents_place().name;
    }
    if dir == applications::location() {
        return APPLICATIONS_NAME.into();
    }
    if let Some(place) = places.iter().find(|p| {
        matches!(
            p.kind,
            PlaceKind::Trash | PlaceKind::Recents | PlaceKind::Network
        ) && p.path == dir
    }) {
        return place.name.clone();
    }
    dir.file_name().map_or_else(
        || dir.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The removable place of `places` whose mount holds `dir`, the innermost when mounts nest.
pub fn removable_holding(dir: &Path, places: &[Place]) -> Option<usize> {
    places
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.kind, PlaceKind::Removable(..)))
        .filter(|(_, p)| p.folder().is_some_and(|m| dir.starts_with(m)))
        .max_by_key(|(_, p)| p.path.components().count())
        .map(|(i, _)| i)
}

/// The user directories listed, in sidebar order: the `user-dirs.dirs` key, the row name and the kind.
const USER_DIRS: [(&str, &str, PlaceKind); 6] = [
    ("DESKTOP", "Desktop", PlaceKind::Desktop),
    ("DOCUMENTS", "Documents", PlaceKind::Documents),
    ("DOWNLOAD", "Downloads", PlaceKind::Downloads),
    ("MUSIC", "Music", PlaceKind::Music),
    ("PICTURES", "Pictures", PlaceKind::Pictures),
    ("VIDEOS", "Videos", PlaceKind::Videos),
];

/// The name of the Applications place, and the title of a window showing it.
pub const APPLICATIONS_NAME: &str = "Applications";

/// The first `applications` folder of the system data folders `data_dirs`, most important first.
pub fn applications_folder(data_dirs: &[PathBuf]) -> Option<PathBuf> {
    data_dirs
        .iter()
        .map(|d| d.join("applications"))
        .find(|d| d.is_dir())
}

/// The `PRETTY_HOSTNAME` of a machine-info file (shell-style `KEY=VALUE` lines, the value optionally
/// quoted); `None` when it is not set or empty.
pub fn parse_pretty_hostname(text: &str) -> Option<String> {
    let value = text
        .lines()
        .rev()
        .find_map(|line| line.trim().strip_prefix("PRETTY_HOSTNAME="))?
        .trim();
    let value = value
        .strip_prefix('"')
        .and_then(|v| v.strip_suffix('"'))
        .or_else(|| value.strip_prefix('\'').and_then(|v| v.strip_suffix('\'')))
        .unwrap_or(value);
    (!value.is_empty()).then(|| value.replace("\\\"", "\"").replace("\\\\", "\\"))
}

/// The computer's user-facing name: the pretty host name, else the host name (as `hostnamectl` shows them).
pub fn computer_name() -> io::Result<String> {
    match std::fs::read_to_string("/etc/machine-info") {
        Ok(text) => {
            if let Some(name) = parse_pretty_hostname(&text) {
                return Ok(name);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e),
    }
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; the name is NUL-terminated when it fits
    if unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } != 0 {
        return Err(io::Error::last_os_error());
    }
    let end = buf
        .iter()
        .position(|b| *b == 0)
        .ok_or_else(|| io::Error::other("the host name is longer than 255 bytes"))?;
    Ok(String::from_utf8_lossy(&buf[..end]).into_owned())
}

/// The user directories of this `home` and assignments that exist, in [`USER_DIRS`] order: a directory
/// that is disabled (set to the home folder) or does not exist is left out.
fn user_folders(
    home: &Path,
    user_dirs: &[(String, PathBuf)],
) -> Vec<(PathBuf, &'static str, PlaceKind)> {
    let mut out = Vec::new();
    for (key, name, kind) in USER_DIRS {
        let assigned = user_dirs
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, p)| p.clone());
        let path = match (assigned, key) {
            (Some(p), _) => p,
            (None, "DESKTOP") => home.join("Desktop"),
            (None, _) => continue,
        };
        if path != home && path.is_dir() {
            out.push((path, name, kind));
        }
    }
    out
}

/// The Recents folder as a place: what the Go menu opens and what a window showing it is titled.
pub fn recents_place() -> Place {
    Place {
        name: "Recents".into(),
        path: recents::location().to_path_buf(),
        group: PlaceGroup::Favorites,
        kind: PlaceKind::Recents,
    }
}

/// The user directories the default Favorites list.
const FAVORITE_USER_DIRS: [PlaceKind; 3] = [
    PlaceKind::Desktop,
    PlaceKind::Documents,
    PlaceKind::Downloads,
];

/// The places of a session with this `home`, user directory assignments, home trash folder
/// (`<trash>/files`), applications folder and computer name: the home folder first, under its folder's name (the
/// user's), as the head of the Favorites. A user directory that is disabled or does not exist is left out, and so is
/// the trash before anything was ever trashed, and Applications when the system has no such folder.
pub fn standard_places(
    home: &Path,
    user_dirs: &[(String, PathBuf)],
    trash_files: &Path,
    applications: Option<&Path>,
    computer: &str,
) -> Vec<Place> {
    let mut places = vec![Place {
        name: home.file_name().map_or_else(
            || home.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        ),
        path: home.to_path_buf(),
        group: PlaceGroup::Favorites,
        kind: PlaceKind::Home,
    }];
    if let Some(path) = applications {
        places.push(Place {
            name: APPLICATIONS_NAME.into(),
            path: path.to_path_buf(),
            group: PlaceGroup::Favorites,
            kind: PlaceKind::Applications,
        });
    }
    for (path, name, kind) in user_folders(home, user_dirs)
        .into_iter()
        .filter(|(_, _, kind)| FAVORITE_USER_DIRS.contains(kind))
    {
        places.push(Place {
            name: name.into(),
            path,
            group: PlaceGroup::Favorites,
            kind,
        });
    }
    places.push(Place {
        name: computer.into(),
        path: PathBuf::from("/"),
        group: PlaceGroup::Locations,
        kind: PlaceKind::Computer,
    });
    if let Some(root) = network_root() {
        places.push(Place {
            name: "Network".into(),
            path: root,
            group: PlaceGroup::Locations,
            kind: PlaceKind::Network,
        });
    }
    if trash_files.is_dir() {
        places.push(trash_place(trash_files));
    }
    places
}

/// The places of a file chooser's sidebar with this `home` and user directory assignments: the home folder as
/// "Home", then Desktop, Documents, Downloads, Music, Pictures and Videos where they exist, all under Favorites. A user
/// directory that is disabled (set to the home folder) is left out, and the desktop defaults to `Desktop` in `home`.
pub fn chooser_places(home: &Path, user_dirs: &[(String, PathBuf)]) -> Vec<Place> {
    let mut places = vec![Place {
        name: "Home".into(),
        path: home.to_path_buf(),
        group: PlaceGroup::Favorites,
        kind: PlaceKind::Home,
    }];
    places.extend(
        user_folders(home, user_dirs)
            .into_iter()
            .map(|(path, name, kind)| Place {
                name: name.into(),
                path,
                group: PlaceGroup::Favorites,
                kind,
            }),
    );
    places
}

/// The folder the network locations of the session appear in, as the session's GIO volume monitor mounts them: the
/// `gvfs` folder of the runtime directory, which holds one folder per mounted share or server. `None` without the
/// runtime directory or while no GVFS daemon has made the folder, so a session with no network service has no row.
pub fn network_root() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from)?;
    let root = runtime.join("gvfs");
    root.is_dir().then_some(root)
}

fn trash_place(trash_files: &Path) -> Place {
    Place {
        name: "Trash".into(),
        path: trash_files.to_path_buf(),
        group: PlaceGroup::Locations,
        kind: PlaceKind::Trash,
    }
}

/// Show the trash place, last of the `standard` first `places`, while `trash_files` exists: it comes with the first
/// item trashed and goes when the trash folder is removed. The number of standard places after.
pub fn follow_trash(places: &mut Vec<Place>, standard: usize, trash_files: &Path) -> usize {
    let shown = places[..standard]
        .iter()
        .any(|p| matches!(p.kind, PlaceKind::Trash));
    match (shown, trash_files.is_dir()) {
        (false, true) => {
            places.insert(standard, trash_place(trash_files));
            standard + 1
        }
        (true, false) => {
            places.retain(|p| !matches!(p.kind, PlaceKind::Trash));
            standard - 1
        }
        _ => standard,
    }
}

/// The folders that wear an icon of their own in listings and the path bar: the home folder, every
/// existing user directory and the places that have one.
pub fn folder_icons(
    home: &Path,
    user_dirs: &[(String, PathBuf)],
    places: &[Place],
) -> Vec<(PathBuf, &'static str)> {
    let mut icons = vec![(home.to_path_buf(), "folder.home")];
    icons.extend(
        user_folders(home, user_dirs)
            .into_iter()
            .filter_map(|(path, _, kind)| Some((path, kind.folder_icon()?))),
    );
    icons.extend(
        places
            .iter()
            .filter_map(|p| Some((p.path.clone(), p.kind.folder_icon()?))),
    );
    icons
}

/// The desktop folder of the running session: the one `user-dirs.dirs` names ([`user_dirs::read`]), else `Desktop` in
/// `home`.
pub fn desktop_folder(home: &Path) -> PathBuf {
    let assigned = user_dirs::read(home).ok().and_then(|assigned| {
        assigned
            .into_iter()
            .rev()
            .find(|(key, _)| key == "DESKTOP")
            .map(|(_, path)| path)
    });
    assigned.unwrap_or_else(|| home.join("Desktop"))
}

/// The sidebar places of a session and the folders with icons of their own.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionPlaces {
    pub places: Vec<Place>,
    pub folder_icons: Vec<(PathBuf, &'static str)>,
}

/// The places of the running session: `user-dirs.dirs` as [`user_dirs::read`] finds it (a missing
/// file assigns nothing), the home trash of [`cupertino_trash::Trash`], the applications folder of the
/// system data folders `data_dirs` and the [`computer_name`].
pub fn session_places(home: &Path, data_dirs: &[PathBuf]) -> io::Result<SessionPlaces> {
    let user_dirs = user_dirs::read(home)?;
    // the place lists the applications of every data folder, the user's own included
    let applications = (applications_folder(data_dirs).is_some()
        || applications::any_installed(&applications::application_dirs()))
    .then(applications::location);
    let places = standard_places(
        home,
        &user_dirs,
        &cupertino_trash::Trash::new().home_trash().join("files"),
        applications,
        &computer_name()?,
    );
    let folder_icons = folder_icons(home, &user_dirs, &places);
    Ok(SessionPlaces {
        places,
        folder_icons,
    })
}
