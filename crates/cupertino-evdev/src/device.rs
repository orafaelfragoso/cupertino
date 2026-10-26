//! An open evdev device: the fd, its queried capabilities and the framer.

use std::os::fd::{AsFd, BorrowedFd, OwnedFd};

use rustix::fs::{fcntl_getfl, fcntl_setfl, OFlags};
use rustix::io::Errno;
use rustix::time::{clock_gettime, ClockId};

use crate::bitset::BitSet;
use crate::error::Error;
use crate::event::{InputEvent, EVENT_SIZE};
use crate::frame::Frame;
use crate::framer::{Framer, FramerOutput, Layout, MtSlotValues, Snapshot};
use crate::info::DeviceInfo;
use crate::ioctl;

/// Events read per `read` call.
const BATCH_EVENTS: usize = 64;

/// Reads `input_event` records from a non-blocking evdev fd until it would block, appending them to `out`.
pub fn read_events(fd: BorrowedFd<'_>, out: &mut Vec<InputEvent>) -> Result<(), Error> {
    let mut buf = [0u8; EVENT_SIZE * BATCH_EVENTS];
    loop {
        match rustix::io::read(fd, &mut buf[..]) {
            Ok(len) if len == 0 || len % EVENT_SIZE != 0 => return Err(Error::TruncatedRecord { len }),
            Ok(len) => out.extend(buf[..len].as_chunks::<EVENT_SIZE>().0.iter().map(InputEvent::from_bytes)),
            Err(Errno::AGAIN) => return Ok(()),
            Err(Errno::INTR) => {}
            Err(errno) => return Err(Error::Io { op: "read", errno }),
        }
    }
}

/// An evdev device opened by the compositor's session and handed over as an fd.
///
/// Register it with an event loop through [`AsFd`]; when it is readable, call [`Device::read_frames`].
pub struct Device {
    fd: OwnedFd,
    info: DeviceInfo,
    framer: Framer,
    /// The open-time resync frame, delivered with the first read.
    queued: Vec<Frame>,
}

impl Device {
    /// Takes an fd for `/dev/input/eventN`: sets it non-blocking and timestamps on `CLOCK_MONOTONIC`, queries the
    /// capabilities, and reads the current state (held keys, switches, contacts) into a first `resynced` frame that
    /// the first [`Device::read_frames`] returns.
    pub fn open(fd: OwnedFd) -> Result<Self, Error> {
        let flags = fcntl_getfl(&fd).map_err(|errno| Error::Io { op: "F_GETFL", errno })?;
        fcntl_setfl(&fd, flags | OFlags::NONBLOCK).map_err(|errno| Error::Io { op: "F_SETFL", errno })?;
        ioctl::set_monotonic_clock(fd.as_fd())?;
        // Changing the clock flushes the kernel queue, so every event queued from here on is stamped at or after
        // this instant. Stamping the snapshot now, before the query and snapshot ioctls, keeps it from outliving
        // events queued during them; those events are replayed on top of the snapshot, which the framer tolerates.
        let now = clock_gettime(ClockId::Monotonic);
        let time_us = now.tv_sec * 1_000_000 + now.tv_nsec / 1000;
        let info = DeviceInfo::query(fd.as_fd())?;
        let framer = Framer::new(Layout::from_info(&info)?);
        let mut device = Self { fd, info, framer, queued: Vec::new() };
        let snapshot = device.snapshot()?;
        device.queued.push(device.framer.apply_snapshot(&snapshot, time_us)?);
        Ok(device)
    }

    /// The capabilities and axis geometry queried at open.
    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    /// Takes (`true`) or releases (`false`) exclusive access with `EVIOCGRAB`.
    pub fn grab(&self, grab: bool) -> Result<(), Error> {
        ioctl::grab(self.fd.as_fd(), grab)
    }

    /// Reads everything the kernel has queued and appends the resulting frames to `out`. Returns when the fd would
    /// block. After `SYN_DROPPED` the device state is re-read and a `resynced` frame stands in for the lost events.
    pub fn read_frames(&mut self, out: &mut Vec<Frame>) -> Result<(), Error> {
        out.append(&mut self.queued);
        let mut events = Vec::new();
        read_events(self.fd.as_fd(), &mut events)?;
        for event in &events {
            match self.framer.push(event)? {
                None => {}
                Some(FramerOutput::Frame(frame)) => out.push(frame),
                Some(FramerOutput::ResyncNeeded { time_us }) => {
                    let snapshot = self.snapshot()?;
                    out.push(self.framer.apply_snapshot(&snapshot, time_us)?);
                    // The snapshot is newer than every event read so far, so the rest of this batch is stale. Events
                    // queued between the read and the snapshot arrive again on the next read; the framer treats them
                    // as replays of state the snapshot already holds.
                    break;
                }
            }
        }
        Ok(())
    }

    /// Reads the device's current state (`EVIOCGKEY`, `EVIOCGSW`, `EVIOCGABS`, `EVIOCGMTSLOTS`).
    pub fn snapshot(&self) -> Result<Snapshot, Error> {
        let fd = self.fd.as_fd();
        let layout = self.framer.layout();
        let abs = layout
            .snapshot_abs_codes()
            .into_iter()
            .map(|code| Ok((code, ioctl::abs_info(fd, code)?.value)))
            .collect::<Result<Vec<_>, Error>>()?;
        let mt = layout
            .snapshot_mt_codes()
            .into_iter()
            .map(|code| Ok(MtSlotValues { code, values: ioctl::mt_slots(fd, code, layout.slot_count)? }))
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Snapshot {
            keys: BitSet::from_bytes(ioctl::key_state(fd)?),
            switches: BitSet::from_bytes(ioctl::switch_state(fd)?),
            abs,
            mt,
        })
    }
}

impl AsFd for Device {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustix::io::write;
    use rustix::pipe::{pipe_with, PipeFlags};

    #[test]
    fn reads_whole_batches_until_would_block() {
        let (reader, writer) = pipe_with(PipeFlags::NONBLOCK).unwrap();
        let events = [
            InputEvent { sec: 5, usec: 1, kind: 1, code: 30, value: 1 },
            InputEvent { sec: 5, usec: 1, kind: 0, code: 0, value: 0 },
        ];
        for event in &events {
            write(&writer, &event.to_bytes()).unwrap();
        }
        let mut out = Vec::new();
        read_events(reader.as_fd(), &mut out).unwrap();
        assert_eq!(out, events);
    }

    #[test]
    fn partial_record_is_an_error() {
        let (reader, writer) = pipe_with(PipeFlags::NONBLOCK).unwrap();
        write(&writer, &[0u8; 10]).unwrap();
        let error = read_events(reader.as_fd(), &mut Vec::new()).unwrap_err();
        assert!(matches!(error, Error::TruncatedRecord { len: 10 }), "{error}");
    }
}
