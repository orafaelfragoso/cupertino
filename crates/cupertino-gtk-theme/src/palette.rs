//! Colours of a generated theme, resolved from the design tokens.
//!
//! Token sources (the catalogs are as designed storage, their sRGB interpretation
//! is): window/view/text/fill/grid/rim/focus colours and the accent set are read by name.
//! Mappings from tokens to toolkit roles are authored (approximate) and documented per field.
//! Colours keep their alpha; [`over`] composites them for toolkits that need opaque values.

use cupertino_theme::recipe::{MaterialOptions, WindowState};
use cupertino_theme::{Appearance, Color, SemanticColor, Theme};

/// User-selectable accent colours (the seven accent sets of the catalogs plus the neutral one).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Accent {
    Blue,
    Purple,
    Pink,
    Red,
    Orange,
    Yellow,
    Green,
    Graphite,
}

impl Accent {
    pub const ALL: [Accent; 8] = [Accent::Blue, Accent::Purple, Accent::Pink, Accent::Red, Accent::Orange, Accent::Yellow, Accent::Green, Accent::Graphite];

    pub fn name(self) -> &'static str {
        match self {
            Accent::Blue => "blue",
            Accent::Purple => "purple",
            Accent::Pink => "pink",
            Accent::Red => "red",
            Accent::Orange => "orange",
            Accent::Yellow => "yellow",
            Accent::Green => "green",
            Accent::Graphite => "graphite",
        }
    }

    /// Name of the colour in the token catalogs.
    pub fn token(self) -> &'static str {
        match self {
            Accent::Blue => "control-accent-blue",
            Accent::Purple => "control-accent-purple",
            Accent::Pink => "control-accent-pink",
            Accent::Red => "control-accent-red",
            Accent::Orange => "control-accent-orange",
            Accent::Yellow => "control-accent-yellow",
            Accent::Green => "control-accent-green",
            Accent::Graphite => "control-accent-no",
        }
    }
}

impl std::str::FromStr for Accent {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Accent::ALL.iter().copied().find(|a| a.name() == s.to_ascii_lowercase()).ok_or_else(|| format!("unknown accent {s:?} (expected one of: {})", Accent::ALL.map(|a| a.name()).join(", ")))
    }
}

/// Source-over of `top` on an opaque `bottom` (result is opaque).
pub fn over(top: Color, bottom: Color) -> Color {
    let a = top.a;
    Color::new(top.r * a + bottom.r * (1.0 - a), top.g * a + bottom.g * (1.0 - a), top.b * a + bottom.b * (1.0 - a), 1.0)
}

/// `#rrggbb` for opaque colours, `rgba(r, g, b, a)` otherwise (CSS and the generated files).
pub fn css(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8();
    if a == 255 {
        format!("#{r:02x}{g:02x}{b:02x}")
    } else {
        format!("rgba({r}, {g}, {b}, {})", trim_float(c.a as f64, 3))
    }
}

