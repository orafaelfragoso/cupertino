//! The clipboard (`wl_data_device`): the selection another client offers, read in a MIME type the reader names, and the
//! selection this client set, served to whoever reads it until another one replaces it. No type is converted into
//! another.

use std::fs::File;
use std::io::{ErrorKind, PipeReader, Read, Write};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fs::{fcntl_setfl, OFlags};
use rustix::io::Errno;
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle,
};

use crate::connection::State;
use crate::{Error, Event};

/// How long a read waits for the offering client.
const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// The most reads of [`Clipboard::receive_async`] running at once.
const MAX_READS: usize = 8;
/// The most a read takes; a longer selection is cut there.
const READ_LIMIT: usize = 16 << 20;
/// How long serving our selection to a reading client may take before the descriptor is dropped.
const SERVE_TIMEOUT: Duration = Duration::from_secs(1);
/// The most reads served at once; a read beyond them gets its descriptor closed at once.
const MAX_SERVES: usize = 8;

/// The current selection of another client, with the MIME types it is offered in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClipboardOffer {
    id: u64,
    mime_types: Vec<String>,
}

impl ClipboardOffer {
    pub fn mime_types(&self) -> &[String] {
        &self.mime_types
    }
}

/// Names one read of [`crate::Connection::receive_async`]; its data arrives as [`Event::Received`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ReadId(u64);

/// A read of another client's selection, in progress in the connection's loop.
#[derive(Debug)]
struct PendingRead {
    id: ReadId,
    offer: u64,
    reader: PipeReader,
    bytes: Vec<u8>,
    deadline: Instant,
}

/// The types an offer announced.
#[derive(Debug, Default)]
pub(crate) struct OfferTypes(Mutex<Vec<String>>);

/// Contents of a selection this client set, one byte string per MIME type.
type Contents = Arc<[(String, Arc<[u8]>)]>;

#[derive(Debug, Default)]
pub(crate) struct Clipboard {
    device: Option<WlDataDevice>,
    selection: Option<(u64, WlDataOffer)>,
    next_offer: u64,
    own: Option<(WlDataSource, Contents)>,
    serving: Arc<AtomicUsize>,
    reads: Vec<PendingRead>,
    /// Reads answered without a pipe, reported with the next pump.
    answered: Vec<(ReadId, Option<Vec<u8>>)>,
    next_read: u64,
}

impl Clipboard {
    pub(crate) fn new(
        manager: Option<&WlDataDeviceManager>,
        seat: Option<&WlSeat>,
        qh: &QueueHandle<State>,
    ) -> Self {
        Clipboard {
            device: manager.zip(seat).map(|(m, s)| m.get_data_device(s, qh, ())),
            ..Clipboard::default()
        }
    }

    /// Set the selection to `contents`; `serial` is that of the input event that asked for it.
    pub(crate) fn set(
        &mut self,
        manager: &WlDataDeviceManager,
        contents: Vec<(String, Vec<u8>)>,
        serial: u32,
        qh: &QueueHandle<State>,
    ) {
        let Some(device) = &self.device else {
            return;
        };
        let source = manager.create_data_source(qh, ());
        for (mime, _) in &contents {
            source.offer(mime.clone());
        }
        device.set_selection(Some(&source), serial);
        let contents = contents
            .into_iter()
            .map(|(mime, bytes)| (mime, Arc::from(bytes)))
            .collect();
        if let Some((old, _)) = self.own.replace((source, contents)) {
            old.destroy();
        }
    }

