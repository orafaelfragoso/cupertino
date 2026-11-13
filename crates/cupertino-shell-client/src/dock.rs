//! Typed wrapper of `cupertino_dock_v1`: the dock role of a layer surface.

use crate::Error;
use cupertino_geometry::Rect;
use cupertino_protocols::client::cupertino_dock_v1::{self, CupertinoDockV1, WindowState};
use cupertino_wayland_client::OutputId;
use wayland_client::WEnum;

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

/// Largest side of a window's picture a dock accepts, in pixels: the server sends at most 256.
pub const MAX_WINDOW_THUMBNAIL: u32 = 256;

/// A picture: `width` × `height` pixels of `wl_shm` ARGB8888 (premultiplied, little-endian), rows packed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailImage {
    pub width: u32,
    pub height: u32,
    pub argb: Vec<u8>,
}

/// The picture of a minimized window, as the server drew it when the window minimized; `None` once the window is no
/// longer minimized. From shell version 22.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DockWindowThumbnail {
    /// The window's id, as in [`DockWindow`].
    pub id: u32,
    pub image: Option<ThumbnailImage>,
}

/// Which windows the open overview shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OverviewMode {
    /// Every window of the current workspace, with the workspaces bar.
    All,
    /// The windows of the application with this id, minimized ones in a lower row.
    App(String),
}

/// A window of the open overview: its id (as in [`DockWindow`]) and its frame on the desktop, in
/// the output's logical coordinates.
#[derive(Clone, Debug, PartialEq)]
pub struct OverviewWindow {
    pub id: u32,
    pub frame: Rect,
}

/// A workspace of the dock's output, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OverviewWorkspace {
    /// The server's id, stable for the workspace's lifetime.
    pub id: u32,
    /// The one on screen.
    pub current: bool,
}

/// One change of a group, in the order the server sent it.
#[derive(Clone, Debug, PartialEq)]
pub enum DockChange {
    /// A window appeared or one of its properties changed.
    Window(DockWindow),
    /// The window's own icon is set, changed or removed; it follows the window's [`DockChange::Window`].
    WindowIcon(DockWindowIcon),
    /// The minimized window's picture is set or removed; it follows the window's [`DockChange::Window`].
    WindowThumbnail(DockWindowThumbnail),
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
    /// The overview opened on the dock's output, or its windows or workspaces changed while open:
    /// the whole window set and workspace list, and the area (output coordinates) the windows are
    /// laid out in. The dock answers with [`Dock::overview_frame`] for every window.
    OverviewChanged {
        mode: OverviewMode,
        area: Rect,
        windows: Vec<OverviewWindow>,
        workspaces: Vec<OverviewWorkspace>,
    },
    /// The overview is leaving: the server animates the windows back.
    OverviewClosed,
    /// The pointer dwelt in a corner of the dock's output: the dock maps it to its action.
    HotCorner(Corner),
    /// A touchpad gesture committed on the dock's output: the dock maps it to its action.
    Gesture(Gesture),
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

    /// The frame window `id` takes in the open overview (output coordinates, rounded outwards to
    /// whole points); the server animates it there.
    pub fn overview_frame(&self, id: u32, rect: &Rect) -> Result<(), Error> {
        let (x, y, width, height) = whole_rect(rect)?;
        self.proxy.overview_frame(id, x, y, width, height);
        Ok(())
    }

    /// Leave the overview activating window `id` (a minimized one is restored).
    pub fn overview_select(&self, id: u32) {
        self.proxy.overview_select(id);
    }

    /// Leave the overview without a pick.
    pub fn overview_close(&self) {
        self.proxy.overview_close();
    }

