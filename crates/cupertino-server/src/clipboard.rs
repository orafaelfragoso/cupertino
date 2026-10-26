//! Clipboard persistence: the server keeps a copy of the clipboard so it outlives the client that copied.
//!
//! When a Wayland client sets the clipboard selection, the server reads the offered types it keeps (text,
//! file lists, rich text, images and PDF, the Wayland side of the design's pasteboard type table,
//! and §7, mapping) one after the other into memory, up
//! to [`CAPTURE_LIMIT`] bytes in all. The client stays the owner and serves every paste while it runs.
//! When it disconnects, the server takes the copy over as a server-side selection and serves pastes from
//! it.
//!
//! UTF-8 text is read once and offered under all its names (`text/plain;charset=utf-8`, `text/plain`,
//! and the legacy names `UTF8_STRING` and `TEXT` that toolkits offer to Wayland too), so a paste finds the
//! name it asks for. The primary selection is not kept: it belongs to its owner by convention, and the
//! reference has none.
//!
//! The owner is the client with keyboard focus when the selection is set, which `wl_data_device` requires
//! of the client that sets it. A clipboard manager (data control) sets it without focus; its selection is
//! then attributed to the focused client, and a copy is taken over if that client exits first (the
//! content is the same; the manager sees its source cancelled).

use std::os::fd::OwnedFd;
use std::sync::Arc;

use smithay::input::Seat;
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::{Interest, Mode, PostAction, RegistrationToken};
use smithay::reexports::rustix;
use smithay::reexports::wayland_server::backend::ClientId;
use smithay::reexports::wayland_server::{DisplayHandle, Resource};
use smithay::wayland::selection::data_device::{
    request_data_device_client_selection, set_data_device_selection,
};
use smithay::wayland::selection::primary_selection::PrimarySelectionState;
use smithay::wayland::selection::{ext_data_control, wlr_data_control};
use smithay::wayland::selection::{SelectionSource, SelectionTarget};
use smithay::{delegate_data_control, delegate_ext_data_control};

use crate::state::Cupertino;

/// Bytes of clipboard content the server copies in all, over every kept type (approximate: no limit is
/// from the design; an uncompressed 5K screen image, 5120 × 2880 × 4 bytes, fits).
pub const CAPTURE_LIMIT: usize = 64 << 20;

/// The names of UTF-8 text, in the order a copy is read by: the first one offered is read.
pub const UTF8_TEXT: [&str; 4] = [
    "text/plain;charset=utf-8",
    "UTF8_STRING",
    "text/plain",
    "TEXT",
];

/// The other kept types, in reading order: file and web lists, rich text, tables, images, PDF. Legacy Latin-1 `STRING` is
/// kept only when no UTF-8 name is offered.
pub const KEPT_TYPES: [&str; 8] = [
    "text/uri-list",
    "text/html",
    "text/rtf",
    "text/tab-separated-values",
    "image/png",
    "image/tiff",
    "application/pdf",
    "STRING",
];

/// A kept clipboard: each offered name with its bytes.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    entries: Vec<(String, Arc<[u8]>)>,
}

impl Snapshot {
    /// The offered names, in order.
    pub fn mime_types(&self) -> Vec<String> {
        self.entries.iter().map(|(m, _)| m.clone()).collect()
    }

    /// The bytes offered as `mime`.
    pub fn get(&self, mime: &str) -> Option<&Arc<[u8]>> {
        self.entries.iter().find(|(m, _)| m == mime).map(|(_, d)| d)
    }

    /// The offer of what was read: UTF-8 text under all its names (in [`UTF8_TEXT`] order), every other type
    /// under its own.
    fn from_captured(captured: Vec<(String, Vec<u8>)>) -> Snapshot {
        let mut entries: Vec<(String, Arc<[u8]>)> = Vec::new();
        for (mime, data) in captured {
            let data: Arc<[u8]> = data.into();
            if UTF8_TEXT.contains(&mime.as_str()) {
                entries.extend(
                    UTF8_TEXT
                        .iter()
                        .map(|name| (name.to_string(), data.clone())),
                );
            } else {
                entries.push((mime, data));
            }
        }
        Snapshot { entries }
    }
}

