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
    /// `zxdg_output_manager_v1`: where each output sits, in logical points ([`crate::OutputInfo::geometry`]).
    OutputLayout,
    /// `zwp_text_input_manager_v3` with a seat: input methods ([`crate::Connection::set_text_input`]).
    TextInput,
    /// `wp_presentation`: when each presented frame reached the screen ([`crate::Event::Presented`]).
    Presentation,
    /// `wl_subcompositor`: surfaces drawn as part of another one ([`crate::Connection::subsurface`]).
    Subsurface,
}

impl Capability {
    pub const ALL: [Capability; 10] = [
        Capability::Seat,
        Capability::Clipboard,
        Capability::FractionalScale,
        Capability::ServerDecoration,
        Capability::Activation,
        Capability::CursorShape,
        Capability::OutputLayout,
        Capability::TextInput,
        Capability::Presentation,
        Capability::Subsurface,
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
            Capability::OutputLayout => &["zxdg_output_manager_v1"],
            Capability::TextInput => &["zwp_text_input_manager_v3", "wl_seat"],
            Capability::Presentation => &["wp_presentation"],
            Capability::Subsurface => &["wl_subcompositor"],
        }
    }

    fn bit(self) -> u16 {
        1 << self as u8
    }
}

/// A protocol outside the standard ones that an extension bound through [`crate::Connection::bind_extension`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Extension {
    pub interface: &'static str,
    /// The version bound: the lower of the compositor's and the highest the extension supports.
    pub version: u32,
}

/// The optional protocols the compositor offers, read once at connect, and the extension protocols bound since.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CapabilitySet {
    standard: u16,
    extensions: Vec<Extension>,
}

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
            .fold(CapabilitySet::default(), |set, c| CapabilitySet {
                standard: set.standard | c.bit(),
                extensions: Vec::new(),
            }))
    }

    pub fn contains(&self, capability: Capability) -> bool {
        self.standard & capability.bit() != 0
    }

    pub fn iter(&self) -> impl Iterator<Item = Capability> + '_ {
        Capability::ALL
            .into_iter()
            .filter(move |c| self.contains(*c))
    }

    /// The extension protocols bound so far, in the order they were bound.
    pub fn extensions(&self) -> &[Extension] {
        &self.extensions
    }

    /// The version of `interface` an extension bound; `None` when none did, so the application falls back.
    pub fn extension(&self, interface: &str) -> Option<u32> {
        self.extensions
            .iter()
            .find(|e| e.interface == interface)
            .map(|e| e.version)
    }

    /// Record a bound extension protocol; binding the same interface again keeps one entry, at the newer version.
    pub(crate) fn add_extension(&mut self, extension: Extension) {
        match self
            .extensions
            .iter_mut()
            .find(|e| e.interface == extension.interface)
        {
            Some(e) => e.version = extension.version,
            None => self.extensions.push(extension),
        }
    }

    /// `Ok` when the compositor offers `capability`, else [`Error::Unsupported`].
    pub(crate) fn require(&self, capability: Capability) -> Result<(), Error> {
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

    #[test]
    fn a_bound_extension_is_listed_once_at_its_latest_version() {
        let mut set = CapabilitySet::from_globals(MINIMAL).unwrap();
        assert_eq!(set.extension("zwlr_layer_shell_v1"), None);
        let layer = |version| Extension {
            interface: "zwlr_layer_shell_v1",
            version,
        };
        set.add_extension(layer(3));
        set.add_extension(layer(4));
        assert_eq!(set.extensions(), [layer(4)]);
        assert_eq!(set.extension("zwlr_layer_shell_v1"), Some(4));
    }
}
