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
    /// `wp_single_pixel_buffer_manager_v1` together with `wp_viewporter`: a surface of one colour shown without drawing
    /// a buffer of its size ([`crate::Connection::present_solid`]).
    SolidColor,
    /// `zwp_linux_dmabuf_v1`: buffers drawn on a GPU ([`crate::Connection::create_dmabuf_buffer`]).
    Dmabuf,
    /// `wp_linux_drm_syncobj_manager_v1`: explicit synchronisation of dmabufs ([`crate::Connection::import_timeline`]).
    ExplicitSync,
}

impl Capability {
    pub const ALL: [Capability; 13] = [
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
        Capability::SolidColor,
        Capability::Dmabuf,
        Capability::ExplicitSync,
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
            Capability::SolidColor => &["wp_single_pixel_buffer_manager_v1", "wp_viewporter"],
            Capability::Dmabuf => &["zwp_linux_dmabuf_v1"],
            Capability::ExplicitSync => &["wp_linux_drm_syncobj_manager_v1"],
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
