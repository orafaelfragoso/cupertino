//! The text clipboard (`wl_data_device`): the selection another client offers, read for a paste, and the text this
//! client copied, served to whoever pastes it until another selection replaces it.

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::AsFd;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use rustix::event::{poll, PollFd, PollFlags, Timespec};
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
/// How long a paste waits for the copying client to write its text.
const READ_TIMEOUT: Duration = Duration::from_secs(1);

/// The types an offer announced.
#[derive(Debug, Default)]
pub(crate) struct OfferTypes(Mutex<Vec<String>>);

#[derive(Debug, Default)]
pub(crate) struct Clipboard {
    device: Option<WlDataDevice>,
    /// The current selection of another client.
    selection: Option<WlDataOffer>,
    /// The selection this client set, with its text.
    own: Option<(WlDataSource, String)>,
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
        if let Some((old, _)) = self.own.replace((source, text)) {
            old.destroy();
        }
        Ok(())
    }

    /// The selection's text: this client's own copy directly, another client's read through a pipe (waiting up to
    /// [`READ_TIMEOUT`]); `None` without a text selection.
    pub(crate) fn text(&self, conn: &Connection) -> Result<Option<String>, ClientError> {
        if let Some((_, text)) = &self.own {
            return Ok(Some(text.clone()));
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
        let deadline = Instant::now() + READ_TIMEOUT;
        let mut bytes = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(None);
            }
            let timeout = Timespec::try_from(left).expect("a second fits a timespec");
            let mut fds = [PollFd::new(&reader, PollFlags::IN)];
            if poll(&mut fds, Some(&timeout)).map_err(|e| ClientError::Io(e.into()))? == 0 {
                return Ok(None);
            }
            match reader.read(&mut chunk).map_err(ClientError::Io)? {
                0 => break,
                n => bytes.extend_from_slice(&chunk[..n]),
            }
        }
        Ok(Some(String::from_utf8_lossy(&bytes).into_owned()))
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
                    // the pasting client reads until the descriptor closes, which dropping the file does
                    let _ = File::from(fd).write_all(text.as_bytes());
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
