//! Which login users the login screen's picker offers, and whether it ends with the "Other…" tile. Pure: the lists
//! of users and administrators are read elsewhere ([`crate::users`]).
//!
//! The design's filter keys with an equivalent here are `HiddenUsersList`, `HideAdminUsers`, `AllowList`, `DenyList`
//! and `SHOWOTHERUSERS_MANAGED`. `Hide500Users`, `HideLocalUsers`, `HideMobileAccounts`, `LocalUserLoginEnabled` and
//! `IncludeNetworkUser` have none: they separate accounts by directory binding, mobile accounts and the user id 500, which
//! this system does not have (the picker already lists only the user id range of `login.defs`).

use crate::users::Account;

/// The filters, by account (login) name. `Filters::default()` removes nobody.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Filters {
    /// `HiddenUsersList`: these accounts are not listed.
    pub hidden: Vec<String>,
    /// `HideAdminUsers`: the administrators (members of the `wheel` group) are not listed.
    pub hide_admin: bool,
    /// `AllowList`: when given, only these accounts are listed. Given and empty lists nobody.
    pub allow: Option<Vec<String>>,
    /// `DenyList`: these accounts are not listed, also when the allow list names them.
    pub deny: Vec<String>,
    /// `SHOWOTHERUSERS_MANAGED`: `Some(true)` shows the "Other…" tile, `Some(false)` hides it, `None` follows the rule of
    /// [`Filters::apply`].
    pub show_other: Option<bool>,
}

/// What the picker offers.
#[derive(Debug)]
pub struct Offer {
    /// The accounts that passed every filter, in the order they came.
    pub users: Vec<Account>,
    /// Whether the picker ends with the "Other…" tile.
    pub other: bool,
}

impl Filters {
    /// Whether any filter is set.
    pub fn is_empty(&self) -> bool {
        *self == Filters::default()
    }

    /// Filter `users`; `admins` are the login names of the administrators, only read when `hide_admin` is set.
    ///
    /// An account is removed when it is on the hidden list or the deny list, is an administrator with `hide_admin`, or is
    /// missing from a given allow list. The "Other…" tile shows when `show_other` says so; without it, when no account is
    /// left, or when a filter removed one ("no visible users", "users were removed by a filter",
    /// "SHOWOTHERUSERS_MANAGED overrides"). The design's third reason, a directory-bound machine that allows network
    /// users, has no equivalent here.
    pub fn apply(&self, users: Vec<Account>, admins: &[String]) -> Offer {
        let total = users.len();
        let named = |list: &[String], user: &Account| list.contains(&user.login);
        let users: Vec<Account> = users
            .into_iter()
            .filter(|user| {
                let removed = named(&self.hidden, user)
                    || named(&self.deny, user)
                    || (self.hide_admin && named(admins, user))
                    || self.allow.as_ref().is_some_and(|allow| !named(allow, user));
                !removed
            })
            .collect();
        let removed_some = users.len() < total;
        let other = self.show_other.unwrap_or(users.is_empty() || removed_some);
        Offer { users, other }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn users(logins: &[&str]) -> Vec<Account> {
        logins
            .iter()
            .map(|login| Account {
                uid: 1000,
                gid: 1000,
                login: (*login).to_owned(),
                gecos: String::new(),
                home: String::new(),
                shell: String::new(),
            })
            .collect()
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_owned()).collect()
    }

    /// The logins offered and whether "Other…" follows, for `filters` over alice, bob, carol and dave with carol and
    /// dave as administrators.
    fn offered(filters: &Filters) -> (Vec<String>, bool) {
        let offer = filters.apply(
            users(&["alice", "bob", "carol", "dave"]),
            &names(&["carol", "dave"]),
        );
        (
            offer.users.into_iter().map(|user| user.login).collect(),
            offer.other,
        )
    }

    #[test]
    fn no_filter_lists_everybody_and_has_no_other_tile() {
        assert_eq!(
            offered(&Filters::default()),
            (names(&["alice", "bob", "carol", "dave"]), false)
        );
    }

    #[test]
    fn a_hidden_user_is_removed_and_other_appears() {
        assert_eq!(
            offered(&Filters {
                hidden: names(&["bob"]),
                ..Filters::default()
            }),
            (names(&["alice", "carol", "dave"]), true)
        );
    }

    #[test]
    fn hiding_admins_removes_the_wheel_members_and_ignores_the_list_without_the_flag() {
        assert_eq!(
            offered(&Filters {
                hide_admin: true,
                ..Filters::default()
            }),
            (names(&["alice", "bob"]), true)
        );
        let offer = Filters::default().apply(users(&["alice", "carol"]), &names(&["carol"]));
        assert_eq!(offer.users.len(), 2);
    }

    #[test]
    fn an_allow_list_keeps_only_its_names() {
        assert_eq!(
            offered(&Filters {
                allow: Some(names(&["bob", "dave", "nobody"])),
                ..Filters::default()
            }),
            (names(&["bob", "dave"]), true)
        );
        assert_eq!(
            offered(&Filters {
                allow: Some(names(&["alice", "bob", "carol", "dave"])),
                ..Filters::default()
            }),
            (names(&["alice", "bob", "carol", "dave"]), false)
        );
    }

    #[test]
    fn a_deny_list_removes_its_names_also_from_the_allow_list() {
        assert_eq!(
            offered(&Filters {
                deny: names(&["alice"]),
                ..Filters::default()
            }),
            (names(&["bob", "carol", "dave"]), true)
        );
        assert_eq!(
            offered(&Filters {
                allow: Some(names(&["alice", "bob"])),
                deny: names(&["alice"]),
                ..Filters::default()
            }),
            (names(&["bob"]), true)
        );
    }

    #[test]
    fn the_filters_combine() {
        let filters = Filters {
            hidden: names(&["alice"]),
            hide_admin: true,
            allow: Some(names(&["alice", "bob", "carol"])),
            deny: names(&["zed"]),
            show_other: None,
        };
        assert_eq!(offered(&filters), (names(&["bob"]), true));
    }

    #[test]
    fn an_empty_allow_list_lists_nobody_and_other_stands_alone() {
        assert_eq!(
            offered(&Filters {
                allow: Some(Vec::new()),
                ..Filters::default()
            }),
            (Vec::new(), true)
        );
    }

    #[test]
    fn a_filter_that_removes_nobody_adds_no_other_tile() {
        assert_eq!(
            offered(&Filters {
                hidden: names(&["nobody"]),
                deny: names(&["nobody"]),
                ..Filters::default()
            }),
            (names(&["alice", "bob", "carol", "dave"]), false)
        );
    }

    #[test]
    fn no_users_at_all_shows_other() {
        assert!(Filters::default().apply(Vec::new(), &[]).other);
    }

    #[test]
    fn the_override_wins_over_the_rule_both_ways() {
        let hide_bob = Filters {
            hidden: names(&["bob"]),
            show_other: Some(false),
            ..Filters::default()
        };
        assert_eq!(
            offered(&hide_bob),
            (names(&["alice", "carol", "dave"]), false)
        );
        assert_eq!(
            offered(&Filters {
                show_other: Some(true),
                ..Filters::default()
            }),
            (names(&["alice", "bob", "carol", "dave"]), true)
        );
        assert!(
            !Filters {
                show_other: Some(false),
                ..Filters::default()
            }
            .apply(Vec::new(), &[])
            .other
        );
    }
}
