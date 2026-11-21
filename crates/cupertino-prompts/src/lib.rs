//! The session's prompts (`cupertino-prompts`): one Kit program, always running, that holds every prompt a program
//! asks the person through the shell:
//!
//! - the authentication prompt of polkit's agent ([`polkit`]): polkitd asks it to authenticate someone for another
//!   program's action;
//! - the keyring's prompt ([`keyring`]): the keyring daemon asks it for a password or a confirmation, and the offer
//!   to move a default keyring that login does not unlock into the login keyring asks through it;
//! - the portal's consent questions ([`consent`]): whether an application may take a screenshot, and whether it may
//!   use keyboard shortcuts while another application has the keyboard, asked over `org.cupertino.Prompts1`.
//!
//! The prompts are drawn through CupertinoKit ([`view`]) on the role the server gives the program: every surface of
//! it is a prompt surface, above every other one, left out of captures and out of desktop control. A prompt shows
//! only while the session is unlocked and is the active one of its seat, which logind tells ([`logind`]); the
//! program finds its own logind session, which polkitd registers the agent for. The program ([`program`]) runs every
//! bus call and every read of the helper as a future on Kit's loop, and the work that blocks on Kit's I/O pool.
#![forbid(unsafe_code)]

pub mod consent;
pub mod keyring;
pub mod logind;
pub mod polkit;
pub mod program;
pub mod text;
pub mod users;
pub mod view;

use std::sync::{Mutex, PoisonError};

use crate::polkit::helper::Route;

/// How the program reaches what it works with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Options {
    /// The routes to polkit's helper, tried in order; `None` finds them where polkit installs them
    /// ([`Route::find`]).
    pub helper: Option<Vec<Route>>,
    /// Whether the program offers to move a default keyring that login does not unlock into the login keyring
    /// ([`keyring::asking`]).
    pub keyring_offer: bool,
}

impl Options {
    /// The program as the session runs it.
    pub fn installed() -> Options {
        Options {
            helper: None,
            keyring_offer: true,
        }
    }
}

/// The options of the program being started, taken by its first model.
static STARTING: Mutex<Option<Options>> = Mutex::new(None);

/// The options [`run`] was given; the installed ones when it was not.
pub(crate) fn starting() -> Options {
    STARTING
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .take()
        .unwrap_or_else(Options::installed)
}

/// Run the prompts with `options` on the connection the server gave the process, until it ends.
pub fn run(options: Options) -> Result<(), String> {
    *STARTING.lock().unwrap_or_else(PoisonError::into_inner) = Some(options);
    let environment = cupertino_kit::app::environment().map_err(|e| e.to_string())?;
    cupertino_kit_shell::run::<program::Prompts>(environment).map_err(|e| e.to_string())
}
