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
