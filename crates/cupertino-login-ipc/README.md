# cupertino-login-ipc

The protocol between the login screen (`cupertino-login --greeter`, unprivileged) and the session manager (`cupertino-sessiond`,
root) over `/run/cupertino-login/greeter.sock`. Pure: bytes in, bytes out, no I/O, so both sides and their tests share one
definition. Layer: `models`. The design is, section 2.4. Passwords travel on this socket
only, never on D-Bus.

## Public API

| Item | Contents |
|---|---|
| `Request` | screen to manager: `Hello { version }`, `Authenticate { account }` (valid with no attempt open), `Answer { conversation, secret }` (valid once, while the prompt with that `conversation` is open; any other answer is dropped), `AuthenticatePower { name, action }` (an administrator's authentication for Restart or Shut Down while a user session is open, an attempt like `Authenticate`), `Cancel` (always valid, ignored with nothing to abort), `Power { action }` (valid after `Hello` on every screen; the machine going down is the success answer) |
| `Account` | `Listed { uid }` (a row of the last list) or `Typed { name }` (the "Other…" name); `is_login_name` is the rule for typed names: 1 to 32 bytes, no whitespace or control character, none of `:` `/` `\`, not starting with `- |
| `Event` | manager to screen: `Hello { version }`, `Welcome { users, show_other, power, will_prompt, retries_until_hint }`, `Users { users, will_prompt }` (`will_prompt`: a user session is open, so Restart and Shut Down ask for an administrator and show an ellipsis), `Prompt { conversation, echo, text }`, `Notice { error, text }`, `Result { outcome }`, `SessionStarting { uid }`, `Reset`, `Refused { reason }`, `PowerRefused { reason } |
| `Outcome` | `Accepted`, `Denied` (a wrong password and an unknown account alike), `Locked`, `Expired`, `Refused`, `Busy`, `Internal` |
| `UserRow`, `Picture`, `PowerMenu`, `ProtocolRefusal` | the rows of the picker (uid, login, full name, optional picture, `logged_in`: the account has a session, so choosing it returns to that session), which power items show, why a connection is refused |
| `PowerAction`, `PowerRefusal` | `Restart`, `ShutDown`, `Sleep`; why the manager did not do one: `OtherUsersLoggedIn`, `Inhibited`, `Disabled` (the settings hide the item), `Failed` (the cause is in the journal) |
| `Decoder` | reads a stream of unknown chunking in exact amounts: `wanted` bytes at most, `push(bytes) returns a frame body when one completes |
| `encode`, `encode_secret`, `decode` | a message to a frame (length prefix, JSON) and back; `encode_secret` allocates its buffer at the full cap so it never moves |
| `Secret` | a password: `Zeroizing<String>`, at most 512 bytes (`PAM_MAX_RESP_SIZE`), `Debug` prints `<secret>, sent as hex |
| `State` | the connection's state machine (`New`, `Idle`, `Authenticating`, `Starting`): `next(request)` and `after(event) |

## Rules

- **Framing:** a `u32` little-endian length, then JSON with a `type` tag. Unknown tags and fields are decode errors
  (`deny_unknown_fields`).
- **Caps:** frames to the manager are at most 2 KiB (`MAX_REQUEST_FRAME`: a 512-byte password in hex plus its envelope);
  frames to the screen at most 4 MiB (`MAX_EVENT_FRAME`). The length prefix is checked before the body is read, so a decoder never
  buffers more than its cap. A zero length is an error.
- **Why 4 MiB, not the design's 512 KiB (approximate, ours):** a picture is binary, and JSON carries it as hex, so one 256 KiB
  picture is 512 KiB of text. The manager sends at most 1 MiB of pictures and 1024 rows per list, which fits.
- **Redaction:** a decode error keeps only its class (`DecodeError::class`: `io`, `syntax`, `data`, `eof`). serde's own text
  embeds the offending value (`invalid type: string "hunter2"), which may be a password, so it is dropped at the boundary and no
  log line may print more.
- **Hex for secrets and pictures:** a hex string holds no character JSON escapes, so the parser never copies it into its scratch
  buffer, which is not zeroed. `Secret` is zeroed on drop; the decoder's body buffers and `encode`'s output are `Zeroizing`.
- **Version:** `VERSION` is 5 (2 added `Power` and `PowerRefused`, 3 `UserRow::logged_in`, 4 `AuthenticatePower` and `will_prompt`, 5 the `conversation` of `Prompt` and `Answer`), the first message each way carries it. A change that adds or removes a message increments it; the
  screen and the manager ship in one package, so there is no negotiation.

## Tests

`cargo test -p cupertino-login-ipc`: a secret round-trips, is hex on the wire, never shows in `Debug` and a malformed one is a decode
error that does not echo it; the state machine follows an attempt and refuses every pair it must; typed account names; a frame survives every chunking; a length over the cap is refused before its body is read,
a zero length and an overrun are refused; a message over the cap is not encoded; unknown types, unknown fields and wrong field
types are errors; a decode error names its class and never the value; the first message must be `Hello` with this version; a power request has one wire form and no other action decodes; the
hex codec round-trips and refuses malformed text.
