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
fn a_name_gets_the_type_of_its_suffix_glob() {
    let dir = data(&[("globs2", "50:text/plain:*.txt\n50:image/png:*.png\n")]);
    let db = load(&[&dir]);
    assert_eq!(db.type_for_name("notes.txt"), Some("text/plain"));
    assert_eq!(db.type_for_name("pic.png"), Some("image/png"));
    assert_eq!(db.type_for_name("pic.jpg"), None);
}

#[test]
fn globs_ignore_case_unless_marked_case_sensitive() {
    let dir = data(&[(
        "globs2",
        "50:text/plain:*.txt\n50:text/x-c:*.C:cs\n50:text/x-c++:*.c\n",
    )]);
    let db = load(&[&dir]);
    assert_eq!(db.type_for_name("NOTES.TXT"), Some("text/plain"));
    assert_eq!(db.type_for_name("a.C"), Some("text/x-c"));
    assert_eq!(db.type_for_name("a.c"), Some("text/x-c++"));
}

#[test]
fn the_longer_pattern_wins_among_equal_weights() {
    let dir = data(&[(
        "globs2",
        "50:application/x-tar:*.tar\n50:application/gzip:*.gz\n50:application/x-compressed-tar:*.tar.gz\n",
    )]);
    let db = load(&[&dir]);
    assert_eq!(
        db.type_for_name("a.tar.gz"),
        Some("application/x-compressed-tar")
    );
    assert_eq!(db.type_for_name("a.gz"), Some("application/gzip"));
}

#[test]
fn the_higher_weight_wins_over_a_longer_pattern() {
    let dir = data(&[(
        "globs2",
        "50:application/x-compressed-tar:*.tar.gz\n80:application/gzip:*.gz\n",
    )]);
    assert_eq!(
        load(&[&dir]).type_for_name("a.tar.gz"),
        Some("application/gzip")
    );
}

#[test]
fn a_literal_name_is_checked_before_patterns() {
    let dir = data(&[(
        "globs2",
        "50:text/x-readme:README*\n10:text/x-makefile:Makefile\n90:text/x-shell:*file\n",
    )]);
    let db = load(&[&dir]);
    assert_eq!(db.type_for_name("Makefile"), Some("text/x-makefile"));
    assert_eq!(db.type_for_name("README.md"), Some("text/x-readme"));
}

#[test]
fn bracket_and_question_mark_patterns_match() {
    let dir = data(&[(
        "globs2",
        "50:text/x-csrc:*.[ch]\n50:text/x-tmp:a?c.tmp\n50:text/x-neg:*.[!a-m]x\n",
    )]);
    let db = load(&[&dir]);
    assert_eq!(db.type_for_name("main.c"), Some("text/x-csrc"));
    assert_eq!(db.type_for_name("main.h"), Some("text/x-csrc"));
    assert_eq!(db.type_for_name("main.o"), None);
    assert_eq!(db.type_for_name("abc.tmp"), Some("text/x-tmp"));
    assert_eq!(db.type_for_name("ac.tmp"), None);
    assert_eq!(db.type_for_name("f.zx"), Some("text/x-neg"));
    assert_eq!(db.type_for_name("f.bx"), None);
}

#[test]
fn a_type_found_by_name_is_canonical() {
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
    assert_eq!(db.file_type(Path::new("/x/a.txt"), false), "text/plain");
    assert_eq!(db.file_type(Path::new("/x/a.bin"), false), OCTET_STREAM);
    assert_eq!(db.file_type(Path::new("/x/a.txt"), true), DIRECTORY);
    assert_eq!(db.file_type(Path::new("/"), false), OCTET_STREAM);
}

#[test]
fn an_empty_database_knows_no_names() {
    let dir = tempfile::tempdir().unwrap();
    let db = MimeDatabase::load(&[dir.path().to_path_buf()]).unwrap();
    assert_eq!(db.type_for_name("a.txt"), None);
    let none = MimeDatabase::load(&[]).unwrap();
    assert_eq!(none.file_type(Path::new("a.txt"), false), OCTET_STREAM);
}

#[test]
fn a_more_important_directory_that_lists_noglobs_drops_the_globs_of_a_type_below() {
    let first = data(&[("globs2", "50:text/x-a:*.a\n50:text/x-b:*.b\n")]);
    let second = data(&[("globs2", "00:text/x-a:__NOGLOBS__\n")]);
    let third = data(&[("globs2", "50:text/x-a:*.a2\n")]);
    let db = load(&[&first, &second, &third]);
    // the first directory is most important: its globs stay; the NOGLOBS of the second removes the third's
    assert_eq!(db.type_for_name("f.a"), Some("text/x-a"));
    assert_eq!(db.type_for_name("f.a2"), None);
    let reversed = load(&[&second, &first]);
    assert_eq!(reversed.type_for_name("f.a"), None);
    assert_eq!(reversed.type_for_name("f.b"), Some("text/x-b"));
}

#[test]
fn globs_of_all_directories_are_merged() {
    let first = data(&[("globs2", "50:text/x-a:*.a\n")]);
    let second = data(&[("globs2", "50:text/x-b:*.b\n")]);
    let db = load(&[&first, &second]);
    assert_eq!(db.type_for_name("f.a"), Some("text/x-a"));
    assert_eq!(db.type_for_name("f.b"), Some("text/x-b"));
}

#[test]
fn aliases_and_icons_come_from_the_most_important_directory() {
    let first = data(&[
        ("aliases", "x/old x/new\n"),
        ("icons", "x/new:first-icon\n"),
    ]);
    let second = data(&[
        ("aliases", "x/old x/other\n"),
        ("icons", "x/new:second-icon\n"),
    ]);
    let db = load(&[&first, &second]);
    assert_eq!(db.canonical("x/old"), "x/new");
    assert_eq!(db.icon_name("x/new"), "first-icon");
}

