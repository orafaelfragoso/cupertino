//! `login.conf` of the session manager.

use cupertino_defaults::login::*;

const COMMANDS: &str =
    "greeter-command = /usr/bin/greeter --flag\nsession-command = /usr/bin/session\n";

fn ok(text: &str) -> Loaded {
    LoginConfig::parse(&format!("{COMMANDS}{text}")).expect("a good file")
}

#[test]
fn every_key_is_read_and_comments_are_ignored() {
    let screen_keys =
        "lock-message = Back soon\nshow-full-name = true\nhost-info = ip\nshow-input-menu = true\n\
         hide-user-avatar-and-name = true\nlarge-clock = false\n";
    let l = ok(&format!(
        "# vt = 9\nvt = 7 # the seventh\ngreeter-user = greeter\nhidden-users = a, b\ndeny-list = c\n\
         allow-list = d,e\nhide-admin-users = true\nshow-other-users = false\nrestart-disabled = true\n\
         shut-down-disabled = true\nsleep-disabled = false\npower-off-disabled = true\n\
         retries-until-hint = 3\nauto-login = bob\nadmin-groups = wheel, admin\n{screen_keys}"
    ));
    assert!(
        l.problems.is_empty() && l.replaced.is_empty(),
        "{:?}",
        l.problems
    );
    let c = l.config;
    assert_eq!(c.greeter_command, ["/usr/bin/greeter", "--flag"]);
    assert_eq!(c.session_command, ["/usr/bin/session"]);
    assert_eq!(c.vt, 7);
    assert_eq!(c.greeter_user, "greeter");
    assert_eq!(c.hidden_users, ["a", "b"]);
    assert_eq!(c.deny_list, ["c"]);
    assert_eq!(c.allow_list.unwrap(), ["d", "e"]);
    assert!(c.hide_admin_users);
    assert_eq!(
        [
            c.show_other_users,
            c.restart_disabled,
            c.shut_down_disabled,
            c.sleep_disabled,
            c.power_off_disabled
        ],
        [Some(false), Some(true), Some(true), Some(false), Some(true)]
    );
    assert_eq!(c.retries_until_hint, 3);
    assert_eq!(c.auto_login.as_deref(), Some("bob"));
    assert_eq!(c.admin_groups.unwrap(), ["wheel", "admin"]);
    assert_eq!(
        c.screens,
        ScreenSettings::parse(screen_keys),
        "the manager and the screens read the screen keys alike"
    );
    assert_eq!(
        ok("allow-list =\n").config.allow_list,
        Some(vec![]),
        "an empty allow list lists nobody"
    );
}

#[test]
fn a_bad_line_is_reported_by_number_and_its_key_takes_its_safe_value() {
    let l = LoginConfig::parse(&format!(
        "{COMMANDS}vt = 99\nhide-admin-users = maybe\nrestart-disabled = x\nauto-login = a:b\nadmin-groups = a b\n\
         allow-list = a,,b\nretries-until-hint = lots\nbogus = 1\ngreeter-user =\n\
         show-other-users: false\nhide-admin-users = false\n\
         large-clock = maybe\nhost-info = mac\nlock-message = {}\nshow-full-name = 1\n",
        "x".repeat(MAX_LOCK_MESSAGE_CHARS + 1)
    ))
    .unwrap();
    assert_eq!(
        l.problems.iter().map(|p| p.0).collect::<Vec<_>>(),
        [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 14, 15, 16, 17]
    );
    for (index, expected) in [
        (0, "expected 1 to 63"),
        (1, "expected true or false"),
        (7, "unknown setting"),
        (9, "key = value"),
        (12, "at most 200 characters"),
    ] {
        assert!(
            l.problems[index].1.contains(expected),
            "problem {index}: {}",
            l.problems[index].1
        );
    }
    let c = l.config;
    assert_eq!(c.vt, 1);
    assert!(
        c.hide_admin_users,
        "fails closed, and a later good line does not undo it"
    );
    assert_eq!(c.restart_disabled, Some(true), "fails closed");
    assert_eq!(c.auto_login, None);
    assert_eq!(c.admin_groups, Some(vec![]), "no administrator");
    assert_eq!(c.allow_list, Some(vec![]));
    assert_eq!(c.show_other_users, Some(true));
    assert_eq!(c.retries_until_hint, 0);
    assert_eq!(c.greeter_user, "cupertino-greeter");
    assert_eq!(c.screens, ScreenSettings::default());
}

