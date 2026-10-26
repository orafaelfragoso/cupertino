//! Virtual terminals the manager holds open. An open `/dev/ttyN` keeps the terminal out of the free list that
//! `VT_OPENQRY` reads, so two sessions are never given one terminal.

use std::fs::File;
use std::io;
use std::os::unix::fs::OpenOptionsExt;

/// The terminal device `vt`.
pub fn device(vt: u32) -> String {
    format!("/dev/tty{vt}")
}

/// Open the terminal `vt` without making it this process's controlling terminal; the descriptor is the hold.
pub fn hold(vt: u32) -> io::Result<File> {
    File::options().read(true).write(true).custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC).open(device(vt))
}

/// `VT_OPENQRY` of `<linux/vt.h>` (`vt_ioctl(2)): the first virtual terminal that no process has open.
const VT_OPENQRY: libc::c_ulong = 0x5600;

/// Hold the first unused virtual terminal above `login_vt`, or `None` when there is none. The query returns the lowest unused
/// terminal; those at or below the login screen's are held for the length of the search so the next query goes on. Holding
/// the answer closes the race with a second allocation.
pub fn allocate(login_vt: u32) -> io::Result<Option<(u32, File)>> {
    let console = File::options().read(true).write(true).custom_flags(libc::O_NOCTTY | libc::O_CLOEXEC).open("/dev/tty0")?;
    let mut skipped = Vec::new();
    loop {
        let mut vt: libc::c_int = 0;
        // Safety: `VT_OPENQRY` stores one `int` through the pointer.
        if unsafe { libc::ioctl(std::os::fd::AsRawFd::as_raw_fd(&console), VT_OPENQRY, &mut vt) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let Ok(vt) = u32::try_from(vt) else { return Ok(None) };
        let terminal = hold(vt)?;
        if vt > login_vt {
            return Ok(Some((vt, terminal)));
        }
        skipped.push(terminal);
    }
}
