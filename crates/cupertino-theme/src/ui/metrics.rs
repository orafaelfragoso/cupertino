//! The Cupertino theme's regular control geometry, type roles, focus ring and caret timing. Control geometry comes
//! from the control size table (`crate::control_sizes`) (not from the theme data), body and caption sizes
//! from the theme's type scale; every other value is a design value of its own (approximate).

use crate::ui::{
    ColorTokens, ControlMetrics, ControlSize, FocusRing, Motion, Size, ThemePreferences,
    ThemeTokens, TypeRole, TypeScale,
};
use crate::{Result, Theme};

/// The smallest pointer and touch target, reached with layout padding.
const MIN_TARGET: f64 = 24.0;
const REGULAR: u16 = 400;
const SEMIBOLD: u16 = 600;
const SECTION_SIZE: f32 = 12.0;
const TITLE_SIZE: f32 = 15.0;
const FOCUS_RING_WIDTH: f64 = 2.0;
const FOCUS_RING_OUTSET: f64 = 2.0;
const FOCUS_RING_RADIUS_EXTRA: f64 = 3.0;
/// How long the caret stays shown, then hidden, while it blinks.
pub const CARET_PHASE_MS: u32 = 500;
/// Body and caption sizes without theme data: the theme data's regular and small sizes.
const NEUTRAL_BODY_SIZE: f64 = 13.0;
const NEUTRAL_CAPTION_SIZE: f64 = 11.0;

impl ControlMetrics {
    /// The regular size with the normal metrics: buttons, fields and the switch from the control size table.
    pub fn regular() -> Self {
        let unset = Size::new(0.0, 0.0);
        ControlMetrics {
            size: ControlSize::Regular,
            compact: false,
            button_height: 0.0,
            button_inset: 0.0,
            button_radius: 0.0,
            field_height: 0.0,
            field_inset: 0.0,
            field_radius: 0.0,
            switch: unset,
            switch_thumb: 0.0,
            switch_thumb_inset: 0.0,
            min_target: Size::new(MIN_TARGET, MIN_TARGET),
        }
        .at_size(ControlSize::Regular, false)
    }
}

impl ThemeTokens {
    /// The Cupertino theme for `preferences`: colours from [`ColorTokens::cupertino`], or the opaque
    /// [`ColorTokens::neutral`] when transparency is reduced; regular control metrics; type roles in the theme's font
    /// family. Fails when the theme lacks a colour or a regular control metric.
    pub fn cupertino(theme: &Theme, preferences: ThemePreferences) -> Result<Self> {
        let typography = theme.tokens.typography();
        let colors = if preferences.reduce_transparency {
            ColorTokens::neutral(preferences.appearance)
        } else {
            ColorTokens::cupertino(&theme.tokens, preferences.appearance)?
        };
        Self::regular(
            preferences,
            colors,
            typography.family.to_owned(),
            typography.sizes.regular,
            typography.sizes.small,
        )
    }

    /// The theme without theme data: the [`ColorTokens::neutral`] colours, the same metrics as
    /// [`Self::cupertino`], body and caption at the theme data's sizes and no font family of its own, so the application's
    /// UI font draws the text. Fails only when a regular control metric is missing.
    pub fn neutral(preferences: ThemePreferences) -> Result<Self> {
        Self::regular(
            preferences,
            ColorTokens::neutral(preferences.appearance),
            String::new(),
            NEUTRAL_BODY_SIZE,
            NEUTRAL_CAPTION_SIZE,
        )
    }

    fn regular(
        preferences: ThemePreferences,
        colors: ColorTokens,
        family: String,
        body: f64,
        caption: f64,
    ) -> Result<Self> {
        let role = |size: f64, weight| TypeRole {
            size: size as f32,
            weight,
        };
        Ok(ThemeTokens {
            preferences,
            colors,
            metrics: ControlMetrics::regular(),
            type_scale: TypeScale {
                family,
                body: role(body, REGULAR),
                caption: role(caption, REGULAR),
                section: TypeRole {
                    size: SECTION_SIZE,
                    weight: SEMIBOLD,
                },
                title: TypeRole {
                    size: TITLE_SIZE,
                    weight: SEMIBOLD,
                },
            },
            focus: FocusRing {
                width: FOCUS_RING_WIDTH,
                outset: FOCUS_RING_OUTSET,
                radius_extra: FOCUS_RING_RADIUS_EXTRA,
            },
            motion: Motion {
                reduce: preferences.reduce_motion,
                caret_phase_ms: CARET_PHASE_MS,
            },
        })
    }
}
