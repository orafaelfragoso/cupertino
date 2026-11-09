//! Gives the process's CupertinoKit windows their surfaces, by `app_id` ([`cupertino_login::view`]): a lock screen
//! window gets its output's `ext-session-lock` surface, a greeter window a full-output overlay layer surface with the
//! keyboard, and a panel a centred overlay layer surface with the keyboard. The roles also hold the session lock:
//! they lock before the first lock surface and unlock once the application stopped wanting the lock and its lock
//! surfaces are gone, and they pass on what the server says that the windows do not hear (the lock's answer, the
//! session's requests, Caps Lock, the outputs).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use cupertino_kit::geometry::Size;
use cupertino_kit::platform::WindowSpec;
use cupertino_login::view::{GREETER_APP_ID, INTERRUPT_APP_ID, LOCK_APP_ID, PANEL_APP_ID};
use cupertino_shell_client::{
    Anchor, Error as ShellError, KeyboardInteractivity, Layer, LayerShell, LayerSurface,
    LayerSurfaceConfig, Margin, SessionChange, SessionLock, SessionLockManager, SessionState,
    ShellEvent,
};
use cupertino_ui_wayland::{RoleEvent, SurfaceRoles};
use cupertino_wayland_client::{
    Connection, Error as WaylandError, ExtensionEvent, OutputId, SurfaceId,
};

/// Namespace of the greeter's layer surfaces for the server.
const GREETER_NAMESPACE: &str = "login-greeter";
/// Namespace of the panels' layer surfaces for the server.
const PANEL_NAMESPACE: &str = "login-panel";

/// What the roles heard for the application.
#[derive(Clone, Debug)]
pub enum Heard {
    /// The outputs the server has described, by registry name, with their size in points.
    Outputs(Vec<(u32, Size)>),
    CapsLock(bool),
    /// The server's answer to the lock of `generation`: locked, or refused or ended.
    Lock {
        generation: u32,
        locked: bool,
    },
    /// The seat went idle (`true`) or input resumed.
    Idle(bool),
    LockRequested {
        explicit: bool,
    },
    LogOutRequested,
    ReduceMotion(bool),
    /// The session lock cannot be taken: the server lacks the protocol.
    NoLock,
}

/// The session lock as the application and the roles share it.
#[derive(Debug, Default)]
pub struct LockWish {
    /// The application wants the session locked.
    pub wanted: bool,
    /// The generation of the lock the roles hold; its answer is the one that counts.
    pub generation: Option<u32>,
}

enum Role {
    Lock,
    Layer(LayerSurface),
}

pub struct LoginRoles {
    layer_shell: LayerShell,
    lock_manager: Option<SessionLockManager>,
    lock: Option<SessionLock>,
    wish: Rc<RefCell<LockWish>>,
    session: Option<SessionState>,
    surfaces: HashMap<SurfaceId, Role>,
    caps_lock: Option<bool>,
    heard: async_channel::Sender<Heard>,
}

/// The outputs `conn` has described, by registry name, with their size.
pub fn outputs(conn: &Connection) -> Vec<(u32, Size)> {
    conn.outputs()
        .into_iter()
        .filter_map(|o| {
            let g = o.geometry?;
            Some((
                o.id.registry_name(),
                Size::new(f64::from(g.width), f64::from(g.height)),
            ))
        })
        .collect()
}

/// The registry name in a window's `app_id` of `prefix.<name>`.
fn output_of(app_id: &str, prefix: &str) -> Option<OutputId> {
    let name = app_id.strip_prefix(prefix)?.strip_prefix('.')?;
    name.parse().ok().map(OutputId::from_registry_name)
}

fn transport(e: ShellError) -> WaylandError {
    match e {
        ShellError::Transport(e) => e,
        ShellError::UnknownOutput => WaylandError::Protocol("the surface's output is gone"),
        _ => WaylandError::Protocol("the server refused the surface"),
    }
}

impl LoginRoles {
    /// Roles on `layer_shell`, locking through `lock_manager` when there is one; `session` follows the session's
    /// display options for Reduce Motion.
    pub fn new(
        layer_shell: LayerShell,
        lock_manager: Option<SessionLockManager>,
        session: Option<SessionState>,
        wish: Rc<RefCell<LockWish>>,
        heard: async_channel::Sender<Heard>,
    ) -> LoginRoles {
        LoginRoles {
            layer_shell,
            lock_manager,
            lock: None,
            wish,
            session,
            surfaces: HashMap::new(),
            caps_lock: None,
            heard,
        }
    }

    fn send(&self, heard: Heard) {
        // the receiver lives as long as the application
        let _ = self.heard.send_blocking(heard);
    }

    /// Take the session lock if the application wants it and none is held. `false` when the server cannot lock.
    fn ensure_lock(&mut self, conn: &Connection) -> bool {
        if self.lock.is_some() {
            return true;
        }
        let Some(manager) = self.lock_manager.as_mut() else {
            return false;
        };
        let lock = manager.lock(conn);
        self.wish.borrow_mut().generation = Some(lock.generation());
        self.lock = Some(lock);
        true
    }

