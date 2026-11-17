//! `login.conf` of the session manager.

use cupertino_defaults::login::*;

const COMMANDS: &str =
    "greeter-command = /usr/bin/greeter --flag\nsession-command = /usr/bin/session\n";

fn ok(text: &str) -> Loaded {
    LoginConfig::parse(&format!("{COMMANDS}{text}")).expect("a good file")
}

#[test]
fn the_required_commands_alone_give_the_defaults() {
    let l = LoginConfig::parse(COMMANDS).unwrap();
    assert!(l.problems.is_empty() && l.replaced.is_empty());
    let c = l.config;
    assert_eq!(c.vt, 1);
    assert_eq!(c.greeter_user, "cupertino-greeter");
    assert_eq!(c.greeter_command, ["/usr/bin/greeter", "--flag"]);
    assert_eq!(c.session_command, ["/usr/bin/session"]);
    assert!(c.hidden_users.is_empty() && c.deny_list.is_empty());
    assert!(!c.hide_admin_users);
    assert_eq!(c.allow_list, None);
    assert_eq!(c.show_other_users, None);
    assert_eq!(c.restart_disabled, None);
    assert_eq!(c.shut_down_disabled, None);
    assert_eq!(c.sleep_disabled, None);
    assert_eq!(c.power_off_disabled, None);
    assert_eq!(c.retries_until_hint, 0);
    assert_eq!(c.auto_login, None);
    assert_eq!(c.admin_groups, None);
    assert_eq!(c.screens, ScreenSettings::default());
}

#[test]
fn keys_override_the_defaults() {
    let c = ok(
        "vt = 7\ngreeter-user = greeter\nhidden-users = a, b\ndeny-list = c\nallow-list = d,e\n\
                hide-admin-users = true\nshow-other-users = false\nrestart-disabled = true\n\
                shut-down-disabled = true\nsleep-disabled = false\npower-off-disabled = true\n\
                retries-until-hint = 3\nauto-login = bob\nadmin-groups = wheel, admin\n",
    )
    .config;
    assert_eq!(c.vt, 7);
    assert_eq!(c.greeter_user, "greeter");
    assert_eq!(c.hidden_users, ["a", "b"]);
    assert_eq!(c.deny_list, ["c"]);
    assert_eq!(c.allow_list.unwrap(), ["d", "e"]);
    assert!(c.hide_admin_users);
    assert_eq!(c.show_other_users, Some(false));
    assert_eq!(c.restart_disabled, Some(true));
    assert_eq!(c.shut_down_disabled, Some(true));
    assert_eq!(c.sleep_disabled, Some(false));
    assert_eq!(c.power_off_disabled, Some(true));
    assert_eq!(c.retries_until_hint, 3);
    assert_eq!(c.auto_login.as_deref(), Some("bob"));
    assert_eq!(c.admin_groups.unwrap(), ["wheel", "admin"]);
}

#[test]
fn a_given_and_empty_allow_list_lists_nobody() {
    assert_eq!(ok("allow-list =\n").config.allow_list, Some(vec![]));
}

#[test]
fn a_bad_line_is_reported_by_number_and_its_key_takes_its_safe_value() {
    let l = LoginConfig::parse(&format!(
        "{COMMANDS}vt = 99\nhide-admin-users = maybe\nrestart-disabled = x\nauto-login = a:b\nadmin-groups = a b\n\
         allow-list = a,,b\nretries-until-hint = lots\nbogus = 1\ngreeter-user =\n"
    ))
    .unwrap();
    assert_eq!(
        l.problems.iter().map(|p| p.0).collect::<Vec<_>>(),
        [3, 4, 5, 6, 7, 8, 9, 10, 11]
    );
    assert!(l.problems[0].1.contains("expected 1 to 63"));
    assert!(l.problems[1].1.contains("expected true or false"));
    assert!(l.problems[7].1.contains("unknown setting"));
    let c = l.config;
    assert_eq!(c.vt, 1);
    assert!(c.hide_admin_users, "fails closed");
    assert_eq!(c.restart_disabled, Some(true), "fails closed");
    assert_eq!(c.auto_login, None);
    assert_eq!(c.admin_groups, Some(vec![]), "no administrator");
    assert_eq!(c.allow_list, Some(vec![]));
    assert_eq!(c.show_other_users, Some(true));
    assert_eq!(c.retries_until_hint, 0);
    assert_eq!(c.greeter_user, "cupertino-greeter");
}

#[test]
fn a_bad_line_wins_over_a_good_line_of_the_same_key_further_down() {
    let l = LoginConfig::parse(&format!(
        "{COMMANDS}hide-admin-users = x\nhide-admin-users = false\n"
    ))
    .unwrap();
    assert!(l.config.hide_admin_users);
}

#[test]
fn a_line_without_equals_is_a_bad_line_of_its_first_word() {
    let l = LoginConfig::parse(&format!(
        "{COMMANDS}hide-admin-users true\nshow-other-users: false\n"
    ))
    .unwrap();
    assert_eq!(l.problems.len(), 2);
    assert!(l.problems[0].1.contains("key = value"));
    assert!(l.config.hide_admin_users);
    assert_eq!(l.config.show_other_users, Some(true));
}

