//! The evdev ioctls, with numbers built from the `linux/input.h` macros.
//!
//! Every `EVIOC*` request is `_IOC(dir, 'E', nr, size)`; the variable-length ones encode the buffer length in `size`,
//! so the kernel never writes past the buffer handed to it.

use core::ffi::{c_int, c_void};
use std::os::fd::BorrowedFd;

use rustix::ioctl::{self, opcode, Direction, IntegerSetter, Ioctl, IoctlOutput, Opcode, Setter};

use crate::codes::{bitmap_bytes, CLOCK_MONOTONIC, KEY_MAX, SW_MAX};
use crate::error::Error;
use crate::info::{AbsInfo, DeviceId, Resolution};

/// The ioctl "type" byte of every evdev request (`'E'` in `linux/input.h`).
const EVDEV_TYPE: u8 = b'E';

// Request numbers (`nr`) from linux/input.h.
const NR_GID: u8 = 0x02; // EVIOCGID: _IOR('E', 0x02, struct input_id)
const NR_GNAME: u8 = 0x06; // EVIOCGNAME(len)
const NR_GPROP: u8 = 0x09; // EVIOCGPROP(len)
const NR_GMTSLOTS: u8 = 0x0a; // EVIOCGMTSLOTS(len)
const NR_GKEY: u8 = 0x18; // EVIOCGKEY(len)
const NR_GSW: u8 = 0x1b; // EVIOCGSW(len)
const NR_GBIT: u8 = 0x20; // EVIOCGBIT(ev, len) is nr 0x20 + ev
const NR_GABS: u8 = 0x40; // EVIOCGABS(abs) is nr 0x40 + abs, size sizeof(struct input_absinfo)

/// `EVIOCGRAB`: `_IOW('E', 0x90, int)`.
pub(crate) const EVIOCGRAB: Opcode = opcode::write::<c_int>(EVDEV_TYPE, 0x90);
/// `EVIOCSCLOCKID`: `_IOW('E', 0xa0, int)`.
pub(crate) const EVIOCSCLOCKID: Opcode = opcode::write::<c_int>(EVDEV_TYPE, 0xa0);

/// `sizeof(struct input_id)`: four `__u16`.
const INPUT_ID_SIZE: usize = 8;
/// `sizeof(struct input_absinfo)`: six `__s32` (value, minimum, maximum, fuzz, flat, resolution).
const INPUT_ABSINFO_SIZE: usize = 24;
/// Longest name read; the kernel truncates longer ones.
const NAME_BUFFER: usize = 256;

/// A read-direction ioctl that fills a caller-owned buffer; the output is the kernel's return value.
struct ReadInto<'a> {
    opcode: Opcode,
    buf: &'a mut [u8],
}

// SAFETY: the opcode is built from `buf.len()` (see `read_into`), so the kernel writes at most that many bytes through
// `as_ptr`, which points at `buf`. The output is the plain return value and needs no pointer cast.
unsafe impl Ioctl for ReadInto<'_> {
    type Output = usize;
    const IS_MUTATING: bool = true;

    fn opcode(&self) -> Opcode {
        self.opcode
    }

    fn as_ptr(&mut self) -> *mut c_void {
        self.buf.as_mut_ptr().cast()
    }

    unsafe fn output_from_ptr(out: IoctlOutput, _: *mut c_void) -> rustix::io::Result<usize> {
        Ok(usize::try_from(out).expect("a successful ioctl returns a non-negative count"))
    }
}

/// Issues `_IOC(read, 'E', nr, buf.len())` into `buf`; returns the byte count the kernel reports.
fn read_into(fd: BorrowedFd<'_>, op: &'static str, nr: u8, buf: &mut [u8]) -> Result<usize, Error> {
    let opcode = opcode::from_components(Direction::Read, EVDEV_TYPE, nr, buf.len());
    // SAFETY: `opcode` encodes `buf.len()` as the transfer size, and `buf` is exactly that long.
    unsafe { ioctl::ioctl(fd, ReadInto { opcode, buf }) }.map_err(|errno| Error::Io { op, errno })
}

fn ne_i32(bytes: &[u8], index: usize) -> i32 {
    i32::from_ne_bytes(
        bytes[index * 4..index * 4 + 4]
            .try_into()
            .expect("4-byte slice"),
    )
}

fn ne_u16(bytes: &[u8], index: usize) -> u16 {
    u16::from_ne_bytes(
        bytes[index * 2..index * 2 + 2]
            .try_into()
            .expect("2-byte slice"),
    )
}

/// `EVIOCGNAME`: the device name, without the trailing NUL.
pub(crate) fn name(fd: BorrowedFd<'_>) -> Result<String, Error> {
    let mut buf = [0u8; NAME_BUFFER];
    read_into(fd, "EVIOCGNAME", NR_GNAME, &mut buf)?;
    let end = buf.iter().position(|&byte| byte == 0).unwrap_or(buf.len());
    String::from_utf8(buf[..end].to_vec()).map_err(|_| Error::NameNotUtf8)
}

/// `EVIOCGID`: bus, vendor, product and version.
pub(crate) fn id(fd: BorrowedFd<'_>) -> Result<DeviceId, Error> {
    let mut buf = [0u8; INPUT_ID_SIZE];
    read_into(fd, "EVIOCGID", NR_GID, &mut buf)?;
    Ok(DeviceId {
        bustype: ne_u16(&buf, 0),
        vendor: ne_u16(&buf, 1),
        product: ne_u16(&buf, 2),
        version: ne_u16(&buf, 3),
    })
}

