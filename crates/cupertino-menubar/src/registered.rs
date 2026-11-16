//! Menus a window registers on the session bus (`com.canonical.AppMenu.Registrar`) instead of announcing them over
//! Wayland, and which of them belongs to the active window.
//!
//! A registration names its window by a number of the program's own: an X11 program gives its X11 window, a program on
//! Wayland a number the server never sees. So the active window is matched to a registration by what the server tells
//! the bar about the process that shows it ([`Client`]):
//! * an X11 program's window, which the X11 bridge shows: among the registered X11 windows, the one whose title is the
//!   active window's (the bridge gives each window the title of its X11 window); among several with that title, the one
//!   the X server has the input focus on. A registered window the X server no longer has is reported gone.
//! * a window on Wayland: the registrations of its own process. One is the window's; several are matched in the order
//!   the windows were made, when the process registered one per window of the application, else the latest stands
//!   for all of them.

use crate::source::KdeAppMenu;

/// One registered menu.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Registration {
    /// Unique bus name of the connection that registered; the menu is read from it.
    pub service: String,
    /// Object path of the `com.canonical.dbusmenu` tree.
    pub path: String,
    /// The process of that connection, `0` when the bus did not say.
    pub pid: u32,
    /// The number the program gave the window.
    pub window: u32,
    /// Larger for a later registration.
    pub serial: u64,
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

/// The registration of the active window, if any.
pub fn resolve(
    registrations: &[Registration],
    active: &ActiveWindow<'_>,
    x11: &mut dyn X11Windows,
) -> Matched {
    match active.client {
        Client::Wayland(0) => Matched::default(),
        Client::Wayland(pid) => Matched {
            menu: own_process(registrations, pid, active.windows).map(Registration::menu),
            gone: Vec::new(),
        },
        Client::X11(bridge) => x11_window(registrations, bridge, active.title, x11),
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
fn x11_window(
    registrations: &[Registration],
    bridge: u32,
    title: &str,
    x11: &mut dyn X11Windows,
) -> Matched {
    // a window registered again counts once, by its latest registration
    let mut latest: Vec<&Registration> = Vec::new();
    for r in registrations {
        match latest.iter_mut().find(|l| l.window == r.window) {
            Some(l) if l.serial < r.serial => *l = r,
            Some(_) => {}
            None => latest.push(r),
        }
    }
    let mut gone = Vec::new();
    let mut titled = Vec::new();
    for r in latest {
        match x11.title(bridge, r.window) {
            X11Title::Title(t) if t == title => titled.push(r),
            X11Title::Title(_) | X11Title::Unknown => {}
            X11Title::Gone => gone.extend(
                registrations
                    .iter()
                    .filter(|g| g.window == r.window)
                    .map(Registration::key),
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
        }
    }

    #[derive(Default)]
    struct FakeX {
        titles: HashMap<u32, String>,
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
    }

    fn active<'a>(client: Client, title: &'a str, windows: &'a [(u32, bool)]) -> ActiveWindow<'a> {
        ActiveWindow {
            client,
            title,
            windows,
        }
    }

    #[test]
    fn an_x11_window_is_matched_by_its_title() {
        let regs = [reg(":1.5", 40, 0x400004, 1), reg(":1.9", 41, 0x600004, 2)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Notes".into());
        x.titles.insert(0x600004, "Chat".into());
        let m = resolve(&regs, &active(Client::X11(7), "Chat", &[]), &mut x);
        assert_eq!(m.menu, Some(regs[1].menu()));
        assert!(m.gone.is_empty());
        let none = resolve(&regs, &active(Client::X11(7), "xterm", &[]), &mut x);
        assert_eq!(none.menu, None, "an X11 window that registered nothing");
    }

    #[test]
    fn x11_windows_of_one_title_go_by_the_input_focus() {
        let regs = [reg(":1.5", 40, 0x400004, 1), reg(":1.5", 40, 0x400010, 2)];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Welcome".into());
        x.titles.insert(0x400010, "Welcome".into());
        x.focus = Some(0x400004);
        let m = resolve(&regs, &active(Client::X11(7), "Welcome", &[]), &mut x);
        assert_eq!(m.menu, Some(regs[0].menu()));
        x.focus = None;
        let m = resolve(&regs, &active(Client::X11(7), "Welcome", &[]), &mut x);
        assert_eq!(m.menu, Some(regs[1].menu()), "else the latest");
    }

    #[test]
    fn a_gone_x11_window_is_reported_and_a_window_registered_again_counts_once() {
        let regs = [
            reg(":1.5", 40, 0x400004, 1),
            reg(":1.5", 40, 0x400004, 3),
            reg(":1.5", 40, 0x400020, 2),
        ];
        let mut x = FakeX::default();
        x.titles.insert(0x400004, "Notes".into());
        let m = resolve(&regs, &active(Client::X11(7), "Notes", &[]), &mut x);
        assert_eq!(m.menu, Some(regs[1].menu()));
        assert_eq!(m.gone, vec![(":1.5".to_string(), 0x400020)]);
        let unknown = resolve(&regs, &active(Client::X11(7), "Notes", &[]), &mut NoX11);
        assert_eq!(unknown, Matched::default(), "no X server: nothing is gone");
    }

    #[test]
    fn a_wayland_window_is_matched_by_its_process() {
        let regs = [reg(":1.17", 100, 1, 1), reg(":1.30", 200, 1, 2)];
        let m = resolve(
            &regs,
            &active(Client::Wayland(200), "", &[(9, true)]),
            &mut NoX11,
        );
        assert_eq!(m.menu, Some(regs[1].menu()));
        let other = resolve(
            &regs,
            &active(Client::Wayland(300), "", &[(9, true)]),
            &mut NoX11,
        );
        assert_eq!(other.menu, None, "another process registered nothing");
        let unknown = resolve(&regs, &active(Client::Wayland(0), "", &[]), &mut NoX11);
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
        let m = resolve(
            &regs,
            &active(Client::Wayland(100), "", &windows),
            &mut NoX11,
        );
        assert_eq!(
            m.menu,
            Some(regs[2].menu()),
            "the second window made, the second number"
        );
        let fewer = [(30, false), (21, true)];
        let m = resolve(&regs, &active(Client::Wayland(100), "", &fewer), &mut NoX11);
        assert_eq!(
            m.menu,
            Some(regs[0].menu()),
            "counts that differ: the latest"
        );
    }
}
