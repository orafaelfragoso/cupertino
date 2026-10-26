//! Stage configuration.
//!
//! The existence of every setting below is as designed. The default values
//! of the layout settings were from the service (table) and are labelled per field; values
//! labelled approximate or unknown must be fitted against recordings. Field names are neutral
//! re-expressions of the verified keys. The strip moves with the window manager's generic frame
//! spring (no strip-specific spring exists).

use cupertino_animation::{Curve, SpringParams};
use cupertino_wm::geometry::Rect;

/// How the windows of one app are grouped (the setting "all windows of an app" versus "one at a
/// time" exists).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grouping {
    /// All windows of an app share one group.
    AllWindows,
    /// Every window is its own group.
    OneAtATime,
}

/// What clicking the desktop does (settings for emptying the stage and for showing the desktop
/// exist).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickDesktop {
    Nothing,
    /// Move the centre group into the strip and reveal the desktop.
    EmptyStage,
}

#[derive(Clone, Debug)]
pub struct StageConfig {
    /// Maximum strip rows. 6.
    pub strip_max_rows: usize,
    /// Minimum strip rows. 4.
    pub strip_min_rows: usize,
    /// Inset of the strip from the left edge of the usable area. 16 pt.
    pub strip_edge_inset: f64,
    /// Width of a strip slot. 170 pt.
    pub strip_width: f64,
    /// Nominal slot height. 170 pt; capped by `strip_max_item_height` and shrinks
    /// when the rows do not fit.
    pub strip_row_height: f64,
    /// Vertical gap between slots. 32 pt.
    pub strip_row_gap: f64,
    /// Largest slot height. 160 pt.
    pub strip_max_item_height: f64,
    /// Preferred smallest slot height. 96 pt; using it to choose the row count is.
    pub strip_preferred_min_item_height: f64,
    /// Smallest slot height. 20 pt.
    pub strip_absolute_min_item_height: f64,
    /// Corner radius of strip thumbnails. 8 pt.
    pub strip_corner_radius: f64,
    /// Vertical inset of the stage area. 0 pt.
    pub stage_vertical_inset: f64,
    /// Smallest horizontal inset of the stage area. 4 pt.
    pub stage_min_horizontal_inset: f64,
    /// Seconds a strip click session stays open. 1.25 s.
    pub click_session_timeout: f64,
    /// Scale distance of a hovered / pressed strip item. 16 pt each.
    pub hover_scale_distance: f64,
    pub click_down_scale_distance: f64,
    /// Vertical padding of the strip inside the usable area. approximate.
    pub strip_vertical_padding: f64,
    /// 0 = top aligned, 0.5 = centred. approximate.
    pub strip_vertical_align: f64,
    /// Padding between a slot and the thumbnail inside it. approximate.
    pub thumbnail_padding: f64,
    /// Horizontal gap between the strip and the stage. 16 pt; the resulting stage
    /// inset is 16 + 170 + 16 = 202 pt as in the verified computed default.
    pub stage_inset_from_strip: f64,
    /// Group app-icon overlay: size and offsets from the slot's bottom-left corner. 40 pt,
    /// left offset −10, bottom offset 10.
    pub icon_size: f64,
    pub icon_left_offset: f64,
    pub icon_bottom_offset: f64,
    /// Horizontal spacing of the icons of a group with two / three apps. −4 / −22.
    pub icon_two_spacing: f64,
    pub icon_three_spacing: f64,
    /// Visible width of the hidden strip. 0 pt.
    pub strip_peek_width: f64,
    /// Always hide the strip until the pointer reaches the edge. default `false`.
    pub auto_hide: bool,
    /// Width of the left-edge strip that reveals a hidden strip. approximate.
    pub reveal_zone_width: f64,
    /// Dwell before a hidden strip reveals (0.15 s) / a shown strip hides
    /// (approximate).
    pub reveal_delay: f64,
    pub hide_delay: f64,
    /// Margin by which a window must intrude into the strip before it hides it. approximate.
    pub overlap_margin: f64,
    /// Hide desktop icons while the stage is active. default `true`; widgets
    /// default to visible.
    pub hide_desktop_items: bool,
    pub click_desktop: ClickDesktop,
    pub grouping: Grouping,
    /// Promote the most recent strip group when the centre group closes instead of leaving the
    /// stage empty. approximate.
    pub promote_on_empty: bool,
    /// Swap / reorder spring: damping ratio 1.0, response 0.38 s (stiffness 273.4, damping 33.07),
    /// the window manager's default spring for frame properties, that the strip
    /// moves with it, as no strip-specific spring exists, is.
    pub swap_curve: Curve,
    /// Alpha spring of a swap: damping ratio 1.0, response 0.30 s (stiffness 438.7, damping 41.89),
    /// the window manager's spring for the alpha property, its use for the strip
    /// is.
    pub alpha_curve: Curve,
    /// Corner-radius spring of a swap: damping ratio 1.0, response 0.20 s (stiffness 987.0,
    /// damping 62.83), the window manager's spring for the corner-radius property,
    /// its use for the strip is.
    pub corner_curve: Curve,
    /// Strip hide / reveal slide: the same 0.38 s spring, which the auto-hide transition's
    /// position properties take (for this use).
    pub slide_curve: Curve,
}

impl StageConfig {
    /// The stage frame inside `usable` while the stage is on: right of the strip column, inset on
    /// the right and top and bottom.
    pub fn stage_frame(&self, usable: Rect) -> Rect {
        let x = usable.x + self.strip_edge_inset + self.strip_width + self.stage_inset_from_strip;
        let right_inset = self.strip_edge_inset.max(self.stage_min_horizontal_inset);
        Rect::new(
            x,
            usable.y + self.stage_vertical_inset,
            (usable.right() - x - right_inset).max(1.0),
            (usable.h - 2.0 * self.stage_vertical_inset).max(1.0),
        )
    }
}

impl Default for StageConfig {
    fn default() -> Self {
        Self {
            strip_max_rows: 6,
            strip_min_rows: 4,
            strip_edge_inset: 16.0,
            strip_width: 170.0,
            strip_row_height: 170.0,
            strip_row_gap: 32.0,
            strip_max_item_height: 160.0,
            strip_preferred_min_item_height: 96.0,
            strip_absolute_min_item_height: 20.0,
            strip_corner_radius: 8.0,
            stage_vertical_inset: 0.0,
            stage_min_horizontal_inset: 4.0,
            click_session_timeout: 1.25,
            hover_scale_distance: 16.0,
            click_down_scale_distance: 16.0,
            strip_vertical_padding: 24.0,
            strip_vertical_align: 0.5,
            thumbnail_padding: 6.0,
            stage_inset_from_strip: 16.0,
            icon_size: 40.0,
            icon_left_offset: -10.0,
            icon_bottom_offset: 10.0,
            icon_two_spacing: -4.0,
            icon_three_spacing: -22.0,
            strip_peek_width: 0.0,
            auto_hide: false,
            reveal_zone_width: 4.0,
            reveal_delay: 0.15,
            hide_delay: 0.3,
            overlap_margin: 0.0,
            hide_desktop_items: true,
            click_desktop: ClickDesktop::EmptyStage,
            grouping: Grouping::AllWindows,
            promote_on_empty: false,
            swap_curve: Curve::Spring(SpringParams::from_perceptual(0.38, 0.0)),
            alpha_curve: Curve::Spring(SpringParams::from_perceptual(0.30, 0.0)),
            corner_curve: Curve::Spring(SpringParams::from_perceptual(0.20, 0.0)),
            slide_curve: Curve::Spring(SpringParams::from_perceptual(0.38, 0.0)),
        }
    }
}
