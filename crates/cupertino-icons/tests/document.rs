//! `document`: the icon name of a MIME type, the corner badges and the cache of file icons.

use crate::support;

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
fn each_type_has_the_icon_name_of_its_kind_or_its_nearest_ancestors() {
    let dir = tempfile::tempdir().unwrap();
    let db = database(dir.path());
    for (mime, name) in [
        ("inode/directory", "folder"),
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
        ("application/x-shellscript", "document.text"),
        ("text/x-csrc", "document.text"),
        ("application/x-compressed-tar", "document.archive"),
        // an office document packed in an archive is not drawn as an archive
        (
            "application/vnd.oasis.opendocument.text",
            "document.generic",
        ),
        ("application/x-unheard-of", "document.generic"),
        ("", "document.generic"),
    ] {
        let got = document_icon_name(&db, mime);
        assert_eq!(got, name, "{mime}");
        assert!(
            FILE_ICON_NAMES.contains(&got),
            "{mime}: {got} is a file icon name"
        );
    }
    for badge in [Badge::Alias, Badge::Locked] {
        assert!(FILE_ICON_NAMES.contains(&badge.icon_name()), "{badge:?}");
    }
}

#[test]
fn a_link_gets_the_alias_badge_before_the_lock_badge() {
    for (is_link, writable, badge) in [
        (true, true, Some(Badge::Alias)),
        (true, false, Some(Badge::Alias)),
        (false, false, Some(Badge::Locked)),
        (false, true, None),
    ] {
        assert_eq!(
            corner_badge(is_link, writable),
            badge,
            "link {is_link}, writable {writable}"
        );
    }
}

#[test]
fn the_badge_sits_at_the_bottom_left_at_the_recorded_rectangles() {
    // (icon side, x, distance from the bottom, side)
    for (icon, x, bottom, side) in [
        (16.0, 1.5, 3.0, 8.0),
        (32.0, 3.0, 6.0, 16.0),
        (512.0, 44.0, 91.0, 256.0),
    ] {
        assert_eq!(badge_rect(icon), (x, icon - bottom - side, side), "{icon}");
    }
    // between the recipes the badge grows; outside them it keeps the end recipes' ratios
    let sides: Vec<f64> = [32.0, 100.0, 200.0, 256.0, 512.0]
        .iter()
        .map(|&s| badge_rect(s).2)
        .collect();
    assert!(sides.windows(2).all(|w| w[0] < w[1]), "{sides:?}");
    for (icon, x, side) in [
        (8.0, 8.0 * 3.0 / 32.0, 8.0 * 0.5),
        (1024.0, 1024.0 * 44.0 / 512.0, 512.0),
    ] {
        let (got_x, _, got_side) = badge_rect(icon);
        assert!(
            (got_x - x).abs() < 1e-9 && (got_side - side).abs() < 1e-9,
            "{icon}"
        );
    }
}

/// A pack with a `folder`, `folder.home`, `document.generic`, `document.text` and `locked.badge`.
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

fn colour(p: &tiny_skia::Pixmap) -> (u8, u8, u8) {
    let c = p.pixel(16, 16).unwrap();
    (c.red(), c.green(), c.blue())
}

#[test]
fn a_file_has_the_icon_of_its_kind_its_special_folder_or_the_generic_document() {
    let dir = tempfile::tempdir().unwrap();
    let home = PathBuf::from("/home/someone");
    let mut icons = FileIcons::with_system(
        SystemIcons::without_host(&pack(&dir.path().join("pack"))),
        database(dir.path()),
        HashMap::from([(home.clone(), "folder.home")]),
    );
    for (row, path, mime, expected) in [
        ("a text file", "/a/b.txt", "text/plain", (255, 255, 0)),
        ("a folder", "/a", "inode/directory", (255, 0, 0)),
        (
            "a kind the pack lacks",
            "/a/b.pdf",
            "application/pdf",
            (0, 0, 255),
        ),
        (
            "a special folder",
            "/home/someone",
            "inode/directory",
            (0, 255, 0),
        ),
        (
            "another folder",
            "/home/other",
            "inode/directory",
            (255, 0, 0),
        ),
        (
            "a file at a special folder's path",
            "/home/someone",
            "text/plain",
            (255, 255, 0),
        ),
    ] {
        assert_eq!(
            colour(icons.document(&PathBuf::from(path), mime, 32)),
            expected,
            "{row}"
        );
    }
    assert_eq!(colour(icons.badge(Badge::Locked, 32)), (255, 0, 255));
    assert_eq!(
        colour(icons.badge(Badge::Alias, 32)),
        (140, 140, 140),
        "the placeholder"
    );
    assert!(icons.pack_has("folder"));
    assert!(!icons.pack_has("document.audio"));
}
