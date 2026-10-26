/// The four appearances the theme data provides complete catalogs for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Appearance {
    Light,
    Dark,
    LightHighContrast,
    DarkHighContrast,
}

impl Appearance {
    pub const ALL: [Appearance; 4] = [
        Appearance::Light,
        Appearance::Dark,
        Appearance::LightHighContrast,
        Appearance::DarkHighContrast,
    ];

    pub fn is_dark(self) -> bool {
        matches!(self, Appearance::Dark | Appearance::DarkHighContrast)
    }

    pub fn is_high_contrast(self) -> bool {
        matches!(
            self,
            Appearance::LightHighContrast | Appearance::DarkHighContrast
        )
    }

    /// The same appearance without the high-contrast catalog.
    pub fn base(self) -> Appearance {
        match self {
            Appearance::LightHighContrast => Appearance::Light,
            Appearance::DarkHighContrast => Appearance::Dark,
            a => a,
        }
    }

    /// The appearance of a session that is `dark` or light, in its high-contrast catalog while Increase Contrast is on
    /// (the design selects the `*AX` catalogs by the contrast bit).
    pub fn of_session(dark: bool, increase_contrast: bool) -> Appearance {
        let base = if dark {
            Appearance::Dark
        } else {
            Appearance::Light
        };
        base.with_high_contrast(increase_contrast)
    }

    pub fn with_high_contrast(self, on: bool) -> Appearance {
        match (self.is_dark(), on) {
            (false, false) => Appearance::Light,
            (true, false) => Appearance::Dark,
            (false, true) => Appearance::LightHighContrast,
            (true, true) => Appearance::DarkHighContrast,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Appearance::Light => "light",
            Appearance::Dark => "dark",
            Appearance::LightHighContrast => "light-high-contrast",
            Appearance::DarkHighContrast => "dark-high-contrast",
        }
    }
}

impl std::str::FromStr for Appearance {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Appearance::ALL
            .iter()
            .copied()
            .find(|a| a.as_str() == s)
            .ok_or_else(|| format!("unknown appearance {s:?}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base_and_high_contrast_roundtrip() {
        for a in Appearance::ALL {
            assert_eq!(a.base().with_high_contrast(a.is_high_contrast()), a);
            assert!(!a.base().is_high_contrast());
            assert_eq!(a.as_str().parse::<Appearance>().unwrap(), a);
            assert_eq!(Appearance::of_session(a.is_dark(), a.is_high_contrast()), a);
        }
    }
}
