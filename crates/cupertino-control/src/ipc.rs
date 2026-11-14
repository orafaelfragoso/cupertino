//! Bounded local framing. The socket is not a grant: the compositor decides each run from the app the peer runs under,
//! named by the peer's process id and start time ([`Inbox::peer`]).
//!
//! Each peer has a reader and a writer thread of its own, so the controller's loop never blocks on a peer: a peer may
//! have one command in flight, and its response is written while the loop serves the other peers.
#[cfg(test)]
#[path = "ipc-tests.rs"]
mod tests;
use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TrySendError},
    Arc,
};
use std::time::Duration;

/// Maximum bytes in one incoming local command, including its newline.
pub const MAX_REQUEST_BYTES: usize = 64 * 1024;
/// Maximum response (a bounded PNG encoded as base64 plus its metadata).
pub const MAX_RESPONSE_BYTES: usize = 180 * 1024 * 1024;
/// Peer-exit checks are independent of whether another process keeps its socket open.
const PEER_CHECK: Duration = Duration::from_millis(250);
/// A stopped reader cannot leave the controller blocked on a response indefinitely.
const WRITE_TIMEOUT: Duration = Duration::from_secs(1);
/// Bytes of `/proc/<pid>/stat` read at most; the line is a few hundred bytes.
const STAT_LIMIT: u64 = 4096;
/// Responses waiting for a peer's writer beyond the one it writes: a peer has one command in flight, so a second
/// response waits only while the first is still written to a peer that sent its next command early.
const RESPONSE_QUEUE: usize = 1;

/// Makes the controller's loop look at its peers again.
pub type Wake = Arc<dyn Fn() + Send + Sync>;

/// A response for a peer's writer.
pub enum Reply {
    /// Known now.
    Now(Result<Value, String>),
    /// Worked out on the writer, off the controller's loop, as encoding a capture.
    Later(Box<dyn FnOnce() -> Result<Value, String> + Send>),
}

