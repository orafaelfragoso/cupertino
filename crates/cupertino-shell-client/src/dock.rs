//! Typed wrapper of `cupertino_dock_v1`: the dock role of a layer surface.

use crate::Error;
use cupertino_geometry::Rect;
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1, WindowState};
use cupertino_wayland_client::OutputId;
use wayland_client::{Proxy, WEnum};

pub use cupertino_protocols::client::cupertino_dock_v1::{AppCommand, Corner, Gesture};

/// A window of the session as the server describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockWindow {
    /// The server's id: stable for the window's lifetime, never reused within a session.
    pub id: u32,
    /// Application id (the desktop entry id when known).
    pub app_id: String,
    pub title: String,
    pub minimized: bool,
    /// The key window of the session.
    pub active: bool,
}

/// Largest window icon side a dock accepts, in pixels: the server sends at most 512.
pub const MAX_WINDOW_ICON: u32 = 512;

/// A window's own icon image: `size` × `size` pixels of `wl_shm` ARGB8888 (premultiplied, little-endian), rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowIconImage {
    pub size: u32,
    pub argb: Vec<u8>,
}

/// The icon a window set for itself (`xdg_toplevel_icon_v1`), as the server copied it; from shell version 17.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockWindowIcon {
    /// The window's id, as in [`DockWindow`].
    pub id: u32,
    /// A key into the icon theme, never a path.
    pub name: Option<String>,
    pub image: Option<WindowIconImage>,
}

/// Shell version of the request that shows one window.
const ACTIVATE_WINDOW_VERSION: u32 = 26;

/// One change of a group, in the order the server sent it.
#[derive(Clone, Debug, PartialEq)]
pub enum DockChange {
    /// A window appeared or one of its properties changed.
    Window(DockWindow),
    /// The window's own icon is set, changed or removed; it follows the window's [`DockChange::Window`].
    WindowIcon(DockWindowIcon),
    /// The window with this id went away.
    WindowClosed(u32),
    /// The server started a program of this application id.
    Launching(String),
    /// The auto-hide shortcut was pressed.
    ToggleAutoHide,
    /// The app-grid action fired (shortcut, hot corner or gesture): open the application grid, or
    /// close it when it is open.
    ToggleAppGrid,
    /// The application switcher opened or its selection moved: the output the panel goes on, the
    /// whole list (application ids, most recently used first) and the selected index. The server
    /// guarantees the list is not empty and the index is inside it.
    SwitcherChanged {
        output: OutputId,
        apps: Vec<String>,
        selected: usize,
    },
    /// The application asked for attention while inactive: bounce its tile once.
    Attention(String),
    /// The application stopped answering (`true`) or answers again (`false`): "Not Responding" in the dock menu, the
    /// switcher and the Force Quit dialog. Also sent once for each application that is unresponsive when the dock binds.
    Unresponsive { app_id: String, unresponsive: bool },
    /// The Force Quit shortcut was pressed: open the Force Quit dialog, or close it when it is open.
    ForceQuitDialog,
    /// The Focus Dock shortcut was pressed: the dock takes the keyboard on its first tile; from shell version 18.
    Engage,
    /// A process of the application that showed a window is still connected (`true`), with or without windows, or no
    /// longer is (`false`): it exited. The first group lists every resident application; from shell version 19.
    AppResident { app_id: String, resident: bool },
    /// The switch ended: `committed` when the selected application was activated, else cancelled.
    SwitcherClosed { committed: bool },
    /// The pointer dwelt in a corner of the dock's output: the dock maps it to its action.
    HotCorner(Corner),
    /// A touchpad gesture committed on the dock's output that no transaction is bound to: the dock maps it to its
    /// action.
    Gesture(Gesture),
    /// The application the launch behind window `id` named (the application id of its activation token), which its
    /// own application id may not be: the dock files the window under it. Before the window's [`DockChange::Window`]
    /// in the group that first lists it, else in the group of the activation that named it; once per window, from
    /// shell version 32.
    WindowLaunch { id: u32, app_id: String },
}

/// One group of events, applied at once. The first group after the object is created lists every
/// window.
pub type DockUpdate = Vec<DockChange>;

/// The dock of one output. Dropping it gives up the role.
pub struct Dock {
    proxy: CupertinoDockV1,
}

impl Dock {
    pub(crate) fn new(proxy: CupertinoDockV1) -> Self {
        Self { proxy }
    }

    /// Raise the application's windows and focus the frontmost; an unknown id is ignored by the
    /// server (the application may have just quit).
    pub fn activate_app(&self, app_id: &str) {
        self.proxy.activate_app(app_id.to_string());
    }

