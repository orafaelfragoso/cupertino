//! CupertinoKit's [`WindowSystem`] on the transport: one [`Connection`], its windows and popups, and the renderer's
//! frames copied into the transport's buffers.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{Wake, Waker};
use std::time::{Duration, Instant};

use cupertino_geometry::Size;
use cupertino_modifiers::Profile;
use cupertino_ui_platform::{
    ActivationRequest, Capabilities, ClipboardData, ClipboardKind, ClipboardToken, ConfigureId,
    CursorShape, KeyEvent, Keysym, PlatformError, PlatformEvent, PopupSpec, PresentOutcome,
    Timestamp, WindowEvent, WindowId, WindowRequest, WindowSpec, WindowSystem,
};
use cupertino_ui_render::CpuFrame;
use cupertino_ui_text::TextInputState;
use cupertino_wayland_client::{
    BufferFormat, Capability, ClipboardOffer, Connection, Error, Event, KeyboardEvent, PixelRect,
    Popup, SurfaceId, TokenRequest, Toplevel, ToplevelConfig,
};

use crate::convert::{self, InputTimeline};
use crate::pixels::convert_rects;

enum Kind {
    Toplevel(Toplevel),
    Popup(Popup),
}

/// What a window whose present was not ready waits for before its next frame is due.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stall {
    Configure,
    Buffer,
}

/// More rectangles than this carried between presents become one for the whole frame.
const MAX_CARRIED: usize = 64;

struct Window {
    id: WindowId,
    surface: SurfaceId,
    kind: Kind,
    parent: Option<WindowId>,
    /// The logical size to draw at when the compositor leaves it to the window.
    size: (u32, u32),
    min_size: Option<Size>,
    max_size: Option<Size>,
    /// Damage of frames that were not presented, carried into the next present.
    carried: Vec<PixelRect>,
    /// The pixel size of the frame shown last; a frame of another size changes all of the window.
    shown: Option<(u32, u32)>,
    /// A frame callback is outstanding: the compositor will say when to draw.
    frame_pending: bool,
    /// The last present was not ready; the next frame waits for this.
    stalled: Option<Stall>,
    /// The runtime asked for a frame.
    frame_wanted: bool,
}

/// Sets the flag [`WindowSystem::dispatch`] reports as [`PlatformEvent::Wake`] and wakes the connection's wait.
struct WakeHandle {
    waker: cupertino_wayland_client::Waker,
    woken: Arc<AtomicBool>,
}

impl Wake for WakeHandle {
    fn wake(self: Arc<Self>) {
        self.wake_by_ref();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.woken.store(true, Ordering::Release);
        self.waker.wake();
    }
}

/// The Wayland window system. Physical modifier keys become logical ones through `profile`.
pub struct WaylandWindowSystem {
    conn: Connection,
    profile: Profile,
    windows: Vec<Window>,
    next_window: u32,
    /// What the window system's clock counts from.
    epoch: Instant,
    input_times: InputTimeline,
    selection: Option<ClipboardOffer>,
    next_clipboard: u64,
    tokens: Vec<(TokenRequest, ActivationRequest)>,
    next_activation: u64,
    /// Events produced outside a dispatch (a clipboard read, a frame that needs no wait), returned by the next one.
    queued: Vec<PlatformEvent>,
    woken: Arc<AtomicBool>,
}

fn platform_error(e: Error) -> PlatformError {
    match e {
        Error::Disconnected(d) => PlatformError::Disconnected(d.to_string()),
        Error::Unsupported(_) => PlatformError::Unsupported,
        e => PlatformError::Protocol(e.to_string()),
    }
}

impl WaylandWindowSystem {
    /// Connect to the compositor named by the environment.
    pub fn connect(profile: Profile) -> Result<WaylandWindowSystem, PlatformError> {
        Connection::connect()
            .map(|conn| WaylandWindowSystem::new(conn, profile))
            .map_err(platform_error)
    }

