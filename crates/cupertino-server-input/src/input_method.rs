//! Text-input bridge of the accent picker: an in-process `zwp_input_method_v2` client.

use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{channel, Receiver, Sender};

use smithay::reexports::calloop::channel::Sender as LoopSender;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_compositor, wl_registry, wl_seat, wl_surface};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::{
    zwp_input_method_manager_v2::ZwpInputMethodManagerV2, zwp_input_method_v2,
    zwp_input_method_v2::ZwpInputMethodV2, zwp_input_popup_surface_v2::ZwpInputPopupSurfaceV2,
};

use crate::kbd_ui::TextOp;

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
    /// Spawn the background `zwp_input_method_v2` client on `client_end` and report events on `events`.
    pub fn start(client_end: UnixStream, events: LoopSender<ImEvent>) -> Option<Self> {
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