    /// End the application, after the person confirmed in the Force Quit dialog. The server kills one that does not
    /// answer its pings at once; any other is asked to close and sent SIGTERM, then SIGKILL after three seconds. The
    /// result is the application's windows closing; an unknown id is ignored.
    pub fn force_quit_app(&self, app_id: &str) {
        self.proxy.force_quit_app(app_id.to_string());
    }

    /// Show one window: the server restores it when it is minimized, switches to its workspace, raises it and gives
    /// it the keyboard; an id that is no window any more is ignored. False, with nothing sent, on a server older than
    /// shell version 26, which can only bring the whole application forward ([`Dock::activate_app`]).
    pub fn activate_window(&self, id: u32) -> bool {
        let known = self.proxy.version() >= ACTIVATE_WINDOW_VERSION;
        if known {
            self.proxy.activate_window(id);
        }
        known
    }

    /// Restore a minimized window out of the tile `rect` (surface-local logical points, rounded
    /// outwards to whole points); an id that is not a minimized window is ignored by the server.
    pub fn restore_window(&self, id: u32, rect: &Rect) -> Result<(), Error> {
        let (x, y, width, height) = whole_rect(rect)?;
        self.proxy.restore_window(id, x, y, width, height);
        Ok(())
    }

    /// The pointer picked entry `index` of the open switcher; the server ignores an index outside
    /// the list and answers with [`DockChange::SwitcherChanged`] when the selection moved.
    pub fn switcher_select(&self, index: usize) -> Result<(), Error> {
        let index = u32::try_from(index).map_err(|_| Error::InvalidShape)?;
        self.proxy.switcher_select(index);
        Ok(())
    }

    /// The pointer was released on entry `index` of the open switcher: the server selects and
    /// commits it (an index outside the list is ignored).
    pub fn switcher_activate(&self, index: usize) -> Result<(), Error> {
        let index = u32::try_from(index).map_err(|_| Error::InvalidShape)?;
        self.proxy.switcher_activate(index);
        Ok(())
    }

    /// The pointer was released on the switcher's panel off the entries: the switch ends.
    pub fn switcher_cancel(&self) {
        self.proxy.switcher_cancel();
    }

    /// The slot a newly minimized window's tile takes (as for [`Dock::restore_window`]); applies
    /// with the next commit of the surface.
    pub fn set_minimize_target(&self, rect: &Rect) -> Result<(), Error> {
        let (x, y, width, height) = whole_rect(rect)?;
        self.proxy.set_minimize_target(x, y, width, height);
        Ok(())
    }

    /// Switch the dock's output to the workspace.
    pub fn switch_workspace(&self, id: u32) {
        self.proxy.switch_workspace(id);
    }

    /// Append a workspace to the dock's output.
    pub fn add_workspace(&self) {
        self.proxy.add_workspace();
    }

    /// Remove a workspace; its windows move to the previous (else the next) one. The server
    /// ignores the last regular workspace.
    pub fn remove_workspace(&self, id: u32) {
        self.proxy.remove_workspace(id);
    }

    /// Move a workspace to position `index` (0-based, clamped to the end) among the output's.
    pub fn move_workspace(&self, id: u32, index: usize) -> Result<(), Error> {
        let index = u32::try_from(index).map_err(|_| Error::InvalidShape)?;
        self.proxy.move_workspace(id, index);
        Ok(())
    }

    /// Run a named compositor action (the names of the shortcut file) as its shortcut would.
    pub fn run_action(&self, name: &str) {
        self.proxy.run_action(name.to_string());
    }

    /// A command of a tile's context menu for the application.
    pub fn app_command(&self, app_id: &str, command: AppCommand) {
        self.proxy.app_command(app_id.to_string(), command);
    }
}

impl Drop for Dock {
    fn drop(&mut self) {
        self.proxy.destroy();
    }
}

/// The smallest whole-point rectangle that contains `rect`; an error for a non-finite one.
fn whole_rect(rect: &Rect) -> Result<(i32, i32, i32, i32), Error> {
    if ![rect.x(), rect.y(), rect.width(), rect.height()]
        .iter()
        .all(|v| v.is_finite())
    {
        return Err(Error::InvalidShape);
    }
    let r = rect.round_out();
    Ok((
        r.x() as i32,
        r.y() as i32,
        r.width() as i32,
        r.height() as i32,
    ))
}

