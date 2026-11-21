//! The session's polkit authentication agent. polkitd asks it to authenticate someone for another program's action;
//! the agent shows the prompt and passes what the person types to polkit's setuid helper, which alone checks the
//! password and tells polkitd the result. No Cupertino daemon asks polkit (architecture rule 5).
//!
//! - [`agent`]: the agent on the system bus, which only polkitd may ask, registered for this process's logind session
//!   and again whenever polkitd starts anew.
//! - [`helper`]: the conversation with polkit's helper.
//! - [`model`]: the prompt and the requests waiting for it, as a pure model.

pub mod agent;
pub mod helper;
pub mod model;

use crate::text::shown;

/// A user polkitd will accept a password of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct User(pub u32);

/// One authentication polkitd asks for. Every text is already made safe to show.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub cookie: String,
    pub action: String,
    pub message: String,
    pub details: Vec<(String, String)>,
    /// The users polkitd accepts, in its order; identities other than users are left out.
    pub users: Vec<User>,
}

/// How an authentication ended for polkitd.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The helper ran and told polkitd its result, or nobody could be authenticated.
    Done,
    /// The person dismissed the prompt, or polkitd cancelled it.
    Cancelled,
}

/// The answer to one authentication polkitd asked for. Dropping its last clone unanswered counts as
/// [`Outcome::Cancelled`].
#[derive(Clone, Debug)]
pub struct Reply(pub(crate) async_channel::Sender<Outcome>);

impl Reply {
    /// A reply and what hears it, for a host that asks the agent itself.
    pub fn new() -> (Reply, async_channel::Receiver<Outcome>) {
        let (sender, receiver) = async_channel::bounded(1);
        (Reply(sender), receiver)
    }

    pub fn send(&self, outcome: Outcome) {
        let _ = self.0.try_send(outcome);
    }
}

/// What the agent hands to whoever shows the prompt.
#[derive(Clone, Debug)]
pub enum Event {
    /// Show `Request`; answer with `Reply` exactly once.
    Begin(Request, Reply),
    /// polkitd no longer wants the authentication of this cookie.
    Cancel(String),
}

/// The rows of the prompt's Details disclosure for `request`: the application the person used (polkitd's subject),
/// the program that asked polkitd (its caller) when that is another one, and the action. Both programs are the
/// executables the kernel names for polkitd's process ids, never what a process says about itself; a process the
/// session's user may not inspect (one running as root) or that is gone has no row. Reads `/proc`: run it on the I/O
/// pool.
pub fn detail_rows(request: &Request) -> Vec<(&'static str, String)> {
    let application = executable(request, "polkit.subject-pid");
    let program = executable(request, "polkit.caller-pid")
        .filter(|program| application.as_ref() != Some(program));
    application
        .map(|a| ("Application", a))
        .into_iter()
        .chain(program.map(|p| ("Program", p)))
        .chain([("Action", request.action.clone())])
        .collect()
}

/// The executable of the process whose id is the detail `key`.
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
fn executable(request: &Request, key: &str) -> Option<String> {
    let pid: u32 = request
        .details
        .iter()
        .find(|(k, _)| k == key)?
        .1
        .parse()
        .ok()?;
    let exe = std::fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    Some(shown(&exe.to_string_lossy()))
}
