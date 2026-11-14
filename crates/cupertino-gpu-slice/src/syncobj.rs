//! DRM syncobj timelines on the render node: a native fence becomes a timeline point (the acquire point), and a
//! release point is waited for through an eventfd the event loop polls.

use std::fs::File;
use std::io;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use drm::control::{syncobj, Device as _};

/// The render node, opened read-write.
pub struct Node(pub File);

impl AsFd for Node {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.0.as_fd()
    }
}

impl drm::Device for Node {}
impl drm::control::Device for Node {}

/// `struct drm_syncobj_handle` of the kernel's interface.
#[repr(C)]
struct SyncobjHandle {
    handle: u32,
    flags: u32,
    fd: i32,
    pad: u32,
    point: u64,
}

/// Import a sync file into the existing syncobj `handle`, which then holds its fence.
const FD_TO_HANDLE_IMPORT_SYNC_FILE: u32 = 1 << 0;

/// `DRM_IOWR(0xC2, struct drm_syncobj_handle)`.
fn fd_to_handle_request() -> libc::c_ulong {
    const READ_WRITE: libc::c_ulong = 3;
    (READ_WRITE << 30)
        | ((std::mem::size_of::<SyncobjHandle>() as libc::c_ulong) << 16)
        | ((b'd' as libc::c_ulong) << 8)
        | 0xC2
}

/// One timeline a buffer's acquire and release points are on, and the binary syncobj a fence passes through.
pub struct Timeline {
    pub handle: syncobj::Handle,
    staging: syncobj::Handle,
}

impl Node {
    /// Whether the kernel can signal an eventfd at a timeline point (Linux 6.6), which release waits need.
    pub fn supports_eventfd(&self) -> bool {
        let Ok(timeline) = self.create_syncobj(false) else {
            return false;
        };
        let event = eventfd();
        let ok = event.as_ref().is_ok_and(|event| {
            self.syncobj_eventfd(timeline, 1, event.as_fd(), false)
                .is_ok()
        });
        let _ = self.destroy_syncobj(timeline);
        ok
    }

    pub fn timeline(&self) -> io::Result<Timeline> {
        Ok(Timeline {
            handle: self.create_syncobj(false)?,
            staging: self.create_syncobj(false)?,
        })
    }

    /// The timeline as a file descriptor, for `wp_linux_drm_syncobj_manager_v1.import_timeline`.
    pub fn export(&self, timeline: &Timeline) -> io::Result<OwnedFd> {
        self.syncobj_to_fd(timeline.handle, false)
    }

    /// Make `fence` (a sync file) the fence of `point` on `timeline`.
    pub fn signal_with(
        &self,
        timeline: &Timeline,
        fence: BorrowedFd<'_>,
        point: u64,
    ) -> io::Result<()> {
        let mut args = SyncobjHandle {
            handle: timeline.staging.into(),
            flags: FD_TO_HANDLE_IMPORT_SYNC_FILE,
            fd: fence.as_raw_fd(),
            pad: 0,
            point: 0,
        };
        // Safety: the request's size is that of `args`, which lives across the call.
        if unsafe { libc::ioctl(self.0.as_raw_fd(), fd_to_handle_request(), &mut args) } != 0 {
            return Err(io::Error::last_os_error());
        }
        self.syncobj_timeline_transfer(timeline.staging, timeline.handle, 0, point)
    }

    /// An eventfd that becomes readable once `point` of `timeline` has signalled.
    pub fn wait_fd(&self, timeline: &Timeline, point: u64) -> io::Result<OwnedFd> {
        let event = eventfd()?;
        self.syncobj_eventfd(timeline.handle, point, event.as_fd(), false)?;
        Ok(event)
    }

    /// Whether `point` of `timeline` has signalled.
    pub fn signalled(&self, timeline: &Timeline, point: u64) -> bool {
        let mut points = [0u64];
        self.syncobj_timeline_query(&[timeline.handle], &mut points, false)
            .is_ok_and(|()| points[0] >= point)
    }

    pub fn destroy(&self, timeline: Timeline) {
        let _ = self.destroy_syncobj(timeline.handle);
        let _ = self.destroy_syncobj(timeline.staging);
    }
}

pub fn eventfd() -> io::Result<OwnedFd> {
    // Safety: eventfd returns a new descriptor or -1.
    let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
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
