//! Desktop entries: locale fallback, list values, visibility, handlers and indexing.

use crate::common;

use common::{desktop, executable, vis, write};
use cupertino_apps::entry::{
    dbus_activatable, dbus_activatable_file, index_applications, index_dirs, listed_application,
    parse_desktop_entry, parse_handler, split_list, Hidden, Locale, Visibility,
};
use std::path::Path;

fn parse(text: &str) -> Result<cupertino_apps::DesktopEntry, Hidden> {
    parse_desktop_entry("a.desktop", text, &Locale::default(), &vis())
}

/// Visibility that checks programs in `dir/bin`.
fn checked(dir: &Path) -> Visibility {
    Visibility {
        skip_try_exec: false,
        path_dirs: vec![dir.join("bin")],
        ..vis()
    }
}

#[test]
fn a_locale_name_lists_its_keys_from_the_most_to_the_least_specific() {
    for (name, expected) in [
        (
            "de_AT.UTF-8@euro",
            &["de_AT@euro", "de_AT", "de@euro", "de"][..],
        ),
        ("en_US.UTF-8", &["en_US", "en"]),
        ("fr", &["fr"]),
        ("C", &[]),
        ("POSIX", &[]),
        ("", &[]),
        ("  ", &[]),
    ] {
        assert_eq!(Locale::parse(name).candidates, expected, "{name:?}");
    }
}

#[test]
fn a_list_splits_on_unescaped_semicolons_and_drops_empty_items() {
    for (text, expected) in [
        ("a;b;c", &["a", "b", "c"][..]),
        ("a;b\\;c;d;", &["a", "b;c", "d"]),
        (";;a;;b;;", &["a", "b"]),
        ("", &[]),
    ] {
        assert_eq!(split_list(text), expected, "{text:?}");
    }
}

#[test]
fn an_entry_reads_its_main_group_with_escapes_and_the_first_of_a_repeated_key() {
    let text = desktop(
        "Files",
        "files %U",
        "GenericName=File Manager\nComment=one\\ntwo\nIcon=folder\nCategories=System;FileManager;\n\
         Keywords=folder;disk;\nPath=/tmp\nStartupWMClass=Files\nTerminal=false\nTryExec=files\n\
         Name=Second",
    );
    let e = parse(&text).unwrap();
    assert_eq!(e.id, "a.desktop");
    assert_eq!(e.name, "Files");
    assert_eq!(e.generic_name, "File Manager");
    assert_eq!(e.comment, "one\ntwo");
    assert_eq!(e.icon.as_deref(), Some("folder"));
    assert_eq!(e.exec, "files %U");
    assert_eq!(e.try_exec.as_deref(), Some("files"));
    assert_eq!(e.categories, ["System", "FileManager"]);
    assert_eq!(e.keywords, ["folder", "disk"]);
    assert_eq!(e.path.as_deref(), Some(Path::new("/tmp")));
    assert_eq!(e.startup_wm_class.as_deref(), Some("Files"));
    assert!(!e.terminal);
    assert!(e.source.is_none());
    assert_eq!(parse(&desktop("A\\sB", "x", "")).unwrap().name, "A B");

    let other_group = "# comment\n\u{feff}[Desktop Entry]\nType=Application\nName=First\nExec=x\n\
                       [Desktop Action a]\nName=Other\nExec=y\n";
    let e = parse(other_group).unwrap();
    assert_eq!((e.name.as_str(), e.exec.as_str()), ("First", "x"));
}

#[test]
fn a_localised_key_uses_the_most_specific_match_and_falls_back_to_the_plain_key() {
    let text = desktop(
        "Files",
        "files",
        "Name[de]=Dateien\nName[de_AT]=Dateien AT\nKeywords[de]=Ordner;",
    );
    for (locale, name, keywords) in [
        ("de_AT.UTF-8", "Dateien AT", &["Ordner"][..]),
        ("de_DE", "Dateien", &["Ordner"]),
        ("fr_FR", "Files", &[]),
    ] {
        let e = parse_desktop_entry("a", &text, &Locale::parse(locale), &vis()).unwrap();
        assert_eq!(e.name, name, "{locale}");
        assert_eq!(e.keywords, keywords, "{locale}");
    }
}

