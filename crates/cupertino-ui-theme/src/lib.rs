//! CupertinoKit theme tokens. Controls read every colour, metric and type size from [`ThemeTokens`], never a
//! literal. Colours are looked up by [`ColorRole`] and the control's [`StateVariant`], so a theme is a table.

use cupertino_geometry::Size;
use std::collections::HashMap;

pub use cupertino_theme::{Appearance, Color, ControlSize, DesignTokens};

mod adapter;
mod metrics;
mod neutral;
mod palette;

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
}

impl ColorRole {
    pub const ALL: [ColorRole; 31] = {
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
        self.map
            .get(&(role, state))
            .or_else(|| self.map.get(&(role, StateVariant::NORMAL)))
            .copied()
    }
}

/// Regular-size control geometry, in logical points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlMetrics {
    pub size: ControlSize,
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
