//! `global-shortcuts.conf`: what the person answered when an app asked to use shortcuts while another app has the keyboard.

use std::os::unix::fs::PermissionsExt;

use cupertino_defaults::desktop_control::Decision;
use cupertino_defaults::global_shortcuts::*;

fn keys(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| s.to_string()).collect()
}

#[test]
fn answers_parse_and_the_first_one_of_an_app_stands() {
    let (c, problems) = GlobalShortcutsConf::parse(
        "# c\nallow = org.example.Meeting CTRL+ALT+t CTRL+ALT+m\ndeny = org.example.Chat\n\
         deny = org.example.Meeting\nallow = org.example.Bare\ndeny = x CTRL+t\nmaybe = x\nbare\nallow =\n",
    );
    assert_eq!(c.answers.len(), 3);
    let meeting = c.answer("org.example.Meeting").unwrap();
    assert_eq!(meeting.decision, Decision::Allow);
    assert_eq!(meeting.triggers, ["CTRL+ALT+t", "CTRL+ALT+m"]);
    assert_eq!(c.decision("org.example.Chat"), Some(Decision::Deny));
    assert_eq!(
        c.answer("org.example.Bare").unwrap().triggers,
        Vec::<String>::new()
    );
    assert_eq!(c.decision("nobody"), None);
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [4, 6, 7, 8, 9]
    );
    assert!(problems[0].message.contains("already answered"));
    assert!(problems[1].message.contains("takes the app id alone"));
    assert!(problems[2].message.contains("unknown setting"));
    assert!(problems[3].message.contains("expected `allow = <app id>"));
    assert!(problems[4].message.contains("is not an app id"));
}

#[test]
fn an_allow_line_remembers_its_keys_and_an_allow_line_without_keys_allows_none() {
    let (c, p) = GlobalShortcutsConf::parse("allow = a.App\n");
    assert!(p.is_empty());
    assert_eq!(c.decision("a.App"), Some(Decision::Allow));
    assert!(c.answer("a.App").unwrap().triggers.is_empty());
}

#[test]
fn allowing_more_keys_adds_them_and_deny_forgets_them() {
    let mut c = GlobalShortcutsConf::default();
    c.allow("a.App", keys(&["CTRL+t"]));
    c.allow("a.App", keys(&["CTRL+t", "CTRL+m"]));
    assert_eq!(c.answer("a.App").unwrap().triggers, ["CTRL+t", "CTRL+m"]);
    c.set("a.App", Decision::Deny);
    assert_eq!(c.decision("a.App"), Some(Decision::Deny));
    assert!(c.answer("a.App").unwrap().triggers.is_empty());
    c.allow("a.App", keys(&["F1"]));
    assert_eq!(
        c.answer("a.App").unwrap().triggers,
        ["F1"],
        "a denied app: these alone"
    );
    c.set("a.App", Decision::Allow);
    assert_eq!(
        c.answer("a.App").unwrap().triggers,
        ["F1"],
        "allow keeps the keys"
    );
}

#[test]
fn an_app_or_a_key_a_line_cannot_hold_is_not_remembered() {
    let mut c = GlobalShortcutsConf::default();
    for bad in ["", "a b", "a#b", "a\tb", &"x".repeat(MAX_APP_ID + 1)] {
        assert!(!is_rememberable(bad), "{bad:?}");
        c.set(bad, Decision::Allow);
        c.allow(bad, keys(&["F1"]));
    }
    assert!(c.answers.is_empty());
    c.allow("a.App", keys(&["F1", "has space", "", "x#y"]));
    assert_eq!(c.answer("a.App").unwrap().triggers, ["F1"]);
    assert!(is_rememberable_trigger("CTRL+ALT+t"));
    assert!(!is_rememberable_trigger(&"k".repeat(MAX_TRIGGER + 1)));
    assert!(is_rememberable_trigger(&"k".repeat(MAX_TRIGGER)));
}

#[test]
fn past_the_most_keys_the_oldest_are_forgotten() {
    let mut c = GlobalShortcutsConf::default();
    c.allow("a.App", (0..MAX_TRIGGERS + 3).map(|i| format!("K{i}")));
    let triggers = &c.answer("a.App").unwrap().triggers;
    assert_eq!(triggers.len(), MAX_TRIGGERS);
    assert_eq!(triggers[0], "K3");
}

#[test]
fn forgetting_an_answer_makes_the_app_ask_again() {
    let mut c = GlobalShortcutsConf::default();
    c.set("a.App", Decision::Deny);
    assert!(c.forget("a.App"));
    assert!(!c.forget("a.App"));
    assert_eq!(c.decision("a.App"), None);
}

#[test]
fn a_change_keeps_the_persons_comments_and_the_lines_not_understood() {
    let (mut c, _) = GlobalShortcutsConf::parse(
        "# mine\nallow = a.App CTRL+t  # why\nsurprise line\ndeny = b.App\n\n",
    );
    c.allow("a.App", keys(&["CTRL+m"]));
    c.forget("b.App");
    c.set("c.App", Decision::Deny);
    assert_eq!(
        c.text().unwrap(),
        "# mine\nallow = a.App CTRL+t CTRL+m  # why\nsurprise line\n\ndeny = c.App\n"
    );
}

#[test]
fn text_refuses_an_answered_app_a_line_cannot_hold() {
    let mut c = GlobalShortcutsConf::default();
    c.answers.push(Answer {
        app: "a b".into(),
        decision: Decision::Allow,
        triggers: vec![],
    });
    assert_eq!(
        c.text().unwrap_err().kind(),
        std::io::ErrorKind::InvalidInput
    );
}

#[test]
fn an_answer_is_stored_changed_and_forgotten_under_the_lock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("global-shortcuts.conf");
    GlobalShortcutsConf::update(&path, |c| c.allow("a.App", keys(&["F1"]))).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "allow = a.App F1\n"
    );
    assert_eq!(
        std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let had = GlobalShortcutsConf::update(&path, |c| c.forget("a.App")).unwrap();
    assert!(had);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "");
    let (back, _) = GlobalShortcutsConf::load(&path).unwrap();
    assert!(back.answers.is_empty());
}

#[test]
fn a_missing_file_answers_nothing_and_a_file_others_can_write_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("global-shortcuts.conf");
    assert_eq!(
        GlobalShortcutsConf::load(&path).unwrap().0,
        GlobalShortcutsConf::default()
    );
    std::fs::write(&path, "allow = a.App\n").unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).unwrap();
    assert_eq!(
        GlobalShortcutsConf::load(&path).unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(GlobalShortcutsConf::update(&path, |c| c.forget("a.App")).is_err());
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "allow = a.App\n",
        "nothing was written over it"
    );
}

#[test]
fn the_variable_and_file_name_are_the_documented_ones() {
    assert_eq!(GLOBAL_SHORTCUTS_ENV, "CUPERTINO_GLOBAL_SHORTCUTS_CONF");
    assert_eq!(GLOBAL_SHORTCUTS_FILE, "global-shortcuts.conf");
    std::env::set_var(GLOBAL_SHORTCUTS_ENV, "/x/gs.conf");
    assert_eq!(GlobalShortcutsConf::path(), Some("/x/gs.conf".into()));
}
