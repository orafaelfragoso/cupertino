//! The colour picker's surfaces: one overlay layer surface on every display, on the connection the server handed the
//! portal, showing a [`Picking`] until the person picks or cancels. The pointer is hidden over the overlay, which draws
//! its own crosshair. One pick runs at a time; a further one fails at once.

use std::error::Error;
use std::sync::mpsc;
use std::time::Instant;

use cupertino_shell_client::{
    Anchor, KeyboardInteractivity, Layer, LayerShell, LayerSurface, LayerSurfaceConfig, Margin,
    ShellEvent,
};
use cupertino_ui_wayland::{Ready, ShmPresenter};
use cupertino_wayland_client::{
    key, Button, Connection, Event, KeyboardEvent, PointerEvent, SurfaceId,
};
use zbus::zvariant::OwnedObjectPath;

use crate::capture::placement;
use crate::pick::{Picked, Picking};
use crate::screenshot::{PickEvent, PickReply};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

/// Layer-shell namespace of the overlay.
const NAMESPACE: &str = "cupertino-portal-color-picker";

/// The overlay of one display.
struct Surface {
    layer: LayerSurface,
    presenter: ShmPresenter,
    /// The display it shows, by index into the pick's displays.
    display: usize,
}

/// The pick on screen.
struct Active {
    handle: OwnedObjectPath,
    reply: PickReply,
    picking: Picking,
    surfaces: Vec<Surface>,
    /// The overlay the pointer was last over, which a move away from it redraws.
    pointed: Option<usize>,
}

/// How the pick on screen ends.
enum End {
    Answer(Picked),
    /// The call fails: the displays changed or the server took a surface away.
    Fail(String),
}

/// The picker, run on the portal's main connection.
pub struct PickOverlay {
    layer_shell: LayerShell,
    picks: mpsc::Receiver<PickEvent>,
    active: Option<Active>,
}

impl PickOverlay {
    /// The picker taking its picks from `picks`; `None` when the server has no layer shell for the portal.
    pub fn bind(conn: &mut Connection, picks: mpsc::Receiver<PickEvent>) -> Option<Self> {
        Some(Self {
            layer_shell: LayerShell::bind(conn)?,
            picks,
            active: None,
        })
    }

    /// Follow `events` (one wait of the connection) and the picks asked for, and draw.
    pub fn update(&mut self, conn: &mut Connection, events: &[Event]) {
        for event in events {
            let end = match &mut self.active {
                Some(active) => active.handle(conn, event),
                None => None,
            };
            if let Some(end) = end {
                self.end(conn, Some(end));
            }
        }
        while let Ok(event) = self.picks.try_recv() {
            match event {
                PickEvent::Pick {
                    handle,
                    displays,
                    reply,
                } => {
                    if self.active.is_some() {
                        eprintln!(
                            "cupertino-portal: {handle} asked for a colour during another pick"
                        );
                        continue;
                    }
                    match self.open(conn, handle, displays, reply) {
                        Ok(active) => self.active = Some(active),
                        Err(e) => eprintln!("cupertino-portal: no colour picker: {e}"),
                    }
                }
                PickEvent::Close { handle } => {
                    if self.active.as_ref().is_some_and(|a| a.handle == handle) {
                        self.end(conn, None);
                    }
                }
            }
        }
        if let Some(active) = &mut self.active {
            if let Err(e) = active.present(conn) {
                self.end(conn, Some(End::Fail(e.to_string())));
            }
        }
    }

    /// An overlay on every display of `displays` that is still where it was copied.
    fn open(
        &self,
        conn: &mut Connection,
        handle: OwnedObjectPath,
        displays: Vec<crate::screenshot::Output>,
        reply: PickReply,
    ) -> Result<Active> {
        let outputs = conn.outputs();
        let mut surfaces = Vec::new();
        for (display, copied) in displays.iter().enumerate() {
            let Some(info) = outputs.iter().find(|info| {
                let at = placement(info);
                (at.position, at.size) == (copied.position, copied.size)
            }) else {
                continue;
            };
            let layer = self.layer_shell.layer_surface(
                conn,
                &LayerSurfaceConfig {
                    namespace: NAMESPACE.into(),
                    output: Some(info.id),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                    size: (0, 0),
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    // one surface takes the keys for the whole overlay, so Escape cancels wherever the pointer is
                    keyboard: if surfaces.is_empty() {
                        KeyboardInteractivity::Exclusive
                    } else {
                        KeyboardInteractivity::None
                    },
                },
            )?;
            surfaces.push(Surface {
                presenter: ShmPresenter::new(layer.surface()),
                layer,
                display,
            });
        }
        if surfaces.is_empty() {
            return Err("no display is where it was copied".into());
        }
        let _ = conn.flush();
        Ok(Active {
            handle,
            reply,
            picking: Picking::new(displays, Instant::now()),
            surfaces,
            pointed: None,
        })
    }

