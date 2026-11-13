//! CupertinoKit theme tokens. Controls read every colour, metric and type size from [`ThemeTokens`], never a
//! literal. Colours are looked up by [`ColorRole`] and the control's [`StateVariant`], so a theme is a table.

use cupertino_geometry::Size;
use std::collections::HashMap;

pub use cupertino_theme::control_sizes::{
    choice_mark, control_geometry, slider_geometry, switch_geometry, ControlFamily,
    ControlGeometry, Corners, SliderGeometry, SwitchGeometry,
};
pub use cupertino_theme::glass::FaceTone;
pub use cupertino_theme::{Appearance, Color, ControlSize, DesignTokens};

mod accent;
mod adapter;
mod metrics;
mod neutral;
mod palette;
mod vibrant;

pub use accent::selection_tint;
pub use vibrant::{
    glass_foreground, vibrant_level, vibrant_secondary, VIBRANT_PLACEHOLDER, VIBRANT_PRIMARY,
};

/// The semantic colours controls draw with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ColorRole {
    WindowBackground,
    Text,
    SecondaryText,
    PlaceholderText,
    ControlFill,
    ControlBorder,
    ControlText,
    DisabledControlFill,
    DisabledControlBorder,
    DisabledControlText,
    DefaultButtonFill,
    DefaultButtonText,
    DestructiveFill,
    DestructiveText,
    SwitchTrack,
    SwitchThumb,
    TextBackground,
    TextFieldBorder,
    SelectedTextBackground,
    SelectedText,
    SelectedControlFill,
    SelectedControlText,
    SecondarySelectedControlFill,
    SecondarySelectedControlText,
    KeyboardFocusIndicator,
    Separator,
    /// Text of a section or table header.
    Header,
    ToolTip,
    ToolTipText,
    /// The overlay scroller's thumb.
    ScrollerThumb,
    /// The layer that dims a window behind a dialog or sheet. Translucent in every palette: it must not hide what it
    /// dims.
    DialogScrim,
    /// Text of a link.
    Link,
    /// A page indicator dot for a page that is not shown; the shown page's dot is `Text`.
    PageDot,
    /// The fill of a group of controls set off from the window, as a module of a panel.
    GroupFill,
    /// A symbol drawn on the switch thumb's colour, as on a panel's level slider.
    SwitchThumbGlyph,
    /// The track of a panel's level slider, a neutral grey under its fill.
    PanelTrack,
    /// The hairline around a panel's level slider track, where the appearance draws one.
    PanelTrackBorder,
    /// The faintest label colour, as the bubble behind a selected icon.
    QuaternaryLabel,
    /// The empty part of a slider's track (a panel's level slider has [`ColorRole::PanelTrack`]); the filled part is
    /// the selected [`ColorRole::ControlFill`].
    SliderTrack,
    /// A sidebar row's selection where it is not the accent: a neutral fill over the sidebar's material, laid over
    /// what is beneath so the material's tint shows through. A file browser's places draw their selected row with it
    /// in an active window; every sidebar draws it, fainter, for its selected row in a window that is not active
    /// (resolved with `window_active` false).
    SidebarSelection,
    /// The faintest text: a window's title and its toolbar's symbols while the window is not active.
    TertiaryText,
    /// A sidebar's labels and symbols while its window is not active: dimmer than the secondary label, so the sidebar
    /// recedes behind its tinted material.
    InactiveSidebarText,
    /// The toolbar row of a window that is not active: an opaque band a little lighter (dark) or darker (light) than
    /// the window background, laid over the top of the content under the row.
    InactiveTitlebar,
    /// The fill of every other row of a table or list, drawn as a rounded stripe behind the rows' content.
    AlternatingRow,
}

impl ColorRole {
    pub const ALL: [ColorRole; 44] = {
        use ColorRole::*;
        [
            WindowBackground,
            Text,
            SecondaryText,
            PlaceholderText,
            ControlFill,
            ControlBorder,
            ControlText,
            DisabledControlFill,
            DisabledControlBorder,
            DisabledControlText,
            DefaultButtonFill,
            DefaultButtonText,
            DestructiveFill,
            DestructiveText,
            SwitchTrack,
            SwitchThumb,
            TextBackground,
            TextFieldBorder,
            SelectedTextBackground,
            SelectedText,
            SelectedControlFill,
            SelectedControlText,
            SecondarySelectedControlFill,
            SecondarySelectedControlText,
            KeyboardFocusIndicator,
            Separator,
            Header,
            ToolTip,
            ToolTipText,
            ScrollerThumb,
            DialogScrim,
            Link,
            PageDot,
            GroupFill,
            SwitchThumbGlyph,
            PanelTrack,
            PanelTrackBorder,
            QuaternaryLabel,
            SliderTrack,
            SidebarSelection,
            TertiaryText,
            InactiveSidebarText,
            InactiveTitlebar,
            AlternatingRow,
        ]
    };
}

