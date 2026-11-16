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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn reg(service: &str, pid: u32, window: u32, serial: u64) -> Registration {
        Registration {
            service: service.into(),
            path: format!("/com/canonical/menu/{window:X}"),
            pid,
            window,
            serial,
            since: serial,
        }
    }

    #[derive(Default)]
    struct FakeX {
        titles: HashMap<u32, String>,
        owners: HashMap<u32, u32>,
        focus: Option<u32>,
    }

    impl X11Windows for FakeX {
        fn title(&mut self, _bridge: u32, window: u32) -> X11Title {
            self.titles
                .get(&window)
                .map_or(X11Title::Gone, |t| X11Title::Title(t.clone()))
        }

        fn focused(&mut self, _bridge: u32) -> Option<u32> {
            self.focus
        }

        fn owner(&mut self, _bridge: u32, window: u32) -> Option<u32> {
            self.owners.get(&window).copied()
        }
    }

    fn active<'a>(client: Client, title: &'a str, windows: &'a [(u32, bool)]) -> ActiveWindow<'a> {
        ActiveWindow {
            client,
            title,
            windows,
        }
    }

    fn x11(title: &str) -> ActiveWindow<'_> {
        active(Client::X11(7), title, &[])
    }

    #[test]
    fn an_x11_window_is_matched_by_its_title() {
        let regs = [reg(":1.5", 40, 0x400004, 1), reg(":1.9", 41, 0x600004, 2)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Notes".into());
        x.titles.insert(0x600004, "Chat".into());
        let seen = &mut SeenX11::new();
        let m = resolve(&regs, &x11("Chat"), &mut x, seen);
        assert_eq!(m.menu, Some(regs[1].menu()));
        assert!(m.gone.is_empty());
        let none = resolve(&regs, &x11("xterm"), &mut x, seen);
        assert_eq!(none.menu, None, "an X11 window that registered nothing");
    }

    #[test]
    fn x11_windows_of_one_title_go_by_the_input_focus() {
        let regs = [reg(":1.5", 40, 0x400004, 1), reg(":1.5", 40, 0x400010, 2)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Welcome".into());
        x.titles.insert(0x400010, "Welcome".into());
        x.focus = Some(0x400004);
        let seen = &mut SeenX11::new();
        let m = resolve(&regs, &x11("Welcome"), &mut x, seen);
        assert_eq!(m.menu, Some(regs[0].menu()));
        x.focus = None;
        let m = resolve(&regs, &x11("Welcome"), &mut x, seen);
        assert_eq!(m.menu, Some(regs[1].menu()), "else the latest");
    }

    #[test]
    fn an_x11_window_the_x_server_had_is_reported_gone_once_it_is_not() {
        let regs = [reg(":1.5", 40, 0x400004, 1), reg(":1.5", 40, 0x400020, 2)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Notes".into());
        x.titles.insert(0x400020, "Draft".into());
        let seen = &mut SeenX11::new();
        let m = resolve(&regs, &x11("Notes"), &mut x, seen);
        assert!(m.gone.is_empty());
        x.titles.remove(&0x400020);
        let m = resolve(&regs, &x11("Notes"), &mut x, seen);
        assert_eq!(m.menu, Some(regs[0].menu()));
        assert_eq!(m.gone, vec![(":1.5".to_string(), 0x400020)]);
        let unknown = resolve(&regs, &x11("Notes"), &mut NoX11, seen);
        assert_eq!(unknown, Matched::default(), "no X server: nothing is gone");
    }

    #[test]
    fn a_wayland_programs_number_is_never_gone_while_an_x11_window_is_active() {
        // a Wayland program registered its window 1, which the X server never had
        let regs = [reg(":1.17", 100, 1, 1)];
        let mut x = FakeX::default();
        x.titles.insert(0x600002, "Terminal".into());
        let seen = &mut SeenX11::new();
        for _ in 0..2 {
            let m = resolve(&regs, &x11("Terminal"), &mut x, seen);
            assert_eq!(m, Matched::default(), "neither gone nor the X11 window's");
        }
        let back = resolve(
            &regs,
            &active(Client::Wayland(100), "", &[(9, true)]),
            &mut x,
            seen,
        );
        assert_eq!(back.menu, Some(regs[0].menu()));
    }

    #[test]
    fn a_window_several_connections_registered_is_its_own_processs_else_the_firsts() {
        // the window's program registered first, another program the same window later
        let regs = [reg(":1.5", 40, 0x400004, 1), reg(":1.66", 66, 0x400004, 2)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Notes".into());
        let seen = &mut SeenX11::new();
        let first = resolve(&regs, &x11("Notes"), &mut x, seen);
        assert_eq!(
            first.menu,
            Some(regs[0].menu()),
            "no owner named: the first"
        );
        // the window names its process: that one, whichever came first
        let late_owner = [reg(":1.66", 66, 0x400004, 1), reg(":1.5", 40, 0x400004, 2)];
        x.owners.insert(0x400004, 40);
        let owned = resolve(&late_owner, &x11("Notes"), &mut x, seen);
        assert_eq!(owned.menu, Some(late_owner[1].menu()));
        x.owners.insert(0x400004, 99);
        let stranger = resolve(&late_owner, &x11("Notes"), &mut x, seen);
        assert_eq!(
            stranger.menu,
            Some(late_owner[0].menu()),
            "an owner that registered nothing: the first"
        );
        // registering again keeps a connection's place
        let mut again = reg(":1.5", 40, 0x400004, 3);
        again.since = 1;
        let regs = [reg(":1.66", 66, 0x400004, 2), again];
        x.owners.clear();
        let kept = resolve(&regs, &x11("Notes"), &mut x, seen);
        assert_eq!(kept.menu, Some(regs[1].menu()));
    }

    #[test]
    fn a_wayland_window_is_matched_by_its_process() {
        let regs = [reg(":1.17", 100, 1, 1), reg(":1.30", 200, 1, 2)];
        let seen = &mut SeenX11::new();
        let m = resolve(
            &regs,
            &active(Client::Wayland(200), "", &[(9, true)]),
            &mut NoX11,
            seen,
        );
        assert_eq!(m.menu, Some(regs[1].menu()));
        let other = resolve(
            &regs,
            &active(Client::Wayland(300), "", &[(9, true)]),
            &mut NoX11,
            seen,
        );
        assert_eq!(other.menu, None, "another process registered nothing");
        let unknown = resolve(
            &regs,
            &active(Client::Wayland(0), "", &[]),
            &mut NoX11,
            seen,
        );
        assert_eq!(unknown.menu, None, "an unknown process matches nobody");
    }

    #[test]
    fn several_wayland_windows_of_a_process_are_matched_in_order() {
        let regs = [
            reg(":1.17", 100, 5, 3),
            reg(":1.17", 100, 1, 1),
            reg(":1.17", 100, 3, 2),
        ];
        let windows = [(30, false), (12, false), (21, true)];
        let seen = &mut SeenX11::new();
        let m = resolve(
            &regs,
            &active(Client::Wayland(100), "", &windows),
            &mut NoX11,
            seen,
        );
        assert_eq!(
            m.menu,
            Some(regs[2].menu()),
            "the second window made, the second number"
        );
        let fewer = [(30, false), (21, true)];
        let m = resolve(
            &regs,
            &active(Client::Wayland(100), "", &fewer),
            &mut NoX11,
            seen,
        );
        assert_eq!(
            m.menu,
            Some(regs[0].menu()),
            "counts that differ: the latest"
        );
    }

    #[test]
    fn the_seen_windows_keep_only_registrations_that_stay() {
        let regs = [reg(":1.5", 40, 0x400004, 1)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Notes".into());
        let seen = &mut SeenX11::new();
        resolve(&regs, &x11("Notes"), &mut x, seen);
        assert_eq!(seen.len(), 1);
        resolve(&[], &x11("Notes"), &mut x, seen);
        assert!(seen.is_empty());
    }
}
