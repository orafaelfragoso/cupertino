//! The session's polkit authentication agent. polkitd asks it to authenticate someone for another program's
//! action; the agent shows the prompt and passes what the person types to polkit's setuid helper, which alone checks
//! the password and tells polkitd the result. No Cupertino daemon asks polkit (architecture rule 5).
//!
//! - [`access`]: what the prompt tells assistive technology.
//! - [`agent`]: the agent on the system bus, which only polkitd may ask.
//! - [`conversation`]: the model's effects carried out: the helper started and answered, polkitd answered.
//! - `surfaces` (with `app`): the scrims' and the panel's layer surfaces.
//! - [`text`]: the strings polkitd and PAM send, made safe to show.
//! - [`helper`]: the conversation with polkit's helper.
//! - [`keyring`]: the keyring's prompter, which asks the keyring's password questions in the same prompt.
//! - [`model`]: the prompt and the requests waiting for it, as a pure model.
//! - [`panel`]: where the prompt's parts sit and what a key or a click on them does.
//! - [`render`]: the scrim and the panel drawn.
//! - [`users`]: how the prompt names the users polkitd offers.
//! - [`zeroing`]: the process's allocator, which zeroes every block it frees.

pub mod access;
pub mod agent;
pub mod conversation;
pub mod helper;
pub mod keyring;
pub mod model;
pub mod panel;
pub mod render;
#[cfg(feature = "app")]
pub mod surfaces;
pub mod text;
pub mod users;
pub mod zeroing;
