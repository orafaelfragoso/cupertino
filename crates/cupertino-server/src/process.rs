//! The process behind a client connection, named so that a reused process id is another process.

use smithay::reexports::wayland_server::{Client, DisplayHandle};

/// A process id and the clock tick the process started at (`/proc/<pid>/stat`, field 22).
pub type ProcessIdentity = (u32, u64);

/// Names the process behind a client; [`peer_process`] in the server.
pub type ProcessOf = Box<dyn Fn(&DisplayHandle, &Client) -> Option<ProcessIdentity>>;

/// The process on the other end of `client`'s socket, from its credentials; `None` when they or `/proc` do not say.
pub fn peer_process(display: &DisplayHandle, client: &Client) -> Option<ProcessIdentity> {
    let pid = u32::try_from(client.get_credentials(display).ok()?.pid).ok()?;
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    Some((pid, start_time(&stat)?))
}

/// Field 22 of a `/proc/<pid>/stat` line. The command name in field 2 may hold spaces and parentheses, so the fields are
/// counted from the last `)`.
fn start_time(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_start_time_is_field_22_even_when_the_command_name_has_parentheses() {
        let stat = "4242 (a (b) c) S 1 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
        assert_eq!(start_time(stat), Some(987654));
    }

    #[test]
    fn a_line_cut_short_has_no_start_time() {
        assert_eq!(start_time("4242 (x) S 1 2"), None);
    }
}
