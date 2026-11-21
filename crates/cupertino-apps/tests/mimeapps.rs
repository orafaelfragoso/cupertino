//! Default applications and the Open With list over a temporary set of XDG directories.

use crate::common;

use common::{desktop, vis, write};
use cupertino_apps::entry::Locale;
use cupertino_apps::mime::MimeDatabase;
use cupertino_apps::mimeapps::{MimeApps, XdgDirs};
use std::path::PathBuf;
use tempfile::TempDir;

/// `config`, `config2`, `home` and `usr` folders in a temporary tree, in the order of the specification.
struct Tree {
    root: TempDir,
}

impl Tree {
    fn new() -> Tree {
        Tree {
            root: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.root.path().join(name)
    }

    fn put(&self, rel: &str, text: &str) {
        write(self.root.path(), rel, text);
    }

    fn app(&self, dir: &str, id: &str, name: &str, extra: &str) {
        self.put(
            &format!("{dir}/applications/{id}"),
            &desktop(name, id, extra),
        );
    }

    fn dirs(&self) -> XdgDirs {
        XdgDirs {
            config_home: self.path("config"),
            config_dirs: vec![self.path("config2")],
            data_home: self.path("home"),
            data_dirs: vec![self.path("usr")],
            current_desktops: vec!["Cupertino".to_string()],
        }
    }

    /// The handlers of the tree, with the MIME database of its `usr` folder.
    fn load(&self) -> MimeApps {
        let db = MimeDatabase::load(&[self.path("usr")]).unwrap();
        MimeApps::load(&self.dirs(), db, &Locale::default(), &vis()).unwrap()
    }
}

fn default_id(apps: &MimeApps, mime: &str) -> Option<String> {
    apps.default_for(mime).map(|e| e.id.clone())
}

fn open_with(apps: &MimeApps, mime: &str) -> Vec<(String, bool)> {
    apps.open_with(mime)
        .into_iter()
        .map(|i| (i.entry.id.clone(), i.is_default))
        .collect()
}

fn ids(apps: &MimeApps, mime: &str) -> Vec<String> {
    open_with(apps, mime).into_iter().map(|(i, _)| i).collect()
}

const TEXT: &str = "MimeType=text/plain;";

#[test]
fn the_default_application_follows_the_lists_and_entries_by_the_specification() {
    // (label, applications as (folder, id, extra), files as (path, text), type, default)
    type Row<'a> = (
        &'a str,
        &'a [(&'a str, &'a str, &'a str)],
        &'a [(&'a str, &'a str)],
        &'a str,
        Option<&'a str>,
    );
    let rows: &[Row] = &[
        ("an entry that lists the type", &[("usr", "a.desktop", TEXT)], &[], "text/plain", Some("a.desktop")),
        ("no handler", &[("usr", "a.desktop", TEXT)], &[], "image/png", None),
        ("nothing installed", &[], &[], "text/plain", None),
        (
            "first id among equals",
            &[("usr", "b.desktop", TEXT), ("usr", "a.desktop", TEXT)],
            &[],
            "text/plain",
            Some("a.desktop"),
        ),
        (
            "the default group picks",
            &[("usr", "a.desktop", TEXT), ("usr", "b.desktop", TEXT)],
            &[("config/mimeapps.list", "[Default Applications]\ntext/plain=b.desktop\n")],
            "text/plain",
            Some("b.desktop"),
        ),
        (
            "a default not installed is skipped",
            &[("usr", "b.desktop", TEXT)],
            &[("config/mimeapps.list", "[Default Applications]\ntext/plain=missing.desktop;b.desktop\n")],
            "text/plain",
            Some("b.desktop"),
        ),
        (
            "the most important list decides",
            &[("usr", "a.desktop", TEXT), ("usr", "b.desktop", TEXT)],
            &[
                ("config/mimeapps.list", "[Default Applications]\ntext/plain=a.desktop\n"),
                ("config2/mimeapps.list", "[Default Applications]\ntext/plain=b.desktop\n"),
            ],
            "text/plain",
            Some("a.desktop"),
        ),
        (
            "added associations make a handler",
            &[("usr", "a.desktop", "")],
            &[("config/mimeapps.list", "[Added Associations]\ntext/plain=a.desktop;\n")],
            "text/plain",
            Some("a.desktop"),
        ),
        (
            "a list for another desktop is not read",
            &[("usr", "a.desktop", TEXT), ("usr", "b.desktop", TEXT)],
            &[("config/other-mimeapps.list", "[Default Applications]\ntext/plain=b.desktop\n")],
            "text/plain",
            Some("a.desktop"),
        ),
        (
            "the desktop's list comes before the plain list of its folder",
            &[("usr", "a.desktop", TEXT), ("usr", "b.desktop", TEXT)],
            &[
                ("config/mimeapps.list", "[Default Applications]\ntext/plain=a.desktop\n"),
                ("config/cupertino-mimeapps.list", "[Default Applications]\ntext/plain=b.desktop\n"),
            ],
            "text/plain",
            Some("b.desktop"),
        ),
        (
            "a handler of a more specific type wins over a default for a broader one",
            &[("usr", "editor.desktop", TEXT), ("usr", "rust.desktop", "MimeType=text/x-rust;")],
            &[
                ("usr/mime/subclasses", "text/x-rust text/plain\n"),
                ("config/mimeapps.list", "[Default Applications]\ntext/plain=editor.desktop\n"),
            ],
            "text/x-rust",
            Some("rust.desktop"),
        ),
        (
            "the broader default stays for the broader type",
            &[("usr", "editor.desktop", TEXT), ("usr", "rust.desktop", "MimeType=text/x-rust;")],
            &[
                ("usr/mime/subclasses", "text/x-rust text/plain\n"),
                ("config/mimeapps.list", "[Default Applications]\ntext/plain=editor.desktop\n"),
            ],
            "text/plain",
            Some("editor.desktop"),
        ),
        (
            "an ancestor's handler",
            &[("usr", "editor.desktop", TEXT)],
            &[],
            "text/x-anything",
            Some("editor.desktop"),
        ),
        (
            "aliases resolve in lists and entries",
            &[("usr", "a.desktop", "MimeType=x/old;"), ("usr", "b.desktop", "MimeType=x/new;")],
            &[
                ("usr/mime/aliases", "x/old x/new\n"),
                ("config/mimeapps.list", "[Default Applications]\nx/old=b.desktop\n"),
            ],
            "x/new",
            Some("b.desktop"),
        ),
        (
            "the first folder with an id defines it",
            &[("home", "a.desktop", TEXT), ("usr", "a.desktop", "MimeType=image/png;")],
            &[],
            "image/png",
            None,
        ),
        (
            "a hidden entry shadows the same id below",
            &[("home", "a.desktop", "Hidden=true\nMimeType=text/plain;"), ("usr", "a.desktop", TEXT)],
            &[],
            "text/plain",
            None,
        ),
        (
            "comments, blank lines and unknown groups are ignored",
            &[("usr", "a.desktop", TEXT), ("usr", "b.desktop", TEXT)],
            &[(
                "config/mimeapps.list",
                "# comment\n\n[Something Else]\ntext/plain=a.desktop\n[Default Applications]\n\ntext/plain = b.desktop ; \n",
            )],
            "text/plain",
            Some("b.desktop"),
        ),
    ];
    for (label, apps, files, mime, expected) in rows {
        let t = Tree::new();
        for (dir, id, extra) in *apps {
            t.app(dir, id, id, extra);
        }
        for (path, text) in *files {
            t.put(path, text);
        }
        assert_eq!(default_id(&t.load(), mime).as_deref(), *expected, "{label}");
    }
}

