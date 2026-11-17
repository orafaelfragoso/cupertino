//! Default applications and the Open With list over a temporary set of XDG directories.

mod common;

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

    fn dirs(&self, desktops: &[&str]) -> XdgDirs {
        XdgDirs {
            config_home: self.path("config"),
            config_dirs: vec![self.path("config2")],
            data_home: self.path("home"),
            data_dirs: vec![self.path("usr")],
            current_desktops: desktops.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn load_with(&self, db: MimeDatabase, desktops: &[&str]) -> MimeApps {
        MimeApps::load(&self.dirs(desktops), db, &Locale::default(), &vis()).unwrap()
    }

    fn load(&self) -> MimeApps {
        self.load_with(MimeDatabase::default(), &["Cupertino"])
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

#[test]
fn an_application_that_lists_a_type_handles_it() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    let apps = t.load();
    assert_eq!(
        default_id(&apps, "text/plain").as_deref(),
        Some("a.desktop")
    );
    assert_eq!(default_id(&apps, "image/png"), None);
    let entry = apps.entry("a.desktop").unwrap();
    assert_eq!(entry.source, Some(t.path("usr/applications/a.desktop")));
    assert!(apps.entry("b.desktop").is_none());
}

#[test]
fn without_a_default_the_first_id_among_equals_is_the_default() {
    let t = Tree::new();
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("a.desktop")
    );
}

#[test]
fn the_default_applications_group_picks_among_the_associated() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=b.desktop\n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("b.desktop")
    );
}

#[test]
fn a_default_that_is_not_installed_is_skipped_for_the_next_listed() {
    let t = Tree::new();
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=missing.desktop;b.desktop\n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("b.desktop")
    );
}

#[test]
fn the_most_important_list_decides_the_default() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=a.desktop\n",
    );
    t.put(
        "config2/mimeapps.list",
        "[Default Applications]\ntext/plain=b.desktop\n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("a.desktop")
    );
}

#[test]
fn added_associations_make_an_application_a_handler() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "");
    t.put(
        "config/mimeapps.list",
        "[Added Associations]\ntext/plain=a.desktop;\n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("a.desktop")
    );
}

#[test]
fn removed_associations_take_an_application_away_from_less_important_levels() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/mimeapps.list",
        "[Removed Associations]\ntext/plain=a.desktop\n",
    );
    let apps = t.load();
    assert_eq!(
        default_id(&apps, "text/plain").as_deref(),
        Some("b.desktop")
    );
    assert_eq!(
        open_with(&apps, "text/plain"),
        [("b.desktop".to_string(), true)]
    );
}

#[test]
fn a_desktop_specific_list_only_sets_defaults() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "");
    t.app("usr", "c.desktop", "C", "MimeType=text/plain;");
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
    let ids: Vec<String> = open_with(&apps, "text/plain")
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ids, ["c.desktop", "a.desktop"]);
}

#[test]
fn a_desktop_specific_list_is_read_only_for_a_current_desktop() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/other-mimeapps.list",
        "[Default Applications]\ntext/plain=b.desktop\n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("a.desktop")
    );
}

#[test]
fn a_desktop_specific_list_comes_before_the_plain_list_of_its_folder() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=a.desktop\n",
    );
    t.put(
        "config/cupertino-mimeapps.list",
        "[Default Applications]\ntext/plain=b.desktop\n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("b.desktop")
    );
}

#[test]
fn a_handler_of_a_more_specific_type_wins_over_a_default_for_a_broader_one() {
    let t = Tree::new();
    t.put("usr/mime/subclasses", "text/x-rust text/plain\n");
    t.app("usr", "editor.desktop", "Editor", "MimeType=text/plain;");
    t.app("usr", "rust.desktop", "Rust", "MimeType=text/x-rust;");
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=editor.desktop\n",
    );
    let db = MimeDatabase::load(&[t.path("usr")]).unwrap();
    let apps = t.load_with(db, &["Cupertino"]);
    assert_eq!(
        default_id(&apps, "text/x-rust").as_deref(),
        Some("rust.desktop")
    );
    assert_eq!(
        default_id(&apps, "text/plain").as_deref(),
        Some("editor.desktop")
    );
}

#[test]
fn a_type_without_a_handler_falls_back_to_the_handler_of_an_ancestor() {
    let t = Tree::new();
    t.app("usr", "editor.desktop", "Editor", "MimeType=text/plain;");
    assert_eq!(
        default_id(&t.load(), "text/x-anything").as_deref(),
        Some("editor.desktop")
    );
}

#[test]
fn aliases_resolve_in_lists_and_in_desktop_entries() {
    let t = Tree::new();
    t.put("usr/mime/aliases", "x/old x/new\n");
    t.app("usr", "a.desktop", "A", "MimeType=x/old;");
    t.app("usr", "b.desktop", "B", "MimeType=x/new;");
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\nx/old=b.desktop\n",
    );
    let db = MimeDatabase::load(&[t.path("usr")]).unwrap();
    let apps = t.load_with(db, &["Cupertino"]);
    assert_eq!(default_id(&apps, "x/new").as_deref(), Some("b.desktop"));
    assert_eq!(default_id(&apps, "x/old").as_deref(), Some("b.desktop"));
    assert_eq!(open_with(&apps, "x/old").len(), 2);
}