/// The types of an offer to read, in reading order: the first UTF-8 text name, then the other kept types.
pub fn types_to_keep(offered: &[String]) -> Vec<String> {
    let has = |m: &str| offered.iter().any(|o| o == m);
    let text = UTF8_TEXT.iter().find(|m| has(m));
    let rest = KEPT_TYPES
        .iter()
        .filter(|m| has(m) && !(**m == "STRING" && text.is_some()));
    text.into_iter()
        .chain(rest)
        .map(|m| m.to_string())
        .collect()
}

/// The clipboard copy of the current client selection.
#[derive(Debug, Default)]
pub struct Clipboard {
    /// Counts selections, so reads of a replaced selection are dropped.
    generation: u64,
    /// The client that owns the selection being copied; `None` once there is nothing to take over.
    owner: Option<ClientId>,
    /// The types still to read, in order.
    queue: Vec<String>,
    /// The read in progress: its type and its event source.
    reading: Option<(String, RegistrationToken)>,
    captured: Vec<(String, Vec<u8>)>,
    total: usize,
}

impl Clipboard {
    /// The types read completely so far (tests and logging).
    pub fn captured_types(&self) -> Vec<&str> {
        self.captured.iter().map(|(m, _)| m.as_str()).collect()
    }

    /// Whether every kept type of the current selection has been read.
    pub fn is_complete(&self) -> bool {
        self.queue.is_empty() && self.reading.is_none()
    }
}

/// Write `data` to the paste's pipe without blocking the loop.
fn serve(state: &mut Cupertino, data: Arc<[u8]>, fd: OwnedFd) {
    if let Err(e) = rustix::fs::fcntl_setfl(&fd, rustix::fs::OFlags::NONBLOCK) {
        tracing::warn!(error = %e, "making the clipboard pipe non-blocking failed; the paste is dropped");
        return;
    }
    let mut written = 0;
    let source = Generic::new(fd, Interest::WRITE, Mode::Level);
    let inserted = state.loop_handle.insert_source(source, move |_, fd, _| {
        while written < data.len() {
            match rustix::io::write(&**fd, &data[written..]) {
                Ok(n) => written += n,
                Err(rustix::io::Errno::AGAIN) => return Ok(PostAction::Continue),
                Err(rustix::io::Errno::INTR) => {}
                // the paste's reader went away (EPIPE) or the pipe broke: it gets no more
                Err(e) => {
                    tracing::debug!(error = %e, "a clipboard paste ended before all data was written");
                    return Ok(PostAction::Remove);
                }
            }
        }
        Ok(PostAction::Remove)
    });
    if let Err(e) = inserted {
        tracing::warn!(error = %e, "adding the clipboard writer failed; the paste is dropped");
    }
}

impl Cupertino {
    /// A client set (`Some`) or cleared (`None`) a selection: forget the previous copy and start copying a
    /// new clipboard selection once smithay has made it current.
    pub fn clipboard_selection_changed(
        &mut self,
        ty: SelectionTarget,
        source: Option<&SelectionSource>,
        seat: &Seat<Self>,
    ) {
        if ty != SelectionTarget::Clipboard {
            return;
        }
        self.clipboard_forget();
        let Some(source) = source else { return };
        let focus = seat.get_keyboard().and_then(|k| k.current_focus());
        let Some(owner) = focus.and_then(|s| self.display_handle.get_client(s.id()).ok()) else {
            tracing::debug!("a clipboard selection without a focused client is not kept");
            return;
        };
        self.clipboard.owner = Some(owner.id());
        self.clipboard.queue = types_to_keep(&source.mime_types());
        self.clipboard.queue.reverse();
        let generation = self.clipboard.generation;
        // smithay makes the selection current after this callback returns
        self.loop_handle
            .insert_idle(move |state| state.clipboard_read_next(generation));
    }

