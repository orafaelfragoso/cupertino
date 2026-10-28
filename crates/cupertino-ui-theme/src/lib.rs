//! CupertinoKit theme tokens. Controls read every colour, metric and type size from [`ThemeTokens`], never a
//! literal. Colours are looked up by [`ColorRole`] and the control's [`StateVariant`], so a theme is a table.

use cupertino_geometry::Size;
use std::collections::HashMap;

pub use cupertino_theme::{Appearance, Color, ControlSize};

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
    DefaultButtonFill,
    DefaultButtonText,
    DestructiveText,
    SwitchTrack,
    SwitchThumb,
    TextBackground,
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
}

/// The state a colour is resolved for. Disabled wins over everything; pressed wins over hover.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StateVariant {
    #[default]
    Normal,
    Hover,
    Pressed,
    Disabled,
    Selected,
    /// Selected in a window that is not active.
    SelectedInactive,
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

    /// The colour of `role` in `state`, or its normal colour when the theme has no entry for that state.
    pub fn get(&self, role: ColorRole, state: StateVariant) -> Option<Color> {
        self.map
            .get(&(role, state))
            .or_else(|| self.map.get(&(role, StateVariant::Normal)))
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
        let colors = ColorTokens::new([
            ((ColorRole::ControlFill, StateVariant::Normal), normal),
            ((ColorRole::ControlFill, StateVariant::Pressed), pressed),
        ]);
        assert_eq!(
            colors.get(ColorRole::ControlFill, StateVariant::Pressed),
            Some(pressed)
        );
        assert_eq!(
            colors.get(ColorRole::ControlFill, StateVariant::Hover),
            Some(normal)
        );
        assert_eq!(colors.get(ColorRole::Text, StateVariant::Normal), None);
    }
}
