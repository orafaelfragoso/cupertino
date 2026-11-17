//! `keyboard.conf`: every key as an `Option`, input sources, the file's choice values and the modifier profile.

use cupertino_defaults::keyboard::*;
use cupertino_keyboard::modifiers::Profile;
use cupertino_keyboard::repeat::CAPS_HOLD_MAX_MS;

#[test]
fn an_empty_file_sets_nothing() {
    let (s, problems) = KeyboardSettings::parse("", None);
    assert!(problems.is_empty());
    assert_eq!(s, KeyboardSettings::default());
    assert_eq!(s.repeat_delay_ms, None);
    assert_eq!(s.input_sources, None);
}

#[test]
fn keys_are_read_and_bad_lines_reported() {
    let (s, problems) = KeyboardSettings::parse(
        "repeat_delay = 400\nrepeat_interval = 30\nrepeat_delay_saved = 500\nmodifier_profile = classic-pc\n\
         caps_lock_delay = 250\ncaps_lock_switch = on\ncaps_lock_hold = 1500\nfn_key = emoji\n\
         layout_keyboard = iso\nlayout_level3 = right-alt\ncompose_key = menu\nslow_keys = on\n\
         slow_keys_delay = 300\nsticky_keys = yes\nsticky_keys_shortcut = off\nmouse_keys = true\n\
         mouse_keys_shortcut = 1\nmouse_keys_delay = 10\nmouse_keys_max_speed = 20\nclient_rewrite = off\n\
         rewrite_terminals = foot, kitty\nrewrite_off = a\naccent_picker = on\naccent_table = /t.json\n\
         accent_keyboards = us, de\naccent_disabled_apps = x,,y\nrepeat_delay = fast\nunknown = 1\n",
        None,
    );
    assert_eq!(
        problems.iter().map(|p| p.line).collect::<Vec<_>>(),
        [27, 28]
    );
    assert!(problems[0].message.contains("expected milliseconds"));
    assert_eq!(
        s.repeat_delay_ms,
        Some(400),
        "a bad later line does not undo the earlier good one"
    );
    assert_eq!(s.repeat_interval_ms, Some(30));
    assert_eq!(s.repeat_delay_saved_ms, Some(500));
    assert_eq!(s.modifier_profile, Some(ModifierProfile::ClassicPc));
    assert_eq!(s.caps_lock_delay_ms, Some(250));
    assert_eq!(s.caps_lock_switch, Some(true));
    assert_eq!(s.caps_lock_hold_ms, Some(1500));
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
        (s.mouse_keys, s.mouse_keys_shortcut),
        (Some(true), Some(true))
    );
    assert_eq!(
        (s.mouse_keys_delay_ms, s.mouse_keys_max_speed),
        (Some(10), Some(20))
    );
    assert_eq!(s.client_rewrite, Some(false));
    assert_eq!(s.rewrite_terminals.unwrap(), ["foot", "kitty"]);
    assert_eq!(s.rewrite_off.unwrap(), ["a"]);
    assert_eq!(s.accent_picker, Some(true));
    assert_eq!(s.accent_table.as_deref(), Some("/t.json"));
    assert_eq!(s.accent_keyboards.unwrap(), ["us", "de"]);
    assert_eq!(s.accent_disabled_apps.unwrap(), ["x", "y"]);
}

#[test]
fn ranges_and_choices_are_checked() {
    let (_, problems) = KeyboardSettings::parse(
        "caps_lock_hold = 0\ncaps_lock_hold = 1501\nslow_keys_delay = 0\nmouse_keys_max_speed = 0\n\
         modifier_profile = mac\nfn_key = jump\nlayout_keyboard = abnt\nlayout_level3 = both\n\
         compose_key = f1\nrepeat_interval = -1\n",
        None,
    );
    assert_eq!(problems.len(), 10);
    assert!(problems[0]
        .message
        .contains(&format!("1 to {CAPS_HOLD_MAX_MS} ms")));
    assert!(problems[2].message.contains("at least 1"));
    assert!(problems[4].message.contains("cupertino, classic-pc"));
    assert!(problems[5]
        .message
        .contains("do-nothing, change-input-source, emoji, dictation"));
    assert!(problems[6].message.contains("ansi, iso, jis"));
    assert!(problems[7].message.contains("both-alt, right-alt, none"));
}

#[test]
fn the_caps_lock_hold_limit_is_the_keyboard_models_constant() {
    assert_eq!(CAPS_HOLD_MAX_MS, 1500);
    let (s, p) = KeyboardSettings::parse(&format!("caps_lock_hold = {CAPS_HOLD_MAX_MS}\n"), None);
    assert!(p.is_empty());
    assert_eq!(s.caps_lock_hold_ms, Some(CAPS_HOLD_MAX_MS));
}

#[test]
fn the_choice_values_have_names_that_read_back() {
    for v in ModifierProfile::ALL {
        assert_eq!(ModifierProfile::from_name(v.name()), Ok(*v));
    }
    for v in FnKey::ALL {
        assert_eq!(FnKey::from_name(v.name()), Ok(*v));
    }
    for v in LayoutKeyboard::ALL {
        assert_eq!(LayoutKeyboard::from_name(v.name()), Ok(*v));
    }
    for v in Level3::ALL {
        assert_eq!(Level3::from_name(v.name()), Ok(*v));
    }
    for v in ComposeKey::ALL {
        assert_eq!(ComposeKey::from_name(v.name()), Ok(*v));
    }
    assert_eq!(ComposeKey::ALL.len(), 6);
    assert_eq!(
        Level3::from_name("x").unwrap_err(),
        "expected both-alt, right-alt, none, got \"x\""
    );
}

