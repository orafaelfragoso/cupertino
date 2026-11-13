//! The Cupertino theme's regular control geometry, type roles, focus ring and caret timing. Control geometry comes
//! from `cupertino_theme`'s control size table (not from the theme data), body and caption sizes
//! from the theme's type scale; every other value is a design value of its own (approximate).

use crate::{
    ColorTokens, ControlMetrics, ControlSize, FocusRing, Motion, Size, ThemePreferences,
    ThemeTokens, TypeRole, TypeScale,
};
use cupertino_theme::{Result, Theme};

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
const CARET_PHASE_MS: u32 = 500;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Appearance, StateVariant};

    fn preferences(appearance: Appearance) -> ThemePreferences {
        ThemePreferences {
            appearance,
            reduce_transparency: false,
            reduce_motion: false,
        }
    }

    #[test]
    fn the_metrics_are_the_regular_size_of_the_control_size_table() {
        let theme = Theme::load().unwrap();
        let tokens = ThemeTokens::cupertino(&theme, preferences(Appearance::Light)).unwrap();
        let m = tokens.metrics;
        assert_eq!((m.size, m.compact), (ControlSize::Regular, false));
        assert_eq!(
            (m.button_height, m.field_height, m.switch),
            (24.0, 24.0, Size::new(54.0, 24.0))
        );
        assert_eq!(
            (
                m.button_inset,
                m.button_radius,
                m.field_inset,
                m.field_radius
            ),
            (13.5, 6.0, 6.0, 6.0)
        );
        assert_eq!((m.switch_thumb, m.switch_thumb_inset), (20.0, 2.0));
        assert_eq!(m.min_target, Size::new(24.0, 24.0));
        assert_eq!(
            tokens.focus,
            FocusRing {
                width: 2.0,
                outset: 2.0,
                radius_extra: 3.0
            }
        );
        assert_eq!(tokens.motion.caret_phase_ms, 500);
    }

    #[test]
    fn other_sizes_follow_the_table_and_keep_the_minimum_target() {
        let regular = ControlMetrics::regular();
        let mini = regular.at_size(ControlSize::Mini, true);
        assert_eq!((mini.button_height, mini.button_radius), (13.0, 4.0));
        assert_eq!(mini.switch, Size::new(38.0, 20.0));
        assert_eq!(mini.min_target, regular.min_target);
        let large = regular.at_size(ControlSize::ExtraLarge, false);
        assert_eq!((large.button_height, large.button_radius), (36.0, 18.0));
        assert_eq!(
            large.geometry(crate::ControlFamily::Stepper).width,
            Some(30.0)
        );
        assert_eq!(regular.at_size(ControlSize::Regular, false), regular);
    }

    #[test]
    fn body_and_caption_follow_the_theme_type_scale() {
        let theme = Theme::load().unwrap();
        let tokens = ThemeTokens::cupertino(&theme, preferences(Appearance::Dark)).unwrap();
        let typography = theme.tokens.typography();
        let t = &tokens.type_scale;
        assert_eq!(t.family, typography.family);
        assert_eq!(t.body.size, typography.sizes.regular as f32);
        assert_eq!(t.caption.size, typography.sizes.small as f32);
        assert_eq!((t.body.weight, t.caption.weight), (400, 400));
        assert_eq!((t.body.size, t.caption.size), (13.0, 11.0));
        assert_eq!((t.section.size, t.section.weight), (12.0, 600));
        assert_eq!((t.title.size, t.title.weight), (15.0, 600));

        let json =
            std::fs::read_to_string(cupertino_theme::spec_dir().join("tokens.json")).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["typography"]["sizes"]["regular"] = 14.into();
        let larger = Theme {
            tokens: cupertino_theme::DesignTokens::from_str(&value.to_string()).unwrap(),
            ..theme
        };
        let tokens = ThemeTokens::cupertino(&larger, preferences(Appearance::Dark)).unwrap();
        assert_eq!(tokens.type_scale.body.size, 14.0);
    }

    #[test]
    fn without_theme_data_the_theme_is_the_neutral_colours_on_the_cupertino_metrics_and_sizes() {
        let theme = Theme::load().unwrap();
        for appearance in Appearance::ALL {
            let neutral = ThemeTokens::neutral(preferences(appearance)).unwrap();
            let cupertino = ThemeTokens::cupertino(&theme, preferences(appearance)).unwrap();
            assert_eq!(neutral.colors, ColorTokens::neutral(appearance));
            assert_eq!(neutral.metrics, cupertino.metrics);
            assert_eq!(neutral.type_scale.family, "");
            assert_eq!(
                TypeScale {
                    family: cupertino.type_scale.family.clone(),
                    ..neutral.type_scale
                },
                cupertino.type_scale
            );
            assert_eq!(
                (neutral.focus, neutral.motion),
                (cupertino.focus, cupertino.motion)
            );
        }
    }

    #[test]
    fn reduced_transparency_makes_every_colour_opaque_and_leaves_high_contrast_alone() {
        let theme = Theme::load().unwrap();
        for appearance in Appearance::ALL {
            let reduced = ThemeTokens::cupertino(
                &theme,
                ThemePreferences {
                    reduce_transparency: true,
                    ..preferences(appearance)
                },
            )
            .unwrap();
            assert!(
                reduced
                    .colors
                    .map
                    .iter()
                    .all(|((role, _), c)| *role == crate::ColorRole::DialogScrim || c.a == 1.0),
                "{appearance:?}"
            );
            assert_eq!(reduced.colors, ColorTokens::neutral(appearance));
            let ordinary = ThemeTokens::cupertino(&theme, preferences(appearance)).unwrap();
            assert_eq!(
                ordinary.colors,
                ColorTokens::cupertino(&theme.tokens, appearance).unwrap()
            );
            if appearance.is_high_contrast() {
                assert_eq!(reduced.colors, ordinary.colors);
            } else {
                assert!(ordinary.colors.map.values().any(|c| c.a < 1.0));
            }
        }
    }

    #[test]
    fn the_switcher_lies_on_switcher_glass_in_every_appearance_until_it_goes_opaque() {
        use crate::Surface;
        let theme = Theme::load().unwrap();
        for appearance in Appearance::ALL {
            for reduce_transparency in [false, true] {
                let look = ThemeTokens::cupertino(
                    &theme,
                    ThemePreferences {
                        reduce_transparency,
                        ..preferences(appearance)
                    },
                )
                .unwrap()
                .surface(Surface::Switcher);
                let opaque = reduce_transparency || appearance.is_high_contrast();
                assert_eq!(
                    look.material,
                    (!opaque).then_some(crate::SWITCHER_GLASS_MATERIAL),
                    "{appearance:?} {reduce_transparency}"
                );
            }
        }
    }

    #[test]
    fn surfaces_ask_for_their_material_and_glass_keeps_its_own_with_reduced_transparency_or_high_contrast(
    ) {
        use crate::{Surface, SurfaceLook};
        let theme = Theme::load().unwrap();
        let look = |appearance, reduce_transparency, surface| {
            ThemeTokens::cupertino(
                &theme,
                ThemePreferences {
                    reduce_transparency,
                    ..preferences(appearance)
                },
            )
            .unwrap()
            .surface(surface)
        };
        let window = |appearance| {
            ColorTokens::cupertino(&theme.tokens, appearance)
                .unwrap()
                .get(crate::ColorRole::WindowBackground, StateVariant::NORMAL)
        };
        assert_eq!(
            look(Appearance::Light, false, Surface::Sidebar),
            SurfaceLook {
                material: Some("Sidebar"),
                fill: window(Appearance::Light)
            }
        );
        assert_eq!(
            look(Appearance::Dark, false, Surface::Menu).material,
            Some(crate::DENSE_GLASS_MATERIAL)
        );
        assert_eq!(
            look(Appearance::Dark, false, Surface::Popover).material,
            Some(crate::DENSE_GLASS_MATERIAL)
        );
        // glass takes the recipe of reduced transparency rather than going opaque
        assert_eq!(
            look(Appearance::Light, true, Surface::Menu).material,
            Some(crate::DENSE_GLASS_MATERIAL)
        );
        assert_eq!(
            look(Appearance::Light, true, Surface::Sidebar).material,
            None
        );
        assert_eq!(
            look(Appearance::Dark, false, Surface::Hud),
            SurfaceLook {
                material: Some("LoginWindowControl"),
                fill: window(Appearance::Dark)
            }
        );
        assert_eq!(
            look(Appearance::Light, false, Surface::Switcher).material,
            Some(crate::SWITCHER_GLASS_MATERIAL)
        );
        assert_eq!(
            look(Appearance::Light, true, Surface::Switcher).material,
            None
        );
        let high_contrast = look(Appearance::DarkHighContrast, false, Surface::Popover);
        assert_eq!(high_contrast.material, Some(crate::DENSE_GLASS_MATERIAL));
        assert_eq!(high_contrast.fill, window(Appearance::DarkHighContrast));
        assert_eq!(
            look(Appearance::DarkHighContrast, false, Surface::Sidebar).material,
            None
        );
        for appearance in Appearance::ALL {
            assert_eq!(look(appearance, true, Surface::Menu).fill.unwrap().a, 1.0);
        }
        // the grid keeps its material, whose recipe follows the options
        for (appearance, reduced) in [
            (Appearance::Dark, false),
            (Appearance::Dark, true),
            (Appearance::DarkHighContrast, false),
        ] {
            assert_eq!(
                look(appearance, reduced, Surface::AppGrid).material,
                Some("AppGrid")
            );
        }
        // the Dock is glass, and its own body with Reduce Transparency
        assert_eq!(
            look(Appearance::Dark, false, Surface::Dock).material,
            Some(crate::GLASS_MATERIAL)
        );
        assert_eq!(
            look(Appearance::DarkHighContrast, false, Surface::Dock).material,
            Some(crate::GLASS_MATERIAL)
        );
        assert_eq!(
            look(Appearance::Light, true, Surface::Dock).material,
            Some("Dock")
        );
        // the running indicator lightens the glass in the dark appearance only
        assert_eq!(
            look(Appearance::Dark, false, Surface::DockIndicator).material,
            Some("RunningIndicator")
        );
        assert_eq!(
            look(Appearance::Dark, true, Surface::DockIndicator).material,
            Some("RunningIndicator")
        );
        for appearance in [Appearance::Light, Appearance::DarkHighContrast] {
            assert_eq!(
                look(appearance, false, Surface::DockIndicator).material,
                None
            );
        }
        // a window's background is opaque, so it is the compositor's whatever the options
        for appearance in Appearance::ALL {
            for reduced in [false, true] {
                assert_eq!(
                    look(appearance, reduced, Surface::WindowBackground).material,
                    Some("WindowBackground")
                );
                assert_eq!(
                    look(appearance, reduced, Surface::TextBackground).material,
                    Some("TextBackground")
                );
            }
        }
    }

    #[test]
    fn preferences_choose_the_colours_and_reduce_motion() {
        let theme = Theme::load().unwrap();
        let prefs = ThemePreferences {
            appearance: Appearance::DarkHighContrast,
            reduce_transparency: true,
            reduce_motion: true,
        };
        let tokens = ThemeTokens::cupertino(&theme, prefs).unwrap();
        assert_eq!(tokens.preferences, prefs);
        assert!(tokens.motion.reduce);
        assert_eq!(
            tokens.colors,
            ColorTokens::cupertino(&theme.tokens, Appearance::DarkHighContrast).unwrap()
        );
        assert!(tokens
            .colors
            .get(crate::ColorRole::Text, StateVariant::NORMAL)
            .is_some());
    }
}