#[test]
fn actions_are_read_in_listed_order_and_incomplete_ones_left_out() {
    let text = format!(
        "{}[Desktop Action second]\nName=Second\nExec=app --second\nIcon=two\n\
         [Desktop Action first]\nName=First\nExec=app --first\n\
         [Desktop Action noexec]\nName=No\n[Desktop Action noname]\nExec=x\n",
        desktop("App", "app", "Actions=first;second;missing;noexec;noname;")
    );
    let e = parse(&text).unwrap();
    let ids: Vec<&str> = e.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["first", "second"]);
    assert_eq!(e.actions[1].icon.as_deref(), Some("two"));
    assert_eq!(e.actions[0].exec, "app --first");
}

#[test]
fn an_entry_is_hidden_for_the_reason_its_keys_give() {
    for (text, expected) in [
        (
            "[Other]\nName=x\nExec=y\n".to_string(),
            Err(Hidden::NotApplication),
        ),
        (String::new(), Err(Hidden::NotApplication)),
        (
            "[Desktop Entry]\nType=Link\nName=x\nExec=y\n".into(),
            Err(Hidden::NotApplication),
        ),
        (
            "[Desktop Entry]\nName=x\nExec=y\n".into(),
            Err(Hidden::NotApplication),
        ),
        (
            "[Desktop Entry]\nType=Application\nExec=y\n".into(),
            Err(Hidden::NotApplication),
        ),
        (
            "[Desktop Entry]\nType=Application\nName=  \nExec=y\n".into(),
            Err(Hidden::NotApplication),
        ),
        (
            "[Desktop Entry]\nType=Application\nName=x\n".into(),
            Err(Hidden::NotApplication),
        ),
        (desktop("A", "a", "NoDisplay=true"), Err(Hidden::NoDisplay)),
        (desktop("A", "a", "Hidden=true"), Err(Hidden::HiddenKey)),
        (desktop("A", "a", "NoDisplay=false\nHidden=false"), Ok(())),
        (
            desktop("A", "a", "OnlyShowIn=GNOME;KDE;"),
            Err(Hidden::OnlyShowIn),
        ),
        (desktop("A", "a", "OnlyShowIn=GNOME;Cupertino;"), Ok(())),
        (
            desktop("A", "a", "NotShowIn=Cupertino;"),
            Err(Hidden::NotShowIn),
        ),
        (desktop("A", "a", "NotShowIn=GNOME;"), Ok(())),
    ] {
        assert_eq!(parse(&text).map(|_| ()), expected, "{text:?}");
    }
}

#[test]
fn try_exec_hides_an_entry_whose_program_is_missing_or_not_executable() {
    let dir = tempfile::tempdir().unwrap();
    let tool = executable(dir.path(), "bin/tool", "#!/bin/sh\n");
    write(dir.path(), "bin/plain", "data");
    let visibility = checked(dir.path());
    for (try_exec, expected) in [
        ("tool".to_string(), Ok(())),
        (tool.display().to_string(), Ok(())),
        ("absent".into(), Err(Hidden::TryExec)),
        ("/nonexistent/tool".into(), Err(Hidden::TryExec)),
        ("plain".into(), Err(Hidden::TryExec)),
    ] {
        let text = desktop("A", "a", &format!("TryExec={try_exec}"));
        let got = parse_desktop_entry("a", &text, &Locale::default(), &visibility).map(|_| ());
        assert_eq!(got, expected, "{try_exec}");
    }
}

#[test]
fn a_handler_keeps_its_mime_types_and_ignores_display_keys_but_not_hidden() {
    let text = desktop("A", "a %f", "MimeType=text/plain;image/png;");
    let h = parse_handler("a", &text, &Locale::default(), &vis()).unwrap();
    assert_eq!(h.mime_types, ["text/plain", "image/png"]);
    assert_eq!(h.entry.name, "A");

    let shown = desktop(
        "A",
        "a",
        "NoDisplay=true\nOnlyShowIn=GNOME;\nNotShowIn=Cupertino;",
    );
    assert!(parse_handler("a", &shown, &Locale::default(), &vis()).is_ok());
    let hidden = desktop("A", "a", "Hidden=true");
    assert_eq!(
        parse_handler("a", &hidden, &Locale::default(), &vis()).map(|_| ()),
        Err(Hidden::HiddenKey)
    );
}

