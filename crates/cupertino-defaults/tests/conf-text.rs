//! The `key = value` text of the settings files, through `ConfText`, and the shared reading and writing of the
//! settings files seen through `power.conf` (a plain table of keys).

use cupertino_defaults::conf::{parse_list, parse_switch, switch_text, ConfText, Line};
use cupertino_defaults::power::{PowerSettings, Timer};

#[test]
fn comments_and_blank_lines_are_blank_and_entries_lose_their_comments() {
    let text = ConfText::parse("# note\n\nkey = value  # tail\nbare\n  spaced=x\n");
    assert_eq!(
        text.lines().collect::<Vec<_>>(),
        [
            (1, Line::Blank),
            (2, Line::Blank),
            (
                3,
                Line::Entry {
                    key: "key",
                    value: "value"
                }
            ),
            (4, Line::Bare("bare")),
            (
                5,
                Line::Entry {
                    key: "spaced",
                    value: "x"
                }
            ),
        ]
    );
}

#[test]
fn set_and_remove_edit_only_the_lines_of_their_key() {
    for (row, before, edit, after) in [
        (
            "set rewrites the last line of a key, keeping indentation and comment",
            "a = 1\n  a = 2  # keep me\nb = 3\n",
            ("a", Some("9")),
            "a = 1\n  a = 9  # keep me\nb = 3\n",
        ),
        (
            "set appends a key the file lacks",
            "a = 1\n",
            ("b", Some("2")),
            "a = 1\nb = 2\n",
        ),
        (
            "remove drops every line of a key",
            "a = 1\nb = 2\na = 3\n",
            ("a", None),
            "b = 2\n",
        ),
    ] {
        let mut text = ConfText::parse(before);
        match edit {
            (key, Some(value)) => text.set(key, value),
            (key, None) => text.remove(key),
        }
        assert_eq!(text.to_text(), after, "{row}");
    }
}

#[test]
fn a_file_loads_as_its_lines_and_a_missing_one_as_empty_text_and_the_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("x.conf");
    std::fs::write(&path, "k = v\n").unwrap();
    assert_eq!(ConfText::load(&path).unwrap().to_text(), "k = v\n");
    let absent = dir.path().join("absent.conf");
    assert_eq!(ConfText::load(&absent).unwrap().to_text(), "");
    let (s, problems) = PowerSettings::load(&absent, None).unwrap();
    assert_eq!(s, PowerSettings::default());
    assert!(problems.is_empty());
}

#[test]
fn a_later_line_of_a_key_wins_and_problems_carry_line_numbers() {
    let (s, problems) = PowerSettings::parse(
        "display_sleep_ac = 30\ndisplay_sleep_ac = 60\nnonsense = 1\nbare\n",
        None,
    );
    assert_eq!(s.ac.display_sleep.duration().unwrap().as_secs(), 60);
    let problems: Vec<_> = problems
        .iter()
        .map(|p| (p.line, p.message.as_str()))
        .collect();
    assert_eq!(problems.len(), 2, "{problems:?}");
    assert!(
        problems[0].0 == 3 && problems[0].1.contains("unknown setting"),
        "{problems:?}"
    );
    assert!(
        problems[1].0 == 4 && problems[1].1.contains("key = value"),
        "{problems:?}"
    );
}

#[test]
fn a_bad_value_keeps_the_last_good_value_of_its_key_only_or_else_the_default() {
    let (good, _) = PowerSettings::parse("display_sleep_ac = 30\nsystem_sleep_ac = 40\n", None);
    let text = "display_sleep_ac = soon\nsystem_sleep_ac = 50\n";
    for (row, last_good, display_sleep) in [
        (
            "with last good settings",
            Some(&good),
            good.ac.display_sleep,
        ),
        ("without", None, PowerSettings::default().ac.display_sleep),
    ] {
        let (s, problems) = PowerSettings::parse(text, last_good);
        assert_eq!(problems.len(), 1, "{row}");
        assert!(
            problems[0].message.starts_with("display_sleep_ac:"),
            "{row}"
        );
        assert_eq!(s.ac.display_sleep, display_sleep, "{row}");
        assert_eq!(s.ac.system_sleep.duration().unwrap().as_secs(), 50, "{row}");
    }
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
    s.ac.display_sleep = Timer::Never;
    let problems = PowerSettings::store(&s, &path).unwrap();
    assert_eq!(
        problems.len(),
        1,
        "the store reports the problems of the file it rewrote"
    );
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "# mine\nfuture_key = 1\ndisplay_sleep_ac = never  # why\n"
    );
    assert_eq!(PowerSettings::load(&path, None).unwrap().0, s);
}

#[test]
fn switches_read_in_every_spelling_and_write_as_on_or_off() {
    for (text, expected) in [
        ("on", true),
        ("true", true),
        ("yes", true),
        ("1", true),
        ("ON", true),
        ("Yes", true),
        ("off", false),
        ("false", false),
        ("no", false),
        ("0", false),
        ("OFF", false),
    ] {
        assert_eq!(parse_switch(text), Ok(expected), "{text}");
    }
    assert!(parse_switch("maybe").unwrap_err().contains("on or off"));
    assert_eq!(
        (switch_text(true).as_str(), switch_text(false).as_str()),
        ("on", "off")
    );
}

#[test]
fn lists_split_on_commas_and_drop_empty_items() {
    assert_eq!(parse_list(" a, ,b,, c "), ["a", "b", "c"]);
}
