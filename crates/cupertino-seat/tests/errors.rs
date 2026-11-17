//! Which errors carry an errno for Smithay.

use cupertino_devices::DevT;
use cupertino_seat::Error;
use rustix::io::Errno;
use smithay::backend::session::AsErrno;
use std::path::PathBuf;

#[test]
fn only_a_stat_failure_carries_its_errno_for_smithay() {
    let stat = Error::Stat {
        path: PathBuf::from("/dev/dri/card0"),
        errno: Errno::NOENT,
    };
    for (error, expected) in [
        (stat, Some(Errno::NOENT.raw_os_error())),
        (Error::SessionLost, None),
        (Error::UnknownFd(1), None),
        (Error::AlreadyTaken(DevT::new(1, 1)), None),
    ] {
        assert_eq!(error.as_errno(), expected, "{error:?}");
    }
}
