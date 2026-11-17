//! `notifications.conf`: the default, the per-application overrides and the policy they give.

use cupertino_defaults::notifications::*;

#[test]
fn an_empty_file_shows_notifications_on_the_lock_screen_without_content_while_locked() {
    let (s, problems) = NotificationSettings::parse("", None);
    assert!(problems.is_empty());
    assert_eq!(s, NotificationSettings::UNSET);
    assert_eq!(s, NotificationSettings::default());
    assert!(s.show_on_lock_screen);
    assert_eq!(s.show_previews, Previews::WhenUnlocked);
    assert!(s.critical_during_dnd);
    assert!(s.apps.is_empty());
    let p = s.policy("org.any.App");
    assert!(p.show_on_lock_screen && p.allow);
    assert_eq!(p.show_previews, Previews::WhenUnlocked);
    assert_eq!(p.alert_style, AlertStyle::Banners);
}

#[test]
fn an_application_override_wins_key_by_key_and_others_follow_the_default() {
    let (s, problems) = NotificationSettings::parse(
        "show_on_lock_screen = off\nshow_previews = never\ncritical_during_dnd = off\n\
         app.chat.show_previews = always\napp.chat.alert_style = none\napp.mail.allow = off\n\
         app.mail.show_on_lock_screen = on\n",
        None,
    );
    assert!(problems.is_empty(), "{problems:?}");
    assert!(!s.critical_during_dnd);
    let chat = s.policy("chat");
    assert!(!chat.show_on_lock_screen, "follows the default");
    assert_eq!(chat.show_previews, Previews::Always);
    assert_eq!(chat.alert_style, AlertStyle::None);
    assert!(chat.allow);
    let mail = s.policy("mail");
    assert!(!mail.allow && mail.show_on_lock_screen);
    assert_eq!(mail.show_previews, Previews::Never);
    let other = s.policy("other");
    assert!(!other.show_on_lock_screen && other.allow);
    assert_eq!(other.show_previews, Previews::Never);
}

#[test]
fn an_id_with_dots_is_split_at_the_last_dot() {
    let (s, problems) = NotificationSettings::parse("app.org.example.App.allow = off\n", None);
    assert!(problems.is_empty());
    assert!(!s.policy("org.example.App").allow);
}

#[test]
fn bad_lines_are_reported_and_a_bad_value_keeps_its_last_good_value() {
    let (good, _) =
        NotificationSettings::parse("show_previews = always\napp.a.allow = off\n", None);
    let (s, problems) = NotificationSettings::parse(
        "show_previews = sometimes\napp.a.allow = maybe\nunknown = 1\napp.a.size = 2\nbare\n",
        Some(&good),
    );
    assert_eq!(problems.len(), 5);
    assert!(problems[0]
        .message
        .contains("always, when-unlocked or never"));
    assert!(problems[1].message.starts_with("app.a.allow:"));
    assert!(problems[2].message.contains("unknown setting"));
    assert_eq!(s.show_previews, Previews::Always);
    assert!(!s.policy("a").allow);
}

#[test]
fn ids_that_would_break_a_key_are_refused() {
    for bad in [
        "",
        "a=b",
        "a#b",
        " a",
        "a ",
        "a.",
        "a\nb",
        &"x".repeat(MAX_APP_ID_BYTES + 1),
    ] {
        assert!(!valid_app_id(bad), "{bad:?}");
    }
    for good in [
        "chat",
        "org.example.App",
        "My App",
        &"x".repeat(MAX_APP_ID_BYTES),
    ] {
        assert!(valid_app_id(good), "{good:?}");
    }
    let (_, problems) =
        NotificationSettings::parse("app..allow = off\napp.a=b.allow = off\n", None);
    assert_eq!(problems.len(), 2);
}

#[test]
fn setting_an_override_stores_it_and_an_empty_override_drops_it() {
    let mut s = NotificationSettings::UNSET;
    assert!(s.set_app(
        "chat",
        AppOverride {
            allow: Some(false),
            ..Default::default()
        }
    ));
    assert!(!s.policy("chat").allow);
    assert!(s.set_app("chat", AppOverride::default()));
    assert!(s.apps.is_empty());
    assert!(!s.set_app(
        "bad=id",
        AppOverride {
            allow: Some(false),
            ..Default::default()
        }
    ));
    assert!(s.apps.is_empty());
}

#[test]
fn previews_and_alert_styles_have_spellings_that_parse_back() {
    for p in Previews::ALL {
        assert_eq!(Previews::parse(p.key()), Ok(p));
    }
    for a in AlertStyle::ALL {
        assert_eq!(AlertStyle::parse(a.key()), Ok(a));
    }
    assert!(Previews::parse("x").is_err());
    assert!(AlertStyle::parse("x")
        .unwrap_err()
        .contains("banners or none"));
}

#[test]
fn content_shows_by_preview_setting_and_lock_state() {
    assert!(Previews::Always.shows_content(true) && Previews::Always.shows_content(false));
    assert!(
        !Previews::WhenUnlocked.shows_content(true) && Previews::WhenUnlocked.shows_content(false)
    );
    assert!(!Previews::Never.shows_content(true) && !Previews::Never.shows_content(false));
}

#[test]
fn a_store_round_trips_keeps_comments_and_removes_a_dropped_override() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("notifications.conf");
    std::fs::write(
        &path,
        "# n\nshow_previews = always  # c\napp.chat.allow = off\napp.chat.alert_style = none\n",
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
    assert!(
        text.starts_with("# n\nshow_previews = always  # c\n"),
        "{text}"
    );
    assert!(!text.contains("app.chat.allow"), "{text}");
    assert!(text.contains("app.chat.alert_style = none\n"));
    assert!(text.contains("app.mail.show_previews = never\n"));
    assert!(text.contains("show_on_lock_screen = off\n"));
    let (back, _) = NotificationSettings::load(&path, None).unwrap();
    assert_eq!(back, s);
}

#[test]
fn the_environment_variable_redirects_the_file() {
    std::env::set_var(NOTIFICATIONS_CONF_ENV, "/x/notifications.conf");
    assert_eq!(
        NotificationSettings::path(),
        Some("/x/notifications.conf".into())
    );
}
