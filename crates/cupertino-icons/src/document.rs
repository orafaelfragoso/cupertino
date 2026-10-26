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

/// Types of programs, matched against the type and its ancestors.
const EXECUTABLE_TYPES: [&str; 3] = [
    "application/x-executable",
    "application/x-pie-executable",
    "application/vnd.appimage",
];

/// The semantic icon name of a file of type `mime`: `folder` for a directory, then by the kind of the type or of its
/// nearest ancestor: PDF, image, audio, video, archive, text (scripts included: the payload has no script icon),
/// program; every other type gets `document.generic`.
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
    } else if ARCHIVE_TYPES.iter().any(|t| kind(t)) {
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

    /// The emblem of `badge` at `px` × `px` device pixels (the placeholder when no pack provides it).
    pub fn badge(&mut self, badge: Badge, px: u32) -> &tiny_skia::Pixmap {
        self.system.raster(badge.icon_name(), px)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fixture;

    fn database(root: &Path) -> MimeDatabase {
        let mime = root.join("data/mime");
        std::fs::create_dir_all(&mime).unwrap();
        std::fs::write(
            mime.join("subclasses"),
            "application/x-shellscript application/x-executable\napplication/x-shellscript text/plain\nimage/svg+xml application/xml\napplication/xml text/plain\napplication/x-compressed-tar application/x-tar\napplication/epub+zip application/zip\n",
        )
        .unwrap();
        MimeDatabase::load(&[root.join("data")]).unwrap()
    }

    #[test]
    fn a_type_maps_to_the_icon_of_its_kind_or_of_its_nearest_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let db = database(dir.path());
        let name = |mime: &str| document_icon_name(&db, mime);
        assert_eq!(name(DIRECTORY), "folder");
        assert_eq!(name("application/pdf"), "document.pdf");
        assert_eq!(name("image/png"), "document.image");
        assert_eq!(
            name("image/svg+xml"),
            "document.image",
            "an image that is also text"
        );
        assert_eq!(name("audio/ogg"), "document.audio");
        assert_eq!(name("video/mp4"), "document.video");
        assert_eq!(name("application/x-compressed-tar"), "document.archive");
        assert_eq!(name("application/epub+zip"), "document.archive");
        assert_eq!(name("text/x-rust"), "document.text");
        assert_eq!(
            name("application/x-shellscript"),
            "document.text",
            "scripts use the text icon"
        );
        assert_eq!(name("application/x-executable"), "document.executable");
        assert_eq!(name("application/x-unknown-thing"), "document.generic");
        assert_eq!(name("application/octet-stream"), "document.generic");
    }

    /// A fixture directory with every file icon and the folders `extra`.
    fn full_assets(root: &Path, extra: &[&str]) -> Assets {
        let names: Vec<&str> = FILE_ICON_NAMES
            .iter()
            .copied()
            .chain(extra.iter().copied())
            .collect();
        fixture::assets(root, &names, &[])
    }

    #[test]
    fn files_get_their_kind_s_icon_and_special_folders_their_own() {
        let dir = tempfile::tempdir().unwrap();
        let assets = full_assets(dir.path(), &["folder.documents"]);
        let docs = PathBuf::from("/home/ada/Documents");
        let mut files = FileIcons::new(
            &assets,
            database(dir.path()),
            HashMap::from([(docs.clone(), "folder.documents")]),
        );
        let mid = |p: &tiny_skia::Pixmap| p.pixel(p.width() / 2, p.height() / 2).unwrap().red();
        let (folder, documents, pdf, generic) = (
            fixture::red("folder"),
            fixture::red("folder.documents"),
            fixture::red("document.pdf"),
            fixture::red("document.generic"),
        );
        assert_eq!(
            mid(files.document(Path::new("/home/ada/Music"), DIRECTORY, 16)),
            folder
        );
        assert_eq!(mid(files.document(&docs, DIRECTORY, 16)), documents);
        assert_eq!(
            mid(files.document(&docs, "application/pdf", 16)),
            pdf,
            "a file named like a special folder is a file"
        );
        assert_eq!(
            mid(files.document(Path::new("/x/y.bin"), "application/x-unknown-thing", 16)),
            generic
        );
        assert_eq!(
            mid(files.document(Path::new("/x/y.png"), "image/png", 16)),
            fixture::red("document.image")
        );
        assert_eq!(
            mid(files.badge(Badge::Locked, 16)),
            fixture::red("locked.badge")
        );
    }

    #[test]
    fn an_icon_the_packs_lack_falls_back_to_a_more_generic_one() {
        let dir = tempfile::tempdir().unwrap();
        let mid = |p: &tiny_skia::Pixmap| p.pixel(p.width() / 2, p.height() / 2).unwrap().red();
        let names: Vec<&str> = FILE_ICON_NAMES
            .iter()
            .copied()
            .filter(|n| *n != "document.image")
            .collect();
        let mut files = FileIcons::new(
            &fixture::assets(dir.path(), &names, &[]),
            database(dir.path()),
            HashMap::from([(PathBuf::from("/h"), "folder.home")]),
        );
        assert_eq!(
            mid(files.document(Path::new("/x/y.png"), "image/png", 16)),
            fixture::red("document.generic"),
            "the kind's icon is missing"
        );
        assert_eq!(
            mid(files.document(Path::new("/h"), DIRECTORY, 16)),
            fixture::red("folder"),
            "the special folder's icon is missing"
        );
        let other = tempfile::tempdir().unwrap();
        let mut none = FileIcons::new(
            &fixture::assets(other.path(), &[], &[]),
            database(other.path()),
            HashMap::new(),
        );
        let placeholder = none
            .document(Path::new("/x/y.png"), "image/png", 16)
            .clone();
        assert_eq!(
            placeholder.pixel(8, 8).unwrap().alpha(),
            255,
            "no pack provides any file icon: the grey placeholder"
        );
        assert_eq!(none.badge(Badge::Locked, 16), &placeholder);
    }

    #[test]
    fn every_icon_name_a_type_maps_to_is_required() {
        let dir = tempfile::tempdir().unwrap();
        let db = database(dir.path());
        for mime in [
            DIRECTORY,
            "application/pdf",
            "image/png",
            "audio/ogg",
            "video/mp4",
            "application/x-tar",
            "text/plain",
            "application/x-executable",
            "application/x-unknown-thing",
        ] {
            assert!(
                FILE_ICON_NAMES.contains(&document_icon_name(&db, mime)),
                "{mime}"
            );
        }
        for badge in [Badge::Alias, Badge::Locked] {
            assert!(FILE_ICON_NAMES.contains(&badge.icon_name()));
        }
    }

    #[test]
    fn the_alias_badge_wins_over_the_lock_badge() {
        assert_eq!(corner_badge(true, false), Some(Badge::Alias));
        assert_eq!(corner_badge(false, false), Some(Badge::Locked));
        assert_eq!(corner_badge(false, true), None);
    }

    #[test]
    fn badge_rects_follow_the_recipe_at_its_sizes() {
        let close = |a: (f64, f64, f64), b: (f64, f64, f64)| {
            (a.0 - b.0).abs() < 1e-9 && (a.1 - b.1).abs() < 1e-9 && (a.2 - b.2).abs() < 1e-9
        };
        assert!(
            close(badge_rect(16.0), (1.5, 5.0, 8.0)),
            "{:?}",
            badge_rect(16.0)
        );
        assert!(close(badge_rect(32.0), (3.0, 10.0, 16.0)));
        assert!(close(badge_rect(512.0), (44.0, 165.0, 256.0)));
        let (x, _, side) = badge_rect(272.0);
        assert!(
            (44.0 / 512.0 * 272.0..3.0 / 32.0 * 272.0).contains(&x),
            "between the recipe's ratios: {x}"
        );
        assert_eq!(side, 136.0);
    }
}
