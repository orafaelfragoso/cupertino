//! `document`: the icon name of a MIME type, the corner badges and the cache of file icons.

#[allow(dead_code)]
mod support;

use std::collections::HashMap;
use std::path::PathBuf;

use cupertino_apps::mime::MimeDatabase;
use cupertino_assets::Assets;
use cupertino_icons::document::{
    badge_rect, corner_badge, document_icon_name, Badge, FileIcons, FILE_ICON_NAMES,
};
use cupertino_icons::SystemIcons;

/// A MIME database with a few types and subclasses in a data directory of `dir`.
fn database(dir: &std::path::Path) -> MimeDatabase {
    support::write(
        &dir.join("mime/subclasses"),
        b"application/x-shellscript text/plain\ntext/x-csrc text/plain\napplication/vnd.oasis.opendocument.text application/zip\napplication/x-compressed-tar application/gzip\n",
    );
    support::write(
        &dir.join("mime/generic-icons"),
        b"application/vnd.oasis.opendocument.text:x-office-document\n",
    );
    MimeDatabase::load(&[dir.to_path_buf()]).unwrap()
}

#[test]
fn a_directory_is_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        document_icon_name(&database(dir.path()), "inode/directory"),
        "folder"
    );
}

#[test]
fn each_kind_of_type_has_its_icon_name() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    for (mime, name) in [
        ("application/pdf", "document.pdf"),
        ("image/png", "document.image"),
        ("image/svg+xml", "document.image"),
        ("audio/mpeg", "document.audio"),
        ("video/mp4", "document.video"),
        ("application/zip", "document.archive"),
        ("application/gzip", "document.archive"),
        ("text/plain", "document.text"),
        ("application/x-executable", "document.executable"),
        ("application/vnd.appimage", "document.executable"),
    ] {
        assert_eq!(document_icon_name(&db, mime), name, "{mime}");
    }
}

#[test]
fn a_type_takes_the_icon_of_its_nearest_ancestor_kind() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    assert_eq!(
        document_icon_name(&db, "application/x-shellscript"),
        "document.text",
        "scripts are text"
    );
    assert_eq!(document_icon_name(&db, "text/x-csrc"), "document.text");
    assert_eq!(
        document_icon_name(&db, "application/x-compressed-tar"),
        "document.archive"
    );
}

#[test]
fn an_office_document_packed_in_an_archive_is_not_drawn_as_an_archive() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    assert_eq!(
        document_icon_name(&db, "application/vnd.oasis.opendocument.text"),
        "document.generic"
    );
}

#[test]
fn every_other_type_is_a_generic_document() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    assert_eq!(
        document_icon_name(&db, "application/x-unheard-of"),
        "document.generic"
    );
    assert_eq!(document_icon_name(&db, ""), "document.generic");
}