    /// Drop the copy and the read in progress: the selection was replaced or cleared.
    pub fn clipboard_forget(&mut self) {
        if let Some((_, token)) = self.clipboard.reading.take() {
            self.loop_handle.remove(token);
        }
        let generation = self.clipboard.generation + 1;
        self.clipboard = Clipboard {
            generation,
            ..Clipboard::default()
        };
    }

    fn clipboard_read_next(&mut self, generation: u64) {
        if generation != self.clipboard.generation || self.clipboard.reading.is_some() {
            return;
        }
        let Some(mime) = self.clipboard.queue.pop() else {
            return;
        };
        // only the server's end is non-blocking: the client writes its end as it would any paste's
        let pipe =
            rustix::pipe::pipe_with(rustix::pipe::PipeFlags::CLOEXEC).and_then(|(read, write)| {
                rustix::fs::fcntl_setfl(&read, rustix::fs::OFlags::NONBLOCK).map(|()| (read, write))
            });
        let (read, write) = match pipe {
            Ok(pair) => pair,
            Err(e) => {
                tracing::warn!(error = %e, "creating a clipboard pipe failed; the clipboard is not kept");
                self.clipboard.queue.clear();
                return;
            }
        };
        if let Err(e) = request_data_device_client_selection(&self.seat, mime.clone(), write) {
            // the selection changed or went away before it could be read; its own change event follows
            tracing::debug!(error = ?e, mime, "the clipboard selection could not be read");
            self.clipboard.queue.clear();
            return;
        }
        let budget = CAPTURE_LIMIT - self.clipboard.total;
        let mut data: Vec<u8> = Vec::new();
        let kind = mime.clone();
        let source = Generic::new(read, Interest::READ, Mode::Level);
        let inserted = self.loop_handle.insert_source(source, move |_, fd, state| {
            let mut chunk = [0u8; 64 * 1024];
            loop {
                match rustix::io::read(&**fd, &mut chunk) {
                    Ok(0) => {
                        state.clipboard_read_done(
                            generation,
                            &kind,
                            Some(std::mem::take(&mut data)),
                        );
                        return Ok(PostAction::Remove);
                    }
                    Ok(n) if data.len() + n > budget => {
                        tracing::info!(
                            mime = kind,
                            limit = CAPTURE_LIMIT,
                            "clipboard content over the limit is not kept"
                        );
                        state.clipboard_read_done(generation, &kind, None);
                        return Ok(PostAction::Remove);
                    }
                    Ok(n) => data.extend_from_slice(&chunk[..n]),
                    Err(rustix::io::Errno::AGAIN) => return Ok(PostAction::Continue),
                    Err(rustix::io::Errno::INTR) => {}
                    Err(e) => {
                        tracing::warn!(error = %e, mime = kind, "reading the clipboard failed");
                        state.clipboard_read_done(generation, &kind, None);
                        return Ok(PostAction::Remove);
                    }
                }
            }
        });
        match inserted {
            Ok(token) => self.clipboard.reading = Some((mime, token)),
            Err(e) => {
                tracing::warn!(error = %e, "adding the clipboard reader failed; the clipboard is not kept");
                self.clipboard.queue.clear();
            }
        }
    }

    /// A read ended: keep its bytes (`Some`) or skip the type (`None`), then read the next type.
    fn clipboard_read_done(&mut self, generation: u64, mime: &str, data: Option<Vec<u8>>) {
        if generation != self.clipboard.generation {
            return;
        }
        self.clipboard.reading = None;
        if let Some(data) = data {
            self.clipboard.total += data.len();
            self.clipboard.captured.push((mime.to_string(), data));
        }
        if self.clipboard.queue.is_empty() {
            return;
        }
        self.loop_handle
            .insert_idle(move |state| state.clipboard_read_next(generation));
    }

