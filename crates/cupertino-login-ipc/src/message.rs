//! The typed messages. Every message is a JSON object with a `type` tag; an unknown tag or field is a decode error.

use std::fmt;

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::hex;
use crate::secret::Secret;

/// The protocol version both sides send in their first message. A change that adds or removes a message increments it: the
/// login screen and the manager ship in one package, so there is no negotiation.
pub const VERSION: u32 = 5;

/// The largest frame the login screen sends: a 512-byte password in hex plus its envelope fits in 2 KiB.
pub const MAX_REQUEST_FRAME: usize = 2 * 1024;

/// The largest frame the manager sends. The design's 512 KiB cannot hold a user list with pictures: a 256 KiB picture is
/// 512 KiB of hex. approximate: 4 MiB holds [`MAX_PICTURE_BYTES_PER_LIST`] of pictures and [`MAX_USERS`] rows.
pub const MAX_EVENT_FRAME: usize = 4 * 1024 * 1024;

/// The largest picture of one user in bytes (approximate, ours).
pub const MAX_PICTURE_BYTES: usize = 256 * 1024;

/// The most picture bytes one user list carries; the manager leaves the pictures of further users out (approximate, ours).
pub const MAX_PICTURE_BYTES_PER_LIST: usize = 1024 * 1024;

/// The most rows one user list carries (approximate, ours); "Other…" reaches the rest.
pub const MAX_USERS: usize = 1024;

/// The most bytes of a user's text fields (login and display name) the manager sends (approximate, ours).
pub const MAX_NAME_BYTES: usize = 256;
#[derive(Clone, PartialEq, Eq)]
pub struct Picture(Vec<u8>);

/// The bytes are longer than [`MAX_PICTURE_BYTES`].
#[derive(Debug, PartialEq, Eq)]
pub struct PictureTooLarge;

impl Picture {
    pub fn new(bytes: Vec<u8>) -> Result<Self, PictureTooLarge> {
        if bytes.len() > MAX_PICTURE_BYTES {
            return Err(PictureTooLarge);
        }
        Ok(Picture(bytes))
    }

    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl fmt::Debug for Picture {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "<picture {} bytes>", self.0.len())
    }
}

impl Serialize for Picture {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&hex::encode(&self.0))
    }
}

struct PictureVisitor;

impl Visitor<'_> for PictureVisitor {
    type Value = Picture;

    fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("a hex-encoded picture")
    }

    fn visit_str<E: de::Error>(self, text: &str) -> Result<Picture, E> {
        let bytes = hex::decode(text).ok_or_else(|| E::custom("picture is not lower-case hex"))?;
        Picture::new(bytes.to_vec()).map_err(|_| E::custom("picture is too large"))
    }
}

impl<'de> Deserialize<'de> for Picture {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_str(PictureVisitor)
    }
}

/// The most bytes of an account name the screen may type for "Other…" (today's `NameEntry` limit, derived from `useradd(8)).
pub const MAX_LOGIN_NAME_BYTES: usize = 32;

/// Whether `name` can be an account name the manager passes to PAM: 1 to [`MAX_LOGIN_NAME_BYTES`] bytes, no whitespace, no
/// control character, none of `:`, `/` and `\`, and not starting with `-`. A name that passes may still be unknown; PAM decides.
pub fn is_login_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_LOGIN_NAME_BYTES
        && !name.starts_with('-')
        && !name
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || matches!(c, ':' | '/' | '\\'))
}

/// Which account an attempt is for.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Account {
    /// A row of the last user list the manager sent.
    Listed { uid: u32 },
    /// The name typed for the "Other…" tile; the manager validates it again.
    Typed { name: String },
}

/// What the power menu asks of the machine.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerAction {
    Restart,
    ShutDown,
    Sleep,
}

/// Why the manager does not carry out a [`PowerAction`]. A refused request changes nothing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PowerRefusal {
    /// Another user's session is open: restarting or shutting down would end it, and the request carries no administrator's
    /// authentication (the screen sends [`Request::AuthenticatePower`] for that).
    OtherUsersLoggedIn,
    /// A program holds a block inhibitor on the operation.
    Inhibited,
    /// The login settings hide the menu item for this action.
    Disabled,
    /// logind or the manager failed; the cause is in the journal only.
    Failed,
}

