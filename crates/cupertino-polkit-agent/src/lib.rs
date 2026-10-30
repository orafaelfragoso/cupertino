//! The session's polkit authentication agent. polkitd asks it to authenticate someone for another program's
//! action; the agent shows the prompt and passes what the person types to polkit's setuid helper, which alone checks
//! the password and tells polkitd the result. No Cupertino daemon asks polkit (architecture rule 5).
//!
//! - [`agent`]: the agent on the system bus, which only polkitd may ask.
//! - [`text`]: the strings polkitd and PAM send, made safe to show.
//! - [`helper`]: the conversation with polkit's helper.
//! - [`model`]: the prompt and the requests waiting for it, as a pure model.

pub mod agent;
pub mod helper;
pub mod model;
pub mod text;