    /// Start reading the selection in `mime`, like [`Clipboard::receive`] without waiting: the data arrives as
    /// [`Event::Received`] from [`Clipboard::pump`], `None` when the offer was replaced, the reader did not finish within
    /// [`READ_TIMEOUT`] or the pipe failed. At most [`MAX_READS`] run at once.
    pub(crate) fn receive_async(
        &mut self,
        conn: &Connection,
        offer: &ClipboardOffer,
        mime: &str,
    ) -> Result<ReadId, Error> {
        if self.reads.len() >= MAX_READS {
            return Err(Error::PermissionDenied("more clipboard reads at once"));
        }
        self.next_read += 1;
        let id = ReadId(self.next_read);
        if let Some((_, contents)) = &self.own {
            let bytes = contents
                .iter()
                .find(|(m, _)| m == mime)
                .map(|(_, bytes)| bytes.to_vec())
                .ok_or_else(|| Error::UnsupportedFormat(mime.to_string()))?;
            self.answered.push((id, Some(bytes)));
            return Ok(id);
        }
        let Some((_, current)) = self.selection.as_ref().filter(|(id, _)| *id == offer.id) else {
            return Err(Error::Cancelled);
        };
        if !offer.mime_types.iter().any(|m| m == mime) {
            return Err(Error::UnsupportedFormat(mime.to_string()));
        }
        let (reader, writer) = std::io::pipe().map_err(Error::Io)?;
        fcntl_setfl(&reader, OFlags::NONBLOCK).map_err(|e| Error::Io(e.into()))?;
        current.receive(mime.to_string(), writer.as_fd());
        drop(writer);
        conn.flush()?;
        self.reads.push(PendingRead {
            id,
            offer: offer.id,
            reader,
            bytes: Vec::new(),
            deadline: Instant::now() + READ_TIMEOUT,
        });
        Ok(id)
    }

    /// Read what the pipes of running reads hold now, and report each read that ended at `now` to `events`.
    pub(crate) fn pump(&mut self, now: Instant, events: &mut Vec<Event>) {
        events.extend(
            self.answered
                .drain(..)
                .map(|(id, data)| Event::Received { id, data }),
        );
        let current = self.selection.as_ref().map(|(id, _)| *id);
        self.reads.retain_mut(|read| {
            let data = if Some(read.offer) == current {
                match drain(read) {
                    Drained::Open if now < read.deadline => return true,
                    Drained::Open | Drained::Failed => None,
                    Drained::Done => Some(std::mem::take(&mut read.bytes)),
                }
            } else {
                None
            };
            events.push(Event::Received { id: read.id, data });
            false
        });
    }

    /// The pipes of running reads, to wait on with the display.
    pub(crate) fn read_fds(&self) -> impl Iterator<Item = BorrowedFd<'_>> {
        self.reads.iter().map(|r| r.reader.as_fd())
    }

    /// When the first running read gives up.
    pub(crate) fn read_deadline(&self) -> Option<Instant> {
        self.reads.iter().map(|r| r.deadline).min()
    }

    /// The selection in `mime`: this client's own directly, another client's read through a pipe. The caller waits up to
    /// [`READ_TIMEOUT`]; a replaced offer or a reader that does not finish in time is [`Error::Cancelled`].
    pub(crate) fn receive(
        &self,
        conn: &Connection,
        offer: &ClipboardOffer,
        mime: &str,
    ) -> Result<Vec<u8>, Error> {
        if let Some((_, contents)) = &self.own {
            return contents
                .iter()
                .find(|(m, _)| m == mime)
                .map(|(_, bytes)| bytes.to_vec())
                .ok_or_else(|| Error::UnsupportedFormat(mime.to_string()));
        }
        let Some((_, current)) = self.selection.as_ref().filter(|(id, _)| *id == offer.id) else {
            return Err(Error::Cancelled);
        };
        if !offer.mime_types.iter().any(|m| m == mime) {
            return Err(Error::UnsupportedFormat(mime.to_string()));
        }
        let (mut reader, writer) = std::io::pipe().map_err(Error::Io)?;
        current.receive(mime.to_string(), writer.as_fd());
        drop(writer);
        conn.flush()?;
        read_limited(&mut reader, Instant::now() + READ_TIMEOUT, READ_LIMIT)
            .map_err(Error::Io)?
            .ok_or(Error::Cancelled)
    }
}

enum Drained {
    /// The writer may send more.
    Open,
    /// The writer closed the pipe, or the read reached [`READ_LIMIT`] and the rest is cut.
    Done,
    Failed,
}

/// Read what `read`'s pipe holds now without waiting.
fn drain(read: &mut PendingRead) -> Drained {
    let mut chunk = [0u8; 4096];
    while read.bytes.len() < READ_LIMIT {
        match read.reader.read(&mut chunk) {
            Ok(0) => return Drained::Done,
            Ok(n) => {
                let room = READ_LIMIT - read.bytes.len();
                read.bytes.extend_from_slice(&chunk[..n.min(room)]);
            }
            Err(e) if e.kind() == ErrorKind::WouldBlock => return Drained::Open,
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return Drained::Failed,
        }
    }
    Drained::Done
}