/// The pointer and enabled state a colour is resolved for. Disabled wins over everything; pressed wins over hover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum InteractionVariant {
    #[default]
    Normal,
    Hover,
    Pressed,
    Disabled,
}

thread_local! {
    /// How far a window that just became active has come from its inactive colours to its active ones, 0 to 1.
    static ACTIVATION: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0) };
}

/// Sets how far the colours of the window being painted have crossed from the inactive to the active ones: 1 is the
/// active colours (the usual state). The painter sets it around the paint of a window and puts 1 back.
pub fn set_activation(progress: f32) {
    ACTIVATION.with(|a| a.set(progress.clamp(0.0, 1.0)));
}

/// The state a colour is resolved for. Keyboard focus is not part of it: the focus ring is drawn on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct StateVariant {
    pub interaction: InteractionVariant,
    pub selected: bool,
    pub window_active: bool,
}

impl StateVariant {
    pub const NORMAL: StateVariant = StateVariant {
        interaction: InteractionVariant::Normal,
        selected: false,
        window_active: true,
    };

    pub const ALL: [StateVariant; 16] = {
        use InteractionVariant::*;
        let mut all = [StateVariant::NORMAL; 16];
        let interactions = [Normal, Hover, Pressed, Disabled];
        let mut i = 0;
        while i < 16 {
            all[i] = StateVariant {
                interaction: interactions[i / 4],
                selected: i % 4 >= 2,
                window_active: i % 2 == 0,
            };
            i += 1;
        }
        all
    };
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct ColorTokens {
    map: HashMap<(ColorRole, StateVariant), Color>,
}

impl ColorTokens {
    pub fn new(entries: impl IntoIterator<Item = ((ColorRole, StateVariant), Color)>) -> Self {
        Self {
            map: entries.into_iter().collect(),
        }
    }

    /// The built-in theme, which needs no theme data. Every colour is opaque but the dialog scrim, which dims.
    pub fn neutral(appearance: Appearance) -> Self {
        match appearance {
            Appearance::Light => neutral::LIGHT,
            Appearance::Dark => neutral::DARK,
            Appearance::LightHighContrast => neutral::HIGH_CONTRAST_LIGHT,
            Appearance::DarkHighContrast => neutral::HIGH_CONTRAST_DARK,
        }
        .colors()
        .resolve()
    }

    /// The Cupertino theme. Light and dark come from `tokens` and fail on a missing token; high contrast is the
    /// built-in palette, which never depends on translucency.
    pub fn cupertino(
        tokens: &DesignTokens,
        appearance: Appearance,
    ) -> cupertino_theme::Result<Self> {
        if appearance.is_high_contrast() {
            return Ok(Self::neutral(appearance));
        }
        Ok(adapter::palette(tokens, appearance)?.resolve())
    }

    /// The colour of `role` in `state`, or its normal colour when the theme has no entry for that state.
    pub fn get(&self, role: ColorRole, state: StateVariant) -> Option<Color> {
        let shown = self.lookup(role, state);
        let progress = ACTIVATION.with(|a| a.get());
        if state.window_active && progress < 1.0 {
            // a window that just became active: its colours cross from the inactive ones
            let before = self.lookup(
                role,
                StateVariant {
                    window_active: false,
                    ..state
                },
            );
            if let (Some(before), Some(shown)) = (before, shown) {
                return Some(before.lerp(shown, progress));
            }
        }
        shown
    }

    fn lookup(&self, role: ColorRole, state: StateVariant) -> Option<Color> {
        self.map
            .get(&(role, state))
            .or_else(|| self.map.get(&(role, StateVariant::NORMAL)))
            .copied()
    }

