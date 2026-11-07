use super::*;

fn entry(id: &str, exec: &str) -> DesktopEntry {
    DesktopEntry {
        id: id.into(),
        name: id.trim_end_matches(".desktop").into(),
        generic_name: String::new(),
        comment: String::new(),
        keywords: Vec::new(),
        categories: Vec::new(),
        icon: None,
        exec: exec.into(),
        try_exec: None,
        terminal: false,
        path: None,
        startup_wm_class: None,
        actions: Vec::new(),
        source: None,
    }
}

fn installed() -> Applications {
    let mut browser = entry("org.example.Browser.desktop", "browser %u");
    browser.startup_wm_class = Some("Navigator".into());
    let mut top = entry("top.desktop", "top");
    top.terminal = true;
    Applications::new(vec![
        entry("org.example.Editor.desktop", "editor --new-window %F"),
        browser,
        entry("foot.desktop", "foot"),
        entry("cupertino-files.desktop", "cupertino-files %U"),
        entry("cupertino-dock.desktop", "cupertino-dock"),
        entry("dock-wrapper.desktop", "/usr/bin/cupertino-menubar --flag"),
        top,
        entry("broken.desktop", "\"unterminated"),
    ])
}

fn mark(session: &str) -> SessionMark {
    SessionMark::new("2f6e1d0a-boot", session).unwrap()
}

#[test]
fn the_state_file_round_trips_and_holds_ids_only() {
    let state = SavedState {
        reopen: true,
        saved_in: Some(mark("3")),
        apps: vec!["foot.desktop".into(), "org.example.Editor.desktop".into()],
    };
    let text = to_text(&state);
    assert_eq!(
        text,
        "# Applications to reopen at the next login, written by cupertino-login.\nversion = 1\nreopen = true\n\
         boot = 2f6e1d0a-boot\nsession = 3\napp = foot.desktop\napp = org.example.Editor.desktop\n"
    );
    assert_eq!(parse(&text), (state, Vec::new()));
}

#[test]
fn a_missing_or_empty_file_is_the_default_choice_on_and_no_applications() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino").join(STATE_NAME);
    assert_eq!(load(&path).unwrap(), (SavedState::default(), Vec::new()));
    assert!(SavedState::default().reopen, "reopening is on by default");
    assert!(choice(&path));
    assert_eq!(parse("").0, SavedState::default());
}

#[test]
fn malformed_lines_are_reported_and_the_rest_applies() {
    let (state, problems) = parse(
        "version = 1\nreopen = maybe\nreopen = false\nnonsense\napp = ../../bin/sh\napp = sh -c 'x'.desktop\n\
         app = foot.desktop\napp = foot.desktop\ncolour = red\nboot = a b\n",
    );
    assert_eq!(
        state,
        SavedState {
            reopen: false,
            saved_in: None,
            apps: vec!["foot.desktop".into()],
        }
    );
    let lines: Vec<usize> = problems.iter().map(|p| p.line).collect();
    assert_eq!(lines, [2, 4, 5, 6, 9, 10]);
}

#[test]
fn another_format_version_keeps_the_choice_but_reads_no_applications() {
    let (state, problems) = parse("version = 2\nreopen = false\napp = foot.desktop\n");
    assert_eq!((state.reopen, state.apps.len()), (false, 0));
    assert_eq!(problems.len(), 1);
    let (state, _) = parse("reopen = true\napp = foot.desktop\n");
    assert!(
        state.apps.is_empty(),
        "a file without a version is not trusted"
    );
}

#[test]
fn at_most_the_cap_of_applications_is_read_and_written() {
    let mut text = String::from("version = 1\n");
    for i in 0..MAX_APPS + 5 {
        text.push_str(&format!("app = app{i}.desktop\n"));
    }
    let (state, problems) = parse(&text);
    assert_eq!((state.apps.len(), problems.len()), (MAX_APPS, 5));
    let written = to_text(&SavedState {
        apps: (0..MAX_APPS + 5)
            .map(|i| format!("app{i}.desktop"))
            .collect(),
        saved_in: Some(mark("1")),
        ..SavedState::default()
    });
    assert_eq!(written.matches("app = ").count(), MAX_APPS);
}

#[test]
fn desktop_ids_are_checked_strictly() {
    for good in [
        "foot.desktop",
        "org.example.App.desktop",
        "kde-org_x.desktop",
    ] {
        assert!(valid_desktop_id(good), "{good}");
    }
    let long = format!("{}.desktop", "a".repeat(MAX_ID_BYTES));
    for bad in [
        "",
        ".desktop",
        "foot",
        "/usr/share/applications/foot.desktop",
        "../foot.desktop",
        ".hidden.desktop",
        "foot .desktop",
        "foot;rm.desktop",
        "føø.desktop",
        long.as_str(),
    ] {
        assert!(!valid_desktop_id(bad), "{bad:?}");
    }
}

