//! Colour value type and the names of the most used semantic colours.

/// sRGB colour with straight (non-premultiplied) alpha, components in `0.0..=1.0`.
///
/// The catalogs store 8-bit values whose colour space is assumed sRGB (storage, the colour space itself is ).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }

    pub fn from_rgba8(c: [u8; 4]) -> Self {
        Self::new(
            c[0] as f32 / 255.0,
            c[1] as f32 / 255.0,
            c[2] as f32 / 255.0,
            c[3] as f32 / 255.0,
        )
    }

    pub fn to_rgba8(self) -> [u8; 4] {
        let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
        [q(self.r), q(self.g), q(self.b), q(self.a)]
    }

    pub fn to_array(self) -> [f32; 4] {
        [self.r, self.g, self.b, self.a]
    }

    pub fn premultiplied(self) -> [f32; 4] {
        [self.r * self.a, self.g * self.a, self.b * self.a, self.a]
    }

    pub fn with_alpha(self, a: f32) -> Self {
        Self { a, ..self }
    }

    pub fn lerp(self, o: Color, t: f32) -> Color {
        let l = |a: f32, b: f32| a + (b - a) * t;
        Color::new(
            l(self.r, o.r),
            l(self.g, o.g),
            l(self.b, o.b),
            l(self.a, o.a),
        )
    }

    pub fn hex(self) -> String {
        let [r, g, b, _] = self.to_rgba8();
        format!("#{r:02X}{g:02X}{b:02X}")
    }
}

macro_rules! names {
    ($(#[$m:meta])* $name:ident { $($variant:ident => $s:literal),+ $(,)? }) => {
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($variant),+ }
        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$variant),+];
            /// Name of the colour in the catalogs.
            pub fn name(self) -> &'static str {
                match self { $($name::$variant => $s),+ }
            }
        }
    };
}

names! {
    /// Frequently used semantic colours present in both the light and the dark catalog (a typo-safe
    /// subset of the named colours; other names are reachable through the string API).
    SemanticColor {
        WindowBackground => "window-background",
        ControlBackground => "control-background",
        TextBackground => "text-background",
        UnderPageBackground => "under-page-background",
        Label => "label",
        SecondaryLabel => "secondary-label",
        TertiaryLabel => "tertiary-label",
        QuaternaryLabel => "quaternary-label",
        ControlText => "control-text",
        DisabledControlText => "disabled-control-text",
        PlaceholderText => "placeholder-text",
        Grid => "grid",
        ContainerBorder => "container-border",
        WindowRim => "window-rim",
        WindowRim2x => "window-rim-2x",
        WindowFrameText => "window-frame-text",
        KeyboardFocusIndicator => "keyboard-focus-indicator",
        Link => "link",
        Header => "header",
        SystemBlue => "system-blue",
        SystemGreen => "system-green",
        SystemIndigo => "system-indigo",
        SystemOrange => "system-orange",
        SystemPink => "system-pink",
        SystemPurple => "system-purple",
        SystemRed => "system-red",
        SystemTeal => "system-teal",
        SystemYellow => "system-yellow",
        SystemGray => "system-gray",
        SystemFill => "system-fill",
        SecondarySystemFill => "secondary-system-fill",
        TertiarySystemFill => "tertiary-system-fill",
        QuaternarySystemFill => "quaternary-system-fill",
        SelectedTextBackground => "selected-text-background",
    }
}

names! {
    /// User-selectable accent colours. The dynamic "current accent" colour is not stored by name
    /// (to be built from these sets at runtime), so the caller selects one explicitly.
    AccentColor {
        Blue => "control-accent-blue",
        Purple => "control-accent-purple",
        Pink => "control-accent-pink",
        Red => "control-accent-red",
        Orange => "control-accent-orange",
        Yellow => "control-accent-yellow",
        Green => "control-accent-green",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba8_roundtrip() {
        for v in [
            [0, 0, 0, 0],
            [255, 255, 255, 255],
            [236, 236, 236, 255],
            [0, 122, 255, 216],
        ] {
            assert_eq!(Color::from_rgba8(v).to_rgba8(), v);
        }
        assert_eq!(Color::from_rgba8([236, 236, 236, 255]).hex(), "#ECECEC");
    }

    #[test]
    fn premultiply() {
        let c = Color::new(1.0, 0.5, 0.0, 0.5);
        assert_eq!(c.premultiplied(), [0.5, 0.25, 0.0, 0.5]);
    }
}
