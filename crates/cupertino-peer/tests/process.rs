//! Holding a process by its pidfd.

use cupertino_peer::{own_parent, ProcessHandle};
use std::io::ErrorKind;
use std::os::unix::net::UnixStream;

fn own_pid() -> u32 {
    std::process::id()
}

/// A process that has exited and been reaped, held through a pidfd opened while it was still a zombie.
fn exited_process() -> ProcessHandle {
    let mut child = std::process::Command::new("true").spawn().unwrap();
    let handle = ProcessHandle::open(child.id()).unwrap();
    child.wait().unwrap();
    handle
}

#[test]
fn a_running_process_is_held_by_its_pid() {
    let handle = ProcessHandle::open(own_pid()).unwrap();
    assert_eq!(handle.pid, own_pid());
    assert!(!handle.has_exited());
}

#[test]
fn a_pid_that_cannot_exist_is_refused_as_invalid_input() {
    for pid in [0, u32::MAX] {
        assert_eq!(
            ProcessHandle::open(pid).unwrap_err().kind(),
            ErrorKind::InvalidInput,
            "pid {pid}"
        );
    }
}

#[test]
fn a_process_that_is_gone_cannot_be_held() {
    let handle = exited_process();
    assert!(ProcessHandle::open(handle.pid).is_err());
}

#[test]
fn a_held_process_that_exits_reports_it() {
    let handle = exited_process();
    assert!(handle.has_exited());
}

#[test]
fn a_signal_to_a_process_that_exited_fails() {
    let handle = exited_process();
    assert!(handle.send(libc::SIGTERM).is_err());
}

#[test]
fn a_signal_reaches_the_held_process() {
    // a window size change is ignored by default, so the test process goes on
    let handle = ProcessHandle::open(own_pid()).unwrap();
    handle.send(libc::SIGWINCH).unwrap();
}

#[test]
fn a_signal_number_that_does_not_exist_is_invalid_input() {
    let handle = ProcessHandle::open(own_pid()).unwrap();
    assert_eq!(
        handle.send(100_000).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
}

#[test]
fn a_descriptor_gives_the_pid_the_kernel_names_for_it() {
    let held = ProcessHandle::open(own_pid()).unwrap();
    let fd = held.fd().try_clone_to_owned().unwrap();
    let from_fd = ProcessHandle::from_pidfd(fd).unwrap();
    assert_eq!(from_fd.pid, own_pid());
}

#[test]
fn a_descriptor_that_is_not_a_pidfd_is_refused() {
    let file = std::fs::File::open("/dev/null").unwrap();
    assert!(ProcessHandle::from_pidfd(file.into()).is_err());
}

#[test]
fn a_clone_holds_the_same_process() {
    let handle = ProcessHandle::open(own_pid()).unwrap();
    let clone = handle.try_clone().unwrap();
    assert_eq!(clone.pid, handle.pid);
    assert!(handle.is(&clone));
}

#[test]
fn two_handles_are_one_process_only_with_the_same_pid() {
    let own = ProcessHandle::open(own_pid()).unwrap();
    let parent = ProcessHandle::open(own_parent()).unwrap();
    assert!(!own.is(&parent));
    assert!(!parent.is(&own));
}

#[test]
fn a_handle_of_a_process_that_exited_is_no_longer_the_same_process() {
    let gone = exited_process();
    let clone = gone.try_clone().unwrap();
    assert!(!gone.is(&clone));
}

#[test]
fn the_parent_of_this_process_is_the_kernels() {
    assert_eq!(own_parent(), std::os::unix::process::parent_id());
    assert!(own_parent() > 0);
}

#[test]
fn the_process_at_the_other_end_of_a_socket_is_held() {
    let (ours, _theirs) = UnixStream::pair().unwrap();
    let peer = ProcessHandle::of_peer(&ours).unwrap();
    assert_eq!(peer.pid, own_pid());
}

#[test]
fn a_descriptor_that_is_no_socket_has_no_peer() {
    let file = std::fs::File::open("/dev/null").unwrap();
    assert!(ProcessHandle::of_peer(&file).is_err());
}