#[test]
fn dbus_activatable_reads_the_main_group_of_the_text_or_file() {
    for (text, expected) in [
        ("[Desktop Entry]\nDBusActivatable=true\n", true),
        ("[Desktop Entry]\nDBusActivatable=false\n", false),
        ("[Desktop Entry]\nName=x\n", false),
        ("[Desktop Action a]\nDBusActivatable=true\n", false),
    ] {
        assert_eq!(dbus_activatable(text), expected, "{text:?}");
    }
    let dir = tempfile::tempdir().unwrap();
    assert!(!dbus_activatable_file(&dir.path().join("none.desktop")));
    let path = write(
        dir.path(),
        "a.desktop",
        "[Desktop Entry]\nDBusActivatable=true\n",
    );
    assert!(dbus_activatable_file(&path));
}

#[test]
fn indexing_gives_the_first_directory_precedence_even_for_a_hidden_entry() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    write(&first, "a.desktop", &desktop("From one", "a", ""));
    write(&second, "a.desktop", &desktop("From two", "a", ""));
    write(&second, "b.desktop", &desktop("Only two", "b", ""));
    write(&first, "c.desktop", &desktop("C", "c", "Hidden=true"));
    write(&second, "c.desktop", &desktop("C again", "c", ""));
    let entries = index_dirs(&[first.clone(), second], &Locale::default(), &vis());
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["From one", "Only two"]);
    assert_eq!(entries[0].source, Some(first.join("a.desktop")));
}

#[test]
fn indexing_walks_folders_in_name_order_joining_subfolders_into_the_id() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "c.desktop", &desktop("C", "c", ""));
    write(dir.path(), "a.desktop", &desktop("A", "a", ""));
    write(
        dir.path(),
        "kde/org.app.desktop",
        &desktop("Nested", "a", ""),
    );
    write(
        dir.path(),
        "notes.txt",
        &desktop("Not a desktop file", "a", ""),
    );
    let dirs = [dir.path().join("missing"), dir.path().to_path_buf()];
    let entries = index_dirs(&dirs, &Locale::default(), &vis());
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["a.desktop", "c.desktop", "kde-org.app.desktop"]);
}

#[test]
fn only_graphical_entries_with_a_findable_program_are_listed_applications() {
    let dir = tempfile::tempdir().unwrap();
    executable(dir.path(), "bin/real", "#!/bin/sh\n");
    let strict = checked(dir.path());
    for (exec, extra, visibility, expected) in [
        ("a", "Terminal=true", vis(), Err(Hidden::Terminal)),
        (
            "a",
            "Categories=System;ConsoleOnly;",
            vis(),
            Err(Hidden::Terminal),
        ),
        ("absent", "", vis(), Ok(())),
        ("real %f", "", strict.clone(), Ok(())),
        ("absent", "", strict.clone(), Err(Hidden::NoProgram)),
        ("\"unbalanced", "", strict.clone(), Err(Hidden::NoProgram)),
        ("%f", "", strict.clone(), Err(Hidden::NoProgram)),
    ] {
        let e = common::entry("a", &desktop("A", exec, extra));
        assert_eq!(
            listed_application(&e, &visibility),
            expected,
            "{exec} {extra}"
        );
    }

    write(dir.path(), "apps/app.desktop", &desktop("App", "app", ""));
    write(
        dir.path(),
        "apps/cli.desktop",
        &desktop("Cli", "cli", "Terminal=true"),
    );
    write(
        dir.path(),
        "apps/hid.desktop",
        &desktop("Hid", "hid", "NoDisplay=true"),
    );
    let apps = index_applications(&[dir.path().join("apps")], &Locale::default(), &vis());
    let ids: Vec<&str> = apps.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["app.desktop"]);
}
