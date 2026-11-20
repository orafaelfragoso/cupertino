//! The users polkitd offers, as the prompt names them, and the login name the helper takes.

use crate::agent::User;
use crate::text::shown;

/// What the password database says about a user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Account {
    pub login: String,
    /// The first field of the comment (GECOS), the full name; may be empty.
    pub full_name: String,
}

/// The account of `user`, when the password database knows it.
pub fn account(user: User) -> Option<Account> {
    let entry = cupertino_kit::account::by_uid(user.0).ok()??;
    Some(Account {
        full_name: entry.full_name().to_owned(),
        login: entry.login,
    })
}

/// How the prompt names each of `users`, in their order: the full name, else the login name; root as the
/// administrator; a full name two users share followed by the login name.
pub fn display_names(users: &[(User, Option<Account>)]) -> Vec<String> {
    let base = |(user, account): &(User, Option<Account>)| match account {
        _ if *user == User(0) => "Administrator (root)".to_owned(),
        Some(a) if !a.full_name.trim().is_empty() => shown(a.full_name.trim()),
        Some(a) => shown(&a.login),
        None => format!("User {}", user.0),
    };
    let names: Vec<String> = users.iter().map(base).collect();
    names
        .iter()
        .zip(users)
        .map(|(name, (_, account))| match account {
            Some(a) if names.iter().filter(|n| *n == name).count() > 1 => {
                format!("{name} ({})", shown(&a.login))
            }
            _ => name.clone(),
        })
        .collect()
}
