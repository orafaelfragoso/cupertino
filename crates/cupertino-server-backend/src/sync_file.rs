//! When a frame's render fence signalled, read from the sync file the renderer exports for it. The kernel keeps each
//! fence's signal time, on the monotonic clock the display's flip timestamps use, and hands it out through the sync
//! file's info request without waiting, so the backend reads it once the frame's flip has come and the late start
//! ([`crate::latch`]) counts the GPU's work in the frame's render time.

use std::os::fd::{AsFd, AsRawFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use smithay::backend::renderer::sync::SyncPoint;

use crate::latch::GpuDone;

/// The longest name the kernel gives a sync file or a fence.
const NAME_LEN: usize = 32;

/// The most fences one frame's sync file is read for; a file merging more is not timed.
const MAX_FENCES: usize = 8;

/// The kernel's description of a sync file.
#[repr(C)]
#[derive(Default)]
struct FileInfo {
    name: [u8; NAME_LEN],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

/// The kernel's description of one fence of a sync file.
#[repr(C)]
#[derive(Clone, Copy)]
struct FenceInfo {
    obj_name: [u8; NAME_LEN],
    driver_name: [u8; NAME_LEN],
    status: i32,
    flags: u32,
    timestamp_ns: u64,
}

const EMPTY_FENCE: FenceInfo = FenceInfo {
    obj_name: [0; NAME_LEN],
    driver_name: [0; NAME_LEN],
    status: 0,
    flags: 0,
    timestamp_ns: 0,
};

/// The sync file's info request: read and write, type `>`, number 4, the size of [`FileInfo`].
const FILE_INFO: u64 =
    (3 << 30) | ((std::mem::size_of::<FileInfo>() as u64) << 16) | (0x3e << 8) | 4;

fn file_info(fd: BorrowedFd<'_>, info: &mut FileInfo) -> bool {
    // The timestamp is optional. An unavailable sample is not worth retrying on the frame thread.
    // SAFETY: `FILE_INFO` encodes the size of `FileInfo`, which is laid out as the kernel's structure; `info`
    // lives across the call, and the fence array its pointer names (when not null) holds `num_fences` entries.
    let result = unsafe {
        libc::ioctl(
            fd.as_raw_fd(),
            FILE_INFO as libc::c_ulong,
            info as *mut FileInfo,
        )
    };
    result == 0
}

/// When every fence of the sync file `fd` had signalled, on the monotonic clock: `None` while one has not, when one
/// failed, or when `fd` is no sync file. Never waits.
pub fn signalled_at(fd: BorrowedFd<'_>) -> Option<Duration> {
    let mut info = FileInfo::default();
    if !file_info(fd, &mut info) || info.status != 1 {
        return None;
    }
    let count = usize::try_from(info.num_fences).ok()?;
    if count == 0 || count > MAX_FENCES {
        return None;
    }
    let mut fences = [EMPTY_FENCE; MAX_FENCES];
    let mut info = FileInfo {
        num_fences: info.num_fences,
        sync_fence_info: fences.as_mut_ptr() as u64,
        ..FileInfo::default()
    };
    if !file_info(fd, &mut info) {
        return None;
    }
    let fences = fences.get(..count)?;
    if fences.iter().any(|f| f.status != 1) {
        return None;
    }
    fences
        .iter()
        .map(|f| Duration::from_nanos(f.timestamp_ns))
        .max()
}

/// What tells when the GPU finished a frame, kept from its render until its flip comes.
#[derive(Debug)]
pub enum RenderFence {
    /// Nothing to wait for: the renderer waited for the GPU, or nothing of the frame was drawn on it.
    WithinCpu,
    /// The render fence, as a sync file.
    File(OwnedFd),
    /// A fence that cannot be exported.
    Unknown,
}

impl RenderFence {
    /// The fence of a frame whose render handed out `sync`; `None` when the renderer drew nothing (a client buffer
    /// scanned out).
    pub fn of(sync: Option<&SyncPoint>) -> RenderFence {
        match sync {
            Some(sync) if sync.contains_fence() => sync
                .export()
                .map_or(RenderFence::Unknown, RenderFence::File),
            _ => RenderFence::WithinCpu,
        }
    }

    /// When the GPU finished the frame started at `started`, read as its flip came at `shown` (both on the monotonic
    /// clock; `None`: the flip has no timestamp of its own). A signal time after the flip is the moment it was looked
    /// at, not the GPU's, so the flip bounds it.
    pub fn done(&self, started: Duration, shown: Option<Duration>) -> GpuDone {
        match self {
            RenderFence::WithinCpu => GpuDone::WithinCpu,
            RenderFence::Unknown => GpuDone::Unknown,
            RenderFence::File(fd) => match signalled_at(fd.as_fd()) {
                Some(at) => {
                    let at = shown.map_or(at, |shown| at.min(shown));
                    GpuDone::At(at.saturating_sub(started))
                }
                None => GpuDone::Unknown,
            },
        }
    }
}
