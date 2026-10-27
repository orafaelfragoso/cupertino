//! Window configures: what the compositor asks of a window, and which buffer may answer which configure.

use crate::Error;
use wayland_protocols::xdg::shell::client::xdg_toplevel;

/// Who draws the window frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Decorations {
    /// The compositor draws it.
    Server,
    /// The client draws its own title bar and border: the compositor offers no decoration protocol or asked for
    /// client-side ones.
    #[default]
    Client,
}

/// The `xdg_toplevel` states of a configure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct WindowStates {
    pub maximized: bool,
    pub fullscreen: bool,
    pub resizing: bool,
    pub activated: bool,
    pub tiled_left: bool,
    pub tiled_right: bool,
    pub tiled_top: bool,
    pub tiled_bottom: bool,
    /// The window is not visible; the client may stop drawing.
    pub suspended: bool,
}

impl WindowStates {
    /// The states of the array an `xdg_toplevel.configure` carries (native-endian `u32` values). Unknown values are
    /// skipped: later protocol versions add states.
    pub(crate) fn from_wire(bytes: &[u8]) -> WindowStates {
        let (words, _) = bytes.as_chunks::<4>();
        words.iter().map(|w| u32::from_ne_bytes(*w)).fold(
            WindowStates::default(),
            |mut s, value| {
                match xdg_toplevel::State::try_from(value) {
                    Ok(xdg_toplevel::State::Maximized) => s.maximized = true,
                    Ok(xdg_toplevel::State::Fullscreen) => s.fullscreen = true,
                    Ok(xdg_toplevel::State::Resizing) => s.resizing = true,
                    Ok(xdg_toplevel::State::Activated) => s.activated = true,
                    Ok(xdg_toplevel::State::TiledLeft) => s.tiled_left = true,
                    Ok(xdg_toplevel::State::TiledRight) => s.tiled_right = true,
                    Ok(xdg_toplevel::State::TiledTop) => s.tiled_top = true,
                    Ok(xdg_toplevel::State::TiledBottom) => s.tiled_bottom = true,
                    Ok(xdg_toplevel::State::Suspended) => s.suspended = true,
                    _ => {}
                }
                s
            },
        )
    }

    /// Whether the compositor fixed the size, so the buffer must match it exactly.
    pub fn size_is_fixed(self) -> bool {
        self.maximized
            || self.fullscreen
            || self.resizing
            || self.tiled_left
            || self.tiled_right
            || self.tiled_top
            || self.tiled_bottom
    }
}

/// A complete configure of an application window. Present a buffer drawn for it to acknowledge it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowConfigure {
    pub serial: u32,
    /// The size the compositor asks for, in logical points; `None` leaves it to the client.
    pub size: Option<(u32, u32)>,
    /// The largest size that fits the work area (`configure_bounds`), when the compositor said.
    pub bounds: Option<(u32, u32)>,
    pub states: WindowStates,
    pub decorations: Decorations,
}

/// A width and height from the wire: `None` when either is zero (the client chooses), negative extents rejected.
pub(crate) fn wire_size(width: i32, height: i32) -> Result<Option<(u32, u32)>, Error> {
    match (u32::try_from(width), u32::try_from(height)) {
        (Ok(0), _) | (_, Ok(0)) => Ok(None),
        (Ok(w), Ok(h)) => Ok(Some((w, h))),
        _ => Err(Error::Protocol("negative configure size")),
    }
}

/// Whether serial `a` is newer than `b`, with wrap-around.
fn newer(a: u32, b: u32) -> bool {
    (a.wrapping_sub(b) as i32) > 0
}

/// The configure serials of one surface: the latest the compositor sent and the latest acknowledged.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Serials {
    latest: Option<u32>,
    acked: Option<u32>,
}

impl Serials {
    /// A configure arrived; serials only go forward.
    pub(crate) fn receive(&mut self, serial: u32) -> Result<(), Error> {
        if let Some(latest) = self.latest.filter(|l| !newer(serial, *l)) {
            return Err(Error::InvalidSerial { serial, latest });
        }
        self.latest = Some(serial);
        Ok(())
    }

    pub(crate) fn latest(self) -> Option<u32> {
        self.latest
    }

    /// A buffer for the latest configure is about to be committed: that serial when it is not acknowledged yet.
    pub(crate) fn present(&mut self) -> Result<Option<u32>, Error> {
        let latest = self.latest.ok_or(Error::NotConfigured)?;
        Ok((self.acked != Some(latest)).then(|| {
            self.acked = Some(latest);
            latest
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(states: &[xdg_toplevel::State]) -> Vec<u8> {
        states
            .iter()
            .flat_map(|s| (*s as u32).to_ne_bytes())
            .chain(99u32.to_ne_bytes())
            .collect()
    }

    #[test]
    fn states_are_read_from_the_wire_and_unknown_ones_skipped() {
        let s = WindowStates::from_wire(&wire(&[
            xdg_toplevel::State::Activated,
            xdg_toplevel::State::TiledLeft,
        ]));
        assert_eq!(
            s,
            WindowStates {
                activated: true,
                tiled_left: true,
                ..WindowStates::default()
            }
        );
        assert!(s.size_is_fixed());
        assert!(!WindowStates::from_wire(&wire(&[xdg_toplevel::State::Activated])).size_is_fixed());
    }

    #[test]
    fn a_zero_extent_leaves_the_size_to_the_client() {
        assert_eq!(wire_size(0, 480).unwrap(), None);
        assert_eq!(wire_size(640, 480).unwrap(), Some((640, 480)));
        assert!(wire_size(-1, 480).is_err());
    }

    #[test]
    fn the_first_buffer_for_a_configure_acknowledges_it_once() {
        let mut serials = Serials::default();
        assert!(matches!(serials.present(), Err(Error::NotConfigured)));
        serials.receive(5).unwrap();
        assert_eq!(serials.present().unwrap(), Some(5));
        assert_eq!(serials.present().unwrap(), None);
    }

    #[test]
    fn only_the_latest_of_several_configures_is_acknowledged() {
        let mut serials = Serials::default();
        serials.receive(5).unwrap();
        serials.receive(6).unwrap();
        assert_eq!(serials.present().unwrap(), Some(6));
    }

    #[test]
    fn serials_only_go_forward_across_the_wrap() {
        let mut serials = Serials::default();
        serials.receive(u32::MAX).unwrap();
        serials.receive(1).unwrap();
        assert!(matches!(
            serials.receive(1),
            Err(Error::InvalidSerial {
                serial: 1,
                latest: 1
            })
        ));
        assert!(serials.receive(u32::MAX).is_err());
    }
}
