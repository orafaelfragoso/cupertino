//! `notifications.conf`: the default, the per-application overrides and the policy they give.

use cupertino_defaults::notifications::*;

#[test]
fn an_application_override_wins_key_by_key_and_others_follow_the_default() {
    let unset = NotificationSettings::parse("", None)
        .0
        .policy("org.any.App");
    assert!(unset.show_on_lock_screen && unset.allow);
    assert_eq!(
        unset.show_previews,
        Previews::WhenUnlocked,
        "no content on the lock screen unless asked"
    );
    assert_eq!(unset.alert_style, AlertStyle::Banners);

    let (s, problems) = NotificationSettings::parse(
        "show_on_lock_screen = off\nshow_previews = never\ncritical_during_dnd = off\n\
         app.chat.show_previews = always\napp.chat.alert_style = none\napp.mail.allow = off\n\
         app.mail.show_on_lock_screen = on\napp.org.example.App.allow = off\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!s.critical_during_dnd);
    for (app, lock_screen, previews, style, allow) in [
        ("chat", false, Previews::Always, AlertStyle::None, true),
        ("mail", true, Previews::Never, AlertStyle::Banners, false),
        (
            "org.example.App",
            false,
            Previews::Never,
            AlertStyle::Banners,
            false,
        ),
        ("other", false, Previews::Never, AlertStyle::Banners, true),
    ] {
        let p = s.policy(app);
        assert_eq!(
            (
                p.show_on_lock_screen,
                p.show_previews,
                p.alert_style,
                p.allow
            ),
            (lock_screen, previews, style, allow),
            "{app}"
        );
    }
}

#[test]
fn rejected_values_are_reported_with_the_expected_choices() {
    for (line, expected) in [
        (
            "show_previews = sometimes",
            "always, when-unlocked or never",
        ),
        ("app.a.alert_style = loud", "banners or none"),
        ("app.a.allow = maybe", "app.a.allow:"),
        ("unknown = 1", "unknown setting"),
        ("app.a.size = 2", "app.a.size"),
        ("app..allow = off", ""),
        ("app.a=b.allow = off", ""),
    ] {
        let (s, problems) = NotificationSettings::parse(&format!("{line}\n"), None);
        assert_eq!(problems.len(), 1, "{line}");
        assert!(
            problems[0].message.contains(expected),
            "{line}: {}",
            problems[0].message
        );
        assert_eq!(s, NotificationSettings::UNSET, "{line}");
    }
}

#[test]
fn ids_that_would_break_a_key_are_refused() {
    let long = "x".repeat(MAX_APP_ID_BYTES + 1);
    let longest = "x".repeat(MAX_APP_ID_BYTES);
    for (id, valid) in [
        ("", false),
        ("a=b", false),
        ("a#b", false),
        (" a", false),
        ("a ", false),
        ("a.", false),
        ("a\nb", false),
        (long.as_str(), false),
        ("chat", true),
        ("org.example.App", true),
        ("My App", true),
        (longest.as_str(), true),
    ] {
        assert_eq!(valid_app_id(id), valid, "{id:?}");
    }
}

#[test]
fn setting_an_override_stores_it_and_an_empty_override_drops_it() {
    let deny = || AppOverride {
        allow: Some(false),
        ..Default::default()
    };
    let mut s = NotificationSettings::UNSET;
    assert!(s.set_app("chat", deny()));
    assert!(!s.policy("chat").allow);
    assert!(s.set_app("chat", AppOverride::default()));
    assert!(s.apps.is_empty());
    assert!(!s.set_app("bad=id", deny()), "an id a key cannot hold");
    assert!(s.apps.is_empty());
}

#[test]
fn content_shows_by_preview_setting_and_lock_state() {
    for (previews, locked, shows) in [
        (Previews::Always, true, true),
        (Previews::Always, false, true),
        (Previews::WhenUnlocked, true, false),
        (Previews::WhenUnlocked, false, true),
        (Previews::Never, true, false),
        (Previews::Never, false, false),
    ] {
        assert_eq!(
            previews.shows_content(locked),
            shows,
            "{previews:?} locked={locked}"
        );
    }
}

#[test]
fn a_store_round_trips_and_removes_a_dropped_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notifications.conf");
    std::fs::write(
        &path,
        "show_previews = always\napp.chat.allow = off\napp.chat.alert_style = none\n",
    )
    .unwrap();
    let (mut s, problems) = NotificationSettings::load(&path, None).unwrap();
    assert!(problems.is_empty());
    s.show_on_lock_screen = false;
    s.set_app(
        "chat",
        AppOverride {
            alert_style: Some(AlertStyle::None),
            ..Default::default()
        },
    );
    s.set_app(
        "mail",
        AppOverride {
            show_previews: Some(Previews::Never),
            ..Default::default()
        },
    );
    assert!(s.store(&path).unwrap().is_empty());
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("app.chat.allow"), "{text}");
    for line in [
        "show_previews = always\n",
        "app.chat.alert_style = none\n",
        "app.mail.show_previews = never\n",
        "show_on_lock_screen = off\n",
    ] {
        assert!(text.contains(line), "{line:?} in {text}");
    }
    let (back, _) = NotificationSettings::load(&path, None).unwrap();
    assert_eq!(back, s);
}
