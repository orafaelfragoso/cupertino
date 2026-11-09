//! Text-input bridge of the accent picker: an in-process `zwp_input_method_v2` client.
//!
//! Smithay serves `zwp_text_input_v3` to applications only while an input method is connected, and it
//! forwards the applications' `enable` / cursor rectangle requests to that input method. The compositor
//! therefore connects *itself* as the input method: a socket pair carries a private Wayland connection
//! whose server end is added to the display and whose client end is driven by a background thread. The
//! thread
//!
//! * tells the compositor whether a text input is active in the focused application (the accent
//!   picker only starts on key repeat then),
//! * turns the picker's [`TextOp`] edits into `delete_surrounding_text`, `set_preedit_string`,
//!   `commit_string` and `commit` requests (applied by Smithay to the application's text input),
//! * creates an input popup surface so that Smithay reports the text cursor rectangle
//!   (`InputMethodHandler::popup_repositioned`), which anchors the accent popup.
//!
//! The bridge is only created when the accent picker is enabled *and* an accent table was loaded: it
//! occupies the single input-method slot of the seat, so an installed input method (for example for
//! other scripts) keeps working otherwise. Without the bridge the picker model and popup work, but nothing
//! can be inserted into applications.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use smithay::reexports::calloop::channel::Sender as LoopSender;
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use smithay::utils::{Logical, Rectangle};
use smithay::wayland::input_method::{InputMethodHandler, PopupSurface};
use smithay::{delegate_input_method_manager, delegate_text_input_manager};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_compositor, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_manager_v2::ZwpInputMethodManagerV2, zwp_input_method_v2,
    zwp_input_method_v2::ZwpInputMethodV2, zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
};

use crate::kbd_ui::TextOp;
use crate::state::{ClientState, Cupertino};

/// What the bridge thread reports.
#[derive(Clone, Debug, PartialEq)]
pub enum ImEvent {
    /// A text input became active / inactive in the focused application.
    Active(bool),
    /// Text around the cursor (for the byte length of characters to delete).
    Surrounding { text: String, cursor: u32 },
}

enum Command {
    Op(TextOp),
    Stop,
}

/// Handle on the bridge thread.
pub struct ImBridge {
    tx: Sender<Command>,
    wake: OwnedFd,
}

impl ImBridge {
    /// Queue an edit for the focused application.
    pub fn send(&self, op: TextOp) -> bool {
        if self.tx.send(Command::Op(op)).is_err() {
            return false;
        }
        wake(&self.wake);
        true
    }
}

impl Drop for ImBridge {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Stop);
        wake(&self.wake);
    }
}

fn wake(fd: &OwnedFd) {
    let one = 1u8;
    // Safety: a valid pipe write end and a one byte buffer.
    unsafe { libc::write(fd.as_raw_fd(), (&one as *const u8).cast(), 1) };
}

struct Client {
    commands: Receiver<Command>,
    events: LoopSender<ImEvent>,
    im: Option<ZwpInputMethodV2>,
    /// `done` events received so far: the serial of the next `commit`.
    serial: u32,
    pending_active: bool,
    active: bool,
    surrounding: Option<(String, u32)>,
    stop: bool,
}

impl Client {
    /// Byte length of the last `chars` characters before the cursor (one byte each when the text is unknown).
    fn bytes_before_cursor(&self, chars: usize) -> u32 {
        let Some((text, cursor)) = &self.surrounding else {
            return chars as u32;
        };
        let end = (*cursor as usize).min(text.len());
        let before = text.get(..end).unwrap_or("");
        let skip = before.chars().count().saturating_sub(chars);
        before
            .char_indices()
            .nth(skip)
            .map_or(0, |(i, _)| (before.len() - i) as u32)
    }