/// What `reader` gives until it closes, at most `limit` bytes; `None` when `deadline` passes first.
fn read_limited(
    reader: &mut (impl Read + AsFd),
    deadline: Instant,
    limit: usize,
) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    while bytes.len() < limit {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        let timeout = Timespec::try_from(left).expect("a deadline fits a timespec");
        let mut fds = [PollFd::new(&*reader, PollFlags::IN)];
        match poll(&mut fds, Some(&timeout)) {
            Ok(0) => return Ok(None),
            Ok(_) | Err(Errno::INTR) => {}
            Err(e) => return Err(e.into()),
        }
        match reader.read(&mut chunk) {
            Ok(0) => return Ok(Some(bytes)),
            Ok(n) => bytes.extend_from_slice(&chunk[..n.min(limit - bytes.len())]),
            Err(e) if e.kind() == ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(Some(bytes))
}

/// Serve `bytes` to `fd` on a thread of its own when fewer than `max` serves run (counted in `serving`); else close
/// `fd` at once. Whether it is served.
fn spawn_serve(
    serving: &Arc<AtomicUsize>,
    max: usize,
    fd: OwnedFd,
    bytes: Arc<[u8]>,
    timeout: Duration,
) -> bool {
    if serving
        .try_update(Ordering::AcqRel, Ordering::Acquire, |n| {
            (n < max).then_some(n + 1)
        })
        .is_err()
    {
        return false;
    }
    let slot = ServeSlot(Arc::clone(serving));
    std::thread::spawn(move || {
        // released when the thread ends, also by a panic
        let _slot = slot;
        serve(fd, &bytes, Instant::now() + timeout);
    });
    true
}

/// A claimed place among the reads served at once; dropping it frees it.
struct ServeSlot(Arc<AtomicUsize>);

impl Drop for ServeSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Write `bytes` to a reading client's `fd` without blocking, until done or `deadline`, then close it.
fn serve(fd: OwnedFd, bytes: &[u8], deadline: Instant) {
    if fcntl_setfl(&fd, OFlags::NONBLOCK).is_err() {
        return;
    }
    let mut file = File::from(fd);
    let mut rest = bytes;
    while !rest.is_empty() {
        let left = deadline.saturating_duration_since(Instant::now());
        let Ok(timeout) = Timespec::try_from(left) else {
            return;
        };
        let mut fds = [PollFd::new(&file, PollFlags::OUT)];
        if left.is_zero() || !matches!(poll(&mut fds, Some(&timeout)), Ok(n) if n > 0) {
            return;
        }
        match file.write(rest) {
            Ok(n) => rest = &rest[n..],
            Err(e) if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::Interrupted => {}
            Err(_) => return,
        }
    }
}

delegate_noop!(State: ignore WlDataDeviceManager);

impl Dispatch<WlDataDevice, ()> for State {
    fn event(
        state: &mut Self,
        _: &WlDataDevice,
        event: wl_data_device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_device::Event::Selection { id } => {
                let clipboard = &mut state.clipboard;
                let offer = id.map(|offer| {
                    clipboard.next_offer += 1;
                    let mime_types = offer
                        .data::<OfferTypes>()
                        .map(|t| t.0.lock().expect("offer types").clone())
                        .unwrap_or_default();
                    let id = clipboard.next_offer;
                    (ClipboardOffer { id, mime_types }, offer)
                });
                let (public, wire) = match offer {
                    Some((public, wire)) => (Some(public.clone()), Some((public.id, wire))),
                    None => (None, None),
                };
                if let Some((_, old)) = std::mem::replace(&mut clipboard.selection, wire) {
                    old.destroy();
                }
                state.events.push(Event::Selection(public));
            }
            // drag-and-drop offers are not taken
            wl_data_device::Event::Enter {
                id: Some(offer), ..
            } => offer.destroy(),
            _ => {}
        }
    }

    event_created_child!(State, WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (WlDataOffer, OfferTypes::default()),
    ]);
}

impl Dispatch<WlDataOffer, OfferTypes> for State {
    fn event(
        _: &mut Self,
        _: &WlDataOffer,
        event: wl_data_offer::Event,
        types: &OfferTypes,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            types.0.lock().expect("offer types").push(mime_type);
        }
    }
}