    /// Switch the dock's output to the workspace, leaving an open overview first.
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

/// The image of a `window_thumbnail` event: `None` for an empty size, a side over [`MAX_WINDOW_THUMBNAIL`], or a file
/// that does not hold exactly the image.
fn thumbnail_image(
    width: u32,
    height: u32,
    pixels: std::os::fd::OwnedFd,
) -> Option<ThumbnailImage> {
    use std::os::unix::fs::FileExt;
    if width == 0 || height == 0 || width.max(height) > MAX_WINDOW_THUMBNAIL {
        return None;
    }
    let file = std::fs::File::from(pixels);
    let len = width as usize * height as usize * 4;
    if file.metadata().ok()?.len() != len as u64 {
        return None;
    }
    let mut argb = vec![0; len];
    file.read_exact_at(&mut argb, 0).ok()?;
    Some(ThumbnailImage {
        width,
        height,
        argb,
    })
}

/// Collects the events of a group until `done`.
#[derive(Debug, Default)]
pub(crate) struct GroupBuilder {
    changes: DockUpdate,
    /// `switcher_entry` events since the last `switcher_changed`.
    switcher_apps: Vec<String>,
    /// The output of the last `switcher_output`, resolved by the caller of [`GroupBuilder::switcher_output`].
    switcher_output: Option<OutputId>,
    /// `overview_window` and `overview_workspace` events since the last `overview_changed`.
    overview_windows: Vec<OverviewWindow>,
    overview_workspaces: Vec<OverviewWorkspace>,
}

/// A rectangle the server sent in whole logical points.
fn rect_of(x: i32, y: i32, width: i32, height: i32) -> Rect {
    Rect::new(
        f64::from(x),
        f64::from(y),
        f64::from(width),
        f64::from(height),
    )
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
            Event::WindowThumbnail {
                id,
                width,
                height,
                pixels,
            } => self
                .changes
                .push(DockChange::WindowThumbnail(DockWindowThumbnail {
                    id,
                    image: thumbnail_image(width, height, pixels),
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
            Event::OverviewWindow {
                id,
                x,
                y,
                width,
                height,
            } => self.overview_windows.push(OverviewWindow {
                id,
                frame: rect_of(x, y, width, height),
            }),
            Event::OverviewWorkspace { id, current } => {
                self.overview_workspaces.push(OverviewWorkspace {
                    id,
                    current: current != 0,
                })
            }
            Event::OverviewChanged {
                mode,
                app_id,
                x,
                y,
                width,
                height,
            } => {
                // a mode a newer server defines is shown as the plain overview
                let mode = if mode == WEnum::Value(cupertino_dock_v1::OverviewMode::App) {
                    OverviewMode::App(app_id)
                } else {
                    OverviewMode::All
                };
                let windows = std::mem::take(&mut self.overview_windows);
                let workspaces = std::mem::take(&mut self.overview_workspaces);
                self.changes.push(DockChange::OverviewChanged {
                    mode,
                    area: rect_of(x, y, width, height),
                    windows,
                    workspaces,
                });
            }
            Event::OverviewClosed => self.changes.push(DockChange::OverviewClosed),
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

#[cfg(test)]
mod tests {
    use super::*;
    use cupertino_dock_v1::Event;
    use wayland_client::WEnum;

    /// Push into a group whose events are all well formed.
    fn push(group: &mut GroupBuilder, event: Event) -> Option<DockUpdate> {
        group.push(event).expect("a well-formed event")
    }

    fn window(id: u32, state: WindowState) -> Event {
        Event::Window {
            id,
            app_id: "org.example.editor".into(),
            title: "notes".into(),
            state: WEnum::Value(state),
        }
    }

    #[test]
    fn a_group_keeps_its_changes_in_order_until_done() {
        let mut group = GroupBuilder::default();
        assert_eq!(
            push(
                &mut group,
                window(4, WindowState::Minimized | WindowState::Active)
            ),
            None
        );
        assert_eq!(push(&mut group, Event::WindowClosed { id: 2 }), None);
        assert_eq!(
            push(
                &mut group,
                Event::Launching {
                    app_id: "foot".into()
                }
            ),
            None
        );
        assert_eq!(push(&mut group, Event::ToggleAutoHide), None);
        assert_eq!(push(&mut group, Event::ToggleAppGrid), None);
        assert_eq!(
            push(&mut group, Event::Done),
            Some(vec![
                DockChange::Window(DockWindow {
                    id: 4,
                    app_id: "org.example.editor".into(),
                    title: "notes".into(),
                    minimized: true,
                    active: true
                }),
                DockChange::WindowClosed(2),
                DockChange::Launching("foot".into()),
                DockChange::ToggleAutoHide,
                DockChange::ToggleAppGrid,
            ])
        );
        assert_eq!(
            push(&mut group, Event::Done),
            None,
            "groups do not leak into each other"
        );
    }

    #[test]
    fn the_switcher_list_ends_at_its_selection() {
        let mut group = GroupBuilder::default();
        group.switcher_output(OutputId::from_registry_name(3));
        for app in ["org.example.b", "org.example.a"] {
            push(&mut group, Event::SwitcherEntry { app_id: app.into() });
        }
        push(&mut group, Event::SwitcherChanged { selected: 1 });
        group.switcher_output(OutputId::from_registry_name(5));
        push(
            &mut group,
            Event::SwitcherEntry {
                app_id: "org.example.c".into(),
            },
        );
        push(&mut group, Event::SwitcherChanged { selected: 0 });
        push(
            &mut group,
            Event::Attention {
                app_id: "org.example.d".into(),
            },
        );
        push(&mut group, Event::SwitcherClosed { committed: 1 });
        push(
            &mut group,
            Event::Unresponsive {
                app_id: "org.example.e".into(),
                state: 1,
            },
        );
        push(&mut group, Event::ForceQuitDialog);
        push(&mut group, Event::Engage);
        push(
            &mut group,
            Event::AppResident {
                app_id: "org.example.f".into(),
                resident: 0,
            },
        );
        assert_eq!(
            push(&mut group, Event::Done),
            Some(vec![
                DockChange::SwitcherChanged {
                    output: OutputId::from_registry_name(3),
                    apps: vec!["org.example.b".into(), "org.example.a".into()],
                    selected: 1
                },
                DockChange::SwitcherChanged {
                    output: OutputId::from_registry_name(5),
                    apps: vec!["org.example.c".into()],
                    selected: 0
                },
                DockChange::Attention("org.example.d".into()),
                DockChange::SwitcherClosed { committed: true },
                DockChange::Unresponsive {
                    app_id: "org.example.e".into(),
                    unresponsive: true
                },
                DockChange::ForceQuitDialog,
                DockChange::Engage,
                DockChange::AppResident {
                    app_id: "org.example.f".into(),
                    resident: false
                },
            ])
        );
    }

    /// A file holding `bytes`, as the server's sealed file would.
    fn file_of(bytes: &[u8], name: &str) -> std::os::fd::OwnedFd {
        use std::io::Write;
        let path = std::env::temp_dir().join(format!(
            "cupertino-shell-client-{}-{name}",
            std::process::id()
        ));
        std::fs::File::create(&path)
            .unwrap()
            .write_all(bytes)
            .unwrap();
        let file = std::fs::File::open(&path).unwrap();
        std::fs::remove_file(&path).unwrap();
        file.into()
    }

    #[test]
    fn a_window_picture_is_read_from_its_file_and_an_empty_or_wrong_one_is_none() {
        let mut group = GroupBuilder::default();
        let pixels = [1u8, 2, 3, 4].repeat(6);
        let event = |id, width, height, bytes: &[u8], name| Event::WindowThumbnail {
            id,
            width,
            height,
            pixels: file_of(bytes, name),
        };
        push(&mut group, event(7, 3, 2, &pixels, "good"));
        push(&mut group, event(8, 0, 0, &[], "none"));
        push(&mut group, event(9, 3, 3, &pixels, "short"));
        push(&mut group, event(10, 300, 1, &[0; 1200], "large"));
        let thumbnail = |id, image| DockChange::WindowThumbnail(DockWindowThumbnail { id, image });
        assert_eq!(
            push(&mut group, Event::Done),
            Some(vec![
                thumbnail(
                    7,
                    Some(ThumbnailImage {
                        width: 3,
                        height: 2,
                        argb: pixels.clone()
                    })
                ),
                thumbnail(8, None),
                thumbnail(9, None),
                thumbnail(10, None),
            ])
        );
    }

    #[test]
    fn a_switcher_list_without_its_output_is_a_protocol_error() {
        let mut group = GroupBuilder::default();
        push(
            &mut group,
            Event::SwitcherEntry {
                app_id: "org.example.a".into(),
            },
        );
        assert!(group.push(Event::SwitcherChanged { selected: 0 }).is_err());
    }

    #[test]
    fn the_overview_group_carries_its_windows_workspaces_and_area() {
        let mut group = GroupBuilder::default();
        push(
            &mut group,
            Event::OverviewWindow {
                id: 4,
                x: 10,
                y: 20,
                width: 300,
                height: 200,
            },
        );
        push(
            &mut group,
            Event::OverviewWindow {
                id: 7,
                x: -5,
                y: 40,
                width: 100,
                height: 100,
            },
        );
        push(&mut group, Event::OverviewWorkspace { id: 1, current: 1 });
        push(&mut group, Event::OverviewWorkspace { id: 2, current: 0 });
        push(
            &mut group,
            Event::OverviewChanged {
                mode: WEnum::Value(cupertino_dock_v1::OverviewMode::App),
                app_id: "org.example.editor".into(),
                x: 0,
                y: 24,
                width: 1000,
                height: 600,
            },
        );
        push(&mut group, Event::OverviewClosed);
        push(
            &mut group,
            Event::HotCorner {
                corner: WEnum::Value(Corner::BottomRight),
            },
        );
        push(
            &mut group,
            Event::Gesture {
                kind: WEnum::Value(Gesture::SwipeUp),
            },
        );
        push(
            &mut group,
            Event::Gesture {
                kind: WEnum::Unknown(77),
            },
        );
        assert_eq!(
            push(&mut group, Event::Done),
            Some(vec![
                DockChange::OverviewChanged {
                    mode: OverviewMode::App("org.example.editor".into()),
                    area: Rect::new(0.0, 24.0, 1000.0, 600.0),
                    windows: vec![
                        OverviewWindow {
                            id: 4,
                            frame: Rect::new(10.0, 20.0, 300.0, 200.0)
                        },
                        OverviewWindow {
                            id: 7,
                            frame: Rect::new(-5.0, 40.0, 100.0, 100.0)
                        }
                    ],
                    workspaces: vec![
                        OverviewWorkspace {
                            id: 1,
                            current: true
                        },
                        OverviewWorkspace {
                            id: 2,
                            current: false
                        }
                    ],
                },
                DockChange::OverviewClosed,
                DockChange::HotCorner(Corner::BottomRight),
                DockChange::Gesture(Gesture::SwipeUp),
            ])
        );
        push(
            &mut group,
            Event::OverviewChanged {
                mode: WEnum::Value(cupertino_dock_v1::OverviewMode::All),
                app_id: String::new(),
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
        );
        let changes = push(&mut group, Event::Done).unwrap();
        assert!(
            matches!(&changes[0], DockChange::OverviewChanged { mode: OverviewMode::All, windows, workspaces, .. } if windows.is_empty() && workspaces.is_empty()),
            "lists do not leak into the next group: {changes:?}"
        );
    }

    #[test]
    fn state_bits_map_to_flags() {
        let mut group = GroupBuilder::default();
        push(&mut group, window(1, WindowState::empty()));
        push(&mut group, window(2, WindowState::Active));
        let changes = push(&mut group, Event::Done).unwrap();
        let flags: Vec<(bool, bool)> = changes
            .iter()
            .map(|c| {
                if let DockChange::Window(w) = c {
                    (w.minimized, w.active)
                } else {
                    unreachable!()
                }
            })
            .collect();
        assert_eq!(flags, [(false, false), (false, true)]);
    }

    #[test]
    fn rectangles_round_outwards_and_must_be_finite() {
        assert_eq!(
            whole_rect(&Rect::new(10.4, 20.5, 40.2, 8.0)).unwrap(),
            (10, 20, 41, 9)
        );
        assert!(matches!(
            whole_rect(&Rect::new(f64::NAN, 0.0, 1.0, 1.0)),
            Err(Error::InvalidShape)
        ));
    }
}
