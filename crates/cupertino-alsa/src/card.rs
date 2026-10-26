//! One sound card's control device, `/dev/snd/controlC<n>`.

use core::ffi::c_void;
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::path::{Path, PathBuf};

use rustix::fs::{self, Mode as FileMode, OFlags};
use rustix::io::Errno;
use rustix::ioctl::{self, Ioctl, IoctlOutput, Opcode};

use crate::abi::{self, CardInfo, ElementId, ElementInfo, Event, EVENT_SIZE, TLV_HEADER_SIZE};
use crate::error::{Error, Result};
use crate::tlv::DbCurve;

/// Directory of the sound device nodes.
pub const DEVICE_DIR: &str = "/dev/snd";

/// An ioctl that reads and writes one caller-owned buffer; the output is nothing.
struct Transfer<'a> {
    opcode: Opcode,
    buf: &'a mut [u8],
}

// SAFETY: every `Transfer` is built with an opcode whose encoded size is the size of the kernel structure the
// buffer holds (the `abi` constants), or the TLV header for `TLV_READ`, where the kernel reads the buffer length from
// the header and the buffer is allocated to that length. `as_ptr` points at `buf`. The output is a plain status.
unsafe impl Ioctl for Transfer<'_> {
    type Output = ();
    const IS_MUTATING: bool = true;

    fn opcode(&self) -> Opcode {
        self.opcode
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.buf.as_mut_ptr().cast()
    }

    unsafe fn output_from_ptr(_: IoctlOutput, _: *mut c_void) -> rustix::io::Result<()> {
        Ok(())
    }
}

/// Issue `opcode` on `fd` with `buf`.
///
/// # Safety
/// `buf` must be at least as large as the kernel reads or writes for `opcode` (the caller passes the arrays and sizes
/// of `abi`, and for `TLV_READ` a buffer of the length stored in its header).
unsafe fn control_ioctl(
    fd: BorrowedFd<'_>,
    op: &'static str,
    opcode: Opcode,
    buf: &mut [u8],
) -> Result<()> {
    // SAFETY: forwarded from this function's contract.
    unsafe { ioctl::ioctl(fd, Transfer { opcode, buf }) }.map_err(|errno| Error::Io { op, errno })
}

/// How a control device is opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpenMode {
    /// Query only: values can be read but no write is possible through the descriptor.
    ReadOnly,
    /// Values can be written.
    ReadWrite,
}

/// An open control device of one card.
#[derive(Debug)]
pub struct Card {
    fd: OwnedFd,
    index: u32,
}

/// The path of the control device of card `index` below `dir`.
pub fn device_path(dir: &Path, index: u32) -> PathBuf {
    dir.join(format!("controlC{index}"))
}

/// The numbers of the cards that have a control device in `dir`, ascending.
pub fn card_indexes(dir: &Path) -> Result<Vec<u32>> {
    let entries = std::fs::read_dir(dir).map_err(|source| Error::ReadDir {
        dir: dir.to_path_buf(),
        source,
    })?;
    let mut indexes: Vec<u32> = entries
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<Vec<_>>>()
        .map_err(|source| Error::ReadDir {
            dir: dir.to_path_buf(),
            source,
        })?
        .iter()
        .filter_map(|name| name.to_str()?.strip_prefix("controlC")?.parse().ok())
        .collect();
    indexes.sort_unstable();
    Ok(indexes)
}

impl Card {
    /// Open the control device of card `index` below `dir` (non-blocking, so event reads return at once).
    pub fn open(dir: &Path, index: u32, mode: OpenMode) -> Result<Self> {
        let access = match mode {
            OpenMode::ReadOnly => OFlags::RDONLY,
            OpenMode::ReadWrite => OFlags::RDWR,
        };
        let fd = fs::open(
            device_path(dir, index),
            access | OFlags::NONBLOCK | OFlags::CLOEXEC,
            FileMode::empty(),
        )
        .map_err(|errno| match errno {
            Errno::NOENT => Error::NoSuchCard { card: index },
            errno => Error::Io {
                op: "open control device",
                errno,
            },
        })?;
        Ok(Self { fd, index })
    }

    /// The card number.
    pub fn index(&self) -> u32 {
        self.index
    }

