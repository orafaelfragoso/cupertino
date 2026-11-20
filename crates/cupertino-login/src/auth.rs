//! Authentication through PAM, on Kit's I/O pool (`cupertino_kit::task::io`), never on the thread that draws.
//!
//! The service name is [`SERVICE`]; its policy is `/etc/pam.d/cupertino-login` (the README has the file). A fresh PAM
//! transaction runs per attempt: `pam_authenticate`, then `pam_acct_mgmt` so that an expired or locked account does not
//! unlock the screen. The messages PAM's modules print (a lockout from `pam_faillock`, a password about to expire) reach
//! the screen through the attempt's `on_notice` while it runs.
//!
//! The password is zeroed when the attempt is done ([`Password`]), and the one copy this module cannot clear, the `CString`
//! that pam-client2 drops once it has copied the answer into PAM's buffer, is zeroed by the process's allocator
//! (`cupertino_login_ipc::zeroing::ZeroOnFree`); PAM clears its own copy.

use std::ffi::{CStr, CString};
use std::time::Duration;

use pam_client2::{Context, ConversationHandler, ErrorCode, Flag};

use crate::model::{AuthOutcome, Password};

/// PAM service name of the lock screen.
pub const SERVICE: &str = "cupertino-login";

/// How long an attempt should take on the I/O pool: PAM's own delay after a wrong password is a few seconds, and a
/// module that asks a slow service (a directory, a fingerprint reader that times out) takes longer; an attempt past this
/// is reported, never cut short.
pub const ATTEMPT_DEADLINE: Duration = Duration::from_secs(60);

/// Answers the password prompt with the attempt's password, refuses every other prompt and passes the modules'
/// messages on.
pub struct PasswordConversation<'a> {
    password: Password,
    on_notice: &'a dyn Fn(String),
}

impl<'a> PasswordConversation<'a> {
    /// The conversation of one attempt with `password`; `on_notice` gets the modules' messages.
    pub fn new(password: Password, on_notice: &'a dyn Fn(String)) -> Self {
        PasswordConversation {
            password,
            on_notice,
        }
    }
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

/// One PAM transaction for `login` with `password`; `on_notice` gets the modules' messages, in the order PAM sent them.
/// It waits for PAM (and its delay after a wrong password): run it on the I/O pool with [`ATTEMPT_DEADLINE`].
pub fn authenticate(login: &str, password: Password, on_notice: &dyn Fn(String)) -> AuthOutcome {
    let mut context = match Context::new(
        SERVICE,
        Some(login),
        PasswordConversation::new(password, on_notice),
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