/// `EVIOCGBIT(ev_type, ..)`: the codes supported for `ev_type` (type 0 returns the supported event types).
pub(crate) fn event_bits(
    fd: BorrowedFd<'_>,
    ev_type: u16,
    max_code: u16,
) -> Result<Vec<u8>, Error> {
    let nr = NR_GBIT + u8::try_from(ev_type).expect("event types are below 0x20");
    let mut buf = vec![0u8; bitmap_bytes(max_code)];
    read_into(fd, "EVIOCGBIT", nr, &mut buf)?;
    Ok(buf)
}

/// `EVIOCGPROP`: the input property bitmap.
pub(crate) fn properties(fd: BorrowedFd<'_>, max_prop: u16) -> Result<Vec<u8>, Error> {
    let mut buf = vec![0u8; bitmap_bytes(max_prop)];
    read_into(fd, "EVIOCGPROP", NR_GPROP, &mut buf)?;
    Ok(buf)
}

/// `EVIOCGABS(code)`: the current `input_absinfo` of one absolute axis.
pub(crate) fn abs_info(fd: BorrowedFd<'_>, code: u16) -> Result<AbsInfo, Error> {
    let nr = NR_GABS + u8::try_from(code).expect("absolute axes are below 0x40");
    let mut buf = [0u8; INPUT_ABSINFO_SIZE];
    read_into(fd, "EVIOCGABS", nr, &mut buf)?;
    Ok(AbsInfo {
        value: ne_i32(&buf, 0),
        min: ne_i32(&buf, 1),
        max: ne_i32(&buf, 2),
        fuzz: ne_i32(&buf, 3),
        flat: ne_i32(&buf, 4),
        resolution: Resolution::from_raw(ne_i32(&buf, 5)),
    })
}

/// `EVIOCGKEY`: the bitmap of keys and buttons currently held.
pub(crate) fn key_state(fd: BorrowedFd<'_>) -> Result<Vec<u8>, Error> {
    let mut buf = vec![0u8; bitmap_bytes(KEY_MAX)];
    read_into(fd, "EVIOCGKEY", NR_GKEY, &mut buf)?;
    Ok(buf)
}

/// `EVIOCGSW`: the bitmap of switches currently on.
pub(crate) fn switch_state(fd: BorrowedFd<'_>) -> Result<Vec<u8>, Error> {
    let mut buf = vec![0u8; bitmap_bytes(SW_MAX)];
    read_into(fd, "EVIOCGSW", NR_GSW, &mut buf)?;
    Ok(buf)
}

/// `EVIOCGMTSLOTS`: the current value of multitouch axis `code` in each of `slot_count` slots.
pub(crate) fn mt_slots(fd: BorrowedFd<'_>, code: u16, slot_count: u32) -> Result<Vec<i32>, Error> {
    let slots = slot_count as usize;
    // `struct input_mt_request_layout`: the axis code in, then one value per slot out.
    let mut buf = vec![0u8; 4 * (slots + 1)];
    buf[..4].copy_from_slice(&u32::from(code).to_ne_bytes());
    read_into(fd, "EVIOCGMTSLOTS", NR_GMTSLOTS, &mut buf)?;
    Ok((1..=slots).map(|index| ne_i32(&buf, index)).collect())
}

/// `EVIOCGRAB`: takes (`true`) or releases (`false`) exclusive access to the device.
pub(crate) fn grab(fd: BorrowedFd<'_>, grab: bool) -> Result<(), Error> {
    // SAFETY: EVIOCGRAB takes its argument by value, as an integer (non-zero grabs); `IntegerSetter` passes it that way.
    let request = unsafe { IntegerSetter::<EVIOCGRAB>::new_usize(usize::from(grab)) };
    // SAFETY: the request is the one `IntegerSetter` was declared for.
    unsafe { ioctl::ioctl(fd, request) }.map_err(|errno| Error::Io {
        op: "EVIOCGRAB",
        errno,
    })
}

/// `EVIOCSCLOCKID`: makes event timestamps use `CLOCK_MONOTONIC`.
pub(crate) fn set_monotonic_clock(fd: BorrowedFd<'_>) -> Result<(), Error> {
    // SAFETY: EVIOCSCLOCKID reads one `int` through the pointer, which is what `Setter<_, c_int>` supplies.
    let request = unsafe { Setter::<EVIOCSCLOCKID, c_int>::new(CLOCK_MONOTONIC) };
    // SAFETY: the request is the one `Setter` was declared for.
    unsafe { ioctl::ioctl(fd, request) }.map_err(|errno| Error::Io {
        op: "EVIOCSCLOCKID",
        errno,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // Request numbers as the C macros expand on 64-bit Linux (x86_64/aarch64 `_IOC` layout).
    #[test]
    fn opcodes_match_the_uapi_macros() {
        let op = |nr: u8, len: usize| opcode::from_components(Direction::Read, EVDEV_TYPE, nr, len);
        assert_eq!(op(NR_GID, INPUT_ID_SIZE), 0x8008_4502);
        assert_eq!(op(NR_GNAME, 256), 0x8100_4506);
        assert_eq!(op(NR_GABS + 0x35, INPUT_ABSINFO_SIZE), 0x8018_4575); // EVIOCGABS(ABS_MT_POSITION_X)
        assert_eq!(op(NR_GKEY, bitmap_bytes(KEY_MAX)), 0x8060_4518);
        assert_eq!(op(NR_GBIT, 4), 0x8004_4520); // EVIOCGBIT(0, 4)
        assert_eq!(op(NR_GMTSLOTS, 4 * 11), 0x802c_450a);
        assert_eq!(EVIOCGRAB, 0x4004_4590);
        assert_eq!(EVIOCSCLOCKID, 0x4004_45a0);
    }
}
