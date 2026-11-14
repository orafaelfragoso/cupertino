//! DRM syncobj timelines on a render node: a native fence (a sync file) becomes a point on a timeline, the acquire
//! point a compositor waits for before it reads a buffer, and a release point the compositor signals once it is done
//! is waited for through an eventfd an event loop polls. The calls are the kernel's syncobj requests on the node; no
//! library is needed.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};
use std::path::Path;

/// A render node opened for syncobj requests.
#[derive(Debug)]
pub struct SyncNode(File);

/// One timeline: the points a buffer's acquire and release are on, and the binary syncobj a sync file passes through
/// on its way to a point.
#[derive(Debug)]
pub struct Timeline {
    handle: u32,
    staging: u32,
}

impl Timeline {
    /// The kernel's handle of the timeline on its node.
    pub fn handle(&self) -> u32 {
        self.handle
    }
}

#[repr(C)]
struct Create {
    handle: u32,
    flags: u32,
}

#[repr(C)]
struct Destroy {
    handle: u32,
    pad: u32,
}

#[repr(C)]
struct Handle {
    handle: u32,
    flags: u32,
    fd: i32,
    pad: u32,
    point: u64,
}

#[repr(C)]
struct Transfer {
    src_handle: u32,
    dst_handle: u32,
    src_point: u64,
    dst_point: u64,
    flags: u32,
    pad: u32,
}

#[repr(C)]
struct TimelineArray {
    handles: u64,
    points: u64,
    count_handles: u32,
    flags: u32,
}

#[repr(C)]
struct EventFd {
    handle: u32,
    flags: u32,
    point: u64,
    fd: i32,
    pad: u32,
}

/// `DRM_IOWR(nr, T)`: read and write, the size of `T`, the DRM type `'d'`.
const fn request<T>(nr: u64) -> u64 {
    const READ_WRITE: u64 = 3;
    (READ_WRITE << 30) | ((std::mem::size_of::<T>() as u64) << 16) | ((b'd' as u64) << 8) | nr
}

const CREATE: u64 = request::<Create>(0xBF);
const DESTROY: u64 = request::<Destroy>(0xC0);
const HANDLE_TO_FD: u64 = request::<Handle>(0xC1);
const FD_TO_HANDLE: u64 = request::<Handle>(0xC2);
const QUERY: u64 = request::<TimelineArray>(0xCB);
const TRANSFER: u64 = request::<Transfer>(0xCC);
const EVENTFD: u64 = request::<EventFd>(0xCF);

/// Import a sync file into an existing syncobj, which then holds its fence.
const IMPORT_SYNC_FILE: u32 = 1 << 0;

impl SyncNode {
    /// The render node at `path`, opened read-write.
    pub fn open(path: &Path) -> io::Result<SyncNode> {
        std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .map(SyncNode)
    }

    fn ioctl<T>(&self, request: u64, args: &mut T) -> io::Result<()> {
        loop {
            // Safety: `request` encodes the size of `T`, and `args` lives across the call.
            let r = unsafe {
                libc::ioctl(self.0.as_raw_fd(), request as libc::c_ulong, args as *mut T)
            };
            if r == 0 {
                return Ok(());
            }
            let e = io::Error::last_os_error();
            if !matches!(e.raw_os_error(), Some(libc::EINTR) | Some(libc::EAGAIN)) {
                return Err(e);
            }
        }
    }

    fn create(&self) -> io::Result<u32> {
        let mut args = Create {
            handle: 0,
            flags: 0,
        };
        self.ioctl(CREATE, &mut args)?;
        Ok(args.handle)
    }

    fn destroy_handle(&self, handle: u32) {
        let mut args = Destroy { handle, pad: 0 };
        let _ = self.ioctl(DESTROY, &mut args);
    }

    fn eventfd_at(&self, handle: u32, point: u64, event: BorrowedFd<'_>) -> io::Result<()> {
        let mut args = EventFd {
            handle,
            flags: 0,
            point,
            fd: event.as_raw_fd(),
            pad: 0,
        };
        self.ioctl(EVENTFD, &mut args)
    }

    /// Whether the kernel signals an eventfd at a timeline point (Linux 6.6), which release waits need.
    pub fn supports_eventfd(&self) -> bool {
        let Ok(handle) = self.create() else {
            return false;
        };
        let ok = eventfd().is_ok_and(|event| self.eventfd_at(handle, 1, event.as_fd()).is_ok());
        self.destroy_handle(handle);
        ok
    }