pub fn trim_float(v: f64, decimals: usize) -> String {
    let s = format!("{v:.decimals$}");
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Palette {
    pub appearance: Appearance,
    pub accent_name: Accent,
    pub dark: bool,

    pub window_bg: Color,
    /// (alpha kept).
    pub window_fg: Color,
    /// lists, text views, cards.
    pub view_bg: Color,
    pub view_fg: Color,
    /// Opaque fill of the `Titlebar` material recipe (active), as the window decorations use it.
    pub headerbar_bg: Color,
    /// Same recipe, inactive window.
    pub headerbar_backdrop: Color,
    pub headerbar_fg: Color,
    pub separator: Color,
    pub border: Color,
    /// Opaque `Sidebar` material fill, falling back to the window background.
    pub sidebar_bg: Color,
    /// Opaque `Menu` material fill (popovers and menus), falling back to the view background.
    pub popover_bg: Color,
    /// /.
    pub tooltip_bg: Color,
    pub tooltip_fg: Color,

    /// The chosen `controlAccent*Color`.
    pub accent: Color,
    /// White (where the catalog has it).
    pub accent_fg: Color,
    /// Selected content: the accent colour.
    pub selection_bg: Color,
    pub selection_fg: Color,
    /// Selection of an unfocused window:.
    pub selection_inactive_bg: Color,
    pub selection_inactive_fg: Color,

    pub destructive: Color,
    pub success: Color,
    pub warning: Color,
    pub error: Color,
    /// visited links use the system purple (approximate).
    pub link: Color,
    pub link_visited: Color,

    pub secondary_fg: Color,
    pub tertiary_fg: Color,
    pub disabled_fg: Color,
    pub placeholder: Color,
    pub fill: Color,
    pub fill_secondary: Color,
    pub fill_tertiary: Color,
    pub focus_ring: Color,
    pub shadow: Color,
    /// the face of push buttons.
    pub control: Color,
    pub under_page: Color,
    /// -like zebra:.
    pub zebra: Color,
}

impl Palette {
    pub fn from_theme(theme: &Theme, dark: bool, high_contrast: bool, accent: Accent) -> Palette {
        let appearance = if dark { Appearance::Dark } else { Appearance::Light }.with_high_contrast(high_contrast);
        let t = &theme.tokens;
        let sem = |c: SemanticColor| t.semantic(appearance, c);
        let named = |n: &str| t.color(appearance, n);
        let fallback_text = if dark { Color::new(1.0, 1.0, 1.0, 0.85) } else { Color::new(0.0, 0.0, 0.0, 0.85) };
        let window_bg = sem(SemanticColor::WindowBackground).unwrap_or(if dark { Color::new(0.196, 0.196, 0.196, 1.0) } else { Color::new(0.925, 0.925, 0.925, 1.0) });
        let view_bg = sem(SemanticColor::ControlBackground).unwrap_or(if dark { Color::new(0.118, 0.118, 0.118, 1.0) } else { Color::new(1.0, 1.0, 1.0, 1.0) });
        let fill_of = |material: &str, window: WindowState| {
            let opts = MaterialOptions { window, reduce_transparency: true, ..Default::default() };
            theme.recipe(appearance, material, &opts).ok().and_then(|r| r.fills.last().map(|f| Color::new(f.rgba[0], f.rgba[1], f.rgba[2], 1.0)))
        };
        let white = Color::new(1.0, 1.0, 1.0, 1.0);
        let text = sem(SemanticColor::Label).unwrap_or(fallback_text);
        let accent_color = named(accent.token()).or_else(|| sem(SemanticColor::SystemBlue)).unwrap_or(Color::new(0.0, 0.48, 1.0, 1.0));
        let purple = sem(SemanticColor::SystemPurple).unwrap_or(Color::new(0.69, 0.32, 0.87, 1.0));
        let red = sem(SemanticColor::SystemRed).unwrap_or(Color::new(1.0, 0.23, 0.19, 1.0));
        Palette {
            appearance,
            accent_name: accent,
            dark,
            window_bg,
            window_fg: text,
            view_bg,
            view_fg: named("text").unwrap_or(if dark { white } else { Color::new(0.0, 0.0, 0.0, 1.0) }),
            headerbar_bg: fill_of("Titlebar", WindowState::Active).unwrap_or(window_bg),
            headerbar_backdrop: fill_of("Titlebar", WindowState::Inactive).unwrap_or(window_bg),
            headerbar_fg: sem(SemanticColor::WindowFrameText).unwrap_or(text),
            separator: sem(SemanticColor::Grid).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.3)),
            border: sem(SemanticColor::ContainerBorder).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.25)),
            sidebar_bg: fill_of("Sidebar", WindowState::Active).unwrap_or(window_bg),
            popover_bg: fill_of("Menu", WindowState::Active).unwrap_or(view_bg),
            tooltip_bg: named("tool-tip").map(|c| over(c, window_bg)).unwrap_or(view_bg),
            tooltip_fg: named("tool-tip-text").unwrap_or(text),
            accent: accent_color,
            accent_fg: named("alternate-selected-control-text").unwrap_or(white),
            selection_bg: accent_color,
            selection_fg: named("alternate-selected-control-text").unwrap_or(white),
            selection_inactive_bg: named("secondary-selected-control").unwrap_or(Color::new(0.86, 0.86, 0.86, 1.0)),
            selection_inactive_fg: text,
            destructive: red,
            success: sem(SemanticColor::SystemGreen).unwrap_or(Color::new(0.16, 0.8, 0.25, 1.0)),
            warning: sem(SemanticColor::SystemOrange).unwrap_or(Color::new(1.0, 0.58, 0.0, 1.0)),
            error: red,
            link: sem(SemanticColor::Link).unwrap_or(accent_color),
            link_visited: purple,
            secondary_fg: sem(SemanticColor::SecondaryLabel).unwrap_or(fallback_text),
            tertiary_fg: sem(SemanticColor::TertiaryLabel).unwrap_or(fallback_text),
            disabled_fg: sem(SemanticColor::DisabledControlText).unwrap_or(fallback_text),
            placeholder: sem(SemanticColor::PlaceholderText).unwrap_or(fallback_text),
            fill: sem(SemanticColor::SystemFill).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.2)),
            fill_secondary: sem(SemanticColor::SecondarySystemFill).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.16)),
            fill_tertiary: sem(SemanticColor::TertiarySystemFill).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.12)),
            focus_ring: sem(SemanticColor::KeyboardFocusIndicator).unwrap_or(accent_color),
            shadow: named("control-shadow").unwrap_or(Color::new(0.0, 0.0, 0.0, 0.27)),
            control: named("control").unwrap_or(white),
            under_page: sem(SemanticColor::UnderPageBackground).unwrap_or(window_bg),
            zebra: sem(SemanticColor::QuaternarySystemFill).unwrap_or(Color::new(0.5, 0.5, 0.5, 0.03)),
        }
    }

    /// Opaque version of `c` over this palette's window background.
    pub fn solid(&self, c: Color) -> Color {
        over(c, self.window_bg)
    }

    /// Opaque version of `c` over the view background.
    pub fn solid_on_view(&self, c: Color) -> Color {
        over(c, self.view_bg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme() -> Theme {
        Theme::load().unwrap()
    }

    #[test]
    fn light_and_dark_use_the_matching_catalog() {
        let t = theme();
        let l = Palette::from_theme(&t, false, false, Accent::Blue);
        let d = Palette::from_theme(&t, true, false, Accent::Blue);
        assert_eq!(l.appearance, Appearance::Light);
        assert_eq!(d.appearance, Appearance::Dark);
        assert_eq!(l.window_bg.hex(), "#ECECEC");
        assert_eq!(d.window_bg.hex(), "#323232");
        assert_eq!(l.view_bg.hex(), "#FFFFFF");
        assert_eq!(d.view_bg.hex(), "#1E1E1E");
        assert_eq!(l.link.hex(), "#0068DA");
        assert_eq!(d.link.hex(), "#419CFF");
        assert!(d.dark && !l.dark);
    }

    #[test]
    fn high_contrast_selects_the_high_contrast_catalog() {
        let t = theme();
        assert_eq!(Palette::from_theme(&t, false, true, Accent::Blue).appearance, Appearance::LightHighContrast);
        assert_eq!(Palette::from_theme(&t, true, true, Accent::Blue).appearance, Appearance::DarkHighContrast);
    }

    #[test]
    fn accent_names_map_to_token_colours() {
        let t = theme();
        for a in Accent::ALL {
            let p = Palette::from_theme(&t, false, false, a);
            assert_eq!(Some(p.accent), t.tokens.color(Appearance::Light, a.token()), "{}", a.name());
            assert_eq!(p.selection_bg, p.accent);
        }
        assert_eq!(Palette::from_theme(&t, false, false, Accent::Blue).accent.hex(), "#007AFF");
        assert_eq!("Purple".parse::<Accent>().unwrap(), Accent::Purple);
        assert!("teal".parse::<Accent>().unwrap_err().contains("expected one of"));
    }

    #[test]
    fn semantic_colours_come_from_tokens() {
        let t = theme();
        let p = Palette::from_theme(&t, true, false, Accent::Blue);
        assert_eq!(p.destructive.hex(), "#FF453A");
        assert_eq!(p.success.hex(), "#32D74B");
        assert_eq!(p.focus_ring.to_rgba8(), t.tokens.semantic(Appearance::Dark, SemanticColor::KeyboardFocusIndicator).unwrap().to_rgba8());
        assert!((p.window_fg.a - 0.847).abs() < 0.002);
        assert_eq!(p.selection_inactive_bg.hex(), "#464646");
    }

    #[test]
    fn titlebar_uses_the_opaque_material_fill() {
        let t = theme();
        let l = Palette::from_theme(&t, false, false, Accent::Blue);
        assert_eq!(l.headerbar_bg.to_rgba8()[3], 255);
        assert_ne!(l.headerbar_bg.to_rgba8(), l.headerbar_backdrop.to_rgba8(), "active and inactive differ");
    }

    #[test]
    fn css_formatting() {
        assert_eq!(css(Color::new(1.0, 0.0, 0.5, 1.0)), "#ff0080");
        assert_eq!(css(Color::from_rgba8([0, 0, 0, 216])), "rgba(0, 0, 0, 0.847)");
        assert_eq!(css(Color::from_rgba8([255, 255, 255, 0])), "rgba(255, 255, 255, 0)");
        assert_eq!(trim_float(1.5, 3), "1.5");
        assert_eq!(trim_float(2.0, 3), "2");
    }

    #[test]
    fn compositing() {
        let c = over(Color::new(0.0, 0.0, 0.0, 0.5), Color::new(1.0, 1.0, 1.0, 1.0));
        assert!((c.r - 0.5).abs() < 1e-6 && c.a == 1.0);
        let p = Palette::from_theme(&theme(), false, false, Accent::Blue);
        assert_eq!(p.solid(p.fill).to_rgba8()[3], 255);
    }
}