#[test]
fn the_state_path_follows_the_xdg_state_directory() {
    assert_eq!(
        state_path(Some("/state".into()), Some("/home/u".into())),
        Some(PathBuf::from("/state/cupertino/restore"))
    );
    assert_eq!(
        state_path(None, Some("/home/u".into())),
        Some(PathBuf::from("/home/u/.local/state/cupertino/restore"))
    );
    assert_eq!(
        state_path(Some("relative".into()), Some("/home/u".into())),
        Some(PathBuf::from("/home/u/.local/state/cupertino/restore")),
        "a relative XDG_STATE_HOME is ignored"
    );
    assert_eq!(state_path(None, None), None);
}

#[test]
fn identities_resolve_by_desktop_id_then_startup_wm_class_ignoring_case() {
    let apps = installed();
    let id = |identity: &str| apps.restorable(identity).map(|e| e.id.clone());
    assert_eq!(
        id("org.example.Editor"),
        Ok("org.example.Editor.desktop".into())
    );
    assert_eq!(
        id("ORG.EXAMPLE.EDITOR.desktop"),
        Ok("org.example.Editor.desktop".into())
    );
    assert_eq!(id("navigator"), Ok("org.example.Browser.desktop".into()));
    assert_eq!(id("cupertino-files"), Ok("cupertino-files.desktop".into()));
    assert_eq!(id("org.example.Unknown"), Err(Skip::NoEntry));
    assert_eq!(id(""), Err(Skip::NoEntry));
}

#[test]
fn shell_programs_terminal_entries_and_unusable_exec_lines_are_never_restorable() {
    let apps = installed();
    assert_eq!(apps.restorable("cupertino-dock").err(), Some(Skip::Shell));
    assert_eq!(
        apps.restorable("cupertino-login").err(),
        Some(Skip::Shell),
        "a shell identity is refused even without an entry"
    );
    assert_eq!(
        apps.restorable("dock-wrapper").err(),
        Some(Skip::Shell),
        "an entry that runs a shell program"
    );
    assert_eq!(apps.restorable("top").err(), Some(Skip::Terminal));
    assert_eq!(apps.restorable("broken").err(), Some(Skip::Exec));
    assert_eq!(
        apps.by_id("cupertino-dock.desktop").err(),
        Some(Skip::Shell)
    );
}

#[test]
fn the_saved_list_keeps_the_quit_order_and_each_entry_once() {
    let (saved, skipped) = saved_list(
        [
            "foot",
            "org.example.Unknown",
            "Navigator",
            "org.example.Browser",
            "cupertino-menubar",
            "FOOT",
        ],
        &installed(),
    );
    assert_eq!(saved, ["foot.desktop", "org.example.Browser.desktop"]);
    assert_eq!(
        skipped,
        [
            ("org.example.Unknown".to_string(), Skip::NoEntry),
            ("org.example.Browser".to_string(), Skip::Duplicate),
            ("cupertino-menubar".to_string(), Skip::Shell),
            ("FOOT".to_string(), Skip::Duplicate),
        ]
    );
}

#[test]
fn saving_with_the_choice_off_keeps_the_choice_and_no_list() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino").join(STATE_NAME);
    save(&path, false, vec!["foot.desktop".into()], Some(mark("4"))).unwrap();
    assert_eq!(
        load(&path).unwrap().0,
        SavedState {
            reopen: false,
            saved_in: None,
            apps: Vec::new()
        }
    );
    assert!(!choice(&path));
    save(&path, true, vec!["foot.desktop".into()], None).unwrap();
    assert!(
        load(&path).unwrap().0.apps.is_empty(),
        "a list whose session is unknown is not kept"
    );
}

#[test]
fn the_state_file_is_private_to_its_owner() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino").join(STATE_NAME);
    save(&path, true, vec!["foot.desktop".into()], Some(mark("4"))).unwrap();
    let mode = std::fs::metadata(&path).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o600);
}

#[test]
fn the_list_is_emptied_on_disk_before_it_is_handed_out_and_only_once() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino").join(STATE_NAME);
    let apps = vec![
        "foot.desktop".to_string(),
        "org.example.Editor.desktop".into(),
    ];
    save(&path, true, apps.clone(), Some(mark("4"))).unwrap();
    assert_eq!(
        take_at_login(&path, Some(&mark("7"))).unwrap(),
        Taken::Start(apps)
    );
    let (left, _) = load(&path).unwrap();
    assert_eq!(
        left,
        SavedState {
            reopen: true,
            saved_in: None,
            apps: Vec::new()
        },
        "the choice stays, the list is gone"
    );
    assert_eq!(
        take_at_login(&path, Some(&mark("7"))).unwrap(),
        Taken::Nothing,
        "a second start in the same session finds nothing"
    );
}