#[test]
fn subclasses_are_merged_across_directories_in_file_order() {
    let first = data(&[("subclasses", "x/a x/p\nx/a x/q\n")]);
    let second = data(&[("subclasses", "x/a x/r\nx/a x/p\n")]);
    let db = load(&[&first, &second]);
    assert_eq!(db.parents("x/a"), ["x/p", "x/q", "x/r", OCTET_STREAM]);
}

#[test]
fn text_types_have_text_plain_as_an_implicit_parent() {
    let db = MimeDatabase::load(&[]).unwrap();
    assert_eq!(db.parents("text/x-rust"), [TEXT_PLAIN, OCTET_STREAM]);
    assert_eq!(db.parents(TEXT_PLAIN), [OCTET_STREAM]);
    assert!(db.parents(OCTET_STREAM).is_empty());
}

#[test]
fn directories_and_volume_types_have_no_octet_stream_parent() {
    let db = MimeDatabase::load(&[]).unwrap();
    assert!(db.parents(DIRECTORY).is_empty());
    assert!(db.parents("x-content/audio-cdda").is_empty());
    assert_eq!(db.ancestors(DIRECTORY), [DIRECTORY]);
}

#[test]
fn ancestors_run_nearest_first_and_end_with_octet_stream() {
    let dir = data(&[(
        "subclasses",
        "image/svg+xml application/xml\napplication/xml text/plain\n",
    )]);
    let db = load(&[&dir]);
    assert_eq!(
        db.ancestors("image/svg+xml"),
        [
            "image/svg+xml",
            "application/xml",
            "text/plain",
            OCTET_STREAM
        ]
    );
    assert_eq!(db.ancestors(OCTET_STREAM), [OCTET_STREAM]);
}

#[test]
fn a_subclass_cycle_ends() {
    let dir = data(&[("subclasses", "x/a x/b\nx/b x/a\n")]);
    let db = load(&[&dir]);
    assert_eq!(db.ancestors("x/a"), ["x/a", "x/b", OCTET_STREAM]);
}

#[test]
fn a_type_is_a_itself_an_ancestor_or_an_alias_of_one() {
    let dir = data(&[("subclasses", "x/a x/b\n"), ("aliases", "x/old x/b\n")]);
    let db = load(&[&dir]);
    assert!(db.is_a("x/a", "x/a"));
    assert!(db.is_a("x/a", "x/b"));
    assert!(db.is_a("x/a", "x/old"));
    assert!(db.is_a("x/a", OCTET_STREAM));
    assert!(!db.is_a("x/b", "x/a"));
    assert!(!db.is_a(DIRECTORY, OCTET_STREAM));
}

#[test]
fn an_icon_name_is_the_icons_entry_or_the_type_with_a_dash() {
    let dir = data(&[("icons", "text/x-special:special-icon\n")]);
    let db = load(&[&dir]);
    assert_eq!(db.icon_name("text/x-special"), "special-icon");
    assert_eq!(db.icon_name("image/png"), "image-png");
}

#[test]
fn a_generic_icon_is_the_entry_or_the_media_type_generic() {
    let dir = data(&[("generic-icons", "application/pdf:x-office-document\n")]);
    let db = load(&[&dir]);
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
fn comments_and_blank_lines_are_skipped() {
    let dir = data(&[("globs2", "# a comment\n\n50:text/plain:*.txt\n")]);
    assert_eq!(load(&[&dir]).type_for_name("a.txt"), Some("text/plain"));
}

#[test]
fn a_glob_line_without_a_number_weight_is_an_error_with_its_line() {
    let dir = data(&[(
        "globs2",
        "# header\n50:text/plain:*.txt\nheavy:text/x:*.x\n",
    )]);
    let err = MimeDatabase::load(&[dir.path().to_path_buf()]).unwrap_err();
    match err {
        LoadError::Parse(e) => {
            assert_eq!(e.line, 3);
            assert_eq!(e.path, dir.path().join("mime/globs2"));
        }
        other => panic!("{other}"),
    }
}

#[test]
fn a_glob_line_with_too_few_fields_is_an_error() {
    let dir = data(&[("globs2", "50:text/plain\n")]);
    let err = MimeDatabase::load(&[dir.path().to_path_buf()]).unwrap_err();
    assert!(matches!(err, LoadError::Parse(ref e) if e.line == 1));
    assert!(err.to_string().contains("globs2:1"));
}

#[test]
fn alias_subclass_and_icon_lines_need_two_fields() {
    for (file, good) in [
        ("aliases", "ok/a ok/b"),
        ("subclasses", "ok/a ok/b"),
        ("icons", "ok/a:icon"),
        ("generic-icons", "ok/a:icon"),
    ] {
        for bad in ["lonely", "ok/a:", "ok/a "] {
            let dir = data(&[(file, &format!("{good}\n{bad}\n"))]);
            match MimeDatabase::load(&[dir.path().to_path_buf()]) {
                Err(LoadError::Parse(e)) => assert_eq!(e.line, 2, "{file}: {bad:?}"),
                other => panic!("{file}: {bad:?} gave {other:?}"),
            }
        }
    }
}

#[test]
fn an_unreadable_database_file_is_an_io_error() {
    let dir = data(&[]);
    // a directory where the file should be
    std::fs::create_dir(dir.path().join("mime/globs2")).unwrap();
    let err = MimeDatabase::load(&[dir.path().to_path_buf()]).unwrap_err();
    assert!(matches!(err, LoadError::Io(..)));
}