    pub fn new(conn: Connection, profile: Profile) -> WaylandWindowSystem {
        WaylandWindowSystem {
            conn,
            profile,
            windows: Vec::new(),
            next_window: 0,
            epoch: Instant::now(),
            input_times: InputTimeline::default(),
            selection: None,
            next_clipboard: 0,
            tokens: Vec::new(),
            next_activation: 0,
            queued: Vec::new(),
            woken: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The transport, for an extension protocol such as the desktop's effects.
    pub fn connection(&mut self) -> &mut Connection {
        &mut self.conn
    }

    /// The surface behind `window`, for an extension protocol.
    pub fn surface(&self, window: WindowId) -> Option<SurfaceId> {
        self.windows
            .iter()
            .find(|w| w.id == window)
            .map(|w| w.surface)
    }

    fn now(&self) -> Timestamp {
        Timestamp(self.epoch.elapsed().as_nanos() as u64)
    }

    fn window(&self, id: WindowId) -> Result<&Window, PlatformError> {
        self.windows
            .iter()
            .find(|w| w.id == id)
            .ok_or(PlatformError::UnknownWindow(id))
    }

    fn window_mut(&mut self, id: WindowId) -> Result<&mut Window, PlatformError> {
        self.windows
            .iter_mut()
            .find(|w| w.id == id)
            .ok_or(PlatformError::UnknownWindow(id))
    }

    fn toplevel(&self, id: WindowId) -> Result<Toplevel, PlatformError> {
        match self.window(id)?.kind {
            Kind::Toplevel(t) => Ok(t),
            Kind::Popup(_) => Err(PlatformError::Unsupported),
        }
    }

    fn add(
        &mut self,
        surface: SurfaceId,
        kind: Kind,
        parent: Option<WindowId>,
        size: (u32, u32),
    ) -> WindowId {
        self.next_window += 1;
        let id = WindowId(self.next_window);
        self.windows.push(Window {
            id,
            surface,
            kind,
            parent,
            size,
            min_size: None,
            max_size: None,
            carried: Vec::new(),
            shown: None,
            frame_pending: false,
            stalled: None,
            frame_wanted: false,
        });
        id
    }

    /// The window at `i` may draw again; whether the frame it asked for meanwhile is due now.
    fn resume(&mut self, i: usize) -> bool {
        let w = &mut self.windows[i];
        w.stalled = None;
        !w.frame_pending && std::mem::take(&mut w.frame_wanted)
    }

    fn send_size_limits(&mut self, window: WindowId) -> Result<Result<(), Error>, PlatformError> {
        let t = self.toplevel(window)?;
        let w = self.window(window)?;
        let (min, max) = (
            w.min_size.map(convert::whole),
            w.max_size.map(convert::whole),
        );
        Ok(self.conn.set_size_limits(t, min, max))
    }

    /// Keep `damage` for the next present.
    fn carry(
        &mut self,
        window: WindowId,
        damage: Vec<PixelRect>,
        frame: &CpuFrame,
    ) -> Result<(), PlatformError> {
        let w = self.window_mut(window)?;
        w.carried.extend(damage);
        bound(&mut w.carried, frame.width, frame.height);
        Ok(())
    }

    fn input_time(&mut self, millis: u32) -> Timestamp {
        let now = self.now().0;
        Timestamp(self.input_times.map(millis, now))
    }

    /// The window system's events for one transport event; none for a surface that is not a window here.
    fn translate(&mut self, event: Event, out: &mut Vec<PlatformEvent>) {
        let on = |windows: &[Window], surface: SurfaceId| {
            windows.iter().position(|w| w.surface == surface)
        };
        let mut due = false;
        let (i, event) = match event {
            Event::Configure { window, configure } => {
                let Some(i) = on(&self.windows, window.surface()) else {
                    return;
                };
                if let Some(size) = configure.size {
                    self.windows[i].size = size;
                }
                due = self.resume(i);
                (i, WindowEvent::Configure(convert::configure(&configure)))
            }
            Event::PopupConfigure { popup, configure } => {
                let Some(i) = on(&self.windows, popup.surface()) else {
                    return;
                };
                self.windows[i].size = (configure.width, configure.height);
                due = self.resume(i);
                (
                    i,
                    WindowEvent::Configure(convert::popup_configure(&configure)),
                )
            }
            Event::Close(window) => match on(&self.windows, window.surface()) {
                Some(i) => (i, WindowEvent::CloseRequested),
                None => return,
            },
            Event::PopupDone(popup) => match on(&self.windows, popup.surface()) {
                Some(i) => (i, WindowEvent::PopupDone),
                None => return,
            },
            Event::Frame(callback) => {
                let Some(i) = on(&self.windows, callback.surface) else {
                    return;
                };
                self.windows[i].frame_pending = false;
                if !self.resume(i) {
                    return;
                }
                (i, WindowEvent::Frame(self.now()))
            }
            Event::ScaleChanged { surface, scale } => match on(&self.windows, surface) {
                Some(i) => (i, WindowEvent::Scale(scale.factor())),
                None => return,
            },
            Event::Pointer {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let serial = self.conn.input_serial().unwrap_or(0);
                let time = self.input_time(time);
                (
                    i,
                    WindowEvent::Pointer(convert::pointer(event, serial), time),
                )
            }
            Event::Keyboard {
                surface,
                time,
                event,
            } => {
                let Some(i) = on(&self.windows, surface) else {
                    return;
                };
                let event = match event {
                    KeyboardEvent::Enter => WindowEvent::Focus(true),
                    KeyboardEvent::Leave => WindowEvent::Focus(false),
                    KeyboardEvent::Key {
                        code,
                        pressed,
                        repeat,
                        keysym,
                        character,
                        mods,
                    } => WindowEvent::Key(
                        KeyEvent {
                            code,
                            keysym: Keysym(keysym),
                            text: character,
                            pressed,
                            repeat,
                            mods: convert::mods(mods, &self.profile),
                        },
                        self.input_time(time),
                    ),
                };
                (i, event)
            }
            Event::TextInput { surface, event } => match on(&self.windows, surface) {
                Some(i) => (i, WindowEvent::Ime(convert::ime(event))),
                None => return,
            },
            Event::Selection(offer) => {
                let mimes = offer.as_ref().map(|o| o.mime_types().to_vec());
                self.selection = offer;
                out.push(PlatformEvent::ClipboardOffered {
                    kind: ClipboardKind::Clipboard,
                    mimes,
                });
                return;
            }
            Event::ActivationToken { request, token } => {
                if let Some(at) = self.tokens.iter().position(|(r, _)| *r == request) {
                    let (_, request) = self.tokens.swap_remove(at);
                    out.push(PlatformEvent::ActivationToken { request, token });
                }
                return;
            }
            Event::OutputsChanged | Event::Extension(_) => return,
        };
        let window = self.windows[i].id;
        out.push(PlatformEvent::Window { window, event });
        if due {
            out.push(PlatformEvent::Window {
                window,
                event: WindowEvent::Frame(self.now()),
            });
        }
    }
}

impl WindowSystem for WaylandWindowSystem {
    fn capabilities(&self) -> Capabilities {
        let has = |c| self.conn.capabilities().contains(c);
        Capabilities {
            fractional_scale: has(Capability::FractionalScale),
            presentation_time: false,
            text_input: has(Capability::TextInput),
            touch: false,
            cursor_shape: has(Capability::CursorShape),
            primary_selection: false,
            drag_and_drop: false,
            server_decorations: has(Capability::ServerDecoration),
            activation: has(Capability::Activation),
            app_effects: false,
        }
    }

    fn create_window(&mut self, spec: &WindowSpec) -> Result<WindowId, PlatformError> {
        let toplevel = self
            .conn
            .toplevel(&ToplevelConfig {
                app_id: spec.app_id.clone(),
                title: spec.title.clone(),
                min_size: spec.min_size.map(convert::whole),
                max_size: spec.max_size.map(convert::whole),
                decorations: convert::decorations(spec.decorations),
            })
            .map_err(platform_error)?;
        let id = self.add(
            toplevel.surface(),
            Kind::Toplevel(toplevel),
            None,
            convert::whole(spec.size),
        );
        let w = self.window_mut(id)?;
        (w.min_size, w.max_size) = (spec.min_size, spec.max_size);
        Ok(id)
    }

    fn create_popup(
        &mut self,
        parent: WindowId,
        spec: &PopupSpec,
    ) -> Result<WindowId, PlatformError> {
        let surface = self.window(parent)?.surface;
        let popup = self
            .conn
            .popup(surface, &convert::placement(spec))
            .map_err(platform_error)?;
        Ok(self.add(
            popup.surface(),
            Kind::Popup(popup),
            Some(parent),
            convert::whole(spec.size),
        ))
    }

    fn destroy(&mut self, window: WindowId) {
        let Ok(surface) = self.window(window).map(|w| w.surface) else {
            return;
        };
        // the transport destroys a window's popups with it
        let _ = self.conn.destroy(surface);
        let mut gone = vec![window];
        while let Some(child) = self
            .windows
            .iter()
            .find(|w| w.parent.is_some_and(|p| gone.contains(&p)) && !gone.contains(&w.id))
        {
            gone.push(child.id);
        }
        self.windows.retain(|w| !gone.contains(&w.id));
    }

    fn request(&mut self, window: WindowId, request: WindowRequest) -> Result<(), PlatformError> {
        let result = match request {
            WindowRequest::SetTitle(title) => {
                let t = self.toplevel(window)?;
                self.conn.set_title(t, &title)
            }
            WindowRequest::SetMinSize(size) => {
                self.window_mut(window)?.min_size = size;
                self.send_size_limits(window)?
            }
            WindowRequest::SetMaxSize(size) => {
                self.window_mut(window)?.max_size = size;
                self.send_size_limits(window)?
            }
            WindowRequest::Move => {
                let t = self.toplevel(window)?;
                self.conn.start_move(t)
            }
            WindowRequest::Resize(edge) => {
                let t = self.toplevel(window)?;
                self.conn.start_resize(t, convert::resize_edge(edge))
            }
            WindowRequest::ShowWindowMenu(at) => {
                let t = self.toplevel(window)?;
                self.conn.show_window_menu(t, at)
            }
            WindowRequest::Maximize(on) => {
                let t = self.toplevel(window)?;
                self.conn.set_maximized(t, on)
            }
            WindowRequest::Fullscreen(on) => {
                let t = self.toplevel(window)?;
                self.conn.set_fullscreen(t, on)
            }
            WindowRequest::Minimize => {
                let t = self.toplevel(window)?;
                self.conn.minimize(t)
            }
            WindowRequest::Activate(None) => {
                let t = self.toplevel(window)?;
                self.conn.activate(t)
            }
            WindowRequest::Activate(Some(token)) => {
                let t = self.toplevel(window)?;
                self.conn.activate_with_token(t, &token)
            }
            WindowRequest::Reposition(spec) => {
                let Kind::Popup(p) = self.window(window)?.kind else {
                    return Err(PlatformError::Unsupported);
                };
                self.window_mut(window)?.size = convert::whole(spec.size);
                self.conn.reposition_popup(p, &convert::placement(&spec))
            }
        };
        result.map_err(platform_error)
    }

    fn set_text_input(&mut self, window: WindowId, state: Option<&TextInputState>) {
        let Ok(surface) = self.window(window).map(|w| w.surface) else {
            return;
        };
        let state = state.map(convert::text_input);
        // without the protocol the field types through key events alone
        let _ = self.conn.set_text_input(surface, state.as_ref());
    }

    fn set_cursor(&mut self, _window: WindowId, shape: Option<CursorShape>) {
        // before the pointer enters, or without cursor shapes, the compositor's cursor stays
        let _ = match shape {
            Some(shape) => self.conn.set_cursor(convert::cursor(shape)),
            None => self.conn.hide_cursor(),
        };
    }

    fn set_clipboard(&mut self, kind: ClipboardKind, data: ClipboardData) {
        if kind == ClipboardKind::Clipboard {
            let _ = self.conn.set_selection(vec![(data.mime, data.bytes)]);
        }
    }

    /// The read finishes before this returns (at most a second, the transport's limit); its data arrives with the
    /// next dispatch.
    fn read_clipboard(&mut self, kind: ClipboardKind, mime: &str) -> ClipboardToken {
        self.next_clipboard += 1;
        let token = ClipboardToken(self.next_clipboard);
        let data = match (&self.selection, kind) {
            (Some(offer), ClipboardKind::Clipboard) => {
                let offer = offer.clone();
                self.conn
                    .receive(&offer, mime)
                    .ok()
                    .map(|bytes| ClipboardData {
                        mime: mime.to_owned(),
                        bytes,
                    })
            }
            _ => None,
        };
        self.queued.push(PlatformEvent::Clipboard { token, data });
        token
    }

    fn request_activation_token(&mut self, window: WindowId) -> ActivationRequest {
        self.next_activation += 1;
        let request = ActivationRequest(self.next_activation);
        if let Ok(surface) = self.window(window).map(|w| w.surface) {
            if let Ok(wire) = self.conn.request_activation_token(surface, None) {
                self.tokens.push((wire, request));
            }
        }
        request
    }

    /// With no frame callback outstanding and no present waiting for a configure or a free buffer nothing paces the
    /// window, so the frame is due at once.
    fn request_frame(&mut self, window: WindowId) {
        let now = self.now();
        let Ok(w) = self.window_mut(window) else {
            return;
        };
        if w.frame_pending || w.stalled.is_some() {
            w.frame_wanted = true;
        } else {
            let id = w.id;
            self.queued.push(PlatformEvent::Window {
                window: id,
                event: WindowEvent::Frame(now),
            });
        }
    }

    /// The transport acknowledges the latest configure with every frame, so `configure` needs no request of its own; a
    /// frame drawn at another size than the latest configure asks is not ready.
    fn present(
        &mut self,
        window: WindowId,
        frame: &CpuFrame,
        _configure: Option<ConfigureId>,
    ) -> Result<PresentOutcome, PlatformError> {
        let w = self.window_mut(window)?;
        let (surface, size) = (w.surface, w.size);
        let resized = w.shown != Some((frame.width, frame.height));
        let mut damage = std::mem::take(&mut w.carried);
        damage.extend(frame.damage.iter().map(|r| PixelRect {
            x: r.x,
            y: r.y,
            width: r.width,
            height: r.height,
        }));
        match copy_and_present(&mut self.conn, surface, size, frame, &damage, resized)? {
            Ok(()) => {
                let w = self.window_mut(window)?;
                w.frame_pending = true;
                w.shown = Some((frame.width, frame.height));
                Ok(PresentOutcome::Presented)
            }
            Err(stall) => {
                self.carry(window, damage, frame)?;
                self.window_mut(window)?.stalled = stall;
                Ok(PresentOutcome::NotReady)
            }
        }
    }

    fn dispatch(
        &mut self,
        timeout: Option<Duration>,
        events: &mut Vec<PlatformEvent>,
    ) -> Result<(), PlatformError> {
        let ready = !self.queued.is_empty() || self.woken.load(Ordering::Acquire);
        events.append(&mut self.queued);
        let wire = self
            .conn
            .wait(if ready { Some(Duration::ZERO) } else { timeout })
            .map_err(platform_error)?;
        for event in wire {
            self.translate(event, events);
        }
        for i in 0..self.windows.len() {
            let w = &self.windows[i];
            if w.stalled == Some(Stall::Buffer)
                && matches!(self.conn.buffer_free(w.surface), Ok(true))
                && self.resume(i)
            {
                events.push(PlatformEvent::Window {
                    window: self.windows[i].id,
                    event: WindowEvent::Frame(self.now()),
                });
            }
        }
        if self.woken.swap(false, Ordering::AcqRel) {
            events.push(PlatformEvent::Wake);
        }
        Ok(())
    }

    fn waker(&self) -> Waker {
        Waker::from(Arc::new(WakeHandle {
            waker: self.conn.waker(),
            woken: Arc::clone(&self.woken),
        }))
    }
}

/// Copy `damage` of `frame`, and whatever the buffer missed (all of it when that is unknown), into a buffer of `surface`
/// and present it, telling the compositor that `damage` changed, or everything when the window was `resized`. When it
/// is not presented, what the window waits for: a free buffer or its first configure; nothing for a frame drawn at
/// another size than the surface asks for, which a frame at that size replaces.
fn copy_and_present(
    conn: &mut Connection,
    surface: SurfaceId,
    size: (u32, u32),
    frame: &CpuFrame,
    damage: &[PixelRect],
    resized: bool,
) -> Result<Result<(), Option<Stall>>, PlatformError> {
    let mut lease = match conn.lease(surface, size, BufferFormat::Argb8888) {
        Ok(lease) => lease,
        Err(Error::NoFreeBuffer) => return Ok(Err(Some(Stall::Buffer))),
        Err(Error::NotConfigured) => return Ok(Err(Some(Stall::Configure))),
        Err(e) => return Err(platform_error(e)),
    };
    let layout = lease.layout();
    if (layout.width(), layout.height()) != (frame.width, frame.height) {
        return Ok(Err(None));
    }
    let inside = |rects: &mut dyn Iterator<Item = &PixelRect>| -> Vec<PixelRect> {
        rects
            .filter_map(|r| clip(*r, frame.width, frame.height))
            .collect()
    };
    let changed = inside(&mut damage.iter());
    let copy = lease
        .missed()
        .map(|missed| inside(&mut changed.iter().chain(missed)));
    let whole = [PixelRect {
        x: 0,
        y: 0,
        width: frame.width,
        height: frame.height,
    }];
    convert_rects(
        &frame.pixels,
        frame.stride as usize,
        lease.pixels_mut(),
        layout.stride(),
        copy.as_deref().unwrap_or(&whole),
    );
    lease
        .present_damage((!resized).then_some(&changed[..]))
        .map_err(platform_error)?;
    Ok(Ok(()))
}

/// Replace more than [`MAX_CARRIED`] rectangles by one for the whole `width` × `height` frame.
fn bound(carried: &mut Vec<PixelRect>, width: u32, height: u32) {
    if carried.len() > MAX_CARRIED {
        *carried = vec![PixelRect {
            x: 0,
            y: 0,
            width,
            height,
        }];
    }
}

/// `r` cut to a `width` × `height` buffer; `None` when nothing is left.
fn clip(r: PixelRect, width: u32, height: u32) -> Option<PixelRect> {
    let (x1, y1) = (
        r.x.saturating_add(r.width).min(width),
        r.y.saturating_add(r.height).min(height),
    );
    (r.x < x1 && r.y < y1).then(|| PixelRect {
        x: r.x,
        y: r.y,
        width: x1 - r.x,
        height: y1 - r.y,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(x: u32) -> PixelRect {
        PixelRect {
            x,
            y: 0,
            width: 1,
            height: 1,
        }
    }

    #[test]
    fn carried_damage_beyond_the_limit_becomes_the_whole_frame() {
        let mut carried: Vec<PixelRect> = (0..MAX_CARRIED as u32).map(r).collect();
        bound(&mut carried, 200, 100);
        assert_eq!(carried.len(), MAX_CARRIED);
        carried.push(r(99));
        bound(&mut carried, 200, 100);
        assert_eq!(
            carried,
            [PixelRect {
                x: 0,
                y: 0,
                width: 200,
                height: 100
            }]
        );
    }

    #[test]
    fn damage_is_cut_to_the_frame() {
        let wide = PixelRect {
            x: 190,
            y: 95,
            width: 50,
            height: 50,
        };
        assert_eq!(
            clip(wide, 200, 100),
            Some(PixelRect {
                x: 190,
                y: 95,
                width: 10,
                height: 5
            })
        );
        assert_eq!(clip(r(200), 200, 100), None);
    }
}
