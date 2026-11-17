//! The `key = value` text of the settings files, through `ConfText`, and the files' shared behaviour seen through
//! `power.conf` (a plain table of keys).

use cupertino_defaults::conf::{ConfText, Line};
use cupertino_defaults::power::PowerSettings;

#[test]
fn comments_and_blank_lines_are_blank_and_entries_lose_their_comments() {
    let text = ConfText::parse("# note\n\nkey = value  # tail\nbare\n  spaced=x\n");
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines[0], (1, Line::Blank));
    assert_eq!(lines[1], (2, Line::Blank));
    assert_eq!(
        lines[2],
        (
            3,
            Line::Entry {
                key: "key",
                value: "value"
            }
        )
    );
    assert_eq!(lines[3], (4, Line::Bare("bare")));
    assert_eq!(
        lines[4],
        (
            5,
            Line::Entry {
                key: "spaced",
                value: "x"
            }
        )
    );
}

#[test]
fn set_rewrites_the_last_line_of_a_key_keeping_indentation_and_comment() {
    let mut text = ConfText::parse("a = 1\n  a = 2  # keep me\nb = 3\n");
    text.set("a", "9");
    assert_eq!(text.to_text(), "a = 1\n  a = 9  # keep me\nb = 3\n");
}

#[test]
fn set_appends_a_key_the_file_lacks() {
    let mut text = ConfText::parse("a = 1\n");
    text.set("b", "2");
    assert_eq!(text.to_text(), "a = 1\nb = 2\n");
}

#[test]
fn remove_drops_every_line_of_a_key() {
    let mut text = ConfText::parse("a = 1\nb = 2\na = 3\n");
    text.remove("a");
    assert_eq!(text.to_text(), "b = 2\n");
}

#[test]
fn a_missing_file_loads_as_empty_text() {
    let dir = tempfile::tempdir().unwrap();
    let text = ConfText::load(&dir.path().join("absent.conf")).unwrap();
    assert_eq!(text.to_text(), "");
}

#[test]
fn a_file_loads_as_its_lines() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    std::fs::write(&path, "k = v\n").unwrap();
    assert_eq!(ConfText::load(&path).unwrap().to_text(), "k = v\n");
}

#[test]
fn a_later_line_of_a_key_wins_and_problems_carry_line_numbers() {
    let (s, problems) = PowerSettings::parse(
        "display_sleep_ac = 30\ndisplay_sleep_ac = 60\nnonsense = 1\nbare\n",
        None,
    );
    assert_eq!(s.ac.display_sleep.duration().unwrap().as_secs(), 60);
    assert_eq!(problems.len(), 2);
    assert_eq!(problems[0].line, 3);
    assert!(
        problems[0].message.contains("unknown setting"),
        "{}",
        problems[0].message
    );
    assert_eq!(problems[1].line, 4);
    assert!(
        problems[1].message.contains("key = value"),
        "{}",
        problems[1].message
    );
}

#[test]
fn a_bad_value_keeps_the_last_good_value_of_its_key_only() {
    let (good, _) = PowerSettings::parse("display_sleep_ac = 30\nsystem_sleep_ac = 40\n", None);
    let (s, problems) = PowerSettings::parse(
        "display_sleep_ac = soon\nsystem_sleep_ac = 50\n",
        Some(&good),
    );
    assert_eq!(problems.len(), 1);
    assert!(problems[0].message.starts_with("display_sleep_ac:"));
    assert_eq!(s.ac.display_sleep, good.ac.display_sleep);
    assert_eq!(s.ac.system_sleep.duration().unwrap().as_secs(), 50);
}

#[test]
fn a_bad_value_without_last_good_settings_is_the_default() {
    let (s, problems) = PowerSettings::parse("display_sleep_ac = soon\n", None);
    assert_eq!(problems.len(), 1);
    assert_eq!(
        s.ac.display_sleep,
        PowerSettings::default().ac.display_sleep
    );
}

#[test]
fn a_store_changes_only_the_lines_of_changed_keys_and_keeps_comments_and_unknown_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("power.conf");
    std::fs::write(
        &path,
        "# mine\nfuture_key = 1\ndisplay_sleep_ac = 30  # why\n",
    )
    .unwrap();
    let (mut s, problems) = PowerSettings::load(&path, None).unwrap();
    assert_eq!(problems.len(), 1, "the unknown key is reported");
    s.ac.display_sleep = cupertino_defaults::power::Timer::Never;
    let before = PowerSettings::store(&s, &path).unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\nfuture_key = 1\ndisplay_sleep_ac = never  # why\n"
    );
}

#[test]
fn switches_read_in_every_spelling() {
    use cupertino_defaults::conf::{parse_switch, switch_text};
    for on in ["on", "true", "yes", "1", "ON", "Yes"] {
        assert_eq!(parse_switch(on), Ok(true), "{on}");
    }
    for off in ["off", "false", "no", "0", "OFF"] {
        assert_eq!(parse_switch(off), Ok(false), "{off}");
    }
    assert!(parse_switch("maybe").unwrap_err().contains("on or off"));
    assert_eq!(switch_text(true), "on");
    assert_eq!(switch_text(false), "off");
}

#[test]
fn lists_split_on_commas_and_drop_empty_items() {
    assert_eq!(
        cupertino_defaults::conf::parse_list(" a, ,b,, c "),
        ["a", "b", "c"]
    );
}
