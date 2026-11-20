//! The agent's file work, each run on Kit's I/O pool (`kit::task::io`), never on the component's loop: the login items
//! and the applications to reopen, planned here and started by the component through the host; the saved choice and
//! list of [`crate::restore`]; the person's name.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use cupertino_apps::exec::CommandSpec;

use crate::autostart;
use crate::restore::{self, Applications, SessionMark, Taken};

/// What the component starts at login: the login items, then the applications the last session saved.
#[derive(Clone, Debug, Default)]
pub struct Planned {
    pub items: Vec<autostart::Item>,
}

/// The session's login items (XDG autostart, [`autostart`]), once per session: a restarted agent finds the run of
/// `session_id` claimed and plans nothing. Problems are reported and plan nothing.
pub fn plan_login_items(session_id: &str) -> Planned {
    let none = Planned::default();
    let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) else {
        eprintln!(
            "cupertino-shell-agent: login items not started: XDG_RUNTIME_DIR is not set, so a second run cannot be told apart"
        );
        return none;
    };
    match autostart::claim_run(Path::new(&runtime), session_id) {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("cupertino-shell-agent: login items ran already in this session");
            return none;
        }
        Err(e) => {
            eprintln!("cupertino-shell-agent: login items not started: {e}");
            return none;
        }
    }
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let dirs = autostart::directories(config_home, std::env::var("XDG_CONFIG_DIRS").ok());
    let found = match autostart::list(&dirs) {
        Ok(found) => found,
        Err(e) => {
            eprintln!("cupertino-shell-agent: login items not started: {e}");
            return none;
        }
    };
    let env = autostart::Environment::from_process();
    let items = found
        .into_iter()
        .filter_map(
            |entry| match autostart::parse(&entry.id, &entry.path, &entry.text, &env) {
                Ok(item) => Some(item),
                Err(skip) => {
                    eprintln!(
                        "cupertino-shell-agent: login item skipped: {}: {skip:?}",
                        entry.id
                    );
                    None
                }
            },
        )
        .collect();
    Planned { items }
}

/// The applications the last session saved to reopen ([`restore`]), each once and only once: the list is emptied on
/// disk before anything is planned, and an application a login item started already (`login_items`) is left out.
pub fn plan_reopen(
    path: Option<&Path>,
    session_id: &str,
    login_items: &HashSet<String>,
) -> Vec<(String, CommandSpec)> {
    let Some(path) = path else {
        eprintln!("cupertino-shell-agent: applications not reopened: neither XDG_STATE_HOME nor HOME is set");
        return Vec::new();
    };
    let current = SessionMark::current(session_id);
    let ids = match restore::take_at_login(path, current.as_ref()) {
        Ok(Taken::Start(ids)) => ids,
        Ok(Taken::Nothing) => return Vec::new(),
        Ok(Taken::Disabled) => {
            eprintln!("cupertino-shell-agent: applications not reopened: the choice is off");
            return Vec::new();
        }
        Ok(Taken::SameSession) => {
            eprintln!("cupertino-shell-agent: applications not reopened: the list was saved in this session");
            return Vec::new();
        }
        Err(e) => {
            eprintln!(
                "cupertino-shell-agent: applications not reopened: the saved list could not be emptied: {e}"
            );
            return Vec::new();
        }
    };
    let mut planned = Vec::new();
    let report = restore::launch(
        &ids,
        &Applications::from_env(),
        login_items,
        |entry, command| {
            planned.push((entry.id.clone(), command));
            true
        },
    );
    for (id, skip) in &report.skipped {
        eprintln!("cupertino-shell-agent: application not reopened: {id}: {skip:?}");
    }
    planned
}

/// The "Open my windows again" choice last stored, or the default without a file.
pub fn reopen_choice(path: Option<&Path>) -> bool {
    path.map_or(restore::REOPEN_DEFAULT, restore::choice)
}

/// Store the "Open my windows again" choice, with no list yet: the list follows just before the action is asked for.
pub fn remember_choice(path: Option<&Path>, reopen: bool) {
    let Some(path) = path else {
        return;
    };
    match restore::save(path, reopen, Vec::new(), None) {
        Ok(()) => eprintln!("cupertino-shell-agent: reopen windows at the next login: {reopen}"),
        Err(e) => eprintln!("cupertino-shell-agent: the reopen choice is not saved: {e}"),
    }
}

/// Save the applications open now (`open`, in quit order), for the next login, just before the action that ends the
/// session is asked for ([`restore`]); with the choice off only the choice is stored.
pub fn save_open_applications(
    path: Option<&Path>,
    reopen: bool,
    open: &[String],
    session_id: Option<&str>,
) {
    let Some(path) = path else {
        return eprintln!(
            "cupertino-shell-agent: applications not saved: neither XDG_STATE_HOME nor HOME is set"
        );
    };
    let (apps, skipped) = if reopen {
        restore::saved_list(open.iter().map(String::as_str), &Applications::from_env())
    } else {
        (Vec::new(), Vec::new())
    };
    for (identity, skip) in &skipped {
        eprintln!(
            "cupertino-shell-agent: application not saved for the next login: {identity}: {skip:?}"
        );
    }
    let mark = session_id.and_then(SessionMark::current);
    if reopen && mark.is_none() {
        eprintln!(
            "cupertino-shell-agent: applications not saved: this boot and session cannot be named"
        );
    }
    match restore::save(path, reopen, apps.clone(), mark) {
        Ok(()) if reopen => eprintln!(
            "cupertino-shell-agent: saved for the next login: {}",
            if apps.is_empty() {
                "no application".to_string()
            } else {
                apps.join(" ")
            }
        ),
        Ok(()) => {}
        Err(e) => {
            eprintln!("cupertino-shell-agent: applications not saved for the next login: {e}")
        }
    }
}

/// The session goes on after all: a list saved for the next login is dropped, so it is not started after some later
/// end of the session.
pub fn discard_saved_applications(path: Option<&Path>) {
    if let Some(path) = path {
        if let Err(e) = restore::discard(path) {
            eprintln!("cupertino-shell-agent: the saved applications are not dropped: {e}");
        }
    }
}

/// The name the session menu shows: the account's full name, else its login name; empty when the account database
/// has no entry.
pub fn user_name() -> String {
    match cupertino_kit::account::current() {
        Ok(Some(account)) => {
            let full = account.full_name().trim();
            if full.is_empty() {
                account.login
            } else {
                full.to_owned()
            }
        }
        Ok(None) => String::new(),
        Err(e) => {
            eprintln!("cupertino-shell-agent: the account could not be read: {e}");
            String::new()
        }
    }
}