#[test]
fn a_missing_or_bad_command_refuses_the_file() {
    for (text, expected) in [
        (
            "session-command = /bin/s\n",
            vec![(0, "greeter-command is required")],
        ),
        (
            "greeter-command = greeter\nsession-command = /bin/s\n",
            vec![(1, "absolute program path")],
        ),
        ("greeter-command = /g\nsession-command =\n", vec![(2, "")]),
        (
            "",
            vec![
                (0, "greeter-command is required"),
                (0, "session-command is required"),
            ],
        ),
    ] {
        let err = LoginConfig::parse(text).unwrap_err();
        assert_eq!(err.len(), expected.len(), "{text:?}: {err:?}");
        for ((line, message), (want_line, want)) in err.iter().zip(&expected) {
            assert_eq!(line, want_line, "{text:?}");
            assert!(message.contains(want), "{text:?}: {message}");
        }
    }
}

#[test]
fn a_command_naming_a_program_of_earlier_versions_runs_its_replacement_and_is_listed() {
    let l = LoginConfig::parse(
        "greeter-command = /usr/lib/cupertino/greeter\nsession-command = /usr/bin/cupertino-session --foo bar\n",
    )
    .unwrap();
    assert_eq!(
        l.config.greeter_command,
        ["/usr/bin/cupertino-server", "--udev"]
    );
    assert_eq!(
        l.config.session_command,
        ["/usr/bin/cupertino-server", "--session", "--foo", "bar"]
    );
    assert_eq!(l.replaced.len(), 2);
    assert_eq!(l.replaced[0].key, "greeter-command");
    assert_eq!(l.replaced[0].program, "/usr/lib/cupertino/greeter");
    assert_eq!(l.replaced[1].key, "session-command");
    assert_eq!(l.replaced[1].command, l.config.session_command);
}

#[test]
fn the_screens_reader_takes_its_keys_and_leaves_bad_lines_out() {
    assert_eq!(ScreenSettings::parse(""), ScreenSettings::default());
    assert!(ScreenSettings::default().large_clock);
    for (text, info) in [
        ("hostname", HostInfo::Hostname),
        ("version", HostInfo::Version),
        ("ip", HostInfo::Ip),
    ] {
        assert_eq!(
            ScreenSettings::parse(&format!("host-info = {text}")).host_info,
            Some(info),
            "{text}"
        );
    }
    let s = ScreenSettings::parse("large-clock = maybe\nshow-input-menu = true\nvt = 9\n");
    assert!(
        s.large_clock && s.show_input_menu,
        "the bad line is left out"
    );
    assert_eq!(ScreenSettings::parse("lock-message =\n").lock_message, None);
    let at_limit = "é".repeat(MAX_LOCK_MESSAGE_CHARS);
    assert_eq!(
        ScreenSettings::parse(&format!("lock-message = {at_limit}"))
            .lock_message
            .as_deref(),
        Some(at_limit.as_str()),
        "the limit counts characters"
    );
}

#[test]
fn the_administrators_groups_come_from_the_file_else_from_the_build() {
    assert!(!default_admin_groups().is_empty());
    assert_eq!(ok("").config.admin_groups, None);
    assert_eq!(
        ok("admin-groups = staff\n").config.admin_groups.unwrap(),
        ["staff"]
    );
}

#[test]
fn load_reads_the_redirected_file_and_a_missing_file_is_an_error_at_line_zero() {
    std::env::set_var(LOGIN_CONF_ENV, "");
    assert_eq!(LoginConfig::path(), std::path::Path::new(LOGIN_CONF_PATH));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("login.conf");
    std::env::set_var(LOGIN_CONF_ENV, &path);
    assert_eq!(LoginConfig::path(), path);
    let err = LoginConfig::load().unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].0, 0);
    std::fs::write(&path, format!("{COMMANDS}vt = 5\nlarge-clock = false\n")).unwrap();
    assert_eq!(LoginConfig::load().unwrap().config.vt, 5);
    assert!(!ScreenSettings::load().large_clock);
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        ScreenSettings::load(),
        ScreenSettings::default(),
        "an unreadable file is the defaults"
    );
}
