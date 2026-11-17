//! The shared MIME database: file names to types, aliases, subclasses and icon names.

mod common;

use common::write;
use cupertino_apps::mime::{
    media_generic_icon, LoadError, MimeDatabase, DIRECTORY, OCTET_STREAM, TEXT_PLAIN,
};
use std::path::{Path, PathBuf};

/// A data directory with the given files under `mime/`.
fn data(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("mime")).unwrap();
    for (name, text) in files {
        write(dir.path(), &format!("mime/{name}"), text);
    }
    dir
}

fn load(dirs: &[&tempfile::TempDir]) -> MimeDatabase {
    let paths: Vec<PathBuf> = dirs.iter().map(|d| d.path().to_path_buf()).collect();
    MimeDatabase::load(&paths).unwrap()
}

#[test]
fn a_name_gets_the_type_of_the_best_matching_glob() {
    for (globs, name, expected) in [
        ("50:text/plain:*.txt\n50:image/png:*.png\n", "notes.txt", Some("text/plain")),
        ("50:text/plain:*.txt\n50:image/png:*.png\n", "pic.png", Some("image/png")),
        ("50:text/plain:*.txt\n", "pic.jpg", None),
        ("# a comment\n\n50:text/plain:*.txt\n", "a.txt", Some("text/plain")),
        ("50:text/plain:*.txt\n", "NOTES.TXT", Some("text/plain")),
        ("50:text/x-c:*.C:cs\n50:text/x-c++:*.c\n", "a.C", Some("text/x-c")),
        ("50:text/x-c:*.C:cs\n50:text/x-c++:*.c\n", "a.c", Some("text/x-c++")),
        (
            "50:application/x-tar:*.tar\n50:application/gzip:*.gz\n50:application/x-compressed-tar:*.tar.gz\n",
            "a.tar.gz",
            Some("application/x-compressed-tar"),
        ),
        ("50:application/x-compressed-tar:*.tar.gz\n50:application/gzip:*.gz\n", "a.gz", Some("application/gzip")),
        ("50:application/x-compressed-tar:*.tar.gz\n80:application/gzip:*.gz\n", "a.tar.gz", Some("application/gzip")),
        ("10:text/x-makefile:Makefile\n90:text/x-shell:*file\n", "Makefile", Some("text/x-makefile")),
        ("50:text/x-readme:README*\n", "README.md", Some("text/x-readme")),
        ("50:text/x-csrc:*.[ch]\n", "main.c", Some("text/x-csrc")),
        ("50:text/x-csrc:*.[ch]\n", "main.h", Some("text/x-csrc")),
        ("50:text/x-csrc:*.[ch]\n", "main.o", None),
        ("50:text/x-tmp:a?c.tmp\n", "abc.tmp", Some("text/x-tmp")),
        ("50:text/x-tmp:a?c.tmp\n", "ac.tmp", None),
        ("50:text/x-neg:*.[!a-m]x\n", "f.zx", Some("text/x-neg")),
        ("50:text/x-neg:*.[!a-m]x\n", "f.bx", None),
    ] {
        let dir = data(&[("globs2", globs)]);
        assert_eq!(load(&[&dir]).type_for_name(name), expected, "{name} with {globs:?}");
    }
}

#[test]
fn a_type_is_canonical_through_the_aliases() {
    let dir = data(&[
        ("globs2", "50:application/x-old-pdf:*.pdf\n"),
        ("aliases", "application/x-old-pdf application/pdf\n"),
    ]);
    let db = load(&[&dir]);
    assert_eq!(db.type_for_name("a.pdf"), Some("application/pdf"));
    assert_eq!(db.canonical("application/x-old-pdf"), "application/pdf");
    assert_eq!(db.canonical("text/plain"), "text/plain");
}

#[test]
fn a_file_is_a_directory_or_its_named_type_or_octet_stream() {
    let dir = data(&[("globs2", "50:text/plain:*.txt\n")]);
    let db = load(&[&dir]);
    let empty = MimeDatabase::load(&[]).unwrap();
    for (db, path, is_dir, expected) in [
        (&db, "/x/a.txt", false, "text/plain"),
        (&db, "/x/a.bin", false, OCTET_STREAM),
        (&db, "/x/a.txt", true, DIRECTORY),
        (&db, "/", false, OCTET_STREAM),
        (&empty, "a.txt", false, OCTET_STREAM),
    ] {
        assert_eq!(
            db.file_type(Path::new(path), is_dir),
            expected,
            "{path} dir={is_dir}"
        );
    }
}

#[test]
fn directories_merge_globs_and_subclasses_and_the_most_important_one_wins_the_rest() {
    let first = data(&[
        ("globs2", "50:text/x-a:*.a\n50:text/x-b:*.b\n"),
        ("aliases", "x/old x/new\n"),
        ("icons", "x/new:first-icon\n"),
        ("subclasses", "x/a x/p\nx/a x/q\n"),
    ]);
    let second = data(&[
        ("globs2", "50:text/x-c:*.c\n"),
        ("aliases", "x/old x/other\n"),
        ("icons", "x/new:second-icon\n"),
        ("subclasses", "x/a x/r\nx/a x/p\n"),
    ]);
    let db = load(&[&first, &second]);
    assert_eq!(db.type_for_name("f.a"), Some("text/x-a"));
    assert_eq!(db.type_for_name("f.c"), Some("text/x-c"));
    assert_eq!(db.canonical("x/old"), "x/new");
    assert_eq!(db.icon_name("x/new"), "first-icon");
    assert_eq!(db.parents("x/a"), ["x/p", "x/q", "x/r", OCTET_STREAM]);
}