#[test]
fn every_name_the_mapping_can_give_is_in_the_list_of_file_icon_names() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    for mime in [
        "inode/directory",
        "application/pdf",
        "image/png",
        "audio/mpeg",
        "video/mp4",
        "application/zip",
        "text/plain",
        "application/x-executable",
        "application/x-other",
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
fn the_badges_have_their_icon_names() {
    assert_eq!(Badge::Alias.icon_name(), "alias.badge");
    assert_eq!(Badge::Locked.icon_name(), "locked.badge");
}

#[test]
fn a_link_gets_the_alias_badge_before_the_lock_badge() {
    assert_eq!(corner_badge(true, true), Some(Badge::Alias));
    assert_eq!(corner_badge(true, false), Some(Badge::Alias));
    assert_eq!(corner_badge(false, false), Some(Badge::Locked));
    assert_eq!(corner_badge(false, true), None);
}

#[test]
fn the_badge_sits_at_the_bottom_left_with_the_same_ratios_at_16_and_32_points() {
    let (x, y, side) = badge_rect(32.0);
    assert_eq!((x, side), (3.0, 16.0));
    assert_eq!(y, 32.0 - 6.0 - 16.0, "measured from the top");
    let (x16, y16, side16) = badge_rect(16.0);
    assert_eq!((x16, y16, side16), (1.5, 16.0 - 3.0 - 8.0, 8.0));
}

#[test]
fn the_badge_at_512_points_is_the_recorded_rectangle() {
    let (x, y, side) = badge_rect(512.0);
    assert_eq!((x, side), (44.0, 256.0));
    assert_eq!(y, 512.0 - 91.0 - 256.0);
}

#[test]
fn between_32_and_512_points_the_badge_grows_between_the_two_recipes() {
    let (_, _, small) = badge_rect(32.0);
    let (_, _, mid) = badge_rect(256.0);
    let (_, _, large) = badge_rect(512.0);
    assert!(small < mid && mid < large);
    let (_, _, a) = badge_rect(100.0);
    let (_, _, b) = badge_rect(200.0);
    assert!(a < b);
}

#[test]
fn sizes_outside_16_to_512_keep_the_ratios_of_the_end_recipes() {
    let (x, _, side) = badge_rect(8.0);
    assert!((x - 8.0 * 3.0 / 32.0).abs() < 1e-9 && (side - 8.0 * 0.5).abs() < 1e-9);
    let (x, _, side) = badge_rect(1024.0);
    assert!((x - 1024.0 * 44.0 / 512.0).abs() < 1e-9 && (side - 512.0).abs() < 1e-9);
}

/// A pack with a `folder`, `document.generic`, `document.text` and `locked.badge`.
fn pack(dir: &std::path::Path) -> Assets {
    for (file, rgba) in [
        ("folder", [255, 0, 0, 255]),
        ("home", [0, 255, 0, 255]),
        ("generic", [0, 0, 255, 255]),
        ("text", [255, 255, 0, 255]),
        ("locked", [255, 0, 255, 255]),
    ] {
        support::write_png(&dir.join(format!("icons/{file}.png")), 32, 32, rgba);
    }
    support::pack(
        dir,
        r#"{"version": 2, "icons": {
            "folder": {"raster": {"32": "icons/folder.png"}},
            "folder.home": {"raster": {"32": "icons/home.png"}},
            "document.generic": {"raster": {"32": "icons/generic.png"}},
            "document.text": {"raster": {"32": "icons/text.png"}},
            "locked.badge": {"raster": {"32": "icons/locked.png"}}
        }}"#,
    )
}

fn file_icons(dir: &std::path::Path, folders: HashMap<PathBuf, &'static str>) -> FileIcons {
    FileIcons::with_system(
        SystemIcons::without_host(&pack(&dir.join("pack"))),
        database(dir),
        folders,
    )
}

fn colour(p: &tiny_skia::Pixmap) -> (u8, u8, u8) {
    let c = p.pixel(16, 16).unwrap();
    (c.red(), c.green(), c.blue())
}

#[test]
fn a_file_has_the_icon_of_its_kind() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = file_icons(dir.path(), HashMap::new());
    assert_eq!(
        colour(icons.document(&PathBuf::from("/a/b.txt"), "text/plain", 32)),
        (255, 255, 0)
    );
    assert_eq!(
        colour(icons.document(&PathBuf::from("/a"), "inode/directory", 32)),
        (255, 0, 0)
    );
}

#[test]
fn a_kind_the_packs_lack_falls_back_to_the_generic_document() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = file_icons(dir.path(), HashMap::new());
    assert_eq!(
        colour(icons.document(&PathBuf::from("/a/b.pdf"), "application/pdf", 32)),
        (0, 0, 255)
    );
}

#[test]
fn a_special_folder_has_its_own_icon_but_a_file_of_that_path_does_not() {
    let dir = tempfile::tempdir().unwrap();
    let home = PathBuf::from("/home/someone");
    let mut icons = file_icons(dir.path(), HashMap::from([(home.clone(), "folder.home")]));
    assert_eq!(
        colour(icons.document(&home, "inode/directory", 32)),
        (0, 255, 0)
    );
    assert_eq!(
        colour(icons.document(&PathBuf::from("/home/other"), "inode/directory", 32)),
        (255, 0, 0)
    );
    assert_eq!(
        colour(icons.document(&home, "text/plain", 32)),
        (255, 255, 0)
    );
}

#[test]
fn a_badge_is_drawn_from_its_icon_and_the_placeholder_when_no_pack_has_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut icons = file_icons(dir.path(), HashMap::new());
    assert_eq!(colour(icons.badge(Badge::Locked, 32)), (255, 0, 255));
    assert_eq!(colour(icons.badge(Badge::Alias, 32)), (140, 140, 140));
}

#[test]
fn the_pack_has_an_icon_when_it_gives_a_file_of_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let icons = file_icons(dir.path(), HashMap::new());
    assert!(icons.pack_has("folder"));
    assert!(!icons.pack_has("document.audio"));
}

#[test]
fn the_icons_keep_the_database_they_were_made_with() {
    let dir = tempfile::tempdir().unwrap();
    let icons = file_icons(dir.path(), HashMap::new());
    assert!(icons.database().is_a("text/x-csrc", "text/plain"));
}
