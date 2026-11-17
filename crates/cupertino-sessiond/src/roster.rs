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
/// `picture` reads one account's picture file, `wallpaper` gives the path of its login screen wallpaper;
/// the pictures of the first accounts are sent until [`MAX_PICTURE_BYTES_PER_LIST`] is used, and the list ends at
/// [`MAX_USERS`] rows ("Other…" reaches the rest), so a frame always fits the cap.
pub fn build(
    config: &LoginConfig,
    accounts: Vec<Account>,
    admins: &[String],
    logged_in: &[u32],
    mut picture: impl FnMut(&Account) -> Option<Vec<u8>>,
    mut wallpaper: impl FnMut(&Account) -> Option<String>,
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
                wallpaper: wallpaper(&account),
            }
        })
        .collect();
    Roster {
        users,
        show_other: offer.other,
    }
}
