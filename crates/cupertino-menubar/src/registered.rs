//! Menus a window registers on the session bus (`com.canonical.AppMenu.Registrar`) instead of announcing them over
//! Wayland, and which of them belongs to the active window.
//!
//! A registration names its window by a number of the program's own: an X11 program gives its X11 window, a program on
//! Wayland a number the server never sees. So the active window is matched to a registration by what the server tells
//! the bar about the process that shows it ([`Client`]):
//! * an X11 program's window, which the X11 bridge shows: among the registered X11 windows, the one whose title is the
//!   active window's (the bridge gives each window the title of its X11 window); among several with that title, the one
//!   the X server has the input focus on. A window several connections registered is the one whose process the window
//!   names as its own (`_NET_WM_PID`), else the connection that registered it first, so another program cannot take
//!   over its menus by registering it later. A registered window the X server once had and no longer has is reported
//!   gone; a number the X server never had (a Wayland program's) is never gone.
//! * a window on Wayland: the registrations of its own process. One is the window's; several are matched in the order
//!   the windows were made, when the process registered one per window of the application, else the latest stands
//!   for all of them.

use std::collections::HashSet;

use crate::source::KdeAppMenu;

/// One registered menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    /// Unique bus name of the connection that registered; the menu is read from it.
    pub service: String,
    /// Object path of the `com.canonical.dbusmenu` tree.
    pub path: String,
    /// The process of that connection.
    pub pid: u32,
    /// The number the program gave the window.
    pub window: u32,
    /// Larger for a later registration.
    pub serial: u64,
    /// The serial of the connection's first registration of this window, kept when it registers the window again.
    pub since: u64,
}

impl Registration {
    /// The menu's address.
    pub fn menu(&self) -> KdeAppMenu {
        KdeAppMenu {
            service: self.service.clone(),
            path: self.path.clone(),
        }
    }

    /// What names the registration: its connection and its window.
    pub fn key(&self) -> (String, u32) {
        (self.service.clone(), self.window)
    }
}

/// The registration that stands for a window several connections registered: the one of the process the window names
/// as its own (`owner`), else the first made. `None` for no registrations.
pub fn preferred<'a>(
    registrations: impl IntoIterator<Item = &'a Registration>,
    owner: Option<u32>,
) -> Option<&'a Registration> {
    let mut first: Option<&Registration> = None;
    for r in registrations {
        if owner.is_some_and(|pid| pid != 0 && pid == r.pid) {
            return Some(r);
        }
        if first.is_none_or(|f| r.since < f.since) {
            first = Some(r);
        }
    }
    first
}

/// The process that shows the active window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Client {
    /// The window's own process.
    Wayland(u32),
    /// The X11 bridge, with its process id.
    X11(u32),
}

/// What the X server says of a window's title.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum X11Title {
    Title(String),
    /// The X server has no such window.
    Gone,
    /// The X server could not be asked.
    Unknown,
}

/// The X server of the X11 bridge.
pub trait X11Windows {
    /// The title of window `window`.
    fn title(&mut self, bridge: u32, window: u32) -> X11Title;
    /// The top-level window that has the input focus.
    fn focused(&mut self, bridge: u32) -> Option<u32>;
    /// The process window `window` names as its own (`_NET_WM_PID`), when it names one.
    fn owner(&mut self, bridge: u32, window: u32) -> Option<u32>;
}

/// No X server: every X11 window is unknown.
pub struct NoX11;

impl X11Windows for NoX11 {
    fn title(&mut self, _bridge: u32, _window: u32) -> X11Title {
        X11Title::Unknown
    }

    fn focused(&mut self, _bridge: u32) -> Option<u32> {
        None
    }

    fn owner(&mut self, _bridge: u32, _window: u32) -> Option<u32> {
        None
    }
}

/// The active window as the bar knows it.
#[derive(Clone, Copy, Debug)]
pub struct ActiveWindow<'a> {
    pub client: Client,
    pub title: &'a str,
    /// The server's ids of the active application's windows, with the active one marked.
    pub windows: &'a [(u32, bool)],
}

/// The match for the active window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Matched {
    pub menu: Option<KdeAppMenu>,
    /// Registrations whose X11 window is gone, by connection and window.
    pub gone: Vec<(String, u32)>,
}

/// Registrations, by connection and window, whose window the X server has had: only those can be gone.
pub type SeenX11 = HashSet<(String, u32)>;

/// The registration of the active window, if any. `seen` keeps, from one call to the next, the registrations whose
/// window the X server has had.
pub fn resolve(
    registrations: &[Registration],
    active: &ActiveWindow<'_>,
    x11: &mut dyn X11Windows,
    seen: &mut SeenX11,
) -> Matched {
    // a registration that went is forgotten, so the set holds no more than the list
    seen.retain(|(service, window)| {
        registrations
            .iter()
            .any(|r| r.window == *window && r.service == *service)
    });
    match active.client {
        Client::Wayland(0) => Matched::default(),
        Client::Wayland(pid) => Matched {
            menu: own_process(registrations, pid, active.windows).map(Registration::menu),
            gone: Vec::new(),
        },
        Client::X11(bridge) => x11_window(registrations, bridge, active.title, x11, seen),
    }
}

/// The registration of the active window among those of its own process `pid`.
fn own_process<'a>(
    registrations: &'a [Registration],
    pid: u32,
    windows: &[(u32, bool)],
) -> Option<&'a Registration> {
    let mut own: Vec<&Registration> = registrations.iter().filter(|r| r.pid == pid).collect();
    match own.len() {
        0 => return None,
        1 => return Some(own[0]),
        _ => {}
    }
    // the server's ids and the program's numbers both grow as windows are made
    own.sort_by_key(|r| r.window);
    let mut ids: Vec<(u32, bool)> = windows.to_vec();
    ids.sort_by_key(|(id, _)| *id);
    if ids.len() == own.len() {
        if let Some(rank) = ids.iter().position(|(_, active)| *active) {
            return Some(own[rank]);
        }
    }
    own.into_iter().max_by_key(|r| r.serial)
}

/// The registration of the X11 window titled `title`.
fn x11_window<'a>(
    registrations: &'a [Registration],
    bridge: u32,
    title: &str,
    x11: &mut dyn X11Windows,
    seen: &mut SeenX11,
) -> Matched {
    // a window registered several times is asked about once
    let mut windows: Vec<u32> = registrations.iter().map(|r| r.window).collect();
    windows.sort_unstable();
    windows.dedup();
    let of = |window: u32| registrations.iter().filter(move |r| r.window == window);
    let mut gone = Vec::new();
    let mut titled: Vec<&'a Registration> = Vec::new();
    for window in windows {
        match x11.title(bridge, window) {
            X11Title::Title(t) => {
                seen.extend(of(window).map(Registration::key));
                if t == title {
                    let owner = x11.owner(bridge, window);
                    titled.extend(preferred(of(window), owner));
                }
            }
            X11Title::Unknown => {}
            // only a window the X server had: a number it never had is a Wayland program's, not an X11 window
            X11Title::Gone => gone.extend(
                of(window)
                    .map(Registration::key)
                    .filter(|key| seen.remove(key)),
            ),
        }
    }
    let chosen = match titled.len() {
        0 => None,
        1 => Some(titled[0]),
        _ => {
            let focused = x11.focused(bridge);
            titled
                .iter()
                .find(|r| Some(r.window) == focused)
                .or_else(|| titled.iter().max_by_key(|r| r.serial))
                .copied()
        }
    };
    Matched {
        menu: chosen.map(Registration::menu),
        gone,
    }
}
