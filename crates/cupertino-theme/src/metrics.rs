//! Control geometry as data and a few shape/shadow tokens.
//! Window corner radii and the shadow of a regular window are as designed.
//!
//! The control table is as designed from rendition metrics, with the meaning
//! of the metric fields; all values are in points at 1×.

use crate::evidence::Evidence;
use crate::window_shadow::{window_shadow, ShadowOption, WindowCornerSize, WINDOW_CORNER_RADIUS_REGULAR, WINDOW_CORNER_RADIUS_SMALL};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ControlSize {
    Regular,
    Small,
    Mini,
    Large,
}

impl ControlSize {
    pub const ALL: [ControlSize; 4] = [ControlSize::Regular, ControlSize::Small, ControlSize::Mini, ControlSize::Large];

    fn index(self) -> usize {
        self as usize
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Control {
    PushButton,
    PopUpButton,
    ComboBox,
    TextField,
    SegmentedControl,
    ToolbarSegmented,
    CheckBoxRadio,
    Switch,
    SliderKnob,
    SliderTrack,
    Stepper,
    DisclosureButton,
    OverlayScrollerThumb,
    LegacyScrollerTrack,
    WindowControlButton,
    TableHeaderClassic,
    TableHeaderModern,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ControlMetric {
    pub control: Control,
    /// Alignment heights ordered regular, small, mini, large.
    pub heights: [Option<f64>; 4],
    /// Widths where the control has a fixed width ordered like `heights`.
    pub widths: [Option<f64>; 4],
    /// Size of the regular-size image asset.
    pub image_regular: Option<(f64, f64)>,
    /// Number of stretchable parts of the asset (3 = horizontal three-part, 9 = nine-slice).
    pub parts: Option<u8>,
}

impl ControlMetric {
    pub fn alignment_height(&self, size: ControlSize) -> Option<f64> {
        self.heights[size.index()]
    }

    pub fn fixed_width(&self, size: ControlSize) -> Option<f64> {
        self.widths[size.index()]
    }
}

pub const CONTROL_METRICS_EVIDENCE: Evidence = Evidence::Inferred;

const N: Option<f64> = None;

const fn h(r: f64, s: f64, m: f64, l: Option<f64>) -> [Option<f64>; 4] {
    [Some(r), Some(s), Some(m), l]
}

const fn m(
    control: Control,
    heights: [Option<f64>; 4],
    image: Option<(f64, f64)>,
    parts: Option<u8>,
) -> ControlMetric {
    ControlMetric { control, heights, widths: [N; 4], image_regular: image, parts }
}

pub const CONTROL_METRICS: [ControlMetric; 17] = [
    m(Control::PushButton, h(20.0, 16.0, 13.0, Some(28.0)), Some((38.0, 32.0)), Some(3)),
    m(Control::PopUpButton, h(20.0, 16.0, 13.0, Some(28.0)), Some((47.0, 25.0)), Some(3)),
    m(Control::ComboBox, h(20.0, 16.0, 13.0, N), Some((44.0, 23.0)), N_PARTS),
    m(Control::TextField, h(20.0, 16.0, 13.0, Some(28.0)), Some((26.0, 22.0)), Some(9)),
    m(Control::SegmentedControl, h(20.0, 16.0, 13.0, Some(28.0)), Some((38.0, 24.0)), Some(6)),
    m(Control::ToolbarSegmented, h(22.0, 18.0, 15.0, Some(28.0)), Some((32.0, 25.0)), N_PARTS),
    m(Control::CheckBoxRadio, h(14.0, 12.0, 10.0, Some(16.0)), Some((18.0, 18.0)), N_PARTS),
    ControlMetric {
        control: Control::Switch,
        heights: h(22.0, 18.0, 15.0, N),
        widths: [Some(38.0), Some(32.0), Some(26.0), N],
        image_regular: Some((42.0, 25.0)),
        parts: N_PARTS,
    },
    m(Control::SliderKnob, h(20.0, 16.0, 13.0, N), Some((24.0, 28.0)), N_PARTS),
    m(Control::SliderTrack, h(4.0, 4.0, 3.0, N), Some((14.0, 4.0)), N_PARTS),
    m(Control::Stepper, h(20.0, 16.0, 13.0, N), Some((19.0, 28.0)), N_PARTS),
    m(Control::DisclosureButton, h(20.0, 16.0, 13.0, Some(24.0)), Some((28.0, 26.0)), N_PARTS),
    m(Control::OverlayScrollerThumb, [Some(12.0), Some(10.0), N, N], Some((24.0, 12.0)), N_PARTS),
    m(Control::LegacyScrollerTrack, [Some(15.0), Some(11.0), N, N], Some((15.0, 15.0)), N_PARTS),
    m(Control::WindowControlButton, [Some(12.0), Some(9.0), Some(9.0), N], Some((16.0, 16.0)), N_PARTS),
    m(Control::TableHeaderClassic, [Some(17.0), N, N, N], N_IMG, N_PARTS),
    m(Control::TableHeaderModern, [Some(28.0), N, N, N], N_IMG, N_PARTS),
];

const N_PARTS: Option<u8> = None;
const N_IMG: Option<(f64, f64)> = None;

pub fn control_metric(c: Control) -> &'static ControlMetric {
    CONTROL_METRICS.iter().find(|m| m.control == c).expect("every control has a metric row")
}

/// Alignment height of `control` at `size`, if that size exists for the control.
pub fn alignment_height(control: Control, size: ControlSize) -> Option<f64> {
    control_metric(control).alignment_height(size)
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShapeTokens {
    /// regular window corner radius 12 pt, supersedes the earlier
    /// 8–9 pt estimate.
    pub window_corner_radius: f64,
    /// small (compact) window corner radius 10 pt.
    pub window_corner_radius_small: f64,
    /// layer archive 4 with a continuous curve.
    pub menu_bar_selection_corner_radius: f64,
    /// window rim is one pixel wide (window rim colour).
    pub window_rim_width_px: f64,
    /// approximate: menu bar height ≈ 24 pt on displays without a notch;
    /// needs runtime verification.
    pub menu_bar_height: f64,
}

impl Default for ShapeTokens {
    fn default() -> Self {
        Self {
            window_corner_radius: WINDOW_CORNER_RADIUS_REGULAR,
            window_corner_radius_small: WINDOW_CORNER_RADIUS_SMALL,
            menu_bar_selection_corner_radius: 4.0,
            window_rim_width_px: 1.0,
            menu_bar_height: 24.0,
        }
    }
}

impl ShapeTokens {
    pub fn window_corner_radius_for(&self, size: WindowCornerSize) -> f64 {
        match size {
            WindowCornerSize::Regular => self.window_corner_radius,
            WindowCornerSize::Small => self.window_corner_radius_small,
        }
    }
}

/// Window shadow of a regular window, taken from the verified table: active
/// σ 20 pt, offset 18 pt, density 0.40; inactive σ 12 pt, offset 8 pt, density 0.25. The tight
/// contact shadow is approximate (the table has no such term). The complete table is in
/// [`crate::window_shadow`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowShadowPreset {
    /// Active window sigma.
    pub sigma: f64,
    /// Active window density.
    pub alpha: f64,
    /// Active window vertical offset.
    pub offset_y: f64,
    /// approximate.
    pub contact_sigma: f64,
    /// Inactive density divided by active density (0.25 / 0.40).
    pub inactive_scale: f64,
    /// Inactive window sigma.
    pub inactive_sigma: f64,
    /// Inactive window vertical offset.
    pub inactive_offset_y: f64,
}

impl Default for WindowShadowPreset {
    fn default() -> Self {
        let a = window_shadow(ShadowOption::Standard, true, false, false);
        let i = window_shadow(ShadowOption::Standard, false, false, false);
        Self {
            sigma: a.sigma,
            alpha: a.density,
            offset_y: a.vertical_offset,
            contact_sigma: 1.0,
            inactive_scale: i.density / a.density,
            inactive_sigma: i.sigma,
            inactive_offset_y: i.vertical_offset,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_design_system_section_3() {
        use Control::*;
        use ControlSize::*;
        assert_eq!(alignment_height(PushButton, Regular), Some(20.0));
        assert_eq!(alignment_height(PushButton, Small), Some(16.0));
        assert_eq!(alignment_height(PushButton, Mini), Some(13.0));
        assert_eq!(alignment_height(PushButton, Large), Some(28.0));
        assert_eq!(alignment_height(ComboBox, Large), None);
        assert_eq!(alignment_height(ToolbarSegmented, Regular), Some(22.0));
        assert_eq!(alignment_height(CheckBoxRadio, Large), Some(16.0));
        assert_eq!(alignment_height(Switch, Regular), Some(22.0));
        assert_eq!(control_metric(Switch).fixed_width(Small), Some(32.0));
        assert_eq!(alignment_height(SliderTrack, Mini), Some(3.0));
        assert_eq!(alignment_height(OverlayScrollerThumb, Mini), None);
        assert_eq!(alignment_height(WindowControlButton, Regular), Some(12.0));
        assert_eq!(alignment_height(TableHeaderModern, Regular), Some(28.0));
        assert_eq!(control_metric(PushButton).image_regular, Some((38.0, 32.0)));
        assert_eq!(control_metric(TextField).parts, Some(9));
    }

    #[test]
    fn shape_and_shadow_tokens_are_verified_values() {
        let s = ShapeTokens::default();
        assert_eq!(s.window_corner_radius, 12.0);
        assert_eq!(s.window_corner_radius_for(WindowCornerSize::Small), 10.0);
        let w = WindowShadowPreset::default();
        assert_eq!((w.sigma, w.alpha, w.offset_y), (20.0, 0.40, 18.0));
        assert_eq!((w.inactive_sigma, w.inactive_offset_y), (12.0, 8.0));
        assert!((w.inactive_scale - 0.625).abs() < 1e-12);
    }

    #[test]
    fn rows_are_unique_and_regular_always_present() {
        for (i, a) in CONTROL_METRICS.iter().enumerate() {
            assert!(a.alignment_height(ControlSize::Regular).is_some());
            for b in &CONTROL_METRICS[i + 1..] {
                assert_ne!(a.control, b.control);
            }
        }
    }

    #[test]
    fn sizes_decrease_regular_to_mini() {
        for m in CONTROL_METRICS.iter() {
            if let (Some(r), Some(s), Some(mi)) = (m.heights[0], m.heights[1], m.heights[2]) {
                assert!(r >= s && s >= mi, "{:?}", m.control);
            }
        }
    }
}
