//! An item's type from its name, then its content, how it is described and which group "arrange by kind" puts it in.

#[allow(dead_code)]
#[path = "support/scratch.rs"]
mod scratch;

use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::mime::{
    detect, entry_mime, kind_description, kind_group, KindGroup, DIRECTORY, UNKNOWN,
};
use scratch::Scratch;

/// The first bytes of a PNG file.
const PNG_HEADER: &[u8] = &[
    0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a, 0, 0, 0, 0x0d,
];

#[test]
fn a_type_comes_from_the_extension_first_then_from_the_content() {
    let scratch = Scratch::new("mime-detect");
    let folder = scratch.folder("a folder.png");
    let rows = [
        ("a folder, whatever its name", folder, true, DIRECTORY),
        (
            "an extension in capitals",
            scratch.file("PHOTO.PNG", b"x"),
            false,
            "image/png",
        ),
        (
            "an extension over the content",
            scratch.file("note.txt", PNG_HEADER),
            false,
            "text/plain",
        ),
        (
            "the content of a file with no extension",
            scratch.file("picture", PNG_HEADER),
            false,
            "image/png",
        ),
        (
            "an unknown extension",
            scratch.file("data.zzzz", PNG_HEADER),
            false,
            "image/png",
        ),
        (
            "content of no type",
            scratch.file("blob", &[0, 159, 146, 150, 1, 2, 3, 4]),
            false,
            UNKNOWN,
        ),
    ];
    for (what, path, is_dir, expected) in rows {
        assert_eq!(detect(&path, is_dir), expected, "{what}");
    }
}

#[test]
fn an_entry_keeps_the_type_it_was_given_and_else_takes_it_from_its_name() {
    let mut given = Entry::synthetic("report.pdf", false, 1);
    given.mime = Some("text/x-custom".into());
    let rows = [
        ("the type it was given", given, "text/x-custom"),
        (
            "a folder",
            Entry::synthetic("report.pdf", true, 0),
            DIRECTORY,
        ),
        (
            "its name",
            Entry::synthetic("report.pdf", false, 1),
            "application/pdf",
        ),
        (
            "a name with no type",
            Entry::synthetic("report.zzzz", false, 1),
            UNKNOWN,
        ),
    ];
    for (what, entry, expected) in rows {
        assert_eq!(entry_mime(&entry), expected, "{what}");
    }
}

#[test]
fn a_type_is_described_as_a_kind_by_a_fixed_wording_or_from_the_type_itself() {
    let rows = [
        ("inode/directory", "Folder"),
        ("text/plain", "Text"),
        ("image/png", "PNG image"),
        ("application/pdf", "PDF document"),
        ("application/zip", "Zip archive"),
        ("text/x-rust", "Rust source code"),
        (
            "application/vnd.oasis.opendocument.spreadsheet",
            "Spreadsheet",
        ),
        ("application/x-compressed-tar", "Compressed tar archive"),
        ("image/webp", "WEBP image"),
        ("text/markdown", "Markdown text"),
        ("video/x-matroska", "Matroska movie"),
        ("application/octet-stream", "Document"),
    ];
    for (mime, expected) in rows {
        assert_eq!(kind_description(mime), expected, "{mime}");
    }
}

#[test]
fn a_type_belongs_to_the_most_specific_group_that_has_it() {
    let rows = [
        (DIRECTORY, KindGroup::Folders),
        ("application/pdf", KindGroup::Pdf),
        ("application/x-executable", KindGroup::Applications),
        ("application/x-desktop", KindGroup::Applications),
        ("text/csv", KindGroup::Spreadsheets),
        ("text/html", KindGroup::Source),
        ("text/plain", KindGroup::Text),
        ("text/x-unheard-of", KindGroup::Text),
        ("image/jpeg", KindGroup::Images),
        ("video/mp4", KindGroup::Movies),
        ("audio/mpeg", KindGroup::Music),
        ("font/ttf", KindGroup::Fonts),
        ("application/zip", KindGroup::Documents),
    ];
    for (mime, expected) in rows {
        assert_eq!(kind_group(mime), expected, "{mime}");
    }
}
