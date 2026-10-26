//! What a hot corner or a touchpad gesture does. The server detects them (dwell, recognizer) and
//! tells the dock of the output; the dock maps them here and carries the action out: nothing, its
//! own application grid, a program, or one of the server's named actions (`run_action`). The
//! triggers, actions and their defaults are `dock.conf` vocabulary, defined with the file in
//! `cupertino_defaults::dock`.

pub use cupertino_defaults::dock::{
    action_text, parse_action, ActionMap, ActionTarget, Trigger, TRIGGER_KEYS,
};
