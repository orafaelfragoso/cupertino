//! The compositor's last word on a connection that failed: a compositor that ends a client for a protocol error sends
//! `wl_display.error` and closes the socket. When the client was writing at that moment, the write fails (the socket
//! was reset) and the client library stops reading, so the error is still unread in the socket. [`unread_error`] reads
//! what is left and finds it, so the client can say why it was disconnected instead of naming the reset.

use std::os::fd::BorrowedFd;

use rustix::net::{recv, RecvFlags};
use wayland_client::backend::protocol::ProtocolError;

/// The protocol id of `wl_display`, which sends the error.
const DISPLAY_ID: u32 = 1;
/// `wl_display.error`'s opcode.
const ERROR_OPCODE: u16 = 0;
/// The header (sender id, size and opcode) and the error's object and code before its message.
const FIXED_BYTES: usize = 16;
/// The most bytes read from the socket: the connection is dead, so this is only how far back the error is looked for.
const MOST_READ: usize = 1 << 20;

/// The `wl_display.error` the compositor sent before it closed the socket on `fd`, read from what the client had not
/// read yet; `None` when nothing is left or the bytes do not end with one. The connection must be dead: the bytes read
/// here are gone for the client library.
pub(crate) fn unread_error(fd: BorrowedFd<'_>) -> Option<ProtocolError> {
    let mut bytes = Vec::new();
    let mut chunk = vec![0u8; 64 * 1024];
    while bytes.len() < MOST_READ {
        match recv(fd, &mut chunk[..], RecvFlags::DONTWAIT) {
            Ok((0, _)) | Err(_) => break,
            Ok((read, _)) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
    display_error(&bytes)
}

fn word(bytes: &[u8], at: usize) -> Option<u32> {
    let word = bytes.get(at..at + 4)?;
    Some(u32::from_ne_bytes(word.try_into().ok()?))
}

/// The `wl_display.error` message `bytes` end with: a whole message from `wl_display`, opcode 0, whose size reaches
/// exactly to the end, with an object, a code and a NUL-terminated message. The bytes may start inside an earlier
/// message, so each offset that leaves a whole number of words is tried from the front. The interface is not known
/// here: the object's id names the object.
fn display_error(bytes: &[u8]) -> Option<ProtocolError> {
    let len = bytes.len();
    if len < FIXED_BYTES + 4 {
        return None;
    }
    (len % 4..=len - FIXED_BYTES - 4).step_by(4).find_map(|at| {
        let message = &bytes[at..];
        let size_opcode = word(message, 4)?;
        let (size, opcode) = ((size_opcode >> 16) as usize, (size_opcode & 0xffff) as u16);
        if word(message, 0)? != DISPLAY_ID || opcode != ERROR_OPCODE || size != message.len() {
            return None;
        }
        let object_id = word(message, 8)?;
        let code = word(message, 12)?;
        let text_len = word(message, FIXED_BYTES)? as usize;
        let text = message.get(FIXED_BYTES + 4..FIXED_BYTES + 4 + text_len)?;
        let (&nul, text) = text.split_last()?;
        if nul != 0 || FIXED_BYTES + 4 + text_len.div_ceil(4) * 4 != size {
            return None;
        }
        Some(ProtocolError {
            code,
            object_id,
            // as the client library names an object it does not know
            object_interface: String::from("<unknown>"),
            message: String::from_utf8_lossy(text).into_owned(),
        })
    })
}