#[test]
fn an_entry_is_found_by_its_id_with_its_source_and_subfolders_join_with_a_dash() {
    let t = Tree::new();
    t.app("home", "a.desktop", "Home A", TEXT);
    t.app("usr", "a.desktop", "Usr A", TEXT);
    t.put(
        "usr/applications/kde/k.desktop",
        &desktop("K", "k", "MimeType=image/png;"),
    );
    let apps = t.load();
    let a = apps.entry("a.desktop").unwrap();
    assert_eq!(a.name, "Home A");
    assert_eq!(a.source, Some(t.path("home/applications/a.desktop")));
    assert!(apps.entry("b.desktop").is_none());
    assert_eq!(
        default_id(&apps, "image/png").as_deref(),
        Some("kde-k.desktop")
    );
}

#[test]
fn removed_associations_and_desktop_lists_change_the_handlers_as_specified() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", TEXT);
    t.app("usr", "b.desktop", "B", TEXT);
    t.put(
        "config/mimeapps.list",
        "[Removed Associations]\ntext/plain=a.desktop\n",
    );
    let apps = t.load();
    assert_eq!(
        open_with(&apps, "text/plain"),
        [("b.desktop".to_string(), true)]
    );

    // a desktop-specific list only sets defaults: its added and removed associations are not read
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", TEXT);
    t.app("usr", "b.desktop", "B", "");
    t.app("usr", "c.desktop", "C", TEXT);
    t.put(
        "config/cupertino-mimeapps.list",
        "[Default Applications]\ntext/plain=c.desktop\n[Added Associations]\ntext/plain=b.desktop\n\
         [Removed Associations]\ntext/plain=a.desktop\n",
    );
    let apps = t.load();
    assert_eq!(
        default_id(&apps, "text/plain").as_deref(),
        Some("c.desktop")
    );
    assert_eq!(ids(&apps, "text/plain"), ["c.desktop", "a.desktop"]);
}

