//! The modifiers of key and pointer input.

/// The logical modifiers held while a key event happened.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    /// The Super key.
    pub logo: bool,
}

impl From<cupertino_wayland_client::Modifiers> for Modifiers {
    fn from(m: cupertino_wayland_client::Modifiers) -> Self {
        Modifiers {
            ctrl: m.ctrl,
            alt: m.alt,
            shift: m.shift,
            logo: m.logo,
        }
    }
}
