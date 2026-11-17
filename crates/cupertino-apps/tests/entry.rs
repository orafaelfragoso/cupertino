//! Desktop entries: locale fallback, list values, visibility, handlers and indexing.

mod common;

use common::{desktop, executable, vis, write};
use cupertino_apps::entry::{
    dbus_activatable, dbus_activatable_file, index_applications, index_dirs, listed_application,
    parse_desktop_entry, parse_handler, split_list, Hidden, Locale, Visibility,
};

fn parse(text: &str) -> Result<cupertino_apps::DesktopEntry, Hidden> {
    parse_desktop_entry("a.desktop", text, &Locale::default(), &vis())
}

#[test]
fn a_locale_name_lists_its_keys_from_the_most_to_the_least_specific() {
    assert_eq!(
        Locale::parse("de_AT.UTF-8@euro").candidates,
        ["de_AT@euro", "de_AT", "de@euro", "de"]
    );
    assert_eq!(Locale::parse("en_US.UTF-8").candidates, ["en_US", "en"]);
    assert_eq!(Locale::parse("fr").candidates, ["fr"]);
}

#[test]
fn the_c_and_posix_locales_and_an_empty_name_have_no_keys() {
    for name in ["C", "POSIX", "", "  "] {
        assert!(Locale::parse(name).candidates.is_empty(), "{name:?}");
    }
}

#[test]
fn a_list_splits_on_semicolons_and_keeps_escaped_ones() {
    assert_eq!(split_list("a;b;c"), ["a", "b", "c"]);
    assert_eq!(split_list("a;b\\;c;d;"), ["a", "b;c", "d"]);
}

#[test]
fn a_list_drops_empty_items() {
    assert_eq!(split_list(";;a;;b;;"), ["a", "b"]);
    assert!(split_list("").is_empty());
}

#[test]
fn an_entry_reads_its_main_keys() {
    let text = desktop(
        "Files",
        "files %U",
        "GenericName=File Manager\nComment=Browse\nIcon=folder\nCategories=System;FileManager;\n\
         Keywords=folder;disk;\nPath=/tmp\nStartupWMClass=Files\nTerminal=false\nTryExec=files",
    );
    let e = parse(&text).unwrap();
    assert_eq!(e.id, "a.desktop");
    assert_eq!(e.name, "Files");
    assert_eq!(e.generic_name, "File Manager");
    assert_eq!(e.comment, "Browse");
    assert_eq!(e.icon.as_deref(), Some("folder"));
    assert_eq!(e.exec, "files %U");
    assert_eq!(e.try_exec.as_deref(), Some("files"));
    assert_eq!(e.categories, ["System", "FileManager"]);
    assert_eq!(e.keywords, ["folder", "disk"]);
    assert_eq!(e.path.as_deref(), Some(std::path::Path::new("/tmp")));
    assert_eq!(e.startup_wm_class.as_deref(), Some("Files"));
    assert!(!e.terminal);
    assert!(e.source.is_none());
}

#[test]
fn a_localised_key_uses_the_most_specific_match_and_falls_back_to_the_plain_key() {
    let text = desktop(
        "Files",
        "files",
        "Name[de]=Dateien\nName[de_AT]=Dateien AT\nKeywords[de]=Ordner;",
    );
    let at = parse_desktop_entry("a", &text, &Locale::parse("de_AT.UTF-8"), &vis()).unwrap();
    assert_eq!(at.name, "Dateien AT");
    let de = parse_desktop_entry("a", &text, &Locale::parse("de_DE"), &vis()).unwrap();
    assert_eq!(de.name, "Dateien");
    assert_eq!(de.keywords, ["Ordner"]);
    let fr = parse_desktop_entry("a", &text, &Locale::parse("fr_FR"), &vis()).unwrap();
    assert_eq!(fr.name, "Files");
    assert!(fr.keywords.is_empty());
}

#[test]
fn escapes_in_values_are_resolved() {
    let e = parse(&desktop("A\\sB", "x", "Comment=one\\ntwo")).unwrap();
    assert_eq!(e.name, "A B");
    assert_eq!(e.comment, "one\ntwo");
}

#[test]
fn the_first_of_a_repeated_key_wins_and_other_groups_do_not_leak_in() {
    let text =
        "# comment\n\u{feff}[Desktop Entry]\nType=Application\nName=First\nName=Second\nExec=x\n\
                [Desktop Action a]\nName=Other\nExec=y\n";
    let e = parse(text).unwrap();
    assert_eq!(e.name, "First");
    assert_eq!(e.exec, "x");
}