#[test]
fn a_missing_command_refuses_the_file_as_line_zero() {
    let err = LoginConfig::parse("session-command = /bin/s\n").unwrap_err();
    assert_eq!(err, [(0, "greeter-command is required".to_string())]);
    let err = LoginConfig::parse("").unwrap_err();
    assert_eq!(err.len(), 2);
}

#[test]
fn a_bad_command_refuses_the_file_at_its_line() {
    let err =
        LoginConfig::parse("greeter-command = greeter\nsession-command = /bin/s\n").unwrap_err();
    assert_eq!(err.len(), 1);
    assert_eq!(err[0].0, 1);
    assert!(err[0].1.contains("absolute program path"));
    assert!(LoginConfig::parse("greeter-command = /g\nsession-command =\n").is_err());
}

#[test]
fn comments_are_ignored() {
    let c = ok("# vt = 9\nvt = 4 # the fourth\n").config;
    assert_eq!(c.vt, 4);
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
fn the_screen_keys_are_read_by_the_manager_and_by_the_screens() {
    let keys =
        "lock-message = Back soon\nshow-full-name = true\nhost-info = ip\nshow-input-menu = true\n\
                hide-user-avatar-and-name = true\nlarge-clock = false\n";
    let s = ScreenSettings::parse(keys);
    assert_eq!(s.lock_message.as_deref(), Some("Back soon"));
    assert!(s.show_full_name && s.show_input_menu && s.hide_user_avatar_and_name);
    assert_eq!(s.host_info, Some(HostInfo::Ip));
    assert!(!s.large_clock);
    assert_eq!(ok(keys).config.screens, s);
}

#[test]
fn the_screens_default_to_the_large_clock_and_nothing_else() {
    let s = ScreenSettings::default();
    assert!(s.large_clock);
    assert!(!s.show_full_name && !s.show_input_menu && !s.hide_user_avatar_and_name);
    assert_eq!((s.lock_message, s.host_info), (None, None));
    assert_eq!(ScreenSettings::parse(""), ScreenSettings::default());
}

#[test]
fn host_info_takes_its_three_names() {
    for (text, info) in [
        ("hostname", HostInfo::Hostname),
        ("version", HostInfo::Version),
        ("ip", HostInfo::Ip),
    ] {
        assert_eq!(
            ScreenSettings::parse(&format!("host-info = {text}")).host_info,
            Some(info)
        );
    }
}

#[test]
fn a_bad_screen_key_takes_its_safe_value_and_is_reported() {
    let l = LoginConfig::parse(&format!(
        "{COMMANDS}large-clock = maybe\nhost-info = mac\nlock-message = {}\nshow-full-name = 1\n",
        "x".repeat(MAX_LOCK_MESSAGE_CHARS + 1)
    ))
    .unwrap();
    assert_eq!(l.problems.len(), 4);
    assert!(l.problems[2].1.contains("at most 200 characters"));
    assert_eq!(l.config.screens, ScreenSettings::default());
    // the screens' own reader leaves a bad line out
    let s = ScreenSettings::parse("large-clock = maybe\nshow-input-menu = true\nvt = 9\n");
    assert!(s.large_clock && s.show_input_menu);
}

#[test]
fn a_lock_message_may_be_empty_and_has_a_character_limit() {
    assert_eq!(ScreenSettings::parse("lock-message =\n").lock_message, None);
    let at_limit = "é".repeat(MAX_LOCK_MESSAGE_CHARS);
    assert_eq!(
        ScreenSettings::parse(&format!("lock-message = {at_limit}"))
            .lock_message
            .as_deref(),
        Some(at_limit.as_str())
    );
    assert_eq!(MAX_LOCK_MESSAGE_CHARS, 200);
}

#[test]
fn the_administrators_groups_come_from_the_file_else_from_the_build() {
    assert_eq!(
        default_admin_groups(),
        DEFAULT_ADMIN_GROUPS.split(',').collect::<Vec<_>>()
    );
    assert!(!default_admin_groups().is_empty());
    assert_eq!(ok("").config.admin_groups, None);
    assert_eq!(
        ok("admin-groups = staff\n").config.admin_groups.unwrap(),
        ["staff"]
    );
}

#[test]
fn the_path_is_the_variable_else_the_system_file() {
    std::env::set_var(LOGIN_CONF_ENV, "");
    assert_eq!(LoginConfig::path(), std::path::Path::new(LOGIN_CONF_PATH));
    assert_eq!(LOGIN_CONF_PATH, "/etc/cupertino/login.conf");
    std::env::set_var(LOGIN_CONF_ENV, "/x/login.conf");
    assert_eq!(LoginConfig::path(), std::path::Path::new("/x/login.conf"));
}

#[test]
fn load_reads_the_redirected_file_and_a_missing_file_is_an_error_at_line_zero() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("login.conf");
    std::env::set_var(LOGIN_CONF_ENV, &path);
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
