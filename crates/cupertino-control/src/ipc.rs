//! Bounded local framing. The socket is not a grant: the compositor decides each run from the app the peer runs under,
//! named by the peer's process id and start time ([`Inbox::peer`]).
#[cfg(test)]
#[path = "ipc-tests.rs"]
mod tests;
use cupertino_wayland_client::Waker;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver},
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
        let socket = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
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
    pub closed: Arc<AtomicBool>,
    pub pending: Arc<AtomicBool>,
    /// The peer's process id and start time in clock ticks since boot; `None` when it could not be read while the
    /// process was the one connected, and the compositor then refuses the run.
    pub peer: Option<(u32, u64)>,
}
pub fn read_peer(stream: &UnixStream, waker: Waker) -> io::Result<Inbox> {
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
    let done = closed.clone();
    let busy = pending.clone();
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
                            waker.wake();
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
        waker.wake();
    });
    Ok(Inbox {
        commands,
        closed,
        pending,
        peer: identity,
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