#[test]
fn a_list_saved_in_this_very_session_or_with_the_choice_off_is_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino").join(STATE_NAME);
    save(&path, true, vec!["foot.desktop".into()], Some(mark("4"))).unwrap();
    assert_eq!(
        take_at_login(&path, Some(&mark("4"))).unwrap(),
        Taken::SameSession
    );
    assert!(load(&path).unwrap().0.apps.is_empty());

    // a session with the same id after a restart is another session
    let other_boot = SessionMark::new("other-boot", "4").unwrap();
    save(&path, true, vec!["foot.desktop".into()], Some(mark("4"))).unwrap();
    assert!(matches!(
        take_at_login(&path, Some(&other_boot)).unwrap(),
        Taken::Start(_)
    ));

    // a hand-edited file with the choice off and a list
    std::fs::write(
        &path,
        "version = 1\nreopen = false\nboot = b\nsession = 1\napp = foot.desktop\n",
    )
    .unwrap();
    assert_eq!(
        take_at_login(&path, Some(&mark("9"))).unwrap(),
        Taken::Disabled
    );
    assert!(load(&path).unwrap().0.apps.is_empty());
    assert!(!choice(&path), "the choice is kept");
}

#[test]
fn a_list_that_cannot_be_emptied_is_not_handed_out() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let state_dir = dir.path().join("cupertino");
    let path = state_dir.join(STATE_NAME);
    save(&path, true, vec!["foot.desktop".into()], Some(mark("4"))).unwrap();
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o500)).unwrap();
    let taken = take_at_login(&path, Some(&mark("5")));
    std::fs::set_permissions(&state_dir, std::fs::Permissions::from_mode(0o700)).unwrap();
    // root ignores the directory's mode; then the list is emptied as usual
    if let Ok(taken) = taken {
        assert!(matches!(taken, Taken::Start(_)));
        assert!(load(&path).unwrap().0.apps.is_empty());
    }
}

#[test]
fn discarding_keeps_the_choice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cupertino").join(STATE_NAME);
    discard(&path).unwrap();
    assert!(!path.exists(), "nothing to discard writes nothing");
    save(&path, true, vec!["foot.desktop".into()], Some(mark("4"))).unwrap();
    discard(&path).unwrap();
    assert_eq!(
        load(&path).unwrap().0,
        SavedState {
            reopen: true,
            saved_in: None,
            apps: Vec::new()
        }
    );
}

#[test]
fn launching_starts_each_installed_entry_once_through_its_exec_line() {
    let apps = installed();
    let ids: Vec<String> = [
        "org.example.Editor.desktop",
        "removed.desktop",
        "foot.desktop",
        "org.example.Editor.desktop",
        "cupertino-dock.desktop",
        "org.example.Browser.desktop",
        "top.desktop",
    ]
    .map(String::from)
    .to_vec();
    let login_items: HashSet<String> = ["foot.desktop".to_string()].into();
    let mut commands = Vec::new();
    let report = launch(&ids, &apps, &login_items, |entry, command| {
        commands.push((entry.id.clone(), command.argv().join(" ")));
        entry.id != "org.example.Browser.desktop"
    });
    assert_eq!(
        commands,
        [
            (
                "org.example.Editor.desktop".to_string(),
                "editor --new-window".to_string()
            ),
            (
                "org.example.Browser.desktop".to_string(),
                "browser".to_string()
            ),
        ],
        "the field codes for files drop out"
    );
    assert_eq!(report.started, ["org.example.Editor.desktop"]);
    assert_eq!(
        report.skipped,
        [
            ("removed.desktop".to_string(), Skip::NoEntry),
            ("foot.desktop".to_string(), Skip::LoginItem),
            ("org.example.Editor.desktop".to_string(), Skip::Duplicate),
            ("cupertino-dock.desktop".to_string(), Skip::Shell),
            ("org.example.Browser.desktop".to_string(), Skip::Failed),
            ("top.desktop".to_string(), Skip::Terminal),
        ]
    );
}

#[test]
fn a_saved_id_is_only_looked_up_never_run() {
    let apps = installed();
    let ids = vec!["sh.desktop".to_string(), "../../bin/sh".to_string()];
    let report = launch(&ids, &apps, &HashSet::new(), |_, _| {
        panic!("nothing is started for an ID without an installed entry")
    });
    assert_eq!(
        report.skipped,
        [
            ("sh.desktop".to_string(), Skip::NoEntry),
            ("../../bin/sh".to_string(), Skip::Invalid),
        ]
    );
}
