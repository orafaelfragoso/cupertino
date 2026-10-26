//! Authentication through PAM, off the thread that draws.
//!
//! The service name is [`SERVICE`]; its policy is `/etc/pam.d/cupertino-login` (the README has the file). A fresh PAM
//! transaction runs per attempt: `pam_authenticate`, then `pam_acct_mgmt` so that an expired or locked account does not
//! unlock the screen. The messages PAM's modules print (a lockout from `pam_faillock`, a password about to expire) reach
//! the screen as [`Event::Notice`].

use std::ffi::{CStr, CString};
use std::sync::mpsc::{channel, Sender};

use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};

use crate::model::{AuthOutcome, Password};

/// PAM service name of the lock screen.
pub const SERVICE: &str = "cupertino-login";

/// What the worker tells the screen.
#[derive(Debug, PartialEq, Eq)]
pub enum Event {
    /// Text a PAM module printed (`PAM_TEXT_INFO`, `PAM_ERROR_MSG`) while the attempt runs.
    Notice(String),
    /// The attempt is decided.
    Outcome(AuthOutcome),
}

/// Runs attempts one after another on its own thread.
pub struct Authenticator {
    attempts: Sender<Password>,
}

impl Authenticator {
    /// Start the worker for the account `login`. `on_event` runs on it: the notices of an attempt in the order PAM sent
    /// them, then its outcome, for each attempt in turn.
    pub fn spawn(
        login: String,
        on_event: impl Fn(Event) + Send + 'static,
    ) -> std::io::Result<Authenticator> {
        let (attempts, queue) = channel::<Password>();
        std::thread::Builder::new()
            .name("login-pam".into())
            .spawn(move || {
                // ends when the `Authenticator` is dropped
                for password in queue {
                    let outcome =
                        authenticate(&login, password, &|text| on_event(Event::Notice(text)));
                    on_event(Event::Outcome(outcome));
                }
            })?;
        Ok(Authenticator { attempts })
    }

    /// Queue an attempt with `password`; it is zeroed when the attempt is done.
    pub fn attempt(&self, password: Password) {
        self.attempts
            .send(password)
            .expect("the worker lives as long as the authenticator");
    }
}

/// Answers the password prompt with the attempt's password, refuses every other prompt and passes the modules'
/// messages on.
struct PasswordConversation<'a> {
    password: Password,
    on_notice: &'a dyn Fn(String),
}

impl ConversationHandler for PasswordConversation<'_> {
    fn prompt_echo_on(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        Err(ErrorCode::CONV_ERR)
    }

    fn prompt_echo_off(&mut self, _prompt: &CStr) -> Result<CString, ErrorCode> {
        // pam-client2 takes the string by value and drops it after copying it into PAM's buffer, without zeroing it:
        // this one copy of the password outlives `self.password`
        CString::new(self.password.as_bytes()).map_err(|_| ErrorCode::CONV_ERR)
    }

    fn text_info(&mut self, message: &CStr) {
        (self.on_notice)(message.to_string_lossy().into_owned());
    }

    fn error_msg(&mut self, message: &CStr) {
        (self.on_notice)(message.to_string_lossy().into_owned());
    }
}

/// One PAM transaction for `login` with `password`; `on_notice` gets the modules' messages.
fn authenticate(login: &str, password: Password, on_notice: &dyn Fn(String)) -> AuthOutcome {
    let mut context = match Context::new(
        SERVICE,
        Some(login),
        PasswordConversation {
            password,
            on_notice,
        },
    ) {
        Ok(context) => context,
        Err(e) => return AuthOutcome::Error(format!("cannot start authentication: {e}")),
    };
    match context.authenticate(Flag::NONE) {
        Ok(()) => {}
        Err(e) if matches!(e.code(), ErrorCode::AUTH_ERR | ErrorCode::MAXTRIES) => {
            return AuthOutcome::Denied
        }
        Err(e) => return AuthOutcome::Error(format!("authentication failed to run: {e}")),
    }
    match context.acct_mgmt(Flag::NONE) {
        Ok(()) => AuthOutcome::Success,
        Err(e) => AuthOutcome::Error(format!("account not available: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use zeroize::Zeroizing;

    #[test]
    fn the_messages_of_pam_modules_become_notices_in_order() {
        let notices = RefCell::new(Vec::new());
        let on_notice = |text: String| notices.borrow_mut().push(text);
        let mut conversation = PasswordConversation {
            password: Zeroizing::new(String::new()),
            on_notice: &on_notice,
        };
        conversation.error_msg(c"The account is locked due to 3 failed logins.");
        conversation.text_info(c"Your password will expire in 2 days.");
        assert_eq!(
            *notices.borrow(),
            [
                "The account is locked due to 3 failed logins.",
                "Your password will expire in 2 days."
            ]
        );
    }
}