#[test]
fn a_more_important_directory_that_lists_noglobs_drops_the_globs_of_a_type_below() {
    let first = data(&[("globs2", "50:text/x-a:*.a\n50:text/x-b:*.b\n")]);
    let second = data(&[("globs2", "00:text/x-a:__NOGLOBS__\n")]);
    let third = data(&[("globs2", "50:text/x-a:*.a2\n")]);
    let db = load(&[&first, &second, &third]);
    assert_eq!(db.type_for_name("f.a"), Some("text/x-a"));
    assert_eq!(db.type_for_name("f.a2"), None);
    let reversed = load(&[&second, &first]);
    assert_eq!(reversed.type_for_name("f.a"), None);
    assert_eq!(reversed.type_for_name("f.b"), Some("text/x-b"));
}

#[test]
fn ancestors_run_nearest_first_through_implicit_parents_and_end() {
    let dir = data(&[(
        "subclasses",
        "image/svg+xml application/xml\napplication/xml text/plain\nx/a x/b\nx/b x/a\n",
    )]);
    let db = load(&[&dir]);
    for (mime, expected) in [
        (
            "image/svg+xml",
            &["image/svg+xml", "application/xml", TEXT_PLAIN, OCTET_STREAM][..],
        ),
        ("text/x-rust", &["text/x-rust", TEXT_PLAIN, OCTET_STREAM]),
        (TEXT_PLAIN, &[TEXT_PLAIN, OCTET_STREAM]),
        (OCTET_STREAM, &[OCTET_STREAM]),
        (DIRECTORY, &[DIRECTORY]),
        ("x-content/audio-cdda", &["x-content/audio-cdda"]),
        ("x/a", &["x/a", "x/b", OCTET_STREAM]),
    ] {
        assert_eq!(db.ancestors(mime), expected, "{mime}");
    }
}

#[test]
fn a_type_is_a_itself_an_ancestor_or_an_alias_of_one() {
    let dir = data(&[("subclasses", "x/a x/b\n"), ("aliases", "x/old x/b\n")]);
    let db = load(&[&dir]);
    for (mime, of, expected) in [
        ("x/a", "x/a", true),
        ("x/a", "x/b", true),
        ("x/a", "x/old", true),
        ("x/a", OCTET_STREAM, true),
        ("x/b", "x/a", false),
        (DIRECTORY, OCTET_STREAM, false),
    ] {
        assert_eq!(db.is_a(mime, of), expected, "{mime} is a {of}");
    }
}

#[test]
fn icon_names_come_from_the_icon_files_or_the_type() {
    let dir = data(&[
        ("icons", "text/x-special:special-icon\n"),
        ("generic-icons", "application/pdf:x-office-document\n"),
    ]);
    let db = load(&[&dir]);
    assert_eq!(db.icon_name("text/x-special"), "special-icon");
    assert_eq!(db.icon_name("image/png"), "image-png");
    assert_eq!(
        db.generic_icon("application/pdf"),
        Some("x-office-document")
    );
    assert_eq!(db.generic_icon("image/png"), None);
    assert_eq!(db.generic_icon_name("application/pdf"), "x-office-document");
    assert_eq!(db.generic_icon_name("image/png"), "image-x-generic");
    assert_eq!(media_generic_icon("audio/flac"), "audio-x-generic");
    assert_eq!(media_generic_icon("weird"), "weird-x-generic");
}

#[test]
fn a_malformed_line_is_a_parse_error_naming_its_file_and_line() {
    let mut cases = vec![
        (
            "globs2",
            "# header\n50:text/plain:*.txt\nheavy:text/x:*.x\n".to_string(),
            3,
        ),
        ("globs2", "50:text/plain\n".to_string(), 1),
    ];
    for (file, good) in [
        ("aliases", "ok/a ok/b"),
        ("subclasses", "ok/a ok/b"),
        ("icons", "ok/a:icon"),
        ("generic-icons", "ok/a:icon"),
    ] {
        for bad in ["lonely", "ok/a:", "ok/a "] {
            cases.push((file, format!("{good}\n{bad}\n"), 2));
        }
    }
    for (file, text, line) in &cases {
        let dir = data(&[(file, text)]);
        match MimeDatabase::load(&[dir.path().to_path_buf()]) {
            Err(LoadError::Parse(e)) => {
                assert_eq!(e.line, *line, "{file}: {text:?}");
                assert_eq!(e.path, dir.path().join("mime").join(file));
            }
            other => panic!("{file}: {text:?} gave {other:?}"),
        }
    }
}

#[test]
fn an_unreadable_database_file_is_an_io_error_and_a_missing_one_is_empty() {
    let dir = data(&[]);
    assert_eq!(load(&[&dir]).type_for_name("a.txt"), None);
    // a directory where the file should be
    std::fs::create_dir(dir.path().join("mime/globs2")).unwrap();
    let err = MimeDatabase::load(&[dir.path().to_path_buf()]).unwrap_err();
    assert!(matches!(err, LoadError::Io(..)));
}
