//! `keyboard.conf`: every key as an `Option`, input sources, the file's choice values and the modifier profile.

use std::path::Path;

use cupertino_defaults::keyboard::*;
use cupertino_keyboard::modifiers::Profile;
use cupertino_keyboard::repeat::CAPS_HOLD_MAX_MS;

#[test]
fn every_key_is_read() {
    let (s, problems) = KeyboardSettings::parse(
        &format!(
            "repeat_delay = 0\nrepeat_interval = 30\nrepeat_delay_saved = 500\nmodifier_profile = classic-pc\n\
             caps_lock_delay = 250\ncaps_lock_switch = on\ncaps_lock_hold = {CAPS_HOLD_MAX_MS}\nfn_key = emoji\n\
             layout_keyboard = iso\nlayout_level3 = right-alt\ncompose_key = menu\nslow_keys = on\n\
             slow_keys_delay = 300\nsticky_keys = yes\nsticky_keys_shortcut = off\nmouse_keys = true\n\
             mouse_keys_shortcut = 1\nmouse_keys_delay = 10\nmouse_keys_max_speed = 20\nclient_rewrite = off\n\
             rewrite_terminals = foot, kitty\nrewrite_off = a\naccent_picker = on\naccent_table = /t.json\n\
             accent_keyboards = us, de\naccent_disabled_apps = x,,y\n\
             input_sources = us, fr(nodeadkeys), de_CH-x, decoded:/layouts/abc.json\n"
        ),
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert_eq!(
        (s.repeat_delay_ms, s.repeat_delay_saved_ms),
        (Some(0), Some(500)),
        "repeat off keeps the saved delay"
    );
    assert_eq!(s.repeat_interval_ms, Some(30));
    assert_eq!(s.modifier_profile, Some(ModifierProfile::ClassicPc));
    assert_eq!(
        (
            s.caps_lock_delay_ms,
            s.caps_lock_switch,
            s.caps_lock_hold_ms
        ),
        (Some(250), Some(true), Some(CAPS_HOLD_MAX_MS))
    );
    assert_eq!(s.fn_key, Some(FnKey::Emoji));
    assert_eq!(s.layout_keyboard, Some(LayoutKeyboard::Iso));
    assert_eq!(s.layout_level3, Some(Level3::RightAlt));
    assert_eq!(s.compose_key, Some(ComposeKey::Menu));
    assert_eq!((s.slow_keys, s.slow_keys_delay_ms), (Some(true), Some(300)));
    assert_eq!(
        (s.sticky_keys, s.sticky_keys_shortcut),
        (Some(true), Some(false))
    );
    assert_eq!(
        (
            s.mouse_keys,
            s.mouse_keys_shortcut,
            s.mouse_keys_delay_ms,
            s.mouse_keys_max_speed
        ),
        (Some(true), Some(true), Some(10), Some(20))
    );
    assert_eq!(s.client_rewrite, Some(false));
    assert_eq!(s.rewrite_terminals.unwrap(), ["foot", "kitty"]);
    assert_eq!(s.rewrite_off.unwrap(), ["a"]);
    assert_eq!(s.accent_picker, Some(true));
    assert_eq!(s.accent_table.as_deref(), Some("/t.json"));
    assert_eq!(s.accent_keyboards.unwrap(), ["us", "de"]);
    assert_eq!(s.accent_disabled_apps.unwrap(), ["x", "y"]);
    let sources = s.input_sources.unwrap();
    let ids: Vec<_> = sources
        .iter()
        .map(|s| (s.id.as_str(), s.layout.as_str(), s.variant.as_str()))
        .collect();
    assert_eq!(
        ids,
        [
            ("us", "us", ""),
            ("fr(nodeadkeys)", "fr", "nodeadkeys"),
            ("de_CH-x", "de_CH-x", ""),
            ("decoded:/layouts/abc.json", "", ""),
        ]
    );
    assert_eq!(
        sources[3].decoded.as_deref(),
        Some(Path::new("/layouts/abc.json"))
    );
}

#[test]
fn rejected_values_are_reported_with_the_expected_range_or_choices() {
    let hold = format!("1 to {CAPS_HOLD_MAX_MS} ms");
    let over = format!("caps_lock_hold = {}", CAPS_HOLD_MAX_MS + 1);
    for (line, expected) in [
        ("repeat_delay = fast", "expected milliseconds"),
        ("repeat_interval = -1", "expected milliseconds"),
        ("caps_lock_hold = 0", hold.as_str()),
        (over.as_str(), hold.as_str()),
        ("slow_keys_delay = 0", "at least 1"),
        ("mouse_keys_max_speed = 0", "at least 1"),
        ("modifier_profile = mac", "cupertino, classic-pc"),
        (
            "fn_key = jump",
            "do-nothing, change-input-source, emoji, dictation",
        ),
        ("layout_keyboard = abnt", "ansi, iso, jis"),
        ("layout_level3 = both", "both-alt, right-alt, none"),
        ("compose_key = f1", "compose_key"),
        ("input_sources = us, fr(", "bad layout"),
        ("input_sources = us, (x)", "bad layout"),
        ("input_sources = us, a b", "bad layout"),
        ("input_sources = us, decoded:", "bad layout"),
        ("input_sources = us, us)", "bad layout"),
    ] {
        let (s, problems) = KeyboardSettings::parse(&format!("{line}\n"), None);
        assert_eq!(problems.len(), 1, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
        assert_eq!(s, KeyboardSettings::default(), "{line} sets nothing");
    }
}

#[test]
fn a_source_list_splits_outside_parentheses_and_a_decoded_path_expands_the_home() {
    assert_eq!(split_sources("us, fr(a,b) , ,de"), ["us", "fr(a,b)", "de"]);
    assert!(split_sources("").is_empty());
    assert_eq!(SourceSpec::parse("  "), None);
    assert_eq!(SourceSpec::parse(" us ").unwrap().id, "us");
    std::env::set_var("HOME", "/home/u");
    for (text, expected) in [("~/x/y", "/home/u/x/y"), ("/abs", "/abs"), ("~x", "~x")] {
        assert_eq!(expand_home(text), Path::new(expected), "{text}");
    }
    assert_eq!(
        SourceSpec::parse("decoded: ~/l.json")
            .unwrap()
            .decoded
            .unwrap(),
        Path::new("/home/u/l.json")
    );
}

#[test]
fn a_store_round_trips_lists_sources_and_choices() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyboard.conf");
    let s = KeyboardSettings {
        repeat_delay_ms: Some(350),
        fn_key: Some(FnKey::Dictation),
        input_sources: Some(vec![
            SourceSpec::parse("us").unwrap(),
            SourceSpec::parse("fr(oss)").unwrap(),
        ]),
        rewrite_terminals: Some(vec!["foot".into(), "kitty".into()]),
        caps_lock_switch: Some(false),
        ..KeyboardSettings::default()
    };
    s.store(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    for line in [
        "repeat_delay = 350\n",
        "input_sources = us, fr(oss)\n",
        "rewrite_terminals = foot, kitty\n",
        "fn_key = dictation\n",
        "caps_lock_switch = off\n",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    assert_eq!(KeyboardSettings::load(&path, None).unwrap().0, s);
}

#[test]
fn the_user_profile_is_the_files_modifier_profile_else_the_default_one() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyboard.conf");
    std::env::set_var(KEYBOARD_CONF_ENV, &path);
    assert_eq!(user_profile(), Profile::cupertino(), "no file");
    for (text, expected) in [
        ("modifier_profile = classic-pc\n", Profile::classic_pc()),
        ("modifier_profile = cupertino\n", Profile::cupertino()),
        ("modifier_profile = mac\n", Profile::cupertino()),
        ("repeat_delay = 5\n", Profile::cupertino()),
    ] {
        std::fs::write(&path, text).unwrap();
        assert_eq!(user_profile(), expected, "{text:?}");
    }
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(user_profile(), Profile::cupertino(), "unreadable");
}