    /// Names of the card.
    pub fn info(&self) -> Result<CardInfo> {
        let mut buf = [0u8; abi::CARD_INFO_SIZE];
        // SAFETY: `buf` is a `snd_ctl_card_info`.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_CARD_INFO",
                abi::CARD_INFO,
                &mut buf,
            )?
        };
        CardInfo::parse(&buf)
    }

    /// The ids of every control of the card.
    pub fn element_ids(&self) -> Result<Vec<ElementId>> {
        let mut list = abi::list_request(0, 0, 0);
        // SAFETY: `list` is a `snd_ctl_elem_list` with no space asked for, so the kernel writes no ids.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_ELEM_LIST",
                abi::ELEM_LIST,
                &mut list,
            )?
        };
        let (_, count) = abi::list_reply(&list);
        // controls added between the two calls are missed; the next listing sees them
        let mut ids = vec![0u8; count as usize * abi::ELEM_ID_SIZE];
        let mut list = abi::list_request(0, count, ids.as_mut_ptr() as u64);
        // SAFETY: `list` asks for `count` ids and `ids` holds `count` 64-byte records at the address stored in it.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_ELEM_LIST",
                abi::ELEM_LIST,
                &mut list,
            )?
        };
        let (used, _) = abi::list_reply(&list);
        let (records, _) = ids.as_chunks::<{ abi::ELEM_ID_SIZE }>();
        records
            .iter()
            .take(used as usize)
            .map(|record| ElementId::parse(record))
            .collect()
    }

    /// What control `id` is.
    pub fn element_info(&self, id: &ElementId) -> Result<ElementInfo> {
        let mut buf = abi::info_request(id)?;
        // SAFETY: `buf` is a `snd_ctl_elem_info`.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_ELEM_INFO",
                abi::ELEM_INFO,
                &mut buf,
            )?
        };
        ElementInfo::parse(&buf)
    }

    /// The info of every control of the card.
    pub fn elements(&self) -> Result<Vec<ElementInfo>> {
        self.element_ids()?
            .iter()
            .map(|id| self.element_info(id))
            .collect()
    }

    /// The values of an integer or boolean control, one per channel.
    pub fn read_values(&self, info: &ElementInfo) -> Result<Vec<i64>> {
        let mut buf = abi::value_request(&info.id, &[])?;
        // SAFETY: `buf` is a `snd_ctl_elem_value`.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_ELEM_READ",
                abi::ELEM_READ,
                &mut buf,
            )?
        };
        abi::value_reply(&buf, info.count)
    }

    /// Write `values` (one per channel) to an integer or boolean control.
    pub fn write_values(&self, info: &ElementInfo, values: &[i64]) -> Result<()> {
        if values.len() != info.count as usize {
            return Err(Error::ValueCount { count: info.count });
        }
        let mut buf = abi::value_request(&info.id, values)?;
        // SAFETY: `buf` is a `snd_ctl_elem_value`.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_ELEM_WRITE",
                abi::ELEM_WRITE,
                &mut buf,
            )
        }
    }

    /// The gain curve of a control; `None` when it has no TLV data or the data holds no gain curve.
    pub fn db_curve(&self, info: &ElementInfo) -> Result<Option<DbCurve>> {
        if !info.access.tlv_readable() {
            return Ok(None);
        }
        // room for the largest block the kernel hands out (`MAX_USER_CONTROL_SIZE` is 512 bytes; driver tables are larger)
        const CAPACITY: usize = 4096;
        let mut buf = vec![0u8; TLV_HEADER_SIZE + CAPACITY];
        buf[0..4].copy_from_slice(&info.id.numid.to_ne_bytes());
        buf[4..8].copy_from_slice(&(CAPACITY as u32).to_ne_bytes());
        // SAFETY: the header states `CAPACITY` bytes and `buf` has that much room after it.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_TLV_READ",
                abi::TLV_READ,
                &mut buf,
            )?
        };
        // the kernel copies the block as stored and leaves the header alone: the block's own second word is its length
        let length = u32::from_ne_bytes(buf[12..16].try_into().expect("4-byte slice")) as usize;
        let block = buf
            .get(TLV_HEADER_SIZE..TLV_HEADER_SIZE + 8 + length)
            .ok_or(Error::MalformedTlv("block longer than the buffer"))?;
        DbCurve::parse(block)
    }

    /// Ask the kernel to queue an event for every change of a control; read them with [`Card::read_events`].
    pub fn subscribe(&self) -> Result<()> {
        let mut on = 1i32.to_ne_bytes();
        // SAFETY: the argument is one `int`.
        unsafe {
            control_ioctl(
                self.fd.as_fd(),
                "SNDRV_CTL_IOCTL_SUBSCRIBE_EVENTS",
                abi::SUBSCRIBE_EVENTS,
                &mut on,
            )
        }
    }

    /// The events queued since the last read; empty when there are none. Needs [`Card::subscribe`] first.
    pub fn read_events(&self) -> Result<Vec<Event>> {
        let mut buf = [0u8; EVENT_SIZE * 16];
        match rustix::io::read(&self.fd, &mut buf) {
            Ok(len) => Event::parse_all(&buf[..len]),
            Err(Errno::AGAIN) => Ok(Vec::new()),
            Err(errno) => Err(Error::Io {
                op: "read control events",
                errno,
            }),
        }
    }
}

impl AsFd for Card {
    /// The descriptor to poll for events.
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}
