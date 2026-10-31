//! The users polkitd offers, as the prompt names them, and the login name the helper takes.

use std::ffi::CStr;

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
    let mut buffer = vec![0u8; 16 * 1024];
    // Safety: an all-zero `passwd` is a valid value for `getpwuid_r` to fill.
    let mut entry: libc::passwd = unsafe { std::mem::zeroed() };
    let mut found = std::ptr::null_mut();
    // Safety: `entry`, `buffer` and `found` outlive the call, and `buffer.len()` is the buffer's size.
    let status = unsafe {
        libc::getpwuid_r(
            user.0,
            &mut entry,
            buffer.as_mut_ptr().cast(),
            buffer.len(),
            &mut found,
        )
    };
    if status != 0 || found.is_null() {
        return None;
    }
    // Safety: on success the fields point to NUL-terminated strings in `buffer`, which is still alive.
    let text = |p: *const libc::c_char| {
        (!p.is_null()).then(|| unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
    };
    Some(Account {
        login: text(entry.pw_name)?,
        full_name: text(entry.pw_gecos)
            .unwrap_or_default()
            .split(',')
            .next()
            .unwrap_or_default()
            .to_owned(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn with(login: &str, full_name: &str) -> Option<Account> {
        Some(Account {
            login: login.into(),
            full_name: full_name.into(),
        })
    }

    #[test]
    fn users_are_named_by_full_name_login_or_id_and_root_is_the_administrator() {
        assert_eq!(
            display_names(&[
                (User(1000), with("ada", "Ada Lovelace")),
                (User(1001), with("bob", "")),
                (User(0), with("root", "root")),
                (User(4242), None),
            ]),
            ["Ada Lovelace", "bob", "Administrator (root)", "User 4242"]
        );
    }

    #[test]
    fn a_full_name_two_users_share_is_told_apart_by_the_login() {
        assert_eq!(
            display_names(&[
                (User(1000), with("ada", "Ada")),
                (User(1001), with("ada2", "Ada")),
                (User(1002), with("cy", "Cy")),
            ]),
            ["Ada (ada)", "Ada (ada2)", "Cy"]
        );
    }
}
