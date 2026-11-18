use cupertino_defaults::login::LoginConfig;
use cupertino_sessiond::caller::{may_end_other_sessions, Asker};
use cupertino_sessiond::roster;
use cupertino_sessiond::users::Account;

#[test]
fn the_picker_filters_accounts_before_reading_their_pictures_and_marks_running_sessions() {
    let base = "greeter-command = /bin/greeter\nsession-command = /bin/session\n";
    for (options, expected, other) in [
        ("", vec!["amy", "bob", "chris"], false),
        (
            "allow-list = bob, chris\ndeny-list = chris",
            vec!["bob"],
            true,
        ),
        (
            "hide-admin-users = true\nshow-other-users = false",
            vec!["amy", "chris"],
            false,
        ),
        ("allow-list =", vec![], true),
    ] {
        let loaded = LoginConfig::parse(&format!("{base}{options}\n")).unwrap();
        assert!(
            loaded.problems.is_empty(),
            "{options}: {:?}",
            loaded.problems
        );
        let accounts = ["amy", "bob", "chris"]
            .into_iter()
            .enumerate()
            .map(|(i, login)| Account {
                login: login.into(),
                uid: 1000 + i as u32,
                gid: 2000,
                gecos: format!("{login} Smith,,"),
                home: format!("/home/{login}"),
                shell: "/bin/sh".into(),
            })
            .collect();
        let mut pictures = vec![];
        let roster = roster::build(
            &loaded.config,
            accounts,
            &["bob".into()],
            &[1001],
            |account| {
                pictures.push(account.login.clone());
                Some(vec![3, 6])
            },
            |_| None,
        );
        assert_eq!(pictures, expected);
        assert_eq!(roster.show_other, other);
        assert_eq!(
            roster
                .users
                .iter()
                .map(|u| u.login.as_str())
                .collect::<Vec<_>>(),
            expected
        );
        for row in roster.users {
            assert_eq!(row.display_name, format!("{} Smith", row.login));
            assert_eq!(row.logged_in, row.login == "bob");
            assert_eq!(row.picture.unwrap().bytes(), [3, 6]);
        }
    }
}

#[test]
fn restarting_with_other_users_logged_in_needs_an_administrator() {
    for (asker, sessions, lookup, expected) in [
        (Asker::Screen, vec![], None, Ok(true)),
        (Asker::Screen, vec![41], None, Ok(false)),
        (Asker::Administrator, vec![41, 42], None, Ok(true)),
        (Asker::Agent { uid: 41 }, vec![41, 41], None, Ok(true)),
        (
            Asker::Agent { uid: 41 },
            vec![41, 42],
            Some(Ok(false)),
            Ok(false),
        ),
        (Asker::Agent { uid: 41 }, vec![42], Some(Ok(true)), Ok(true)),
        (
            Asker::Agent { uid: 41 },
            vec![42],
            Some(Err("no group database")),
            Err("no group database"),
        ),
    ] {
        let result = may_end_other_sessions(asker, &sessions, |uid| {
            assert_eq!(uid, 41);
            lookup.expect("group lookup not needed")
        });
        assert_eq!(result, expected, "{asker:?} {sessions:?}");
    }
}