    /// A new timeline with no point signalled.
    pub fn timeline(&self) -> io::Result<Timeline> {
        let handle = self.create()?;
        match self.create() {
            Ok(staging) => Ok(Timeline { handle, staging }),
            Err(e) => {
                self.destroy_handle(handle);
                Err(e)
            }
        }
    }

    /// The timeline as a file descriptor, to hand to a compositor (`import_timeline`).
    pub fn export(&self, timeline: &Timeline) -> io::Result<OwnedFd> {
        let mut args = Handle {
            handle: timeline.handle,
            flags: 0,
            fd: -1,
            pad: 0,
            point: 0,
        };
        self.ioctl(HANDLE_TO_FD, &mut args)?;
        // Safety: the kernel returned a new descriptor.
        Ok(unsafe { OwnedFd::from_raw_fd(args.fd) })
    }

    /// Make the fence of `sync_file` the fence of `point` on `timeline`: the point signals when the fence does.
    pub fn signal_with(
        &self,
        timeline: &Timeline,
        sync_file: BorrowedFd<'_>,
        point: u64,
    ) -> io::Result<()> {
        let mut import = Handle {
            handle: timeline.staging,
            flags: IMPORT_SYNC_FILE,
            fd: sync_file.as_raw_fd(),
            pad: 0,
            point: 0,
        };
        self.ioctl(FD_TO_HANDLE, &mut import)?;
        let mut transfer = Transfer {
            src_handle: timeline.staging,
            dst_handle: timeline.handle,
            src_point: 0,
            dst_point: point,
            flags: 0,
            pad: 0,
        };
        self.ioctl(TRANSFER, &mut transfer)
    }

    /// An eventfd that becomes readable once `point` of `timeline` has signalled.
    pub fn wait_fd(&self, timeline: &Timeline, point: u64) -> io::Result<OwnedFd> {
        let event = eventfd()?;
        self.eventfd_at(timeline.handle, point, event.as_fd())?;
        Ok(event)
    }

    /// The last point of `timeline` that has signalled.
    pub fn signalled_point(&self, timeline: &Timeline) -> io::Result<u64> {
        let handles = [timeline.handle];
        let mut points = [0u64];
        let mut args = TimelineArray {
            handles: handles.as_ptr() as u64,
            points: points.as_mut_ptr() as u64,
            count_handles: 1,
            flags: 0,
        };
        self.ioctl(QUERY, &mut args)?;
        Ok(points[0])
    }

    /// Whether `point` of `timeline` has signalled.
    pub fn signalled(&self, timeline: &Timeline, point: u64) -> bool {
        self.signalled_point(timeline).is_ok_and(|p| p >= point)
    }

    pub fn destroy(&self, timeline: Timeline) {
        self.destroy_handle(timeline.handle);
        self.destroy_handle(timeline.staging);
    }
}

/// A new non-blocking eventfd.
pub fn eventfd() -> io::Result<OwnedFd> {
    // Safety: eventfd returns a new descriptor or -1.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // Safety: a new descriptor of our own.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

/// Wait until `fd` is readable, at most `timeout_ms` (negative: without end). Whether it became readable.
pub fn wait_readable(fd: BorrowedFd<'_>, timeout_ms: i32) -> bool {
    let mut pfd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // Safety: one valid pollfd.
        let n = unsafe { libc::poll(&mut pfd, 1, timeout_ms) };
        if n >= 0 {
            return n == 1;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_requests_carry_the_kernel_s_numbers_and_sizes() {
        // the values of the kernel's DRM_IOCTL_SYNCOBJ_* on 64-bit Linux
        assert_eq!(CREATE, 0xC008_64BF);
        assert_eq!(DESTROY, 0xC008_64C0);
        assert_eq!(HANDLE_TO_FD, 0xC018_64C1);
        assert_eq!(FD_TO_HANDLE, 0xC018_64C2);
        assert_eq!(QUERY, 0xC018_64CB);
        assert_eq!(TRANSFER, 0xC020_64CC);
        assert_eq!(EVENTFD, 0xC018_64CF);
    }

    #[test]
    fn a_fresh_eventfd_is_not_readable() {
        let fd = eventfd().unwrap();
        assert!(!wait_readable(fd.as_fd(), 0));
    }
}
