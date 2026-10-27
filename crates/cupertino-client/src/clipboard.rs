//! The text clipboard (`wl_data_device`): the selection another client offers, read for a paste, and the text this
//! client copied, served to whoever pastes it until another selection replaces it.

use std::fs::File;
use std::io::{ErrorKind, Read, Write};
use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
use rustix::fs::{fcntl_setfl, OFlags};
use wayland_client::protocol::wl_data_device::{self, WlDataDevice};
use wayland_client::protocol::wl_data_device_manager::WlDataDeviceManager;
use wayland_client::protocol::wl_data_offer::{self, WlDataOffer};
use wayland_client::protocol::wl_data_source::{self, WlDataSource};
use wayland_client::{
    delegate_noop, event_created_child, Connection, Dispatch, Proxy, QueueHandle,
};

use crate::{ClientError, State};

/// The text types offered on copy, preferred first when reading a paste.
const TEXT_TYPES: [&str; 3] = ["text/plain;charset=utf-8", "text/plain", "UTF8_STRING"];
/// How long a paste waits for the copying client to write its text; the caller's thread waits with it.
const READ_TIMEOUT: Duration = Duration::from_secs(1);
/// The most a paste reads; the rest of a longer selection is left out.
const READ_LIMIT: usize = 1 << 20;
/// How long serving our copy to a pasting client may take before the descriptor is dropped.
const SERVE_TIMEOUT: Duration = Duration::from_secs(1);
/// The most pastes served at once; a paste asked for beyond it gets nothing (its descriptor is closed at once).
const MAX_SERVES: usize = 8;

/// The types an offer announced.
#[derive(Debug, Default)]
pub(crate) struct OfferTypes(Mutex<Vec<String>>);

#[derive(Debug, Default)]
pub(crate) struct Clipboard {
    device: Option<WlDataDevice>,
    /// The current selection of another client.
    selection: Option<WlDataOffer>,
    /// The selection this client set, with its text.
    own: Option<(WlDataSource, Arc<str>)>,
    /// Pastes being served now.
    serving: Arc<AtomicUsize>,
}

impl Clipboard {
    pub(crate) fn new(
        manager: Option<&WlDataDeviceManager>,
        seat: &wayland_client::protocol::wl_seat::WlSeat,
        qh: &QueueHandle<State>,
    ) -> Self {
        Clipboard {
            device: manager.map(|m| m.get_data_device(seat, qh, ())),
            ..Clipboard::default()
        }
    }

    /// Offer `text` as the selection; `serial` is that of the input event that asked for it.
    pub(crate) fn set_text(
        &mut self,
        manager: Option<&WlDataDeviceManager>,
        text: String,
        serial: u32,
        qh: &QueueHandle<State>,
    ) -> Result<(), ClientError> {
        let (Some(manager), Some(device)) = (manager, &self.device) else {
            return Err(ClientError::MissingGlobal("wl_data_device_manager"));
        };
        let source = manager.create_data_source(qh, ());
        for t in TEXT_TYPES {
            source.offer(t.to_string());
        }
        device.set_selection(Some(&source), serial);
        if let Some((old, _)) = self.own.replace((source, text.into())) {
            old.destroy();
        }
        Ok(())
    }

    /// The selection's text: this client's own copy directly, another client's read through a pipe (the caller waits
    /// up to [`READ_TIMEOUT`] and reads at most [`READ_LIMIT`] bytes); `None` without a text selection or when the
    /// other client does not finish in time.
    pub(crate) fn text(&self, conn: &Connection) -> Result<Option<String>, ClientError> {
        if let Some((_, text)) = &self.own {
            return Ok(Some(text.to_string()));
        }
        let Some(offer) = &self.selection else {
            return Ok(None);
        };
        let types = offer
            .data::<OfferTypes>()
            .map(|t| t.0.lock().expect("offer types").clone())
            .unwrap_or_default();
        let Some(mime) = TEXT_TYPES.iter().find(|t| types.iter().any(|o| o == *t)) else {
            return Ok(None);
        };
        let (mut reader, writer) = std::io::pipe().map_err(ClientError::Io)?;
        offer.receive(mime.to_string(), writer.as_fd());
        drop(writer);
        conn.flush()?;
        let bytes = read_limited(&mut reader, Instant::now() + READ_TIMEOUT, READ_LIMIT)
            .map_err(ClientError::Io)?;
        Ok(bytes.map(|b| text_of(&b)))
    }
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
        if poll(&mut fds, Some(&timeout))? == 0 {
            return Ok(None);
        }
        match reader.read(&mut chunk)? {
            0 => return Ok(Some(bytes)),
            n => bytes.extend_from_slice(&chunk[..n.min(limit - bytes.len())]),
        }
    }
    Ok(Some(bytes))
}

