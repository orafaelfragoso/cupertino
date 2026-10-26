//! Control geometry of a generated theme.
//!
//! Heights come from the control alignment table of `cupertino-theme` (meaning of the
//! rendition metrics, regular size). The window radius is as designed (12 pt),
//! the menu item radius is the menu-bar selection radius (4 pt). Everything else
//! (control radii, focus ring, padding, scrollbar thickness, popover radius) is approximate and a
//! field of [`Metrics`], so it can be changed without touching the generators.

use cupertino_theme::metrics::{alignment_height, Control, ControlSize};
use cupertino_theme::Theme;

#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    pub size: ControlSizeName,
    /// Visual heights in px (1 pt = 1 px at scale 1).
    pub button_height: f64,
    pub entry_height: f64,
    pub check_size: f64,
    pub switch_height: f64,
    pub switch_width: f64,
    pub slider_knob: f64,
    pub slider_track: f64,
    pub scrollbar_thickness: f64,
    pub table_header_height: f64,
    pub toolbar_button_height: f64,

    pub window_radius: f64,
    /// approximate.
    pub button_radius: f64,
    pub entry_radius: f64,
    pub popover_radius: f64,
    /// selection in menus.
    pub menu_item_radius: f64,

    pub focus_ring_width: f64,
    pub padding_x: f64,
    pub spacing: f64,
    /// UI font point size for toolkits that take points (px × 0.75, rounded to 0.5; approximate).
    pub font_pt: f64,
    pub font_family: String,
    pub rim_width: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ControlSizeName {
    Regular,
    Small,
    Large,
}

impl ControlSizeName {
    fn size(self) -> ControlSize {
        match self {
            ControlSizeName::Regular => ControlSize::Regular,
            ControlSizeName::Small => ControlSize::Small,
            ControlSizeName::Large => ControlSize::Large,
        }
    }
}

impl std::str::FromStr for ControlSizeName {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "regular" => Ok(Self::Regular),
            "small" => Ok(Self::Small),
            "large" => Ok(Self::Large),
            _ => Err(format!("unknown control size {s:?} (regular, small or large)")),
        }
    }
}

impl Metrics {
    pub fn from_theme(theme: &Theme, size: ControlSizeName) -> Metrics {
        let s = size.size();
        let h = |c: Control, fallback: f64| alignment_height(c, s).or_else(|| alignment_height(c, ControlSize::Regular)).unwrap_or(fallback);
        let typ = theme.tokens.typography();
        let font_px = typ.substitute_size(typ.sizes.regular);
        Metrics {
            size,
            button_height: h(Control::PushButton, 20.0),
            entry_height: h(Control::TextField, 20.0),
            check_size: h(Control::CheckBoxRadio, 14.0),
            switch_height: h(Control::Switch, 22.0),
            switch_width: cupertino_theme::metrics::control_metric(Control::Switch).fixed_width(s).unwrap_or(38.0),
            slider_knob: h(Control::SliderKnob, 20.0),
            slider_track: h(Control::SliderTrack, 4.0),
            scrollbar_thickness: h(Control::OverlayScrollerThumb, 12.0),
            table_header_height: h(Control::TableHeaderModern, 28.0),
            toolbar_button_height: h(Control::ToolbarSegmented, 22.0),
            window_radius: theme.shapes.window_corner_radius,
            button_radius: 6.0,
            entry_radius: 6.0,
            popover_radius: 10.0,
            menu_item_radius: theme.shapes.menu_bar_selection_corner_radius,
            focus_ring_width: 3.0,
            padding_x: 8.0,
            spacing: 8.0,
            font_pt: ((font_px * 0.75) * 2.0).round() / 2.0,
            font_family: typ.family.to_string(),
            rim_width: theme.shapes.window_rim_width_px,
        }
    }

    /// Row height of lists and menus (button height plus 2 px of breathing room, approximate).
    pub fn row_height(&self) -> f64 {
        self.button_height + 8.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regular_metrics_match_the_control_table() {
        let theme = Theme::load().unwrap();
        let m = Metrics::from_theme(&theme, ControlSizeName::Regular);
        assert_eq!(m.button_height, 20.0);
        assert_eq!(m.entry_height, 20.0);
        assert_eq!(m.check_size, 14.0);
        assert_eq!(m.switch_width, 38.0);
        assert_eq!(m.window_radius, 12.0);
        assert_eq!(m.menu_item_radius, 4.0);
        assert_eq!(m.font_family, "Inter");
        assert_eq!(m.font_pt, 9.5, "13 pt × 0.96 × 0.75 = 9.36 → 9.5");
    }

    #[test]
    fn sizes_change_the_heights() {
        let theme = Theme::load().unwrap();
        let s = Metrics::from_theme(&theme, ControlSizeName::Small);
        let l = Metrics::from_theme(&theme, ControlSizeName::Large);
        assert_eq!(s.button_height, 16.0);
        assert_eq!(l.button_height, 28.0);
        assert_eq!(l.switch_height, 22.0, "controls without a large size fall back to regular");
        assert!("regular".parse::<ControlSizeName>().is_ok());
        assert!("huge".parse::<ControlSizeName>().is_err());
    }
}