    /// Take the overlay down and answer `end`; `None` leaves the call unanswered, as when the frontend closed it.
    fn end(&mut self, conn: &mut Connection, end: Option<End>) {
        let Some(active) = self.active.take() else {
            return;
        };
        for surface in &active.surfaces {
            let _ = conn.destroy(surface.layer.surface());
        }
        let _ = conn.flush();
        match end {
            Some(End::Answer(picked)) => active.reply.send(picked),
            Some(End::Fail(reason)) => {
                eprintln!(
                    "cupertino-portal: the colour pick for {} failed: {reason}",
                    active.handle
                )
            }
            None => {}
        }
    }
}

impl Active {
    fn surface(&self, id: SurfaceId) -> Option<usize> {
        self.surfaces.iter().position(|s| s.layer.surface() == id)
    }

    fn handle(&mut self, conn: &mut Connection, event: &Event) -> Option<End> {
        match event {
            Event::Frame(frame) => {
                if let Some(i) = self.surface(frame.surface) {
                    self.surfaces[i].presenter.frame_done();
                }
            }
            Event::Extension(event) => match event.downcast_ref::<ShellEvent>() {
                Some(&ShellEvent::Configure { surface, size }) => {
                    let i = self.surface(surface)?;
                    let s = &mut self.surfaces[i];
                    match s.layer.configured(size) {
                        Ok(size) => s.presenter.configured(size),
                        Err(e) => return Some(End::Fail(e.to_string())),
                    }
                }
                Some(&ShellEvent::Closed(surface)) if self.surface(surface).is_some() => {
                    return Some(End::Fail("the server closed the overlay".to_owned()))
                }
                _ => {}
            },
            Event::OutputsChanged => {
                let outputs = conn.outputs();
                let moved = self.surfaces.iter().any(|s| {
                    let copied = &self.picking.displays()[s.display];
                    !outputs.iter().any(|info| {
                        let at = placement(info);
                        (at.position, at.size) == (copied.position, copied.size)
                    })
                });
                if moved {
                    return Some(End::Fail("the displays changed".to_owned()));
                }
            }
            Event::Keyboard {
                surface,
                event:
                    KeyboardEvent::Key {
                        code: key::ESCAPE,
                        pressed: true,
                        ..
                    },
                ..
            } if self.surface(*surface).is_some() => {
                return Some(End::Answer(self.picking.escape()));
            }
            Event::Pointer { surface, event, .. } => {
                let i = self.surface(*surface)?;
                let origin = self.picking.displays()[self.surfaces[i].display].position;
                let global = |x: f64, y: f64| (x + f64::from(origin.0), y + f64::from(origin.1));
                let picked = match event {
                    PointerEvent::Enter(p) => {
                        // the overlay draws its own crosshair
                        let _ = conn.hide_cursor();
                        self.picking.moved(global(p.x, p.y));
                        None
                    }
                    PointerEvent::Motion(p) => {
                        self.picking.moved(global(p.x, p.y));
                        None
                    }
                    PointerEvent::Leave => {
                        self.picking.left();
                        None
                    }
                    PointerEvent::Button {
                        button: Button::Left,
                        pressed,
                        position,
                    } => self.picking.button(
                        *pressed,
                        global(position.x, position.y),
                        Instant::now(),
                    ),
                    PointerEvent::Button { .. } | PointerEvent::Scroll { .. } => None,
                };
                self.point(i);
                return picked.map(End::Answer);
            }
            _ => {}
        }
        None
    }

    /// The pointer is over overlay `i` (or just left it): redraw it and the one it was over before.
    fn point(&mut self, i: usize) {
        self.surfaces[i].presenter.request_redraw();
        if let Some(before) = self.pointed.replace(i).filter(|&before| before != i) {
            self.surfaces[before].presenter.request_redraw();
        }
    }

    /// Draw every overlay that can take a frame.
    fn present(&mut self, conn: &mut Connection) -> Result<()> {
        for s in &mut self.surfaces {
            loop {
                match s.presenter.poll(conn)? {
                    Ready::Idle => break,
                    // a draw follows every scale change
                    Ready::ScaleChanged(_) => {}
                    Ready::Draw => {
                        let (picking, display) = (&self.picking, s.display);
                        s.presenter.draw(conn, None, |pixmap, scale| {
                            picking.paint(display, pixmap, scale.factor() as f32)
                        })?;
                    }
                }
            }
        }
        let _ = conn.flush();
        Ok(())
    }
}
