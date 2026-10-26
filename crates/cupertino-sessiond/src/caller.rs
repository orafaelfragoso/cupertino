//! Who may call the manager's bus interface (`org.cupertino.LoginManager1`): the rule of `cupertino-powerd` (no polkit,
//! ARCHITECTURE rule 5). The bus daemon names the caller's process; logind names that process's session; this module decides
//! from what logind said. It is pure so the decision is tested without a bus.

use std::fmt;

/// The only seat: the design has one console.
pub const SEAT: &str = "seat0";

/// What logind says about the session a process belongs to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallerSession {
    /// The logind session id.
    pub id: String,
    /// The user the session belongs to.
    pub uid: u32,
    /// `Class`: `user`, `greeter`, `manager`, ...
    pub class: String,
    /// The seat's name; empty for a session without a seat.
    pub seat: String,
    /// Whether the session is in the foreground.
    pub active: bool,
}

/// What a method requires of its caller.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// A user session on the seat that is in the foreground: the person at the console.
    ActiveUserSession,
    /// A user session on the seat, in the foreground or not (a session announces itself before it is shown).
    UserSession,
}

/// Why a caller is refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Denial {
    /// The process belongs to no session (a system service, for example).
    NoSession,
    /// The session is not of class `user` (the login screen's session has its socket instead).
    NotAUserSession,
    /// The session is not on the console's seat.
    NotOnSeat,
    /// The session is in the background.
    NotActive,
    /// The process runs as another user than its session's: a process of a session that changed its user is not the session.
    OtherUser,
}

impl fmt::Display for Denial {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Denial::NoSession => "the caller is not in a login session",
            Denial::NotAUserSession => "the caller's session is not a user session",
            Denial::NotOnSeat => "the caller's session is not on the console's seat",
            Denial::NotActive => "the caller's session is not the active session",
            Denial::OtherUser => "the caller does not run as the user of its session",
        })
    }
}

/// Decide whether the process of `session` (`None`: logind knows none), running as `caller_uid`, meets `need`. Returns the
/// session the call then acts for: methods bind to the caller's own session and take no session argument.
pub fn authorise(session: Option<CallerSession>, caller_uid: u32, need: Need) -> Result<CallerSession, Denial> {
    let session = session.ok_or(Denial::NoSession)?;
    if session.class != "user" {
        return Err(Denial::NotAUserSession);
    }
    if session.seat != SEAT {
        return Err(Denial::NotOnSeat);
    }
    if session.uid != caller_uid {
        return Err(Denial::OtherUser);
    }
    if need == Need::ActiveUserSession && !session.active {
        return Err(Denial::NotActive);
    }
    Ok(session)
}

/// Who asks to restart or shut the machine down, for [`may_end_other_sessions`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Asker {
    /// The login screen, with no administrator's authentication.
    Screen,
    /// The login screen, after an administrator proved her password for the action.
    Administrator,
    /// The agent of the active session of this user.
    Agent { uid: u32 },
}

