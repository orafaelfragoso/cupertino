//! The standard places of the sidebar: Recents, Applications, Desktop, Documents and Downloads under
//! "Favorites", the design's default Favorites in its order without its nearby-sharing item,
//! which has no counterpart here; the computer (the file system root, under the machine's name)
//! and the trash under "Locations" (approximate, a screenshot). The home folder and the
//! other user directories are not in the sidebar but keep their folder icons ([`SessionPlaces`]).
//!
//! * Recents is the virtual folder of [`crate::recents`].
//! * Applications is the first `applications` folder of `$XDG_DATA_DIRS` (XDG Base
//!   Directory and Desktop Entry standards: where the system's desktop entries are installed),
//!   a real folder rather than a merged virtual one (approximate: the design lists application
//!   bundles of one folder; the desktop entries of the other data folders, the user's own included,
//!   are not shown there).
//! * The computer's name is the pretty host name of `/etc/machine-info` (machine-info(5),
//!   the name the design shows is the user-facing computer name), else the host name.
//!
//! The user directories come from `$XDG_CONFIG_HOME/user-dirs.dirs` (xdg-user-dirs:
//! `XDG_<NAME>_DIR="$HOME/<path>"` or an absolute path; a directory set to `$HOME` itself is disabled;
//! the desktop defaults to `$HOME/Desktop` when the file does not name it). The group titles are
//! the design sidebar's; the order of the user directories is approximate (the design lists
//! Desktop, Documents and Downloads and has no Linux counterpart for the others).

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
    Trash,
    /// A file system of a removable drive (`crate::udisks`), in this state.
    Removable(Volume),
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
    /// The semantic name of the sidebar's template symbol.
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
            PlaceKind::Trash => "symbol.trash",
            PlaceKind::Removable(_) => "symbol.externaldrive",
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
            PlaceKind::Recents | PlaceKind::Trash | PlaceKind::Removable(_) => None,
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
    /// The row of a removable file system named `name`, mounted at `mount`.
    pub fn removable(name: &str, mount: Option<&Path>, busy: bool) -> Place {
        let volume = match (busy, mount) {
            (true, _) => Volume::Busy,
            (false, Some(_)) => Volume::Mounted,
            (false, None) => Volume::Unmounted,
        };
        Place {
            name: name.into(),
            path: mount.map(Path::to_path_buf).unwrap_or_default(),
            group: PlaceGroup::Locations,
            kind: PlaceKind::Removable(volume),
        }
    }

    /// The folder a drop or a click goes to: none for a removable file system that is not mounted.
    pub fn folder(&self) -> Option<&Path> {
        (!self.path.as_os_str().is_empty()).then_some(self.path.as_path())
    }

    /// Whether the row shows the eject button: a mounted removable file system.
    pub fn ejectable(&self) -> bool {
        self.kind == PlaceKind::Removable(Volume::Mounted)
    }
}

