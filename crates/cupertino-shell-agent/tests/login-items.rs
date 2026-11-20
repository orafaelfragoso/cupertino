//! The login items (XDG autostart) and the reopened applications: what is started, what is saved, and that a list is
//! never started twice.
#![expect(
    clippy::disallowed_methods,
    reason = "a test writes its fixtures off any UI thread; the crate's clippy.toml guards the component's loop"
)]

use cupertino_apps::entry::{parse_desktop_entry, DesktopEntry, Locale, Visibility};
use cupertino_shell_agent::autostart::{self, Environment, Fields, Skip};
use cupertino_shell_agent::restore::{self, Applications, SavedState, SessionMark, Taken};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

fn environment(desktops: &[&str]) -> Environment {
    Environment {
        desktops: desktops.iter().map(|d| d.to_string()).collect(),
        executable: Box::new(|name| name == "present"),
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join("shell-agent")
        .join(name);
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("scratch directory");
    dir
}

fn entry(id: &str, text: &str) -> DesktopEntry {
    let visibility = Visibility {
        current_desktops: vec!["Cupertino".into()],
        path_dirs: Vec::new(),
        skip_try_exec: true,
    };
    parse_desktop_entry(id, text, &Locale::default(), &visibility).expect("a listed entry")
}

#[test]
fn an_autostart_entry_runs_unless_its_keys_say_otherwise() {
    let env = environment(&["Cupertino"]);
    let path = Path::new("/etc/xdg/autostart/a.desktop");
    let item = autostart::parse(
        "a.desktop",
        path,
        "[Desktop Entry]\nType=Application\nExec=tool --flag %U\nPath=/srv\n",
        &env,
    )
    .expect("starts");
    assert_eq!(item.argv, ["tool", "--flag"]);
    assert_eq!(item.dir.as_deref(), Some(Path::new("/srv")));
    let skip = |text: &str| autostart::parse("a.desktop", path, text, &env).err();
    assert_eq!(
        skip("[Desktop Entry]\nType=Link\nExec=x\n"),
        Some(Skip::NotApplication)
    );
    assert_eq!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nHidden=true\n"),
        Some(Skip::Hidden)
    );
    assert_eq!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nOnlyShowIn=GNOME;\n"),
        Some(Skip::OnlyShowIn)
    );
    assert_eq!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nNotShowIn=Cupertino;\n"),
        Some(Skip::NotShowIn)
    );
    assert_eq!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nX-GNOME-Autostart-enabled=false\n"),
        Some(Skip::Disabled)
    );
    assert_eq!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nTerminal=true\n"),
        Some(Skip::Terminal)
    );
    assert!(matches!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nTryExec=absent\n"),
        Some(Skip::TryExec(_))
    ));
    assert_eq!(
        skip("[Desktop Entry]\nType=Application\nExec=x\nTryExec=present\n"),
        None
    );
}

#[test]
fn exec_field_codes_are_expanded_or_dropped() {
    let fields = Fields {
        icon: Some("app".into()),
        name: Some("App".into()),
        file: "/etc/xdg/autostart/app.desktop".into(),
    };
    assert_eq!(
        autostart::exec_argv("run %i %c %k %f 100%% \"two words\"", &fields).expect("splits"),
        [
            "run",
            "--icon",
            "app",
            "App",
            "/etc/xdg/autostart/app.desktop",
            "100%",
            "two words"
        ]
    );
    assert!(autostart::exec_argv("run \"open", &fields).is_err());
    assert!(autostart::exec_argv("run %z", &fields).is_err());
}

#[test]
fn a_user_entry_shadows_a_system_one_and_system_items_come_first() {
    let root = scratch("autostart-order");
    let (system, user) = (root.join("system"), root.join("user"));
    for dir in [&system, &user] {
        std::fs::create_dir_all(dir).expect("directory");
    }
    std::fs::write(
        system.join("b.desktop"),
        "[Desktop Entry]\nType=Application\nExec=system-b\n",
    )
    .expect("write");
    std::fs::write(
        system.join("shared.desktop"),
        "[Desktop Entry]\nType=Application\nExec=system\n",
    )
    .expect("write");
    std::fs::write(
        user.join("shared.desktop"),
        "[Desktop Entry]\nType=Application\nExec=user\n",
    )
    .expect("write");
    std::fs::write(
        user.join("a.desktop"),
        "[Desktop Entry]\nType=Application\nExec=user-a\n",
    )
    .expect("write");
    let found = autostart::list(&[user.clone(), system.clone()]).expect("listed");
    let ids: Vec<&str> = found.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["b.desktop", "a.desktop", "shared.desktop"]);
    let shared = found
        .iter()
        .find(|f| f.id == "shared.desktop")
        .expect("shared");
    assert!(
        shared.path.starts_with(&user),
        "the user's copy wins: {}",
        shared.path.display()
    );
}

#[test]
fn a_session_claims_its_login_items_once() {
    let runtime = scratch("claim");
    assert!(autostart::claim_run(&runtime, "c3").expect("claimed"));
    assert!(!autostart::claim_run(&runtime, "c3").expect("claimed already"));
    assert!(autostart::claim_run(&runtime, "c4").expect("another session"));
}