pub struct Listener {
    pub socket: UnixListener,
    path: PathBuf,
    inode: u64,
    _lock: std::fs::File,
}
pub fn socket_path(runtime: &Path, session: &str) -> io::Result<PathBuf> {
    if session.is_empty()
        || !session
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b))
    {
        return Err(io::Error::other("invalid session name"));
    }
    Ok(runtime
        .join("cupertino")
        .join(format!("control-{session}.sock")))
}
impl Listener {
    pub fn bind(runtime: &Path, session: &str) -> io::Result<Self> {
        let path = socket_path(runtime, session)?;
        let dir = path.parent().unwrap();
        let meta = std::fs::symlink_metadata(runtime)?;
        // Safety: getuid has no arguments or mutable process state.
        let uid = unsafe { libc::getuid() };
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other("runtime directory is not private"));
        }
        match std::fs::create_dir(dir) {
            Ok(()) => std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?,
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
        let meta = std::fs::symlink_metadata(dir)?;
        if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other("controller directory is not private"));
        }
        let lock = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(path.with_extension("lock"))?;
        let meta = lock.metadata()?;
        if !meta.is_file() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
            return Err(io::Error::other("controller lock is not private"));
        }
        // Safety: flock operates on the valid descriptor; this file keeps the lease until Listener drops.
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(io::Error::other("controller is already running"));
        }
        if let Ok(meta) = std::fs::symlink_metadata(&path) {
            if !meta.file_type().is_socket() || meta.uid() != uid {
                return Err(io::Error::other("controller socket path is occupied"));
            }
            std::fs::remove_file(&path)?;
        }
        // Bound under a name of its own, made private and listening, then moved into place: a client finds the
        // socket only once a connection to it is accepted, never between the bind and the listen.
        let staging = path.with_extension(format!("{}.new", std::process::id()));
        let _ = std::fs::remove_file(&staging);
        let socket = UnixListener::bind(&staging)?;
        let placed = std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o600))
            .and_then(|()| std::fs::rename(&staging, &path));
        if let Err(e) = placed {
            let _ = std::fs::remove_file(&staging);
            return Err(e);
        }
        let inode = std::fs::symlink_metadata(&path)?.ino();
        Ok(Self {
            socket,
            path,
            inode,
            _lock: lock,
        })
    }
}
impl Drop for Listener {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
pub struct Inbox {
    pub commands: Receiver<Vec<u8>>,
    /// Set once the peer exited, closed its end, broke the framing or stopped reading.
    pub closed: Arc<AtomicBool>,
    /// The peer's process id and start time in clock ticks since boot; `None` when it could not be read while the
    /// process was the one connected, and the compositor then refuses the run.
    pub peer: Option<(u32, u64)>,
    replies: SyncSender<Reply>,
    stream: UnixStream,
}
impl Inbox {
    /// Hand the response to the peer's writer. An error means the peer does not wait for responses in turn and is to
    /// be disconnected.
    pub fn respond(&self, reply: Reply) -> Result<(), &'static str> {
        match self.replies.try_send(reply) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Err("local client does not read its responses"),
            Err(TrySendError::Disconnected(_)) => Err("local client disconnected"),
        }
    }
}
impl Drop for Inbox {
    fn drop(&mut self) {
        // ends both threads, also a writer in the middle of a response
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }
}
/// The peer's user id, which must be the controller's own.
fn same_user(stream: &UnixStream) -> io::Result<()> {
    let mut credentials = libc::ucred {
        pid: 0,
        uid: 0,
        gid: 0,
    };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    // Safety: credentials and len are valid storage for SO_PEERCRED.
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            (&mut credentials as *mut libc::ucred).cast(),
            &mut len,
        )
    };
    if result != 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: getuid has no arguments or mutable process state.
    if credentials.uid != unsafe { libc::getuid() } {
        return Err(io::Error::other("controller peer has another uid"));
    }
    Ok(())
}
/// Tell a peer of the controller's own user at once why it is not served, then close it.
pub fn refuse(stream: UnixStream, error: &str) {
    if same_user(&stream).is_err() {
        return;
    }
    // a fresh connection's buffer takes the one line: this never waits on the peer
    let mut stream = stream;
    if stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_ok() {
        let _ = write_json(&mut stream, &json!({"ok":false,"error":error}));
    }
    let _ = stream.shutdown(std::net::Shutdown::Both);
}
/// Check a new peer and start its reader and writer. `wake` is called when a command arrives or the peer is gone.
pub fn read_peer(stream: &UnixStream, wake: Wake) -> io::Result<Inbox> {
    same_user(stream)?;
    let peer = cupertino_peer::ProcessHandle::of_peer(stream)?;
    let identity = start_time(peer.pid)
        // the start read names the connected process only while its pidfd shows it running
        .filter(|_| !peer.has_exited())
        .map(|start| (peer.pid, start));
    let (mut reader, closed, pending) = (
        stream.try_clone()?,
        Arc::new(AtomicBool::new(false)),
        Arc::new(AtomicBool::new(false)),
    );
    reader.set_read_timeout(Some(PEER_CHECK))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let (tx, commands) = mpsc::sync_channel(1);
    let (replies, responses) = mpsc::sync_channel::<Reply>(RESPONSE_QUEUE);
    let done = closed.clone();
    let busy = pending.clone();
    let waker = wake.clone();
    std::thread::spawn(move || {
        let mut line = Vec::new();
        let mut bytes = [0; 4096];
        'read: loop {
            if peer.has_exited() {
                break;
            }
            match reader.read(&mut bytes) {
                Ok(0) => break,
                Ok(n) => {
                    for &byte in &bytes[..n] {
                        line.push(byte);
                        if line.len() > MAX_REQUEST_BYTES {
                            break 'read;
                        }
                        if byte == b'\n' {
                            if busy.swap(true, Ordering::SeqCst)
                                || tx.try_send(std::mem::take(&mut line)).is_err()
                            {
                                break 'read;
                            }
                            waker();
                        }
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) => {}
                Err(_) => break,
            }
        }
        done.store(true, Ordering::SeqCst);
        let _ = reader.shutdown(std::net::Shutdown::Both);
        waker();
    });
    let mut writer = stream.try_clone()?;
    let (failed, idle) = (closed.clone(), pending);
    std::thread::spawn(move || {
        for reply in responses {
            let result = match reply {
                Reply::Now(result) => result,
                Reply::Later(work) => work(),
            };
            let value = match result {
                Ok(data) => json!({"ok":true,"data":data}),
                Err(error) => json!({"ok":false,"error":error}),
            };
            // the peer may send its next command as soon as it has read this response
            idle.store(false, Ordering::SeqCst);
            if write_json(&mut writer, &value).is_err() {
                failed.store(true, Ordering::SeqCst);
                let _ = writer.shutdown(std::net::Shutdown::Both);
                wake();
                break;
            }
        }
    });
    Ok(Inbox {
        commands,
        closed,
        peer: identity,
        replies,
        stream: stream.try_clone()?,
    })
}
/// Field 22 of `/proc/<pid>/stat`, the process's start time, from a bounded read. The command name in field 2 may hold
/// spaces and parentheses, so the fields are counted from the last `)`.
fn start_time(pid: u32) -> Option<u64> {
    let mut stat = String::new();
    std::fs::File::open(format!("/proc/{pid}/stat"))
        .ok()?
        .take(STAT_LIMIT)
        .read_to_string(&mut stat)
        .ok()?;
    start_of(&stat)
}
fn start_of(stat: &str) -> Option<u64> {
    let after_name = &stat[stat.rfind(')')? + 1..];
    after_name.split_whitespace().nth(19)?.parse().ok()
}
pub fn read_line(reader: &mut impl Read, limit: usize) -> io::Result<Vec<u8>> {
    let mut line = Vec::new();
    let mut byte = [0];
    loop {
        match reader.read(&mut byte)? {
            0 => {
                return if line.is_empty() {
                    Ok(line)
                } else {
                    Err(io::Error::other("truncated message"))
                }
            }
            _ => {
                line.push(byte[0]);
                if line.len() > limit {
                    return Err(io::Error::other("message exceeds limit"));
                }
                if byte[0] == b'\n' {
                    return Ok(line);
                }
            }
        }
    }
}
pub fn write_json(stream: &mut impl Write, value: &serde_json::Value) -> io::Result<()> {
    serde_json::to_writer(&mut *stream, value)?;
    stream.write_all(b"\n")?;
    stream.flush()
}