impl Dispatch<WlDataSource, ()> for State {
    fn event(
        state: &mut Self,
        source: &WlDataSource,
        event: wl_data_source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let own = state.clipboard.own.as_ref().filter(|(s, _)| s == source);
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let bytes =
                    own.and_then(|(_, contents)| contents.iter().find(|(m, _)| *m == mime_type));
                if let Some((_, bytes)) = bytes {
                    // off the event loop: a reader that is slow or never reads does not hold up this client
                    spawn_serve(
                        &state.clipboard.serving,
                        MAX_SERVES,
                        fd,
                        Arc::clone(bytes),
                        SERVE_TIMEOUT,
                    );
                }
            }
            wl_data_source::Event::Cancelled => {
                if own.is_some() {
                    state.clipboard.own = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIMIT: usize = 10_000;

    #[test]
    fn a_read_stops_at_the_limit() {
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        // more than a pipe holds: the writer fails once the reader stops at the limit and closes
        let feeder = std::thread::spawn(move || writer.write_all(&vec![b'a'; 100 * LIMIT]));
        let deadline = Instant::now() + Duration::from_secs(5);
        let bytes = read_limited(&mut reader, deadline, LIMIT).unwrap().unwrap();
        assert_eq!(bytes.len(), LIMIT);
        drop(reader);
        assert!(feeder.join().unwrap().is_err());
    }

    #[test]
    fn a_read_from_a_client_that_never_finishes_gives_nothing_at_the_deadline() {
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"partial").unwrap();
        let start = Instant::now();
        let bytes = read_limited(&mut reader, start + Duration::from_millis(50), LIMIT).unwrap();
        assert_eq!(bytes, None);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn serving_a_client_that_does_not_read_gives_up_at_the_deadline_and_closes() {
        let (mut reader, writer) = std::io::pipe().unwrap();
        let start = Instant::now();
        serve(
            OwnedFd::from(writer),
            &vec![b'x'; 1 << 20],
            start + Duration::from_millis(100),
        );
        assert!(
            start.elapsed() < Duration::from_secs(1),
            "{:?}",
            start.elapsed()
        );
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert!(
            !got.is_empty() && got.len() < 1 << 20,
            "{} bytes, then the end",
            got.len()
        );
    }

    #[test]
    fn at_most_max_reads_are_served_at_once_and_the_rest_closed_at_once() {
        let serving = Arc::new(AtomicUsize::new(0));
        let bytes: Arc<[u8]> = vec![b'x'; 1 << 20].into();
        let mut readers = Vec::new();
        for _ in 0..2 {
            let (reader, writer) = std::io::pipe().unwrap();
            assert!(spawn_serve(
                &serving,
                2,
                OwnedFd::from(writer),
                Arc::clone(&bytes),
                Duration::from_secs(5)
            ));
            readers.push(reader);
        }
        let (mut refused, writer) = std::io::pipe().unwrap();
        assert!(!spawn_serve(
            &serving,
            2,
            OwnedFd::from(writer),
            Arc::clone(&bytes),
            Duration::from_secs(5)
        ));
        let mut got = Vec::new();
        refused.read_to_end(&mut got).unwrap();
        assert_eq!(got.len(), 0, "the third read is closed at once");
        for mut reader in readers {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            assert_eq!(got.len(), 1 << 20);
        }
    }

    #[test]
    fn serving_a_paste_writes_everything_to_a_reading_client() {
        let (mut reader, writer) = std::io::pipe().unwrap();
        let text: Vec<u8> = (0..200_000u32).map(|i| i as u8).collect();
        let expected = text.clone();
        let server = std::thread::spawn(move || {
            serve(
                OwnedFd::from(writer),
                &text,
                Instant::now() + Duration::from_secs(5),
            )
        });
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        server.join().unwrap();
        assert_eq!(got, expected);
    }

    #[test]
    fn a_serve_that_panics_still_frees_its_place() {
        let serving = Arc::new(AtomicUsize::new(1));
        let slot = ServeSlot(Arc::clone(&serving));
        let thread = std::thread::spawn(move || {
            let _slot = slot;
            panic!("a serve that fails");
        });
        assert!(thread.join().is_err());
        assert_eq!(serving.load(Ordering::Acquire), 0);
    }
}