#[test]
fn sources_parse() {
    let (s, p) = KeyboardSettings::parse(
        "input_sources = us, fr(nodeadkeys), de_CH-x, decoded:/layouts/abc.json\n",
        None,
    );
    assert!(p.is_empty(), "{p:?}");
    let list = s.input_sources.unwrap();
    assert_eq!(list.len(), 4);
    assert_eq!(
        (list[0].layout.as_str(), list[0].variant.as_str()),
        ("us", "")
    );
    assert_eq!(list[1].id, "fr(nodeadkeys)");
    assert_eq!(
        (list[1].layout.as_str(), list[1].variant.as_str()),
        ("fr", "nodeadkeys")
    );
    assert_eq!(
        list[3].decoded.as_deref(),
        Some(std::path::Path::new("/layouts/abc.json"))
    );
    assert_eq!(list[3].layout, "");
}

#[test]
fn a_bad_input_source_is_a_problem_naming_it() {
    for bad in ["fr(", "(x)", "a b", "decoded:", "us)"] {
        let (s, p) = KeyboardSettings::parse(&format!("input_sources = us, {bad}\n"), None);
        assert_eq!(p.len(), 1, "{bad}");
        assert!(
            p[0].message.contains("bad layout"),
            "{bad}: {}",
            p[0].message
        );
        assert_eq!(s.input_sources, None);
    }
}

#[test]
fn a_source_spec_parses_one_entry_and_a_decoded_path_expands_the_home() {
    assert_eq!(SourceSpec::parse("  "), None);
    assert_eq!(SourceSpec::parse(" us ").unwrap().id, "us");
    std::env::set_var("HOME", "/home/u");
    assert_eq!(expand_home("~/x/y"), std::path::Path::new("/home/u/x/y"));
    assert_eq!(expand_home("/abs"), std::path::Path::new("/abs"));
    assert_eq!(expand_home("~x"), std::path::Path::new("~x"));
    assert_eq!(
        SourceSpec::parse("decoded: ~/l.json")
            .unwrap()
            .decoded
            .unwrap(),
        std::path::Path::new("/home/u/l.json")
    );
}

#[test]
fn commas_inside_a_variants_parentheses_do_not_split_a_value() {
    assert_eq!(split_sources("us, fr(a,b) , ,de"), ["us", "fr(a,b)", "de"]);
    assert!(split_sources("").is_empty());
}

#[test]
fn a_store_round_trips_lists_sources_and_choices() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyboard.conf");
    std::fs::write(&path, "# kb\nrepeat_delay = 300  # quick\nfuture = 1\n").unwrap();
    let (mut s, p) = KeyboardSettings::load(&path, None).unwrap();
    assert_eq!(p.len(), 1);
    s.repeat_delay_ms = Some(350);
    s.fn_key = Some(FnKey::Dictation);
    s.input_sources = Some(vec![
        SourceSpec::parse("us").unwrap(),
        SourceSpec::parse("fr(oss)").unwrap(),
    ]);
    s.rewrite_terminals = Some(vec!["foot".into(), "kitty".into()]);
    s.caps_lock_switch = Some(false);
    s.store(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.starts_with("# kb\nrepeat_delay = 350  # quick\nfuture = 1\n"),
        "{text}"
    );
    assert!(text.contains("input_sources = us, fr(oss)\n"));
    assert!(text.contains("rewrite_terminals = foot, kitty\n"));
    assert!(text.contains("fn_key = dictation\n") && text.contains("caps_lock_switch = off\n"));
    assert_eq!(KeyboardSettings::load(&path, None).unwrap().0, s);
}

#[test]
fn repeat_off_keeps_the_saved_delay() {
    let (s, p) = KeyboardSettings::parse("repeat_delay = 0\nrepeat_delay_saved = 450\n", None);
    assert!(p.is_empty());
    assert_eq!(s.repeat_delay_ms, Some(0));
    assert_eq!(s.repeat_delay_saved_ms, Some(450));
}

#[test]
fn a_bad_value_keeps_the_last_good_one() {
    let (good, _) = KeyboardSettings::parse("repeat_delay = 400\nfn_key = emoji\n", None);
    let (s, p) = KeyboardSettings::parse("repeat_delay = x\nfn_key = dictation\n", Some(&good));
    assert_eq!(p.len(), 1);
    assert_eq!(s.repeat_delay_ms, Some(400));
    assert_eq!(s.fn_key, Some(FnKey::Dictation));
}

#[test]
fn the_user_profile_is_the_files_modifier_profile() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyboard.conf");
    std::env::set_var(KEYBOARD_CONF_ENV, &path);
    assert_eq!(KeyboardSettings::path(), Some(path.clone()));
    std::fs::write(&path, "modifier_profile = classic-pc\n").unwrap();
    assert_eq!(user_profile(), Profile::classic_pc());
    std::fs::write(&path, "modifier_profile = cupertino\n").unwrap();
    assert_eq!(user_profile(), Profile::cupertino());
}

#[test]
fn the_user_profile_is_the_default_one_when_the_file_or_the_key_cannot_be_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("keyboard.conf");
    std::env::set_var(KEYBOARD_CONF_ENV, &path);
    assert_eq!(user_profile(), Profile::cupertino(), "no file");
    std::fs::write(&path, "modifier_profile = mac\n").unwrap();
    assert_eq!(user_profile(), Profile::cupertino(), "a bad value");
    std::fs::write(&path, "repeat_delay = 5\n").unwrap();
    assert_eq!(user_profile(), Profile::cupertino(), "no key");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    assert_eq!(user_profile(), Profile::cupertino(), "unreadable");
}