/// Messages from the login screen to the manager.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Request {
    /// The first message: the version the screen speaks.
    Hello { version: u32 },
    /// Start an attempt. Valid only when no attempt is open.
    Authenticate { account: Account },
    /// The answer to the `Prompt` with this `conversation`. Valid once, while that prompt is open; any other is dropped.
    Answer { conversation: u64, secret: Secret },
    /// Abort the open attempt (Escape, back to the picker). Always valid; with no attempt it is ignored.
    Cancel {},
    /// Restart or shut down with an administrator's authentication, for the case where [`PowerRefusal::OtherUsersLoggedIn`] would
    /// refuse a plain `Power`: `name` is the administrator's typed account name. An attempt like `Authenticate`, with
    /// prompts, answers and a `Result`; once it is accepted the manager carries the action out, and a refusal comes back as
    /// [`Event::PowerRefused`] followed by `Reset`. Valid only when no attempt is open; `action` is not `Sleep` (that needs no
    /// authentication).
    AuthenticatePower { name: String, action: PowerAction },
    /// A power menu item. Valid after `Hello`; a refusal comes back as [`Event::PowerRefused`], success as the machine going down.
    Power { action: PowerAction },
}

/// One account the screen lists.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UserRow {
    pub uid: u32,
    /// The account name PAM authenticates.
    pub login: String,
    /// The full name from the account's comment field, else the account name.
    pub display_name: String,
    pub picture: Option<Picture>,
    /// The account has a session the manager started: choosing it and authenticating returns to that session instead of
    /// starting a new one.
    pub logged_in: bool,
}

/// Which items of the power menu show; the screen hides the whole item when none does.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PowerMenu {
    pub restart: bool,
    pub shut_down: bool,
    pub sleep: bool,
}

/// How an attempt ended. `Denied` covers a wrong password and an unknown account: there is no "no such user".
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    /// The account is authenticated; the session starts.
    Accepted,
    Denied,
    /// The account is locked out (`pam_faillock`), or a PAM module said why the account is refused before the password was asked for.
    Locked,
    /// The account or its password has expired; the screen cannot change a password.
    Expired,
    /// The account may not log in (`PAM_PERM_DENIED`, `nologin`).
    Refused,
    /// The manager cannot take the attempt now.
    Busy,
    /// Anything else; the cause is in the journal only.
    Internal,
}

/// Why the manager refuses the connection or the last message; the connection closes after it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProtocolRefusal {
    /// The screen speaks another version.
    Version,
    /// The message is not allowed in the current state.
    State,
}

/// Messages from the manager to the login screen.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum Event {
    /// The first message: the version the manager speaks.
    Hello {
        version: u32,
    },
    /// Everything the screen needs; sent after `Hello`.
    ///
    /// `will_prompt` is true while a user session is open: Restart and Shut Down then ask for an administrator
    /// ([`Request::AuthenticatePower`]) and the menu shows them with an ellipsis.
    Welcome {
        users: Vec<UserRow>,
        show_other: bool,
        power: PowerMenu,
        will_prompt: bool,
        retries_until_hint: u32,
    },
    /// The user list, and `will_prompt` as in `Welcome`, changed (a session started or ended).
    Users {
        users: Vec<UserRow>,
        will_prompt: bool,
    },
    /// PAM asks. `echo` false is a password field; true is a visible field whose text also shows as the notice.
    Prompt {
        /// Names this prompt; the `Answer` repeats it.
        conversation: u64,
        echo: bool,
        text: String,
    },
    /// PAM info or error text. Not acknowledged: PAM's conversation returns at once.
    Notice {
        error: bool,
        text: String,
    },
    /// The end of an attempt.
    Result {
        outcome: Outcome,
    },
    /// The session of `uid` is being created; the spinner runs until `Reset`.
    SessionStarting {
        uid: u32,
    },
    /// The screen is shown again: clear every field and go back to the picker.
    Reset,
    Refused {
        reason: ProtocolRefusal,
    },
    /// The manager did not carry out a `Power` request.
    PowerRefused {
        reason: PowerRefusal,
    },
}