    fn apply(&mut self, op: TextOp) {
        let Some(im) = self.im.clone() else { return };
        if !self.active {
            return;
        }
        match op {
            TextOp::Preedit {
                delete_before,
                text,
            } => {
                if delete_before > 0 {
                    im.delete_surrounding_text(self.bytes_before_cursor(delete_before), 0);
                }
                let len = text.len() as i32;
                im.set_preedit_string(text, 0, len);
            }
            TextOp::Commit {
                delete_before,
                text,
            } => {
                if delete_before > 0 {
                    im.delete_surrounding_text(self.bytes_before_cursor(delete_before), 0);
                }
                im.set_preedit_string(String::new(), 0, 0);
                im.commit_string(text);
            }
            TextOp::Restore { text } => {
                im.set_preedit_string(String::new(), 0, 0);
                im.commit_string(text);
            }
        }
        im.commit(self.serial);
    }
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Client {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for Client {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
ignore_events!(
    wl_seat::WlSeat,
    wl_compositor::WlCompositor,
    wl_surface::WlSurface,
    ZwpInputMethodManagerV2,
    ZwpInputPopupSurfaceV2
);

impl Dispatch<ZwpInputMethodV2, ()> for Client {
    fn event(
        state: &mut Self,
        _: &ZwpInputMethodV2,
        event: zwp_input_method_v2::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwp_input_method_v2::Event::Activate => state.pending_active = true,
            zwp_input_method_v2::Event::Deactivate => state.pending_active = false,
            zwp_input_method_v2::Event::SurroundingText { text, cursor, .. } => {
                state.surrounding = Some((text, cursor))
            }
            zwp_input_method_v2::Event::Done => {
                state.serial = state.serial.wrapping_add(1);
                if state.pending_active != state.active {
                    state.active = state.pending_active;
                    let _ = state.events.send(ImEvent::Active(state.active));
                }
                if let (true, Some((text, cursor))) = (state.active, state.surrounding.clone()) {
                    let _ = state.events.send(ImEvent::Surrounding { text, cursor });
                }
            }
            zwp_input_method_v2::Event::Unavailable => {
                tracing::warn!("input method bridge: another input method owns the seat");
                state.stop = true;
            }
            _ => {}
        }
    }
}

fn run(
    stream: UnixStream,
    commands: Receiver<Command>,
    events: LoopSender<ImEvent>,
    wake_rd: OwnedFd,
) -> Result<(), String> {
    let conn = Connection::from_socket(stream).map_err(|e| e.to_string())?;
    let (globals, mut queue) = registry_queue_init::<Client>(&conn).map_err(|e| e.to_string())?;
    let qh = queue.handle();
    let manager: ZwpInputMethodManagerV2 = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("zwp_input_method_manager_v2: {e}"))?;
    let seat: wl_seat::WlSeat = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_seat: {e}"))?;
    let compositor: wl_compositor::WlCompositor = globals
        .bind(&qh, 1..=1, ())
        .map_err(|e| format!("wl_compositor: {e}"))?;
    let im = manager.get_input_method(&seat, &qh, ());
    // an (unmapped) popup surface makes the compositor report the text cursor rectangle
    let surface = compositor.create_surface(&qh, ());
    let _popup = im.get_input_popup_surface(&surface, &qh, ());
    surface.commit();
    let mut client = Client {
        commands,
        events,
        im: Some(im),
        serial: 0,
        pending_active: false,
        active: false,
        surrounding: None,
        stop: false,
    };
    loop {
        queue
            .dispatch_pending(&mut client)
            .map_err(|e| e.to_string())?;
        while let Ok(cmd) = client.commands.try_recv() {
            match cmd {
                Command::Op(op) => client.apply(op),
                Command::Stop => client.stop = true,
            }
        }
        if client.stop {
            return Ok(());
        }
        conn.flush().map_err(|e| e.to_string())?;
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        let mut fds = [
            libc::pollfd {
                fd: guard.connection_fd().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: wake_rd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        // Safety: two valid pollfd entries.
        let n = unsafe { libc::poll(fds.as_mut_ptr(), 2, -1) };
        if n < 0 {
            continue;
        }
        if fds[1].revents & libc::POLLIN != 0 {
            let mut buf = [0u8; 64];
            // Safety: reading into a local buffer from a valid fd.
            unsafe { libc::read(wake_rd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len()) };
        }
        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            guard.read().map_err(|e| e.to_string())?;
        }
    }
}

impl Cupertino {
    /// Connect the in-process input method (see the module documentation). Needs the display to already
    /// export the input-method global (created in `Cupertino::new` when the picker is configured).
    pub fn start_input_method(&mut self, events: LoopSender<ImEvent>) -> Option<ImBridge> {
        let (server_end, client_end) = UnixStream::pair().ok()?;
        let data = Arc::new(ClientState {
            internal: true,
            ..Default::default()
        });
        if let Err(e) = self.display_handle.insert_client(server_end, data) {
            tracing::warn!(error = %e, "input method bridge: cannot add the client");
            return None;
        }
        let mut fds = [0i32; 2];
        // Safety: a valid two element array for pipe2.
        if unsafe { libc::pipe2(fds.as_mut_ptr(), libc::O_CLOEXEC | libc::O_NONBLOCK) } != 0 {
            return None;
        }
        // Safety: both descriptors were just created and are owned here.
        let (rd, wr) = unsafe { (OwnedFd::from_raw_fd(fds[0]), OwnedFd::from_raw_fd(fds[1])) };
        let (tx, rx) = channel();
        let spawned = std::thread::Builder::new()
            .name("cupertino-input-method".into())
            .spawn(move || {
                if let Err(e) = run(client_end, rx, events, rd) {
                    tracing::warn!(error = %e, "input method bridge stopped");
                }
            });
        spawned.ok()?;
        Some(ImBridge { tx, wake: wr })
    }

    /// Connect the bridge when the display exports the input-method global (an accent table was loaded).
    pub fn connect_input_method(&mut self) -> bool {
        use smithay::reexports::calloop::channel::{channel, Event};
        if self.input_method_state.is_none() || self.input_method.is_some() {
            return false;
        }
        let (tx, rx) = channel::<ImEvent>();
        let inserted = self.loop_handle.insert_source(rx, |event, _, state| {
            if let Event::Msg(ev) = event {
                state.on_input_method_event(ev);
            }
        });
        if inserted.is_err() {
            return false;
        }
        self.input_method = self.start_input_method(tx);
        self.input_method.is_some()
    }

    /// A report of the bridge thread.
    pub fn on_input_method_event(&mut self, ev: ImEvent) {
        match ev {
            ImEvent::Active(active) => {
                self.overlays.accent.bridge = active;
                if !active {
                    self.accent_dismiss();
                }
            }
            ImEvent::Surrounding { .. } => {}
        }
    }

    /// The text cursor of the focused application in global logical coordinates, from the rectangle the
    /// application sent (surface-local) and the window's content origin.
    pub fn text_cursor_global(&self) -> Option<cupertino_geometry::Rect> {
        let r = self.text_cursor?;
        let id = self.wm.key_window()?;
        let g = self.wm.window(id)?.geometry();
        let ins = self.entry(id).map(|e| e.insets()).unwrap_or_default();
        Some(cupertino_geometry::Rect::new(
            g.x + ins.left + f64::from(r.loc.x),
            g.y + ins.top + f64::from(r.loc.y),
            f64::from(r.size.w.max(1)),
            f64::from(r.size.h.max(1)),
        ))
    }

    /// Deliver one edit to the focused client's text input through the bridge.
    pub fn deliver_text_op(&self, op: &TextOp) -> bool {
        self.input_method
            .as_ref()
            .is_some_and(|b| b.send(op.clone()))
    }
}

impl InputMethodHandler for Cupertino {
    fn new_popup(&mut self, surface: PopupSurface) {
        self.text_cursor = Some(surface.text_input_rectangle());
        self.input_popup = Some(surface);
    }

    fn dismiss_popup(&mut self, surface: PopupSurface) {
        self.text_cursor = None;
        if self
            .input_popup
            .as_ref()
            .is_some_and(|p| p.wl_surface() == surface.wl_surface())
        {
            self.input_popup = None;
        }
    }

    fn popup_repositioned(&mut self, surface: PopupSurface) {
        self.text_cursor = Some(surface.text_input_rectangle());
        self.input_popup = Some(surface);
    }

    fn parent_geometry(&self, _parent: &WlSurface) -> Rectangle<i32, Logical> {
        Rectangle::default()
    }
}
delegate_input_method_manager!(Cupertino);
delegate_text_input_manager!(Cupertino);
