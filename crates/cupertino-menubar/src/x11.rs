//! The X server of the X11 bridge, asked for the titles of X11 windows that registered menus, the process each names as
//! its own and the window with the input focus ([`crate::registered::X11Windows`]). The connection is made the first time the active window is an
//! X11 program's, when the bridge runs, so the bar never starts the bridge itself; it is made again after it broke.
//! The display is `DISPLAY` (with `XAUTHORITY`, which the server gives the shell processes), else the one the bridge
//! was started for, from its command line.

use x11rb::connection::Connection as _;
use x11rb::errors::ReplyError;
use x11rb::protocol::xproto::{self, Atom, AtomEnum, ConnectionExt as _, Window};
use x11rb::protocol::ErrorKind;
use x11rb::rust_connection::RustConnection;

use crate::registered::{X11Title, X11Windows};

/// Most bytes of a title read.
const MAX_TITLE_WORDS: u32 = 1024;
/// Most levels a focused window is followed up to its top-level window.
const MAX_DEPTH: usize = 32;

struct Link {
    conn: RustConnection,
    root: Window,
    net_wm_name: Atom,
    utf8_string: Atom,
    active_window: Atom,
    net_wm_pid: Atom,
}

/// A lazily made connection to the bridge's X server.
#[derive(Default)]
pub struct XServer {
    link: Option<Link>,
}

impl XServer {
    fn link(&mut self, bridge: u32) -> Option<&Link> {
        if self.link.is_none() {
            self.link = connect(bridge);
        }
        self.link.as_ref()
    }

    /// Drop a connection that broke, so the next question makes a new one.
    fn broke(&mut self, e: &ReplyError) {
        if matches!(e, ReplyError::ConnectionError(_)) {
            self.link = None;
        }
    }
}

/// The display the bridge was started for.
fn bridge_display(bridge: u32) -> Option<String> {
    display_argument(&std::fs::read(format!("/proc/{bridge}/cmdline")).ok()?)
}

/// The first argument of a NUL-separated command line that names a display (`:1`, `:12.0`).
fn display_argument(cmdline: &[u8]) -> Option<String> {
    cmdline
        .split(|b| *b == 0)
        .filter_map(|a| std::str::from_utf8(a).ok())
        .find(|a| a.starts_with(':') && a[1..].chars().all(|c| c.is_ascii_digit() || c == '.'))
        .map(str::to_string)
}

fn connect(bridge: u32) -> Option<Link> {
    let display = std::env::var("DISPLAY")
        .ok()
        .filter(|d| !d.is_empty())
        .or_else(|| bridge_display(bridge))?;
    let (conn, screen) = RustConnection::connect(Some(&display)).ok()?;
    let root = conn.setup().roots.get(screen)?.root;
    let atom = |name: &[u8]| -> Option<Atom> {
        Some(conn.intern_atom(false, name).ok()?.reply().ok()?.atom)
    };
    let (net_wm_name, utf8_string, active_window, net_wm_pid) = (
        atom(b"_NET_WM_NAME")?,
        atom(b"UTF8_STRING")?,
        atom(b"_NET_ACTIVE_WINDOW")?,
        atom(b"_NET_WM_PID")?,
    );
    Some(Link {
        conn,
        root,
        net_wm_name,
        utf8_string,
        active_window,
        net_wm_pid,
    })
}

fn get_property(
    link: &Link,
    window: Window,
    property: Atom,
    kind: Atom,
) -> Result<xproto::GetPropertyReply, ReplyError> {
    link.conn
        .get_property(false, window, property, kind, 0, MAX_TITLE_WORDS)
        .map_err(ReplyError::from)?
        .reply()
}

fn title(link: &Link, window: Window) -> Result<String, ReplyError> {
    let utf8 = get_property(link, window, link.net_wm_name, link.utf8_string)?;
    if !utf8.value.is_empty() {
        return Ok(String::from_utf8_lossy(&utf8.value).into_owned());
    }
    // the old property holds Latin-1 text
    let plain = get_property(link, window, AtomEnum::WM_NAME.into(), AtomEnum::ANY.into())?;
    Ok(plain.value.iter().map(|b| char::from(*b)).collect())
}

/// The child of the root window that holds `window`.
fn top_level(link: &Link, mut window: Window) -> Result<Option<Window>, ReplyError> {
    for _ in 0..MAX_DEPTH {
        if window == x11rb::NONE
            || window == link.root
            || window == u32::from(xproto::InputFocus::POINTER_ROOT)
        {
            return Ok(None);
        }
        let tree = link
            .conn
            .query_tree(window)
            .map_err(ReplyError::from)?
            .reply()?;
        if tree.parent == link.root || tree.parent == x11rb::NONE {
            return Ok(Some(window));
        }
        window = tree.parent;
    }
    Ok(None)
}

fn focused(link: &Link) -> Result<Option<Window>, ReplyError> {
    let focus = link
        .conn
        .get_input_focus()
        .map_err(ReplyError::from)?
        .reply()?
        .focus;
    if let Some(window) = top_level(link, focus)? {
        return Ok(Some(window));
    }
    let active = get_property(link, link.root, link.active_window, AtomEnum::WINDOW.into())?;
    Ok(active
        .value32()
        .and_then(|mut v| v.next())
        .filter(|w| *w != x11rb::NONE))
}

impl X11Windows for XServer {
    fn title(&mut self, bridge: u32, window: u32) -> X11Title {
        let Some(link) = self.link(bridge) else {
            return X11Title::Unknown;
        };
        match title(link, window) {
            Ok(title) => X11Title::Title(title),
            Err(ReplyError::X11Error(e)) if e.error_kind == ErrorKind::Window => X11Title::Gone,
            Err(e) => {
                self.broke(&e);
                X11Title::Unknown
            }
        }
    }

    fn focused(&mut self, bridge: u32) -> Option<u32> {
        let link = self.link(bridge)?;
        match focused(link) {
            Ok(window) => window,
            Err(e) => {
                self.broke(&e);
                None
            }
        }
    }

    fn owner(&mut self, bridge: u32, window: u32) -> Option<u32> {
        let link = self.link(bridge)?;
        match get_property(link, window, link.net_wm_pid, AtomEnum::CARDINAL.into()) {
            Ok(reply) => reply
                .value32()
                .and_then(|mut v| v.next())
                .filter(|p| *p != 0),
            Err(e) => {
                self.broke(&e);
                None
            }
        }
    }
}