#[test]
fn the_state_file_round_trips_and_ignores_what_it_does_not_trust() {
    let state = SavedState {
        reopen: true,
        saved_in: SessionMark::new("boot-1", "c2"),
        apps: vec!["org.example.Editor.desktop".into(), "foot.desktop".into()],
    };
    let (read, problems) = restore::parse(&restore::to_text(&state));
    assert_eq!(read, state);
    assert!(problems.is_empty());

    let (read, problems) = restore::parse(
        "version = 1\nreopen = false\napp = /usr/bin/evil\napp = ok.desktop\napp = ok.desktop\n",
    );
    assert!(!read.reopen);
    assert_eq!(read.apps, ["ok.desktop"]);
    assert_eq!(problems.len(), 1);
    let (read, _) = restore::parse("version = 9\nreopen = false\napp = ok.desktop\n");
    assert!(!read.reopen && read.apps.is_empty());
}

#[test]
fn the_shell_and_hidden_or_terminal_entries_are_never_saved() {
    let apps = Applications::new(vec![
        entry("org.example.Editor.desktop", "[Desktop Entry]\nType=Application\nName=Editor\nExec=editor %F\nStartupWMClass=EditorWin\n"),
        entry("term-tool.desktop", "[Desktop Entry]\nType=Application\nName=Tool\nExec=tool\nTerminal=true\n"),
        entry("cupertino-shell.desktop", "[Desktop Entry]\nType=Application\nName=Shell\nExec=cupertino-shell\n"),
    ]);
    for program in [
        "cupertino-shell",
        "cupertino-shell-agent",
        "cupertino-server",
        "cupertino-login",
    ] {
        assert!(
            restore::SHELL_PROGRAMS.contains(&program),
            "{program} is the shell's"
        );
    }
    let (saved, skipped) = restore::saved_list(
        [
            "editorwin",
            "org.example.Editor",
            "term-tool",
            "cupertino-shell",
            "cupertino-shell-agent",
            "unknown",
        ],
        &apps,
    );
    assert_eq!(saved, ["org.example.Editor.desktop"]);
    let reasons: Vec<(&str, restore::Skip)> = skipped
        .iter()
        .map(|(id, skip)| (id.as_str(), skip.clone()))
        .collect();
    assert_eq!(
        reasons,
        [
            ("org.example.Editor", restore::Skip::Duplicate),
            ("term-tool", restore::Skip::Terminal),
            ("cupertino-shell", restore::Skip::Shell),
            ("cupertino-shell-agent", restore::Skip::Shell),
            ("unknown", restore::Skip::NoEntry),
        ]
    );
}

#[test]
fn a_saved_list_is_emptied_before_it_starts_and_never_twice() {
    let path = scratch("restore").join("restore");
    let mark = SessionMark::new("boot-1", "c2");
    restore::save(
        &path,
        true,
        vec!["a.desktop".into(), "b.desktop".into()],
        mark.clone(),
    )
    .expect("saved");
    // the agent restarted in the session that saved the list (a refused restart, then a failure): dropped
    assert_eq!(
        restore::take_at_login(&path, mark.as_ref()).expect("taken"),
        Taken::SameSession
    );
    let (left, _) = restore::load(&path).expect("read");
    assert!(left.apps.is_empty() && left.saved_in.is_none() && left.reopen);
    assert_eq!(
        restore::take_at_login(&path, mark.as_ref()).expect("taken"),
        Taken::Nothing
    );
}

#[test]
fn a_list_from_the_last_session_starts_once_skipping_login_items() {
    let path = scratch("restore-next").join("restore");
    restore::save(
        &path,
        true,
        vec!["a.desktop".into(), "b.desktop".into(), "a.desktop".into()],
        SessionMark::new("boot-1", "c2"),
    )
    .expect("saved");
    let next = SessionMark::new("boot-2", "c2");
    let Taken::Start(ids) = restore::take_at_login(&path, next.as_ref()).expect("taken") else {
        panic!("the last session's list starts");
    };
    assert_eq!(
        restore::take_at_login(&path, next.as_ref()).expect("taken"),
        Taken::Nothing
    );
    assert!(restore::choice(&path), "the choice stays");

    let apps = Applications::new(vec![
        entry(
            "a.desktop",
            "[Desktop Entry]\nType=Application\nName=A\nExec=a %U\n",
        ),
        entry(
            "b.desktop",
            "[Desktop Entry]\nType=Application\nName=B\nExec=b\n",
        ),
    ]);
    let login_items: HashSet<String> = ["b.desktop".to_string()].into();
    let mut started = Vec::new();
    let report = restore::launch(&ids, &apps, &login_items, |entry, command| {
        started.push((entry.id.clone(), command.program.clone()));
        true
    });
    assert_eq!(started, [("a.desktop".to_string(), "a".to_string())]);
    assert_eq!(report.started, ["a.desktop"]);
    assert_eq!(
        report.skipped,
        [("b.desktop".to_string(), restore::Skip::LoginItem)]
    );
}

#[test]
fn a_list_saved_with_the_choice_off_is_dropped() {
    let path = scratch("restore-off").join("restore");
    restore::store(
        &path,
        &SavedState {
            reopen: false,
            saved_in: SessionMark::new("boot-1", "c2"),
            apps: vec!["a.desktop".into()],
        },
    )
    .expect("stored");
    assert_eq!(
        restore::take_at_login(&path, None).expect("taken"),
        Taken::Disabled
    );
    assert!(!restore::choice(&path));
}