/// The removable place of `places` whose mount holds `dir`, the innermost when mounts nest.
pub fn removable_holding(dir: &Path, places: &[Place]) -> Option<usize> {
    places
        .iter()
        .enumerate()
        .filter(|(_, p)| matches!(p.kind, PlaceKind::Removable(_)))
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

/// The `XDG_<NAME>_DIR` assignments of a `user-dirs.dirs` file as `(NAME, path)`, `$HOME` expanded
/// against `home`. Lines that are comments, malformed or relative without `$HOME/` are skipped, as
/// xdg-user-dirs does.
pub fn parse_user_dirs(text: &str, home: &Path) -> Vec<(String, PathBuf)> {
    text.lines()
        .filter_map(|line| {
            let line = line.trim();
            let (key, value) = line.split_once('=')?;
            let name = key.trim().strip_prefix("XDG_")?.strip_suffix("_DIR")?;
            let value = value.trim().strip_prefix('"')?.strip_suffix('"')?;
            let path = if value == "$HOME" {
                home.to_path_buf()
            } else if let Some(rest) = value.strip_prefix("$HOME/") {
                home.join(rest)
            } else if value.starts_with('/') {
                PathBuf::from(value)
            } else {
                return None;
            };
            Some((name.to_string(), path))
        })
        .collect()
}

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

/// The user directories the default Favorites list.
const FAVORITE_USER_DIRS: [PlaceKind; 3] = [
    PlaceKind::Desktop,
    PlaceKind::Documents,
    PlaceKind::Downloads,
];

/// The places of a session with this `home`, user directory assignments, home trash folder
/// (`<trash>/files`), applications folder and computer name. A user directory that is disabled or does
/// not exist is left out, and so is the trash before anything was ever trashed, and Applications when
/// the system has no such folder.
pub fn standard_places(
    home: &Path,
    user_dirs: &[(String, PathBuf)],
    trash_files: &Path,
    applications: Option<&Path>,
    computer: &str,
) -> Vec<Place> {
    let mut places = vec![Place {
        name: "Recents".into(),
        path: crate::recents::location().to_path_buf(),
        group: PlaceGroup::Favorites,
        kind: PlaceKind::Recents,
    }];
    if let Some(path) = applications {
        places.push(Place {
            name: "Applications".into(),
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
    if trash_files.is_dir() {
        places.push(Place {
            name: "Trash".into(),
            path: trash_files.to_path_buf(),
            group: PlaceGroup::Locations,
            kind: PlaceKind::Trash,
        });
    }
    places
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

/// The sidebar places of a session and the folders with icons of their own.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionPlaces {
    pub places: Vec<Place>,
    pub folder_icons: Vec<(PathBuf, &'static str)>,
}

/// The places of the running session: `user-dirs.dirs` from the configuration directory (a missing
/// file assigns nothing), the home trash of [`crate::trash::Trash`], the applications folder of the
/// system data folders `data_dirs` and the [`computer_name`].
pub fn session_places(home: &Path, data_dirs: &[PathBuf]) -> io::Result<SessionPlaces> {
    let config = cupertino_defaults::config_home().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            "neither XDG_CONFIG_HOME nor HOME is set",
        )
    })?;
    let user_dirs = match std::fs::read_to_string(config.join("user-dirs.dirs")) {
        Ok(text) => parse_user_dirs(&text, home),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(e),
    };
    let applications = applications_folder(data_dirs);
    let places = standard_places(
        home,
        &user_dirs,
        &crate::trash::Trash::new().home_trash().join("files"),
        applications.as_deref(),
        &computer_name()?,
    );
    let folder_icons = folder_icons(home, &user_dirs, &places);
    Ok(SessionPlaces {
        places,
        folder_icons,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_dirs_expand_home_and_skip_what_xdg_user_dirs_skips() {
        let text = "# written by xdg-user-dirs-update\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"/data/incoming\"\nXDG_MUSIC_DIR=\"$HOME\"\nXDG_VIDEOS_DIR=\"relative\"\nnot an assignment\n";
        let home = Path::new("/home/ada");
        assert_eq!(
            parse_user_dirs(text, home),
            [
                ("DESKTOP".to_string(), PathBuf::from("/home/ada/Desktop")),
                ("DOWNLOAD".to_string(), PathBuf::from("/data/incoming")),
                ("MUSIC".to_string(), PathBuf::from("/home/ada"))
            ]
        );
    }

    #[test]
    fn favorites_are_the_reference_defaults_that_exist_and_other_user_dirs_keep_their_icons() {
        let home = tempfile::tempdir().unwrap();
        for d in ["Desktop", "Bilder", "Documents", "Eingang"] {
            std::fs::create_dir(home.path().join(d)).unwrap();
        }
        let trash = home.path().join(".local/share/Trash/files");
        // Documents is not assigned, Music is disabled, Pictures is not a Favorite; the desktop is the default one
        let user_dirs = [
            ("PICTURES".to_string(), home.path().join("Bilder")),
            ("MUSIC".to_string(), home.path().to_path_buf()),
            ("DOWNLOAD".to_string(), home.path().join("Eingang")),
        ];
        let places = standard_places(home.path(), &user_dirs, &trash, None, "ada's laptop");
        let rows: Vec<(&str, PlaceGroup)> =
            places.iter().map(|p| (p.name.as_str(), p.group)).collect();
        assert_eq!(
            rows,
            [
                ("Recents", PlaceGroup::Favorites),
                ("Desktop", PlaceGroup::Favorites),
                ("Downloads", PlaceGroup::Favorites),
                ("ada's laptop", PlaceGroup::Locations)
            ]
        );
        assert_eq!(places[0].path, PathBuf::from("recent:///"));
        assert_eq!(places[2].path, home.path().join("Eingang"));
        assert_eq!(
            places.iter().map(|p| p.kind.symbol()).collect::<Vec<_>>(),
            [
                "symbol.recents",
                "symbol.desktop",
                "symbol.download",
                "symbol.computer"
            ]
        );
        assert_eq!(places[3].kind.folder_icon(), Some("computer"));
        let icons = folder_icons(home.path(), &user_dirs, &places);
        let icon = |p: PathBuf| icons.iter().find(|(path, _)| *path == p).map(|(_, i)| *i);
        assert_eq!(icon(home.path().to_path_buf()), Some("folder.home"));
        assert_eq!(
            icon(home.path().join("Bilder")),
            Some("folder.pictures"),
            "not in the sidebar, still its icon"
        );
        assert_eq!(icon(PathBuf::from("/")), Some("computer"));

        let apps = home.path().join("share/applications");
        std::fs::create_dir_all(&apps).unwrap();
        let data_dirs = [home.path().join("missing"), home.path().join("share")];
        assert_eq!(applications_folder(&data_dirs), Some(apps.clone()));
        let places = standard_places(home.path(), &user_dirs, &trash, Some(&apps), "ada's laptop");
        assert_eq!(places[1].name, "Applications");
        assert_eq!(
            (
                places[1].path.as_path(),
                places[1].kind.symbol(),
                places[1].kind.folder_icon()
            ),
            (
                apps.as_path(),
                "symbol.applications",
                Some("folder.applications")
            )
        );

        std::fs::create_dir_all(&trash).unwrap();
        let places = standard_places(home.path(), &user_dirs, &trash, None, "ada's laptop");
        assert_eq!(
            places.last().map(|p| (p.name.as_str(), p.path.as_path())),
            Some(("Trash", trash.as_path()))
        );
    }

    #[test]
    fn a_folder_is_on_the_innermost_mounted_removable_file_system_holding_it() {
        let mut places = standard_places(
            Path::new("/home/ada"),
            &[],
            Path::new("/nonexistent"),
            None,
            "box",
        );
        places.push(Place::removable(
            "Card",
            Some(Path::new("/run/media/ada/CARD")),
            false,
        ));
        places.push(Place::removable("Stick", None, false));
        places.push(Place::removable(
            "Inner",
            Some(Path::new("/run/media/ada/CARD/inner")),
            false,
        ));
        let first = places.len() - 3;
        assert_eq!(places[first].kind, PlaceKind::Removable(Volume::Mounted));
        assert_eq!(
            places[first + 1].folder(),
            None,
            "not mounted: nowhere to go or drop"
        );
        assert_eq!(
            removable_holding(Path::new("/run/media/ada/CARD/DCIM"), &places),
            Some(first)
        );
        assert_eq!(
            removable_holding(Path::new("/run/media/ada/CARD/inner/x"), &places),
            Some(first + 2)
        );
        assert_eq!(
            removable_holding(Path::new("/run/media/ada/CARDS"), &places),
            None
        );
        assert_eq!(
            removable_holding(Path::new("/home/ada"), &places),
            None,
            "the computer is not removable"
        );
        assert_eq!(
            Place::removable("Card", Some(Path::new("/run/media/ada/CARD")), true).kind,
            PlaceKind::Removable(Volume::Busy)
        );
    }

    #[test]
    fn the_pretty_host_name_is_read_quoted_or_bare_and_empty_means_unset() {
        assert_eq!(
            parse_pretty_hostname("CHASSIS=laptop\nPRETTY_HOSTNAME=\"Ada's Laptop\"\n"),
            Some("Ada's Laptop".into())
        );
        assert_eq!(
            parse_pretty_hostname("PRETTY_HOSTNAME='Work \"PC\"'"),
            Some("Work \"PC\"".into())
        );
        assert_eq!(
            parse_pretty_hostname("PRETTY_HOSTNAME=box"),
            Some("box".into())
        );
        assert_eq!(
            parse_pretty_hostname("PRETTY_HOSTNAME=\"\"\nICON_NAME=computer"),
            None
        );
        assert_eq!(parse_pretty_hostname("ICON_NAME=computer-laptop"), None);
    }
}