/// Whether `asker` may restart or shut down while the users `session_uids` (one per user session) have sessions open, which
/// that ends. The screen only when none is open; an administrator's authentication always; an agent when every open session is
/// its own user's or the user is an administrator (`is_administrator` is asked only then).
pub fn may_end_other_sessions<E>(asker: Asker, session_uids: &[u32], is_administrator: impl FnOnce(u32) -> Result<bool, E>) -> Result<bool, E> {
    match asker {
        Asker::Screen => Ok(session_uids.is_empty()),
        Asker::Administrator => Ok(true),
        Asker::Agent { uid } if session_uids.iter().all(|session| *session == uid) => Ok(true),
        Asker::Agent { uid } => is_administrator(uid),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> CallerSession {
        CallerSession { id: "3".to_owned(), uid: 1000, class: "user".to_owned(), seat: SEAT.to_owned(), active: true }
    }

    #[test]
    fn a_process_in_no_session_is_refused_whoever_it_runs_as() {
        for need in [Need::ActiveUserSession, Need::UserSession] {
            assert_eq!(authorise(None, 0, need), Err(Denial::NoSession));
            assert_eq!(authorise(None, 1000, need), Err(Denial::NoSession));
        }
    }

    #[test]
    fn the_active_user_session_of_the_seat_is_the_caller_it_binds_to() {
        assert_eq!(authorise(Some(session()), 1000, Need::ActiveUserSession), Ok(session()));
    }

    #[test]
    fn a_background_session_may_announce_itself_but_not_act() {
        let background = CallerSession { active: false, ..session() };
        assert_eq!(authorise(Some(background.clone()), 1000, Need::ActiveUserSession), Err(Denial::NotActive));
        assert_eq!(authorise(Some(background.clone()), 1000, Need::UserSession), Ok(background));
    }

    #[test]
    fn only_a_user_session_on_the_seat_counts() {
        let greeter = CallerSession { class: "greeter".to_owned(), ..session() };
        let manager = CallerSession { class: "manager".to_owned(), ..session() };
        let remote = CallerSession { seat: String::new(), ..session() };
        let other_seat = CallerSession { seat: "seat1".to_owned(), ..session() };
        for need in [Need::ActiveUserSession, Need::UserSession] {
            assert_eq!(authorise(Some(greeter.clone()), 1000, need), Err(Denial::NotAUserSession));
            assert_eq!(authorise(Some(manager.clone()), 1000, need), Err(Denial::NotAUserSession));
            assert_eq!(authorise(Some(remote.clone()), 1000, need), Err(Denial::NotOnSeat));
            assert_eq!(authorise(Some(other_seat.clone()), 1000, need), Err(Denial::NotOnSeat));
        }
    }

    #[test]
    fn the_login_screen_may_end_sessions_only_with_an_administrators_password() {
        let never = |_: u32| -> Result<bool, ()> { panic!("the screen does not ask who is an administrator") };
        assert_eq!(may_end_other_sessions(Asker::Screen, &[], never), Ok(true));
        assert_eq!(may_end_other_sessions(Asker::Screen, &[1000], never), Ok(false));
        assert_eq!(may_end_other_sessions(Asker::Administrator, &[1000, 1001], never), Ok(true));
    }

    #[test]
    fn an_agent_may_end_its_own_sessions_and_others_only_as_an_administrator() {
        let never = |_: u32| -> Result<bool, ()> { panic!("the user's own sessions need no administrator") };
        assert_eq!(may_end_other_sessions(Asker::Agent { uid: 1000 }, &[1000], never), Ok(true));
        assert_eq!(may_end_other_sessions(Asker::Agent { uid: 1000 }, &[1000, 1000], never), Ok(true));
        let ask = |administrator: bool| move |uid: u32| -> Result<bool, ()> { assert_eq!(uid, 1000); Ok(administrator) };
        assert_eq!(may_end_other_sessions(Asker::Agent { uid: 1000 }, &[1000, 1001], ask(false)), Ok(false));
        assert_eq!(may_end_other_sessions(Asker::Agent { uid: 1000 }, &[1000, 1001], ask(true)), Ok(true));
        // a failure to read the account database is an error, not a refusal or a pass
        assert_eq!(may_end_other_sessions(Asker::Agent { uid: 1000 }, &[1001], |_| Err("unreadable")), Err("unreadable"));
    }

    #[test]
    fn a_process_of_another_user_inside_the_session_is_not_the_session() {
        // `sudo -u other` inside the session, and root inside it, keep the session's cgroup and lose its identity
        for caller in [0, 1001] {
            assert_eq!(authorise(Some(session()), caller, Need::ActiveUserSession), Err(Denial::OtherUser));
            assert_eq!(authorise(Some(session()), caller, Need::UserSession), Err(Denial::OtherUser));
        }
    }
}