#[test]
fn the_first_directory_with_an_id_defines_it() {
    let t = Tree::new();
    t.app("home", "a.desktop", "Home A", "MimeType=text/plain;");
    t.app("usr", "a.desktop", "Usr A", "MimeType=image/png;");
    let apps = t.load();
    assert_eq!(apps.entry("a.desktop").unwrap().name, "Home A");
    assert_eq!(default_id(&apps, "image/png"), None);
}

#[test]
fn a_hidden_entry_is_not_installed_and_shadows_the_same_id_below() {
    let t = Tree::new();
    t.app(
        "home",
        "a.desktop",
        "Hidden A",
        "Hidden=true\nMimeType=text/plain;",
    );
    t.app("usr", "a.desktop", "Usr A", "MimeType=text/plain;");
    let apps = t.load();
    assert!(apps.entry("a.desktop").is_none());
    assert_eq!(default_id(&apps, "text/plain"), None);
}

#[test]
fn desktop_ids_of_subfolders_join_with_a_dash() {
    let t = Tree::new();
    t.put(
        "usr/applications/kde/a.desktop",
        &desktop("A", "a", "MimeType=text/plain;"),
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("kde-a.desktop")
    );
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
    let ids =
        |mime| -> Vec<String> { open_with(&apps, mime).into_iter().map(|(i, _)| i).collect() };
    assert_eq!(ids("text/plain"), ["usr.desktop"]);
    assert_eq!(ids("image/png"), ["home.desktop", "usr.desktop"]);
}

#[test]
fn open_with_puts_the_default_first_and_sorts_the_rest_naturally_by_name() {
    let t = Tree::new();
    for (id, name) in [
        ("a.desktop", "app10"),
        ("b.desktop", "App2"),
        ("c.desktop", "beta"),
        ("d.desktop", "Zed"),
    ] {
        t.app("usr", id, name, "MimeType=text/plain;");
    }
    t.put(
        "config/mimeapps.list",
        "[Default Applications]\ntext/plain=d.desktop\n",
    );
    let order: Vec<(String, bool)> = open_with(&t.load(), "text/plain");
    let expected: Vec<(String, bool)> = [
        ("d.desktop", true),
        ("b.desktop", false),
        ("a.desktop", false),
        ("c.desktop", false),
    ]
    .map(|(i, d)| (i.to_string(), d))
    .to_vec();
    assert_eq!(order, expected);
}

#[test]
fn equal_names_in_open_with_fall_back_to_the_desktop_file_id() {
    let t = Tree::new();
    t.app("usr", "b.desktop", "Same", "MimeType=text/plain;");
    t.app("usr", "a.desktop", "Same", "MimeType=text/plain;");
    let ids: Vec<String> = open_with(&t.load(), "text/plain")
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ids, ["a.desktop", "b.desktop"]);
}

#[test]
fn open_with_includes_the_handlers_of_ancestors() {
    let t = Tree::new();
    t.put("usr/mime/subclasses", "x/special x/base\n");
    t.app("usr", "base.desktop", "Base", "MimeType=x/base;");
    t.app("usr", "special.desktop", "Special", "MimeType=x/special;");
    let db = MimeDatabase::load(&[t.path("usr")]).unwrap();
    let apps = t.load_with(db, &["Cupertino"]);
    let ids: Vec<String> = open_with(&apps, "x/special")
        .into_iter()
        .map(|(i, _)| i)
        .collect();
    assert_eq!(ids, ["special.desktop", "base.desktop"]);
}

#[test]
fn octet_stream_handlers_join_open_with_only_when_nothing_is_more_specific() {
    let t = Tree::new();
    t.app(
        "usr",
        "any.desktop",
        "Any",
        "MimeType=application/octet-stream;",
    );
    t.app("usr", "text.desktop", "Text", "MimeType=text/plain;");
    let apps = t.load();
    let ids =
        |mime| -> Vec<String> { open_with(&apps, mime).into_iter().map(|(i, _)| i).collect() };
    assert_eq!(ids("text/plain"), ["text.desktop"]);
    assert_eq!(ids("image/png"), ["any.desktop"]);
}

#[test]
fn open_with_is_empty_for_a_type_nothing_handles() {
    let t = Tree::new();
    assert!(t.load().open_with("image/png").is_empty());
}

#[test]
fn missing_folders_and_lists_are_skipped() {
    let t = Tree::new();
    let apps = t.load();
    assert_eq!(default_id(&apps, "text/plain"), None);
}

#[test]
fn an_unreadable_list_file_is_an_error_naming_the_path() {
    let t = Tree::new();
    std::fs::create_dir_all(t.path("config/mimeapps.list")).unwrap();
    let err = MimeApps::load(
        &t.dirs(&[]),
        MimeDatabase::default(),
        &Locale::default(),
        &vis(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("mimeapps.list"), "{err}");
}

#[test]
fn the_data_directories_run_from_the_data_home_down() {
    let t = Tree::new();
    let dirs = t.dirs(&[]);
    assert_eq!(dirs.all_data_dirs(), [t.path("home"), t.path("usr")]);
}

#[test]
fn comments_blank_lines_and_unknown_groups_in_a_list_are_ignored() {
    let t = Tree::new();
    t.app("usr", "a.desktop", "A", "MimeType=text/plain;");
    t.app("usr", "b.desktop", "B", "MimeType=text/plain;");
    t.put(
        "config/mimeapps.list",
        "# comment\n\n[Something Else]\ntext/plain=a.desktop\n[Default Applications]\n\ntext/plain = b.desktop ; \n",
    );
    assert_eq!(
        default_id(&t.load(), "text/plain").as_deref(),
        Some("b.desktop")
    );
}