/// The image of a `window_icon` event: `None` for size 0, a size over [`MAX_WINDOW_ICON`], or a file that does not
/// hold exactly the image.
fn icon_image(size: u32, pixels: std::os::fd::OwnedFd) -> Option<WindowIconImage> {
    use std::os::unix::fs::FileExt;
    if size == 0 || size > MAX_WINDOW_ICON {
        return None;
    }
    let file = std::fs::File::from(pixels);
    let len = size as usize * size as usize * 4;
    if file.metadata().ok()?.len() != len as u64 {
        return None;
    }
    let mut argb = vec![0; len];
    file.read_exact_at(&mut argb, 0).ok()?;
    Some(WindowIconImage { size, argb })
}

/// Collects the events of a group until `done`.
#[derive(Debug, Default)]
pub(crate) struct GroupBuilder {
    changes: DockUpdate,
    /// `switcher_entry` events since the last `switcher_changed`.
    switcher_apps: Vec<String>,
    /// The output of the last `switcher_output`, resolved by the caller of [`GroupBuilder::switcher_output`].
    switcher_output: Option<OutputId>,
}

impl GroupBuilder {
    /// `switcher_output`: the output (the caller maps the server's `wl_output` to its id).
    pub(crate) fn switcher_output(&mut self, output: OutputId) {
        self.switcher_output = Some(output);
    }

    /// Add an event; the finished group when it was a `done`. An empty group yields nothing. A
    /// `switcher_changed` without the `switcher_output` the protocol puts first is an error.
    pub(crate) fn push(
        &mut self,
        event: cupertino_dock_v1::Event,
    ) -> Result<Option<DockUpdate>, &'static str> {
        use cupertino_dock_v1::Event;
        match event {
            Event::Window {
                id,
                app_id,
                title,
                state,
            } => {
                // bits a newer server defines are not ours to act on
                let state = WindowState::from_bits_truncate(u32::from(state));
                self.changes.push(DockChange::Window(DockWindow {
                    id,
                    app_id,
                    title,
                    minimized: state.contains(WindowState::Minimized),
                    active: state.contains(WindowState::Active),
                }));
            }
            Event::WindowIcon {
                id,
                name,
                size,
                pixels,
            } => self.changes.push(DockChange::WindowIcon(DockWindowIcon {
                id,
                name: (!name.is_empty()).then_some(name),
                image: icon_image(size, pixels),
            })),
            Event::WindowClosed { id } => self.changes.push(DockChange::WindowClosed(id)),
            Event::Launching { app_id } => self.changes.push(DockChange::Launching(app_id)),
            Event::ToggleAutoHide => self.changes.push(DockChange::ToggleAutoHide),
            Event::ToggleAppGrid => self.changes.push(DockChange::ToggleAppGrid),
            Event::SwitcherEntry { app_id } => self.switcher_apps.push(app_id),
            Event::SwitcherChanged { selected } => {
                let apps = std::mem::take(&mut self.switcher_apps);
                let output = self
                    .switcher_output
                    .take()
                    .ok_or("switcher_changed without switcher_output")?;
                self.changes.push(DockChange::SwitcherChanged {
                    output,
                    apps,
                    selected: selected as usize,
                });
            }
            Event::Attention { app_id } => self.changes.push(DockChange::Attention(app_id)),
            Event::Unresponsive { app_id, state } => self.changes.push(DockChange::Unresponsive {
                app_id,
                unresponsive: state != 0,
            }),
            Event::ForceQuitDialog => self.changes.push(DockChange::ForceQuitDialog),
            Event::Engage => self.changes.push(DockChange::Engage),
            Event::AppResident { app_id, resident } => self.changes.push(DockChange::AppResident {
                app_id,
                resident: resident != 0,
            }),
            Event::SwitcherClosed { committed } => self.changes.push(DockChange::SwitcherClosed {
                committed: committed != 0,
            }),
            Event::WindowLaunch { id, app_id } => {
                self.changes.push(DockChange::WindowLaunch { id, app_id })
            }
            // a corner or gesture a newer server defines has no action here
            Event::HotCorner {
                corner: WEnum::Value(corner),
            } => self.changes.push(DockChange::HotCorner(corner)),
            Event::Gesture {
                kind: WEnum::Value(kind),
            } => self.changes.push(DockChange::Gesture(kind)),
            Event::HotCorner {
                corner: WEnum::Unknown(_),
            }
            | Event::Gesture {
                kind: WEnum::Unknown(_),
            } => {}
            Event::Done => {
                let changes = std::mem::take(&mut self.changes);
                return Ok((!changes.is_empty()).then_some(changes));
            }
            _ => {}
        }
        Ok(None)
    }
}