#[test]
fn actions_are_read_in_the_order_the_entry_lists_them() {
    let text = format!(
        "{}[Desktop Action second]\nName=Second\nExec=app --second\nIcon=two\n\
         [Desktop Action first]\nName=First\nExec=app --first\n",
        desktop("App", "app", "Actions=first;second;missing;")
    );
    let e = parse(&text).unwrap();
    let ids: Vec<&str> = e.actions.iter().map(|a| a.id.as_str()).collect();
    assert_eq!(ids, ["first", "second"]);
    assert_eq!(e.actions[1].icon.as_deref(), Some("two"));
    assert_eq!(e.actions[0].exec, "app --first");
}

#[test]
fn an_action_without_a_name_or_an_exec_is_left_out() {
    let text = format!(
        "{}[Desktop Action noexec]\nName=No\n[Desktop Action noname]\nExec=x\n",
        desktop("App", "app", "Actions=noexec;noname;")
    );
    assert!(parse(&text).unwrap().actions.is_empty());
}

#[test]
fn a_file_without_the_main_group_is_not_an_application() {
    assert_eq!(
        parse("[Other]\nName=x\nExec=y\n"),
        Err(Hidden::NotApplication)
    );
    assert_eq!(parse(""), Err(Hidden::NotApplication));
}

#[test]
fn only_type_application_is_an_application() {
    let link = "[Desktop Entry]\nType=Link\nName=x\nExec=y\n";
    assert_eq!(parse(link), Err(Hidden::NotApplication));
    let none = "[Desktop Entry]\nName=x\nExec=y\n";
    assert_eq!(parse(none), Err(Hidden::NotApplication));
}

#[test]
fn an_entry_needs_a_name_and_an_exec() {
    let no_name = "[Desktop Entry]\nType=Application\nExec=y\n";
    assert_eq!(parse(no_name), Err(Hidden::NotApplication));
    let blank_name = "[Desktop Entry]\nType=Application\nName=  \nExec=y\n";
    assert_eq!(parse(blank_name), Err(Hidden::NotApplication));
    let no_exec = "[Desktop Entry]\nType=Application\nName=x\n";
    assert_eq!(parse(no_exec), Err(Hidden::NotApplication));
}

#[test]
fn no_display_and_hidden_remove_an_entry() {
    assert_eq!(
        parse(&desktop("A", "a", "NoDisplay=true")),
        Err(Hidden::NoDisplay)
    );
    assert_eq!(
        parse(&desktop("A", "a", "Hidden=true")),
        Err(Hidden::HiddenKey)
    );
    assert!(parse(&desktop("A", "a", "NoDisplay=false\nHidden=false")).is_ok());
}

#[test]
fn only_show_in_needs_one_of_the_current_desktops() {
    assert_eq!(
        parse(&desktop("A", "a", "OnlyShowIn=GNOME;KDE;")),
        Err(Hidden::OnlyShowIn)
    );
    assert!(parse(&desktop("A", "a", "OnlyShowIn=GNOME;Cupertino;")).is_ok());
}

#[test]
fn not_show_in_hides_on_a_listed_desktop() {
    assert_eq!(
        parse(&desktop("A", "a", "NotShowIn=Cupertino;")),
        Err(Hidden::NotShowIn)
    );
    assert!(parse(&desktop("A", "a", "NotShowIn=GNOME;")).is_ok());
}

#[test]
fn try_exec_hides_an_entry_whose_program_is_missing() {
    let dir = tempfile::tempdir().unwrap();
    let tool = executable(dir.path(), "bin/tool", "#!/bin/sh\n");
    let checked = Visibility {
        skip_try_exec: false,
        path_dirs: vec![dir.path().join("bin")],
        ..vis()
    };
    let parse_with = |extra: &str| {
        parse_desktop_entry("a", &desktop("A", "a", extra), &Locale::default(), &checked)
    };
    assert!(parse_with("TryExec=tool").is_ok());
    assert!(parse_with(&format!("TryExec={}", tool.display())).is_ok());
    assert_eq!(parse_with("TryExec=absent"), Err(Hidden::TryExec));
    assert_eq!(
        parse_with("TryExec=/nonexistent/tool"),
        Err(Hidden::TryExec)
    );
}

#[test]
fn try_exec_counts_a_file_that_is_not_executable_as_missing() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "bin/plain", "data");
    let checked = Visibility {
        skip_try_exec: false,
        path_dirs: vec![dir.path().join("bin")],
        ..vis()
    };
    let text = desktop("A", "a", "TryExec=plain");
    assert_eq!(
        parse_desktop_entry("a", &text, &Locale::default(), &checked),
        Err(Hidden::TryExec)
    );
}