    fn layer(
        &mut self,
        conn: &mut Connection,
        config: &LayerSurfaceConfig,
    ) -> Result<SurfaceId, WaylandError> {
        let layer = self
            .layer_shell
            .layer_surface(conn, config)
            .map_err(transport)?;
        let surface = layer.surface();
        self.surfaces.insert(surface, Role::Layer(layer));
        Ok(surface)
    }
}

impl SurfaceRoles for LoginRoles {
    fn create(
        &mut self,
        conn: &mut Connection,
        spec: &WindowSpec,
    ) -> Option<Result<SurfaceId, WaylandError>> {
        if let Some(output) = output_of(&spec.app_id, LOCK_APP_ID) {
            if !self.ensure_lock(conn) {
                return Some(Err(WaylandError::Protocol(
                    "the server lacks ext_session_lock_manager_v1",
                )));
            }
            let lock = self.lock.as_ref().expect("taken above");
            let surface = match lock.lock_surface(conn, output) {
                Ok(surface) => surface,
                Err(e) => return Some(Err(transport(e))),
            };
            self.surfaces.insert(surface, Role::Lock);
            return Some(Ok(surface));
        }
        if let Some(output) = output_of(&spec.app_id, GREETER_APP_ID) {
            return Some(self.layer(
                conn,
                &LayerSurfaceConfig {
                    namespace: GREETER_NAMESPACE.to_owned(),
                    output: Some(output),
                    layer: Layer::Overlay,
                    anchor: Anchor::Top | Anchor::Right | Anchor::Bottom | Anchor::Left,
                    size: (0, 0),
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    keyboard: KeyboardInteractivity::Exclusive,
                },
            ));
        }
        if spec.app_id == PANEL_APP_ID || spec.app_id == INTERRUPT_APP_ID {
            // the panels show on the first output the server has described
            let output = conn
                .outputs()
                .into_iter()
                .find(|o| o.geometry.is_some())
                .map(|o| o.id);
            return Some(self.layer(
                conn,
                &LayerSurfaceConfig {
                    namespace: PANEL_NAMESPACE.to_owned(),
                    output,
                    layer: Layer::Overlay,
                    // no anchor: the server centres the surface on the output
                    anchor: Anchor::empty(),
                    size: (spec.size.width as u32, spec.size.height as u32),
                    margin: Margin::default(),
                    exclusive_zone: -1,
                    keyboard: KeyboardInteractivity::Exclusive,
                },
            ));
        }
        None
    }

    fn event(&mut self, event: &ExtensionEvent) -> Option<RoleEvent> {
        if let Some(SessionChange::DisplayOptions(options)) =
            self.session.as_mut().and_then(|s| s.apply(event))
        {
            self.send(Heard::ReduceMotion(options.reduce_motion));
        }
        match *event.downcast_ref::<ShellEvent>()? {
            ShellEvent::Configure { surface, size } => match self.surfaces.get(&surface)? {
                Role::Lock => Some(RoleEvent::Configure { surface, size }),
                // a configure without a size closes the window
                Role::Layer(layer) => Some(match layer.configured(size) {
                    Ok(size) => RoleEvent::Configure { surface, size },
                    Err(_) => RoleEvent::Closed(surface),
                }),
            },
            ShellEvent::Closed(surface) => {
                self.surfaces.get(&surface)?;
                Some(RoleEvent::Closed(surface))
            }
            ShellEvent::Lock { generation, locked } => {
                self.send(Heard::Lock { generation, locked });
                None
            }
            ShellEvent::Idle(idle) => {
                self.send(Heard::Idle(idle));
                None
            }
            ShellEvent::LockRequested { explicit } => {
                self.send(Heard::LockRequested { explicit });
                None
            }
            ShellEvent::LogOutRequested => {
                self.send(Heard::LogOutRequested);
                None
            }
            ShellEvent::SessionLock { .. }
            | ShellEvent::MenuBar { .. }
            | ShellEvent::Dock { .. }
            | ShellEvent::AppActivation { .. } => None,
        }
    }

    fn destroyed(&mut self, surface: SurfaceId) {
        self.surfaces.remove(&surface);
    }

    fn outputs_changed(&mut self, conn: &Connection) {
        self.send(Heard::Outputs(outputs(conn)));
    }

    /// Follow the application's wish: lock before its first lock surface (also with no output at all), and unlock once
    /// it no longer wants the lock and its lock surfaces are destroyed. Pass Caps Lock on when it changes.
    fn prepare(&mut self, conn: &mut Connection) {
        let caps_lock = conn.modifiers().caps_lock;
        if self.caps_lock != Some(caps_lock) {
            self.caps_lock = Some(caps_lock);
            self.send(Heard::CapsLock(caps_lock));
        }
        let wanted = self.wish.borrow().wanted;
        if wanted && self.lock.is_none() && !self.ensure_lock(conn) {
            self.wish.borrow_mut().wanted = false;
            self.send(Heard::NoLock);
        }
        let lock_surfaces = self.surfaces.values().any(|r| matches!(r, Role::Lock));
        if !wanted && !lock_surfaces {
            if let Some(lock) = self.lock.take() {
                lock.unlock();
                self.wish.borrow_mut().generation = None;
            }
        }
    }
}