    /// After the clients were dispatched: when the owner of the selection being kept has disconnected, the
    /// server's copy becomes the clipboard. A type still being read when the owner went is incomplete and
    /// not offered.
    pub fn clipboard_check_owner(&mut self) {
        let Some(owner) = self.clipboard.owner.clone() else {
            return;
        };
        if self
            .display_handle
            .backend_handle()
            .get_client_data(owner)
            .is_ok()
        {
            return;
        }
        let captured = std::mem::take(&mut self.clipboard.captured);
        self.clipboard_forget();
        if captured.is_empty() {
            return;
        }
        let snapshot = Arc::new(Snapshot::from_captured(captured));
        let mime_types = snapshot.mime_types();
        tracing::info!(types = ?mime_types, "the clipboard's client exited; the server keeps its content");
        set_data_device_selection(&self.display_handle, &self.seat, mime_types, snapshot);
    }

    /// Serve a paste of the kept clipboard from its snapshot (the selection's user data).
    pub fn clipboard_serve_snapshot(&mut self, snapshot: &Snapshot, mime: &str, fd: OwnedFd) {
        match snapshot.get(mime) {
            Some(data) => serve(self, data.clone(), fd),
            None => tracing::warn!(
                mime,
                "a paste asked the kept clipboard for a type it does not offer"
            ),
        }
    }
}

/// The data control globals of clipboard managers, `ext-data-control-v1` and its predecessor
/// `zwlr-data-control-v1`, both with the primary selection. Every client may bind them, as every client may
/// bind the layer shell: the server has no sandbox identity (security-context) to deny them to.
pub fn data_control_states(
    dh: &DisplayHandle,
    primary: &PrimarySelectionState,
) -> (
    ext_data_control::DataControlState,
    wlr_data_control::DataControlState,
) {
    (
        ext_data_control::DataControlState::new::<Cupertino, _>(dh, Some(primary), |_| true),
        wlr_data_control::DataControlState::new::<Cupertino, _>(dh, Some(primary), |_| true),
    )
}

impl ext_data_control::DataControlHandler for Cupertino {
    fn data_control_state(&self) -> &ext_data_control::DataControlState {
        &self.ext_data_control_state
    }
}
delegate_ext_data_control!(Cupertino);

impl wlr_data_control::DataControlHandler for Cupertino {
    fn data_control_state(&self) -> &wlr_data_control::DataControlState {
        &self.wlr_data_control_state
    }
}
delegate_data_control!(Cupertino);

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn utf8_text_is_read_once_and_other_types_follow_in_order() {
        let offered = strings(&[
            "image/png",
            "TEXT",
            "text/plain;charset=utf-8",
            "STRING",
            "application/x-private",
            "text/html",
        ]);
        assert_eq!(
            types_to_keep(&offered),
            ["text/plain;charset=utf-8", "text/html", "image/png"]
        );
        assert_eq!(
            types_to_keep(&strings(&["STRING", "x/y"])),
            ["STRING"],
            "Latin-1 only without UTF-8"
        );
        assert!(types_to_keep(&strings(&["application/x-private"])).is_empty());
    }

    #[test]
    fn the_snapshot_offers_text_under_every_utf8_name() {
        let s = Snapshot::from_captured(vec![
            ("UTF8_STRING".into(), b"hi".to_vec()),
            ("image/png".into(), vec![1, 2]),
        ]);
        assert_eq!(
            s.mime_types(),
            [
                "text/plain;charset=utf-8",
                "UTF8_STRING",
                "text/plain",
                "TEXT",
                "image/png"
            ]
        );
        assert_eq!(s.get("text/plain").map(|d| &d[..]), Some(&b"hi"[..]));
        assert_eq!(s.get("image/png").map(|d| &d[..]), Some(&[1u8, 2][..]));
        assert!(s.get("STRING").is_none());
    }
}