    /// The same table with each colour replaced by `f(role, state, colour)`.
    pub fn map(&self, f: impl Fn(ColorRole, StateVariant, Color) -> Color) -> Self {
        Self {
            map: self
                .map
                .iter()
                .map(|(&(role, state), &color)| ((role, state), f(role, state, color)))
                .collect(),
        }
    }
}

/// The control geometry of one control size, in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlMetrics {
    pub size: ControlSize,
    /// The compact metrics: tighter heights and room at the three smaller sizes.
    pub compact: bool,
    pub button_height: f64,
    pub button_inset: f64,
    pub button_radius: f64,
    pub field_height: f64,
    pub field_inset: f64,
    pub field_radius: f64,
    pub switch: Size,
    pub switch_thumb: f64,
    pub switch_thumb_inset: f64,
    /// The smallest pointer and touch target, reached with layout padding.
    pub min_target: Size,
}

impl ControlMetrics {
    /// The geometry of `family` at these metrics' size.
    pub fn geometry(&self, family: ControlFamily) -> ControlGeometry {
        control_geometry(family, self.size, self.compact)
    }

    /// These metrics at another size, the buttons', fields' and switch's geometry following it; the minimum target
    /// and the other values stay.
    pub fn at_size(self, size: ControlSize, compact: bool) -> Self {
        let button = control_geometry(ControlFamily::PushButton, size, compact);
        let field = control_geometry(ControlFamily::TextField, size, compact);
        let switch = switch_geometry(compact);
        Self {
            size,
            compact,
            button_height: button.height,
            button_inset: button.inset,
            button_radius: button.corners.radius(button.height),
            field_height: field.height,
            field_inset: field.inset,
            field_radius: field.corners.radius(field.height),
            switch: Size::new(switch.track.0, switch.track.1),
            switch_thumb: switch.thumb.1,
            switch_thumb_inset: switch.inset,
            ..self
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeRole {
    pub size: f32,
    /// OpenType weight.
    pub weight: u16,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TypeScale {
    pub family: String,
    pub body: TypeRole,
    pub caption: TypeRole,
    pub section: TypeRole,
    pub title: TypeRole,
}

/// The keyboard focus ring, drawn outside the control's chrome without changing its layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FocusRing {
    pub width: f64,
    pub outset: f64,
    /// Added to the chrome's corner radius at the stroke's centre.
    pub radius_extra: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Motion {
    pub reduce: bool,
    pub caret_phase_ms: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ThemePreferences {
    pub appearance: Appearance,
    pub reduce_transparency: bool,
    pub reduce_motion: bool,
}

/// A surface that shows a compositor material behind it where the compositor offers one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Surface {
    Sidebar,
    Menu,
    Popover,
    /// A heads-up display over everything, such as the volume and brightness overlay.
    Hud,
    /// The application grid's panel over the desktop.
    AppGrid,
    /// The application switcher's panel.
    Switcher,
    /// The Dock's bar and its stack panels: glass, and with Reduce Transparency a body in the wallpaper's colour.
    Dock,
    /// A running indicator under a Dock tile in the dark appearance, which lightens what lies behind it; elsewhere it
    /// asks for no material and the dot paints itself.
    DockIndicator,
    /// The Dock's bar itself: bar glass, clear glass with only a little lift of white, in both appearances; with
    /// Reduce Transparency and in high contrast the same as [`Surface::Dock`].
    DockBar,
    /// The name over a Dock tile: regular glass, light or dark by appearance (with the contrast recipe in high
    /// contrast); opaque with reduced transparency.
    Tooltip,
    /// A window's own background drawn by the compositor, so glass over it in the window reads the window rather than
    /// the desktop; opaque, so kept with reduced transparency and in high contrast.
    WindowBackground,
    /// The colour behind a form's groups drawn by the compositor, as [`Surface::WindowBackground`] is, so glass over it
    /// in the window shows; opaque, so kept with reduced transparency and in high contrast.
    TextBackground,
    /// The band a toolbar row lays over a page scrolled under it: what passes under the row shows blurred through a
    /// veil of the window's colour; opaque with reduced transparency and in high contrast.
    ToolbarBand,
}

/// How a surface paints: the compositor material to ask for, if any, and the opaque fill to paint where there is none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceLook {
    /// A material name of the theme data (`materials.json`).
    pub material: Option<&'static str>,
    /// `None` when the theme has no window background colour: the surface then paints nothing.
    pub fill: Option<Color>,
}

/// Everything a control reads from the theme.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeTokens {
    pub preferences: ThemePreferences,
    pub colors: ColorTokens,
    pub metrics: ControlMetrics,
    pub type_scale: TypeScale,
    pub focus: FocusRing,
    pub motion: Motion,
}

/// The material name of the glass of a floating bar of icons, the Dock's.
pub const BAR_GLASS_MATERIAL: &str = "BarGlass";
/// The material name of regular glass: the compositor draws glass rather than a recipe of the design tokens.
pub const GLASS_MATERIAL: &str = "Glass";
/// The material name of dense glass, the glass of popovers and cards.
pub const DENSE_GLASS_MATERIAL: &str = "DenseGlass";
/// The material name of the glass of menus.
pub const MENU_GLASS_MATERIAL: &str = "MenuGlass";
/// The material name of the glass of the tiles of a panel of modules.
pub const MODULE_GLASS_MATERIAL: &str = "ModuleGlass";
/// The material name of the glass of a window's sidebar.
pub const SIDEBAR_GLASS_MATERIAL: &str = "SidebarGlass";
/// The material name of the glass of the application switcher.
pub const SWITCHER_GLASS_MATERIAL: &str = "SwitcherGlass";
/// The material name of clear glass.
pub const CLEAR_GLASS_MATERIAL: &str = "ClearGlass";

impl ThemeTokens {
    /// The look of `surface`. Menus, popovers and the Dock are glass, which keeps its shape with reduced transparency
    /// and in high contrast (the compositor draws the glass recipe of those options, and the Dock its own body with
    /// Reduce Transparency); the application grid keeps its material too, whose recipe has variants of its own for
    /// those options. The sidebar, the switcher, a tooltip and the heads-up display keep their glass in high contrast,
    /// with the compositor's contrast recipe, and go opaque with Reduce Transparency. The other surfaces then ask for no
    /// material and are opaque.
    pub fn surface(&self, surface: Surface) -> SurfaceLook {
        let prefs = &self.preferences;
        let opaque = prefs.reduce_transparency || prefs.appearance.is_high_contrast();
        let contrast = prefs.appearance.is_high_contrast() && !prefs.reduce_transparency;
        let (material, kept) = match surface {
            Surface::Sidebar => (SIDEBAR_GLASS_MATERIAL, contrast),
            Surface::Menu => (MENU_GLASS_MATERIAL, true),
            Surface::Popover => (DENSE_GLASS_MATERIAL, true),
            Surface::Hud => ("LoginWindowControl", contrast),
            Surface::AppGrid => ("AppGrid", true),
            Surface::Switcher => (SWITCHER_GLASS_MATERIAL, contrast),
            Surface::Dock if prefs.reduce_transparency => ("Dock", true),
            Surface::Dock => (GLASS_MATERIAL, true),
            Surface::DockBar if !opaque => (BAR_GLASS_MATERIAL, true),
            Surface::DockBar => return self.surface(Surface::Dock),
            Surface::Tooltip => (GLASS_MATERIAL, contrast),
            Surface::DockIndicator => (
                "RunningIndicator",
                prefs.appearance.is_dark() && !prefs.appearance.is_high_contrast(),
            ),
            Surface::WindowBackground => ("WindowBackground", true),
            Surface::TextBackground => ("TextBackground", true),
            Surface::ToolbarBand => ("ToolbarBand", false),
        };
        let material = match surface {
            Surface::DockIndicator => kept.then_some(material),
            _ => (kept || !opaque).then_some(material),
        };
        let fill = match surface {
            Surface::TextBackground => ColorRole::TextBackground,
            _ => ColorRole::WindowBackground,
        };
        SurfaceLook {
            material,
            fill: self.colors.get(fill, StateVariant::NORMAL),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_without_its_own_colour_falls_back_to_normal() {
        let normal = Color::new(0.0, 0.0, 0.0, 1.0);
        let pressed = Color::new(1.0, 1.0, 1.0, 1.0);
        let state = |interaction| StateVariant {
            interaction,
            ..StateVariant::NORMAL
        };
        let colors = ColorTokens::new([
            ((ColorRole::ControlFill, StateVariant::NORMAL), normal),
            (
                (ColorRole::ControlFill, state(InteractionVariant::Pressed)),
                pressed,
            ),
        ]);
        assert_eq!(
            colors.get(ColorRole::ControlFill, state(InteractionVariant::Pressed)),
            Some(pressed)
        );
        assert_eq!(
            colors.get(ColorRole::ControlFill, state(InteractionVariant::Hover)),
            Some(normal)
        );
        assert_eq!(colors.get(ColorRole::Text, StateVariant::NORMAL), None);
    }
}
