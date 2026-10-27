//! Which globals the compositor offers: the ones the transport requires and the optional ones it uses when present.

use crate::Error;

/// A global the transport needs, with the lowest version it uses.
pub(crate) struct Requirement {
    pub interface: &'static str,
    pub minimum_version: u32,
}

/// The globals without which no window can be shown.
pub(crate) const REQUIRED: [Requirement; 3] = [
    Requirement {
        interface: "wl_compositor",
        minimum_version: 4,
    },
    Requirement {
        interface: "wl_shm",
        minimum_version: 1,
    },
    Requirement {
        interface: "xdg_wm_base",
        minimum_version: 1,
    },
];

/// An optional protocol the transport uses when the compositor offers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Capability {
    /// `wl_seat`: pointer and keyboard input.
    Seat,
    /// `wl_data_device_manager`: the clipboard.
    Clipboard,
    /// `wp_fractional_scale_manager_v1` together with `wp_viewporter`: buffers at a fractional scale.
    FractionalScale,
    /// `zxdg_decoration_manager_v1`: the compositor may draw the window frame. Without it the client draws it.
    ServerDecoration,
    /// `xdg_activation_v1`: asking for focus.
    Activation,
    /// `wp_cursor_shape_manager_v1`: named cursors drawn by the compositor.
    CursorShape,
}

impl Capability {
    pub const ALL: [Capability; 6] = [
        Capability::Seat,
        Capability::Clipboard,
        Capability::FractionalScale,
        Capability::ServerDecoration,
        Capability::Activation,
        Capability::CursorShape,
    ];

    /// The globals that must all be advertised for the capability.
    fn interfaces(self) -> &'static [&'static str] {
        match self {
            Capability::Seat => &["wl_seat"],
            Capability::Clipboard => &["wl_data_device_manager"],
            Capability::FractionalScale => &["wp_fractional_scale_manager_v1", "wp_viewporter"],
            Capability::ServerDecoration => &["zxdg_decoration_manager_v1"],
            Capability::Activation => &["xdg_activation_v1"],
            Capability::CursorShape => &["wp_cursor_shape_manager_v1"],
        }
    }

    fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// The optional protocols the compositor offers, read once at connect.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet(u8);

impl CapabilitySet {
    /// Check the required globals and collect the optional ones from the registry's `(interface, version)` list.
    pub(crate) fn from_globals<'a>(
        globals: impl IntoIterator<Item = (&'a str, u32)> + Clone,
    ) -> Result<CapabilitySet, Error> {
        let version = |name: &str| {
            globals
                .clone()
                .into_iter()
                .filter(|(interface, _)| *interface == name)
                .map(|(_, v)| v)
                .max()
        };
        for r in &REQUIRED {
            if version(r.interface).is_none_or(|v| v < r.minimum_version) {
                return Err(Error::MissingRequiredProtocol {
                    interface: r.interface,
                    minimum_version: r.minimum_version,
                });
            }
        }
        Ok(Capability::ALL
            .into_iter()
            .filter(|c| c.interfaces().iter().all(|i| version(i).is_some()))
            .fold(CapabilitySet::default(), |set, c| {
                CapabilitySet(set.0 | c.bit())
            }))
    }

    pub fn contains(self, capability: Capability) -> bool {
        self.0 & capability.bit() != 0
    }

    pub fn iter(self) -> impl Iterator<Item = Capability> {
        Capability::ALL
            .into_iter()
            .filter(move |c| self.contains(*c))
    }

    /// `Ok` when the compositor offers `capability`, else [`Error::Unsupported`].
    pub(crate) fn require(self, capability: Capability) -> Result<(), Error> {
        if self.contains(capability) {
            Ok(())
        } else {
            Err(Error::Unsupported(capability))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: [(&str, u32); 3] = [("wl_compositor", 6), ("wl_shm", 1), ("xdg_wm_base", 6)];

    #[test]
    fn the_three_core_globals_are_enough_to_connect() {
        let set = CapabilitySet::from_globals(MINIMAL).unwrap();
        assert_eq!(set.iter().count(), 0);
    }

    #[test]
    fn a_missing_or_old_required_global_names_itself() {
        let missing = CapabilitySet::from_globals([("wl_compositor", 6), ("wl_shm", 1)]);
        assert!(matches!(
            missing,
            Err(Error::MissingRequiredProtocol {
                interface: "xdg_wm_base",
                minimum_version: 1
            })
        ));
        let old =
            CapabilitySet::from_globals([("wl_compositor", 3), ("wl_shm", 1), ("xdg_wm_base", 6)]);
        assert!(matches!(
            old,
            Err(Error::MissingRequiredProtocol {
                interface: "wl_compositor",
                minimum_version: 4
            })
        ));
    }

    #[test]
    fn optional_globals_become_capabilities_and_fractional_scale_needs_the_viewporter() {
        let mut globals = MINIMAL.to_vec();
        globals.extend([
            ("wl_seat", 7),
            ("wp_fractional_scale_manager_v1", 1),
            ("zxdg_decoration_manager_v1", 1),
            ("zwlr_layer_shell_v1", 4),
        ]);
        let set = CapabilitySet::from_globals(globals.iter().copied()).unwrap();
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [Capability::Seat, Capability::ServerDecoration]
        );
        globals.push(("wp_viewporter", 1));
        let set = CapabilitySet::from_globals(globals.iter().copied()).unwrap();
        assert!(set.contains(Capability::FractionalScale));
        assert!(matches!(
            set.require(Capability::Clipboard),
            Err(Error::Unsupported(Capability::Clipboard))
        ));
    }
}
