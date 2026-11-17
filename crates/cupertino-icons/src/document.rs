//! Icons of files and folders: the semantic icon name of a MIME type, the corner badge of a file and where
//! it sits, and a cache of both as pixmaps for the file manager.
//!
//! The system draws a file with the icon of its kind (there: the type's icon, the nearest
//! ancestor's, then the generic document icon). The kinds the asset directory has are in [`document_icon_name`],
//! the one place that maps a MIME type to a `document.* name, on the shared MIME database (mapping).

use crate::SystemIcons;
use cupertino_apps::mime::{MimeDatabase, DIRECTORY, TEXT_PLAIN};
use cupertino_assets::Assets;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Types of archives and compressed files, matched against the type and its ancestors.
const ARCHIVE_TYPES: [&str; 12] = [
    "application/zip",
    "application/x-tar",
    "application/gzip",
    "application/x-bzip2",
    "application/x-xz",
    "application/zstd",
    "application/x-7z-compressed",
    "application/vnd.rar",
    "application/x-rar",
    "application/x-lzma",
    "application/x-compress",
    "application/x-cpio",
];

/// The prefix of the shared MIME database's generic icons of office documents (`x-office-spreadsheet`, ...). A document
/// whose format is packed in an archive is a subclass of the archive's type, but it is not drawn as an archive.
const OFFICE_GENERIC_ICON: &str = "x-office-";

/// Types of programs, matched against the type and its ancestors.
const EXECUTABLE_TYPES: [&str; 3] = [
    "application/x-executable",
    "application/x-pie-executable",
    "application/vnd.appimage",
];

/// The semantic icon name of a file of type `mime`: `folder` for a directory, then by the kind of the type or of its
/// nearest ancestor: PDF, image, audio, video, archive (not an office document packed in one), text (scripts
/// included: the payload has no script icon), program; every other type gets `document.generic`.
pub fn document_icon_name(db: &MimeDatabase, mime: &str) -> &'static str {
    let kind = |ty: &str| db.is_a(mime, ty);
    let media = |prefix: &str| db.ancestors(mime).iter().any(|a| a.starts_with(prefix));
    if db.canonical(mime) == DIRECTORY {
        "folder"
    } else if kind("application/pdf") {
        "document.pdf"
    } else if media("image/") {
        "document.image"
    } else if media("audio/") {
        "document.audio"
    } else if media("video/") {
        "document.video"
    } else if ARCHIVE_TYPES.iter().any(|t| kind(t))
        && !db
            .generic_icon(mime)
            .is_some_and(|icon| icon.starts_with(OFFICE_GENERIC_ICON))
    {
        "document.archive"
    } else if kind(TEXT_PLAIN) {
        "document.text"
    } else if EXECUTABLE_TYPES.iter().any(|t| kind(t)) {
        "document.executable"
    } else {
        "document.generic"
    }
}

/// A badge drawn over a file icon. The design has more (caution, unsupported and the
/// file-provider states); these two have a file-system counterpart.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Badge {
    /// The item is an alias; here a symbolic link.
    Alias,
    /// The item is locked; here the user cannot write it (counterpart of the design's lock).
    Locked,
}

impl Badge {
    /// The semantic name of the badge's icon.
    pub fn icon_name(self) -> &'static str {
        match self {
            Badge::Alias => "alias.badge",
            Badge::Locked => "locked.badge",
        }
    }
}

/// The bottom-left badge of an item: the alias badge wins over the lock badge.
pub fn corner_badge(symlink: bool, writable: bool) -> Option<Badge> {
    match (symlink, writable) {
        (true, _) => Some(Badge::Alias),
        (false, false) => Some(Badge::Locked),
        (false, true) => None,
    }
}

/// Badge rectangles of the design's document recipe: icon size, then origin and side in a
/// bottom-left coordinate system. Values; their pairing with the badge field.
const BADGE_RECTS: [(f64, f64, f64, f64); 3] = [
    (16.0, 1.5, 3.0, 8.0),
    (32.0, 3.0, 6.0, 16.0),
    (512.0, 44.0, 91.0, 256.0),
];

