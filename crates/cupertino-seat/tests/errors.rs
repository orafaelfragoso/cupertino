//! What the errors say, which one chains to a cause, and which carry an errno for Smithay.

use cupertino_devices::DevT;
use cupertino_seat::Error;
use rustix::io::Errno;
use smithay::backend::session::AsErrno;
use std::error::Error as _;
use std::path::PathBuf;

#[test]
fn errors_name_the_device_descriptor_or_text_they_are_about() {
    assert_eq!(
        Error::AlreadyTaken(DevT::new(226, 128)).to_string(),
        "device 226:128 is already taken"
    );
    assert_eq!(
        Error::UnknownFd(7).to_string(),
        "descriptor 7 was not handed out by this session"
    );
    assert_eq!(
        Error::InvalidVt(-1).to_string(),
        "invalid virtual terminal number -1"
    );
    assert_eq!(
        Error::UnknownPauseType("sleep".to_owned()).to_string(),
        "unknown PauseDevice type \"sleep\""
    );
    assert_eq!(
        Error::NotCharDevice(PathBuf::from("/tmp/file")).to_string(),
        "/tmp/file is not a character device"
    );
    assert_eq!(Error::SessionLost.to_string(), "the session is closed");
    assert_eq!(
        Error::SignalsEnded.to_string(),
        "the logind signal stream ended"
    );
    assert_eq!(
        Error::SessionIdNotUnicode.to_string(),
        "XDG_SESSION_ID is not valid Unicode"
    );
}

#[test]
fn a_stat_failure_names_the_path_and_carries_the_errno_for_smithay() {
    let error = Error::Stat {
        path: PathBuf::from("/dev/dri/card0"),
        errno: Errno::NOENT,
    };
    assert!(error.to_string().starts_with("stat /dev/dri/card0: "));
    assert_eq!(error.as_errno(), Some(Errno::NOENT.raw_os_error()));
}

#[test]
fn only_a_stat_failure_has_an_errno() {
    assert_eq!(Error::SessionLost.as_errno(), None);
    assert_eq!(Error::UnknownFd(1).as_errno(), None);
    assert_eq!(Error::AlreadyTaken(DevT::new(1, 1)).as_errno(), None);
}

#[test]
fn a_bus_failure_names_the_step_and_chains_to_the_cause() {
    let error = Error::Bus {
        what: "TakeControl",
        source: Box::new(zbus::Error::InvalidReply),
    };
    assert!(error.to_string().starts_with("TakeControl: "));
    assert!(error.source().is_some());
}

#[test]
fn a_thread_failure_chains_to_the_io_error() {
    let error = Error::Thread(std::io::Error::other("no threads left"));
    assert_eq!(
        error.to_string(),
        "starting the signal reader: no threads left"
    );
    assert!(error.source().is_some());
}

#[test]
fn errors_without_a_cause_chain_to_nothing() {
    assert!(Error::SessionLost.source().is_none());
    assert!(Error::ActiveInvalidated.source().is_none());
    assert!(Error::UnknownFd(3).source().is_none());
}
