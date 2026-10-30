//! What the login screen is told about the accounts: the rows of the picker, whether "Other…" follows, and which power items
//! show. Pure over the accounts it is given; the reading of files and pictures is injected.

use cupertino_defaults::login::LoginConfig;
use cupertino_login_ipc::message::{MAX_NAME_BYTES, MAX_PICTURE_BYTES_PER_LIST, MAX_USERS};
use cupertino_login_ipc::{Picture, PowerMenu, UserRow};

use crate::filter::Filters;
use crate::users::Account;

/// The picker's content.
#[derive(Debug, PartialEq, Eq)]
pub struct Roster {
    pub users: Vec<UserRow>,
    pub show_other: bool,
}

/// The filters of the configuration.
pub fn filters(config: &LoginConfig) -> Filters {
    Filters {
        hidden: config.hidden_users.clone(),
        hide_admin: config.hide_admin_users,
        allow: config.allow_list.clone(),
        deny: config.deny_list.clone(),
        show_other: config.show_other_users,
    }
}

/// The power items that show: an item shows unless it is disabled, by its own key if that is present and
/// by `power-off-disabled` otherwise; an absent key counts as not disabled.
pub fn power_menu(config: &LoginConfig) -> PowerMenu {
    let shown = |own: Option<bool>| !own.or(config.power_off_disabled).unwrap_or(false);
    PowerMenu {
        restart: shown(config.restart_disabled),
        shut_down: shown(config.shut_down_disabled),
        sleep: shown(config.sleep_disabled),
    }
}

/// The roster for `accounts` (with `admins` the logins of the administrators; `logged_in` the user ids that have a session).
/// `picture` reads one account's picture file;
/// the pictures of the first accounts are sent until [`MAX_PICTURE_BYTES_PER_LIST`] is used, and the list ends at
/// [`MAX_USERS`] rows ("Other…" reaches the rest), so a frame always fits the cap.
pub fn build(
    config: &LoginConfig,
    accounts: Vec<Account>,
    admins: &[String],
    logged_in: &[u32],
    mut picture: impl FnMut(&Account) -> Option<Vec<u8>>,
) -> Roster {
    let offer = filters(config).apply(accounts, admins);
    let mut picture_bytes = 0;
    let users = offer
        .users
        .into_iter()
        .filter(|account| {
            account.login.len() <= MAX_NAME_BYTES && account.display_name().len() <= MAX_NAME_BYTES
        })
        .take(MAX_USERS)
        .map(|account| {
            let bytes = if picture_bytes < MAX_PICTURE_BYTES_PER_LIST {
                picture(&account)
            } else {
                None
            };
            // a picture that would pass the budget is left out whole
            let bytes =
                bytes.filter(|bytes| picture_bytes + bytes.len() <= MAX_PICTURE_BYTES_PER_LIST);
            picture_bytes += bytes.as_ref().map_or(0, Vec::len);
            let picture = bytes.and_then(|bytes| Picture::new(bytes).ok());
            UserRow {
                uid: account.uid,
                login: account.login.clone(),
                display_name: account.display_name(),
                picture,
                logged_in: logged_in.contains(&account.uid),
            }
        })
        .collect();
    Roster {
        users,
        show_other: offer.other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(extra: &str) -> LoginConfig {
        LoginConfig::parse(&format!(
            "greeter-command = /bin/g\nsession-command = /bin/s\n{extra}"
        ))
        .unwrap()
        .config
    }

    fn account(uid: u32, login: &str) -> Account {
        Account {
            uid,
            gid: uid,
            login: login.to_owned(),
            gecos: format!("{login} Full"),
            home: format!("/home/{login}"),
            shell: "/bin/bash".to_owned(),
        }
    }

    #[test]
    fn rows_carry_the_uid_and_the_full_name_and_filters_apply() {
        let roster = build(
            &config("hidden-users = bob\n"),
            vec![account(1000, "alice"), account(1001, "bob")],
            &[],
            &[],
            |_| None,
        );
        assert_eq!(
            roster.users,
            [UserRow {
                uid: 1000,
                login: "alice".into(),
                display_name: "alice Full".into(),
                picture: None,
                logged_in: false
            }]
        );
        assert!(roster.show_other, "a filter removed one");
    }

    #[test]
    fn a_row_is_logged_in_when_its_uid_has_a_session() {
        let roster = build(
            &config(""),
            vec![account(1000, "alice"), account(1001, "bob")],
            &[],
            &[1001],
            |_| None,
        );
        assert_eq!(
            roster
                .users
                .iter()
                .map(|row| (row.login.as_str(), row.logged_in))
                .collect::<Vec<_>>(),
            [("alice", false), ("bob", true)]
        );
    }

    #[test]
    fn pictures_stop_at_the_budget_and_a_picture_over_it_is_left_out_whole() {
        // four pictures of 250 KiB fit in 1 MiB; the fifth would pass the budget
        let accounts = (0..5)
            .map(|i| account(1000 + i, &format!("u{i}")))
            .collect();
        let roster = build(&config(""), accounts, &[], &[], |_| {
            Some(vec![1; 250 * 1024])
        });
        let pictures: Vec<bool> = roster.users.iter().map(|u| u.picture.is_some()).collect();
        assert_eq!(pictures, [true, true, true, true, false]);
    }

    #[test]
    fn the_list_ends_at_the_row_cap() {
        let accounts = (0..MAX_USERS as u32 + 5)
            .map(|i| account(2000 + i, &format!("u{i}")))
            .collect();
        assert_eq!(
            build(&config(""), accounts, &[], &[], |_| None).users.len(),
            MAX_USERS
        );
    }

    #[test]
    fn an_own_key_beats_power_off_disabled_and_an_absent_key_shows_the_item() {
        assert_eq!(
            power_menu(&config("")),
            PowerMenu {
                restart: true,
                shut_down: true,
                sleep: true
            }
        );
        assert_eq!(
            power_menu(&config(
                "power-off-disabled = true\nsleep-disabled = false\n"
            )),
            PowerMenu {
                restart: false,
                shut_down: false,
                sleep: true
            }
        );
        assert_eq!(
            power_menu(&config("restart-disabled = true\n")),
            PowerMenu {
                restart: false,
                shut_down: true,
                sleep: true
            }
        );
    }
}