#[test]
fn a_handler_keeps_its_mime_types_in_file_order() {
    let text = desktop("A", "a %f", "MimeType=text/plain;image/png;");
    let h = parse_handler("a", &text, &Locale::default(), &vis()).unwrap();
    assert_eq!(h.mime_types, ["text/plain", "image/png"]);
    assert_eq!(h.entry.name, "A");
}

#[test]
fn a_handler_ignores_no_display_and_the_desktop_lists_but_not_hidden() {
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
fn dbus_activatable_reads_the_main_group_only() {
    assert!(dbus_activatable("[Desktop Entry]\nDBusActivatable=true\n"));
    assert!(!dbus_activatable(
        "[Desktop Entry]\nDBusActivatable=false\n"
    ));
    assert!(!dbus_activatable("[Desktop Entry]\nName=x\n"));
    assert!(!dbus_activatable(
        "[Desktop Action a]\nDBusActivatable=true\n"
    ));
}

#[test]
fn dbus_activatable_file_is_false_for_a_missing_file() {
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
fn indexing_gives_the_first_directory_precedence_for_an_id() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    write(&first, "a.desktop", &desktop("From one", "a", ""));
    write(&second, "a.desktop", &desktop("From two", "a", ""));
    write(&second, "b.desktop", &desktop("Only two", "b", ""));
    let entries = index_dirs(&[first.clone(), second], &Locale::default(), &vis());
    let names: Vec<&str> = entries.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["From one", "Only two"]);
    assert_eq!(entries[0].source, Some(first.join("a.desktop")));
}

#[test]
fn a_hidden_entry_still_shadows_the_same_id_further_down() {
    let dir = tempfile::tempdir().unwrap();
    let (first, second) = (dir.path().join("one"), dir.path().join("two"));
    write(&first, "a.desktop", &desktop("A", "a", "Hidden=true"));
    write(&second, "a.desktop", &desktop("A again", "a", ""));
    assert!(index_dirs(&[first, second], &Locale::default(), &vis()).is_empty());
}

#[test]
fn indexing_joins_subfolders_into_the_id_and_skips_other_files() {
    let dir = tempfile::tempdir().unwrap();
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
    let entries = index_dirs(&[dir.path().to_path_buf()], &Locale::default(), &vis());
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].id, "kde-org.app.desktop");
}

#[test]
fn indexing_lists_files_in_name_order_and_skips_missing_folders() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "c.desktop", &desktop("C", "c", ""));
    write(dir.path(), "a.desktop", &desktop("A", "a", ""));
    write(dir.path(), "b.desktop", &desktop("B", "b", ""));
    let dirs = [dir.path().join("missing"), dir.path().to_path_buf()];
    let entries = index_dirs(&dirs, &Locale::default(), &vis());
    let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["a.desktop", "b.desktop", "c.desktop"]);
}

#[test]
fn a_command_line_program_is_not_a_listed_application() {
    let terminal = common::entry("a", &desktop("A", "a", "Terminal=true"));
    assert_eq!(listed_application(&terminal, &vis()), Err(Hidden::Terminal));
    let console = common::entry("a", &desktop("A", "a", "Categories=System;ConsoleOnly;"));
    assert_eq!(listed_application(&console, &vis()), Err(Hidden::Terminal));
}

#[test]
fn an_exec_without_a_findable_program_is_not_listed() {
    let dir = tempfile::tempdir().unwrap();
    executable(dir.path(), "bin/real", "#!/bin/sh\n");
    let checked = Visibility {
        skip_try_exec: false,
        path_dirs: vec![dir.path().join("bin")],
        ..vis()
    };
    let listed =
        |exec: &str| listed_application(&common::entry("a", &desktop("A", exec, "")), &checked);
    assert_eq!(listed("real %f"), Ok(()));
    assert_eq!(listed("absent"), Err(Hidden::NoProgram));
    assert_eq!(listed("\"unbalanced"), Err(Hidden::NoProgram));
    assert_eq!(listed("%f"), Err(Hidden::NoProgram));
}

#[test]
fn the_program_check_is_skipped_with_skip_try_exec() {
    let e = common::entry("a", &desktop("A", "absent", ""));
    assert_eq!(listed_application(&e, &vis()), Ok(()));
}

#[test]
fn applications_are_the_indexed_entries_that_may_be_listed() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "app.desktop", &desktop("App", "app", ""));
    write(
        dir.path(),
        "cli.desktop",
        &desktop("Cli", "cli", "Terminal=true"),
    );
    write(
        dir.path(),
        "hid.desktop",
        &desktop("Hid", "hid", "NoDisplay=true"),
    );
    let apps = index_applications(&[dir.path().to_path_buf()], &Locale::default(), &vis());
    let ids: Vec<&str> = apps.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(ids, ["app.desktop"]);
}
