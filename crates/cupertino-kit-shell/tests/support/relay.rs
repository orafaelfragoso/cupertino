//! A relay between a component and the stand-in: it passes everything both ways, file descriptors included, and counts
//! what the component sent, so a test knows whether the component sent anything after a moment it names.

use std::io::{IoSlice, IoSliceMut};
use std::mem::MaybeUninit;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::{Arc, Condvar, Mutex, PoisonError};

use rustix::event::{poll, PollFd, PollFlags};
use rustix::net::{
    recvmsg, sendmsg, RecvAncillaryBuffer, RecvAncillaryMessage, RecvFlags, SendAncillaryBuffer,
    SendAncillaryMessage, SendFlags,
};

use super::PATIENCE;

/// The most descriptors one read carries.
const MAX_FDS: usize = 28;
/// The bytes one read takes at most.
const CHUNK: usize = 4096;

/// What the relay counts of the component's side.
pub struct Relay {
    /// The bytes the relay read from the component; locked across each read, so that with what waits unread it is
    /// all the component sent.
    read: Mutex<u64>,
    /// The relay's end of the component's socket, whose unread bytes count as sent.
    component: UnixStream,
    /// The component's side closed.
    closed: (Mutex<bool>, Condvar),
}

impl Relay {
    /// Relays between `component`, the relay's end of the component's socket, and `server`, the stand-in's client end.
    pub fn start(component: UnixStream, server: UnixStream) -> Arc<Relay> {
        let relay = Arc::new(Relay {
            read: Mutex::new(0),
            component: component.try_clone().expect("relay socket"),
            closed: (Mutex::new(false), Condvar::new()),
        });
        let running = Arc::clone(&relay);
        std::thread::Builder::new()
            .name("stand-in-relay".into())
            .spawn(move || running.run(&component, &server))
            .expect("relay thread");
        relay
    }

    /// The bytes the component sent so far: those the relay read and those waiting for it.
    pub fn sent(&self) -> u64 {
        let read = self.read.lock().unwrap_or_else(PoisonError::into_inner);
        *read + rustix::io::ioctl_fionread(&self.component).expect("unread bytes")
    }

    /// Waits up to [`PATIENCE`] for the component's side to close; the bytes it sent in all.
    pub fn closed(&self) -> u64 {
        let (closed, changed) = &self.closed;
        let closed = closed.lock().unwrap_or_else(PoisonError::into_inner);
        let (closed, _) = changed
            .wait_timeout_while(closed, PATIENCE, |closed| !*closed)
            .unwrap_or_else(PoisonError::into_inner);
        assert!(*closed, "the component's connection did not close");
        *self.read.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(&self, component: &UnixStream, server: &UnixStream) {
        let mut server_open = true;
        loop {
            let mut fds = vec![PollFd::new(component, PollFlags::IN)];
            if server_open {
                fds.push(PollFd::new(server, PollFlags::IN));
            }
            if poll(&mut fds, None).is_err() {
                continue;
            }
            let from_component = !fds[0].revents().is_empty();
            let from_server = fds.get(1).is_some_and(|fd| !fd.revents().is_empty());
            if from_component {
                let mut read = self.read.lock().unwrap_or_else(PoisonError::into_inner);
                match receive(component) {
                    Some((bytes, fds)) => {
                        *read += bytes.len() as u64;
                        drop(read);
                        send(server, &bytes, &fds);
                    }
                    None => {
                        drop(read);
                        let _ = server.shutdown(std::net::Shutdown::Both);
                        let (closed, changed) = &self.closed;
                        *closed.lock().unwrap_or_else(PoisonError::into_inner) = true;
                        changed.notify_all();
                        return;
                    }
                }
            }
            if from_server {
                match receive(server) {
                    Some((bytes, fds)) => send(component, &bytes, &fds),
                    None => {
                        server_open = false;
                        let _ = component.shutdown(std::net::Shutdown::Both);
                    }
                }
            }
        }
    }
}

/// What one read of `socket` gave; `None` once it closed or failed.
fn receive(socket: &UnixStream) -> Option<(Vec<u8>, Vec<OwnedFd>)> {
    let mut bytes = vec![0u8; CHUNK];
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut ancillary = RecvAncillaryBuffer::new(&mut space);
    let got = recvmsg(
        socket,
        &mut [IoSliceMut::new(&mut bytes)],
        &mut ancillary,
        RecvFlags::DONTWAIT | RecvFlags::CMSG_CLOEXEC,
    );
    let got = match got {
        Ok(got) => got.bytes,
        Err(rustix::io::Errno::AGAIN | rustix::io::Errno::INTR) => {
            return Some((Vec::new(), Vec::new()))
        }
        Err(_) => return None,
    };
    let mut fds = Vec::new();
    for message in ancillary.drain() {
        if let RecvAncillaryMessage::ScmRights(rights) = message {
            fds.extend(rights);
        }
    }
    if got == 0 && fds.is_empty() {
        return None;
    }
    bytes.truncate(got);
    Some((bytes, fds))
}

/// Writes `bytes` with `fds` to `socket`; a side that closed takes nothing.
fn send(socket: &UnixStream, bytes: &[u8], fds: &[OwnedFd]) {
    if bytes.is_empty() && fds.is_empty() {
        return;
    }
    let borrowed: Vec<BorrowedFd<'_>> = fds.iter().map(AsFd::as_fd).collect();
    let mut space = [MaybeUninit::uninit(); rustix::cmsg_space!(ScmRights(MAX_FDS))];
    let mut ancillary = SendAncillaryBuffer::new(&mut space);
    if !borrowed.is_empty() {
        ancillary.push(SendAncillaryMessage::ScmRights(&borrowed));
    }
    let mut left = bytes;
    let mut first = true;
    while !left.is_empty() || first {
        let mut none = SendAncillaryBuffer::default();
        let control = if first { &mut ancillary } else { &mut none };
        match sendmsg(socket, &[IoSlice::new(left)], control, SendFlags::NOSIGNAL) {
            Ok(n) => left = &left[n..],
            Err(rustix::io::Errno::INTR) => continue,
            Err(_) => return,
        }
        first = false;
    }
}