/// `bytes` as text: invalid sequences replaced, a character cut by the read limit dropped.
fn text_of(bytes: &[u8]) -> String {
    let whole = match std::str::from_utf8(bytes) {
        Err(e) if e.error_len().is_none() => &bytes[..e.valid_up_to()],
        _ => bytes,
    };
    String::from_utf8_lossy(whole).into_owned()
}

/// Serve `text` to `fd` on a thread of its own when fewer than `max` serves run (counted in `serving`); else close
/// `fd` at once. Whether it is served.
fn spawn_serve(
    serving: &Arc<AtomicUsize>,
    max: usize,
    fd: OwnedFd,
    text: Arc<str>,
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
        serve(fd, text.as_bytes(), Instant::now() + timeout);
    });
    true
}

/// A claimed place among the pastes served at once; dropping it frees it.
struct ServeSlot(Arc<AtomicUsize>);

impl Drop for ServeSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Write `text` to a pasting client's `fd` without blocking, until done or `deadline`, then close it. A client that
/// does not read gets what fits in the pipe.
fn serve(fd: OwnedFd, text: &[u8], deadline: Instant) {
    if fcntl_setfl(&fd, OFlags::NONBLOCK).is_err() {
        return;
    }
    let mut file = File::from(fd);
    let mut rest = text;
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
                if let Some(old) = std::mem::replace(&mut state.clipboard.selection, id) {
                    old.destroy();
                }
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
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                if let Some((_, text)) = state.clipboard.own.as_ref().filter(|(s, _)| s == source) {
                    // off the event loop: a pasting client that reads slowly or not at all never holds up this one; it
                    // reads until the descriptor closes, which the end of `serve` does
                    spawn_serve(
                        &state.clipboard.serving,
                        MAX_SERVES,
                        fd,
                        Arc::clone(text),
                        SERVE_TIMEOUT,
                    );
                }
            }
            wl_data_source::Event::Cancelled => {
                if state
                    .clipboard
                    .own
                    .as_ref()
                    .is_some_and(|(s, _)| s == source)
                {
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
    fn a_paste_stops_at_the_limit() {
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
    fn a_paste_from_a_client_that_never_finishes_gives_nothing_at_the_deadline() {
        let (mut reader, mut writer) = std::io::pipe().unwrap();
        writer.write_all(b"partial").unwrap();
        let start = Instant::now();
        let bytes = read_limited(&mut reader, start + Duration::from_millis(50), LIMIT).unwrap();
        assert_eq!(bytes, None);
        assert!(start.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn a_character_cut_by_the_limit_is_dropped() {
        assert_eq!(text_of("aé".as_bytes()), "aé");
        assert_eq!(text_of(&"aé".as_bytes()[..2]), "a");
        assert_eq!(text_of(b"a\xFFb"), "a\u{FFFD}b");
    }

    #[test]
    fn serving_a_paste_writes_everything_to_a_reading_client() {
        let (mut reader, writer) = std::io::pipe().unwrap();
        let text = vec![b'x'; 200_000];
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
    fn at_most_max_pastes_are_served_at_once_and_the_rest_closed_at_once() {
        let serving = Arc::new(AtomicUsize::new(0));
        let text: Arc<str> = "x".repeat(1 << 20).into();
        let mut readers = Vec::new();
        for _ in 0..3 {
            let (reader, writer) = std::io::pipe().unwrap();
            assert!(spawn_serve(
                &serving,
                3,
                OwnedFd::from(writer),
                Arc::clone(&text),
                Duration::from_secs(5)
            ));
            readers.push(reader);
        }
        let (mut refused, writer) = std::io::pipe().unwrap();
        assert!(!spawn_serve(
            &serving,
            3,
            OwnedFd::from(writer),
            Arc::clone(&text),
            Duration::from_secs(5)
        ));
        let mut got = Vec::new();
        refused.read_to_end(&mut got).unwrap();
        assert_eq!(got.len(), 0, "the fourth paste is closed at once");
        assert_eq!(serving.load(Ordering::Acquire), 3);
        for mut reader in readers {
            let mut got = Vec::new();
            reader.read_to_end(&mut got).unwrap();
            assert_eq!(got.len(), 1 << 20);
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        while serving.load(Ordering::Acquire) > 0 {
            assert!(Instant::now() < deadline, "the serves never ended");
            std::thread::sleep(Duration::from_millis(5));
        }
        let (mut reader, writer) = std::io::pipe().unwrap();
        assert!(
            spawn_serve(
                &serving,
                3,
                OwnedFd::from(writer),
                text,
                Duration::from_secs(5)
            ),
            "a slot frees when a serve ends"
        );
        let mut got = Vec::new();
        reader.read_to_end(&mut got).unwrap();
        assert_eq!(got.len(), 1 << 20);
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