/// Where the badge sits on an icon `size` points square: `(x, y, side) from the icon's top-left corner.
/// The recipe's ratios at 16 and 32 points are the same; between 32 and 512 points they are interpolated
/// linearly in the icon size, and clamped outside 16…512 (approximate in between).
pub fn badge_rect(size: f64) -> (f64, f64, f64) {
    let ratios = |(s, x, y, side): (f64, f64, f64, f64)| (x / s, y / s, side / s);
    let (lo, hi) = (ratios(BADGE_RECTS[1]), ratios(BADGE_RECTS[2]));
    let t = ((size - BADGE_RECTS[1].0) / (BADGE_RECTS[2].0 - BADGE_RECTS[1].0)).clamp(0.0, 1.0);
    let lerp = |a: f64, b: f64| a + (b - a) * t;
    let (x, y, side) = (
        lerp(lo.0, hi.0) * size,
        lerp(lo.1, hi.1) * size,
        lerp(lo.2, hi.2) * size,
    );
    (x, size - y - side, side)
}

/// Every raster icon [`document_icon_name`] and [`Badge::icon_name`] can name.
pub const FILE_ICON_NAMES: [&str; 11] = [
    "folder",
    "document.pdf",
    "document.image",
    "document.audio",
    "document.video",
    "document.archive",
    "document.text",
    "document.executable",
    "document.generic",
    "alias.badge",
    "locked.badge",
];

/// File and badge icons as pixmaps, by type and device-pixel size, cached for the life of the value.
pub struct FileIcons {
    system: SystemIcons,
    db: MimeDatabase,
    /// Folders with an icon of their own (the home folder, the user directories) and its semantic name.
    folders: HashMap<PathBuf, &'static str>,
}

impl FileIcons {
    /// File icons from `assets`. An icon of [`FILE_ICON_NAMES`] or of the `folders` that no pack provides is reported at
    /// startup (see [`SystemIcons::required`]); [`FileIcons::document`] then falls back to a more generic icon.
    pub fn new(
        assets: &Assets,
        db: MimeDatabase,
        folders: HashMap<PathBuf, &'static str>,
    ) -> FileIcons {
        let names: Vec<&str> = FILE_ICON_NAMES
            .iter()
            .copied()
            .chain(folders.values().copied())
            .collect();
        FileIcons {
            system: SystemIcons::required(assets, &names, &[]),
            db,
            folders,
        }
    }

    /// File icons drawn by `system`, so a caller controls the host themes (the packs and nothing else, with
    /// [`SystemIcons::without_host`]): tests and captures that must not read the icon themes of the machine.
    pub fn with_system(
        system: SystemIcons,
        db: MimeDatabase,
        folders: HashMap<PathBuf, &'static str>,
    ) -> FileIcons {
        FileIcons {
            system,
            db,
            folders,
        }
    }

    /// The MIME database the icons are resolved with.
    pub fn database(&self) -> &MimeDatabase {
        &self.db
    }

    /// The icon of the file or folder at `path` of type `mime` at `px` × `px` device pixels: a special folder's own
    /// icon, else the icon of the type's kind, else `document.generic`; the placeholder when the packs provide none.
    pub fn document(&mut self, path: &Path, mime: &str, px: u32) -> &tiny_skia::Pixmap {
        let special = self
            .folders
            .get(path)
            .copied()
            .filter(|_| self.db.canonical(mime) == DIRECTORY);
        let kind = document_icon_name(&self.db, mime);
        let name = special
            .into_iter()
            .chain([kind, "document.generic"])
            .find(|n| self.system.has_raster(n, px))
            .unwrap_or(kind);
        self.system.raster(name, px)
    }

    /// Whether an asset pack gives the raster icon `name` as a file of its own ([`SystemIcons::pack_has_raster`]).
    pub fn pack_has(&self, name: &str) -> bool {
        self.system.pack_has_raster(name)
    }

    /// The emblem of `badge` at `px` × `px` device pixels (the placeholder when no pack provides it).
    pub fn badge(&mut self, badge: Badge, px: u32) -> &tiny_skia::Pixmap {
        self.system.raster(badge.icon_name(), px)
    }
}
