//! The messages between the manager and a worker over their socketpair. Same framing as the login screen's protocol. Nothing
//! the login screen sends is copied into these messages except the answers to PAM's prompts.

use cupertino_login_ipc::{Outcome, Secret};
use serde::{Deserialize, Serialize};

/// The largest frame either side sends (approximate, ours).
pub const MAX_FRAME: usize = 16 * 1024;

/// The class of the logind session a worker opens (`XDG_SESSION_CLASS`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionClass {
    User,
    Greeter,
}

impl SessionClass {
    pub fn name(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Greeter => "greeter",
        }
    }
}

/// What a worker does when the account is authenticated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Mode {
    /// Open the account's session and run its leader.
    Open,
    /// The account has a session already (a fast user switch): end the PAM transaction and exit, opening nothing.
    Verify,
}

/// Manager to worker.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum ToWorker {
    /// The first message: everything the worker needs, all of it from root-owned configuration.
    Begin {
        /// The PAM service.
        service: String,
        login: String,
        /// The terminal of the session, `/dev/ttyN`.
        tty: String,
        vt: u32,
        class: SessionClass,
        mode: Mode,
        /// The leader's program and arguments.
        command: Vec<String>,
        /// Extra `NAME=value` entries of the leader's environment.
        environment: Vec<String>,
        /// `ENV_PATH` of `login.defs`.
        path: String,
        /// The login screen starts with the screen reader on (the machine's `screen_reader_at_login`).
        screen_reader: bool,
        /// How many attempts in a row were refused before this one, for the journal's `login.auth.failed`.
        #[serde(default)]
        failures: u64,
    },
    /// The answer to the last `Prompt`.
    Answer { secret: Secret },
    /// Give up the attempt: the pending prompt fails.
    Cancel {},
    /// To the login screen's worker: whether the login screen is on the console. Its screen reader runs only while it is.
    Shown { shown: bool },
}

/// Worker to manager.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum FromWorker {
    /// PAM asks the user; `Answer` or `Cancel` follows.
    Prompt { echo: bool, text: String },
    /// PAM printed `text` (`error` for `PAM_ERROR_MSG`, else `PAM_TEXT_INFO`); no reply.
    Notice { error: bool, text: String },
    /// Authentication and the account check are decided. After `Accepted` the worker opens the session; after any other
    /// outcome it exits.
    Decided { outcome: Outcome },
    /// The session is open and the leader runs; `uid` is the user it belongs to.
    Started {
        session_id: String,
        leader: u32,
        uid: u32,
    },
    /// The worker gives up; `reason` is for the journal and holds no secret.
    Failed { reason: String },
}
