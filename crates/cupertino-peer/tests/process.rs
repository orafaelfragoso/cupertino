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
fn a_process_is_held_by_its_pid_only_while_it_runs() {
    let handle = ProcessHandle::open(own_pid()).unwrap();
    assert_eq!(handle.pid, own_pid());
    assert!(!handle.has_exited());
    for pid in [0, u32::MAX] {
        assert_eq!(
            ProcessHandle::open(pid).unwrap_err().kind(),
            ErrorKind::InvalidInput,
            "pid {pid}"
        );
    }
    let gone = exited_process();
    assert!(gone.has_exited());
    assert!(ProcessHandle::open(gone.pid).is_err());
}

#[test]
fn a_signal_reaches_the_held_process_only() {
    // a window size change is ignored by default, so the test process goes on
    let handle = ProcessHandle::open(own_pid()).unwrap();
    handle.send(libc::SIGWINCH).unwrap();
    assert_eq!(
        handle.send(100_000).unwrap_err().kind(),
        ErrorKind::InvalidInput
    );
    assert!(exited_process().send(libc::SIGTERM).is_err());
}

#[test]
fn a_descriptor_is_held_only_when_it_is_a_pidfd_or_a_socket() {
    let held = ProcessHandle::open(own_pid()).unwrap();
    let fd = held.fd().try_clone_to_owned().unwrap();
    assert_eq!(ProcessHandle::from_pidfd(fd).unwrap().pid, own_pid());
    let (ours, _theirs) = UnixStream::pair().unwrap();
    assert_eq!(ProcessHandle::of_peer(&ours).unwrap().pid, own_pid());
    let file = || std::fs::File::open("/dev/null").unwrap();
    assert!(ProcessHandle::from_pidfd(file().into()).is_err());
    assert!(ProcessHandle::of_peer(&file()).is_err());
}

#[test]
fn two_handles_are_one_process_while_it_runs() {
    let own = ProcessHandle::open(own_pid()).unwrap();
    let clone = own.try_clone().unwrap();
    assert_eq!(clone.pid, own.pid);
    assert!(own.is(&clone));
    let parent = ProcessHandle::open(own_parent()).unwrap();
    assert_eq!(parent.pid, std::os::unix::process::parent_id());
    assert!(!own.is(&parent));
    assert!(!parent.is(&own));
    let gone = exited_process();
    assert!(!gone.is(&gone.try_clone().unwrap()));
}