#[test]
fn a_list_in_an_application_folder_reaches_only_entries_at_or_below_it() {
    let t = Tree::new();
    t.app("home", "home.desktop", "Home", "");
    t.app("usr", "usr.desktop", "Usr", "");
    t.put(
        "usr/applications/mimeapps.list",
        "[Added Associations]\ntext/plain=home.desktop;usr.desktop\n",
    );
    t.put(
        "home/applications/mimeapps.list",
        "[Added Associations]\nimage/png=home.desktop;usr.desktop\n",
    );
    let apps = t.load();
    assert_eq!(ids(&apps, "text/plain"), ["usr.desktop"]);
    assert_eq!(ids(&apps, "image/png"), ["home.desktop", "usr.desktop"]);
}

#[test]
fn open_with_puts_the_default_first_and_sorts_the_rest_naturally_by_name_then_id() {
    let t = Tree::new();
    for (id, name) in [
        ("a.desktop", "app10"),
        ("b.desktop", "App2"),
        ("c.desktop", "beta"),
        ("d.desktop", "Zed"),
        ("f.desktop", "Same"),
        ("e.desktop", "Same"),
    ] {
        t.app("usr", id, name, TEXT);
    }
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=d.desktop\n",
    );
    let expected: Vec<(String, bool)> = [
        ("d.desktop", true),
        ("b.desktop", false),
        ("a.desktop", false),
        ("c.desktop", false),
        ("e.desktop", false),
        ("f.desktop", false),
    ]
    .map(|(i, d)| (i.to_string(), d))
    .to_vec();
    assert_eq!(open_with(&t.load(), "text/plain"), expected);
}

#[test]
fn open_with_includes_ancestors_and_octet_stream_only_when_nothing_is_more_specific() {
    let t = Tree::new();
    t.put("usr/mime/subclasses", "x/special x/base\n");
    t.put("usr/mime/aliases", "x/old x/base\n");
    t.app("usr", "base.desktop", "Base", "MimeType=x/old;");
    t.app("usr", "special.desktop", "Special", "MimeType=x/special;");
    t.app(
        "usr",
        "any.desktop",
        "Any",
        "MimeType=application/octet-stream;",
    );
    t.app("usr", "text.desktop", "Text", TEXT);
    let apps = t.load();
    for (mime, expected) in [
        ("x/special", &["special.desktop", "base.desktop"][..]),
        ("x/old", &["base.desktop"]),
        ("text/plain", &["text.desktop"]),
        ("image/png", &["any.desktop"]),
    ] {
        assert_eq!(ids(&apps, mime), expected, "{mime}");
    }
    assert!(Tree::new().load().open_with("image/png").is_empty());
}

#[test]
fn an_unreadable_list_file_is_an_error_naming_the_path() {
    let t = Tree::new();
    std::fs::create_dir_all(t.path("config/mimeapps.list")).unwrap();
    let err = MimeApps::load(
        &t.dirs(),
        MimeDatabase::default(),
        &Locale::default(),
        &vis(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("mimeapps.list"), "{err}");
}
