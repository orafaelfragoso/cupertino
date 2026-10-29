//! Built-in palettes that need no theme data, pack or desktop service. Every colour is opaque, so nothing depends on
//! what is drawn underneath. The neutral palettes keep the look of the Cupertino light and dark themes where it reaches
//! the contrast they require; the high-contrast palettes are used by both themes.

use crate::palette::{Palette, StateColors};
use crate::ColorRole::*;

const BLACK: [u8; 4] = [0, 0, 0, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

const fn gray(level: u8) -> [u8; 4] {
    [level, level, level, 255]
}

/// Neutral light: the Cupertino light colours, translucent ones flattened onto the surface they are drawn on, with text
/// raised to 4.5:1 and control borders and the focus ring to 3:1 against what they are drawn on.
pub(crate) const LIGHT: Palette<[u8; 4]> = Palette {
    normal: [
        (WindowBackground, gray(236)),
        (Text, BLACK),
        (SecondaryText, gray(105)),
        (PlaceholderText, gray(118)),
        (ControlFill, WHITE),
        (ControlBorder, gray(130)),
        (ControlText, gray(36)),
        (DisabledControlFill, gray(242)),
        (DisabledControlBorder, gray(212)),
        (DisabledControlText, gray(104)),
        (DefaultButtonFill, [0, 105, 230, 255]),
        (DefaultButtonText, WHITE),
        (DestructiveFill, [215, 45, 38, 255]),
        (DestructiveText, WHITE),
        (SwitchTrack, gray(213)),
        (SwitchThumb, WHITE),
        (TextBackground, WHITE),
        (TextFieldBorder, gray(130)),
        (SelectedTextBackground, [166, 209, 255, 255]),
        (SelectedText, BLACK),
        (SelectedControlFill, [0, 105, 230, 255]),
        (SelectedControlText, WHITE),
        (SecondarySelectedControlFill, gray(220)),
        (SecondarySelectedControlText, gray(36)),
        (KeyboardFocusIndicator, [0, 105, 230, 255]),
        (Separator, gray(230)),
        (Header, gray(105)),
        (ToolTip, gray(254)),
        (ToolTipText, gray(36)),
    ],
    states: StateColors {
        emphasized_border: gray(100),
        pressed_default_fill: [0, 85, 190, 255],
        pressed_destructive_fill: [175, 28, 22, 255],
        selected_switch_track: [0, 122, 255, 255],
        pressed_switch_track: gray(189),
        pressed_selected_switch_track: [0, 104, 218, 255],
        disabled_switch_track: gray(220),
        disabled_selected_switch_track: [166, 198, 232, 255],
        disabled_switch_thumb: gray(248),
    },
};

/// Neutral dark: the Cupertino dark colours, translucent ones flattened onto the surface they are drawn on, with the
/// same ratios as [`LIGHT`].
pub(crate) const DARK: Palette<[u8; 4]> = Palette {
    normal: [
        (WindowBackground, gray(50)),
        (Text, WHITE),
        (SecondaryText, gray(163)),
        (PlaceholderText, gray(140)),
        (ControlFill, gray(101)),
        (ControlBorder, gray(139)),
        (ControlText, WHITE),
        (DisabledControlFill, gray(62)),
        (DisabledControlBorder, gray(86)),
        (DisabledControlText, gray(170)),
        (DefaultButtonFill, [0, 105, 230, 255]),
        (DefaultButtonText, WHITE),
        (DestructiveFill, [200, 40, 32, 255]),
        (DestructiveText, WHITE),
        (SwitchTrack, gray(70)),
        (SwitchThumb, WHITE),
        (TextBackground, gray(30)),
        (TextFieldBorder, gray(128)),
        (SelectedTextBackground, [20, 62, 109, 255]),
        (SelectedText, WHITE),
        (SelectedControlFill, [0, 105, 230, 255]),
        (SelectedControlText, WHITE),
        (SecondarySelectedControlFill, gray(70)),
        (SecondarySelectedControlText, gray(224)),
        (KeyboardFocusIndicator, [37, 173, 255, 255]),
        (Separator, gray(26)),
        (Header, WHITE),
        (ToolTip, gray(40)),
        (ToolTipText, gray(224)),
    ],
    states: StateColors {
        emphasized_border: gray(159),
        pressed_default_fill: [0, 85, 190, 255],
        pressed_destructive_fill: [165, 28, 22, 255],
        selected_switch_track: [0, 122, 255, 255],
        pressed_switch_track: gray(91),
        pressed_selected_switch_track: [0, 104, 218, 255],
        disabled_switch_track: gray(70),
        disabled_selected_switch_track: [54, 82, 112, 255],
        disabled_switch_thumb: gray(180),
    },
};

/// High-contrast light: text at least 4.5:1 and borders, tracks, thumbs and the focus ring at least 3:1 against what
/// they are drawn on; a hovered or pressed border turns black.
pub(crate) const HIGH_CONTRAST_LIGHT: Palette<[u8; 4]> = Palette {
    normal: [
        (WindowBackground, gray(246)),
        (Text, BLACK),
        (SecondaryText, gray(60)),
        (PlaceholderText, gray(107)),
        (ControlFill, WHITE),
        (ControlBorder, gray(118)),
        (ControlText, BLACK),
        (DisabledControlFill, gray(235)),
        (DisabledControlBorder, gray(128)),
        (DisabledControlText, gray(90)),
        (DefaultButtonFill, [0, 64, 221, 255]),
        (DefaultButtonText, WHITE),
        (DestructiveFill, [215, 0, 21, 255]),
        (DestructiveText, WHITE),
        (SwitchTrack, gray(118)),
        (SwitchThumb, WHITE),
        (TextBackground, WHITE),
        (TextFieldBorder, gray(118)),
        (SelectedTextBackground, [179, 201, 247, 255]),
        (SelectedText, BLACK),
        (SelectedControlFill, [0, 64, 221, 255]),
        (SelectedControlText, WHITE),
        (SecondarySelectedControlFill, gray(220)),
        (SecondarySelectedControlText, BLACK),
        (KeyboardFocusIndicator, [0, 64, 128, 255]),
        (Separator, gray(128)),
        (Header, gray(60)),
        (ToolTip, WHITE),
        (ToolTipText, BLACK),
    ],
    states: StateColors {
        emphasized_border: BLACK,
        pressed_default_fill: [0, 48, 170, 255],
        pressed_destructive_fill: [175, 0, 17, 255],
        selected_switch_track: [0, 64, 221, 255],
        pressed_switch_track: gray(80),
        pressed_selected_switch_track: [0, 48, 170, 255],
        disabled_switch_track: gray(200),
        disabled_selected_switch_track: [150, 170, 215, 255],
        disabled_switch_thumb: gray(248),
    },
};

/// High-contrast dark: the same ratios as [`HIGH_CONTRAST_LIGHT`]; the switch thumb is dark so it stands out on both
/// the off and the on track, and a hovered or pressed border turns white.
pub(crate) const HIGH_CONTRAST_DARK: Palette<[u8; 4]> = Palette {
    normal: [
        (WindowBackground, gray(50)),
        (Text, WHITE),
        (SecondaryText, gray(210)),
        (PlaceholderText, gray(170)),
        (ControlFill, gray(24)),
        (ControlBorder, gray(160)),
        (ControlText, WHITE),
        (DisabledControlFill, gray(62)),
        (DisabledControlBorder, gray(140)),
        (DisabledControlText, gray(170)),
        (DefaultButtonFill, [23, 116, 218, 255]),
        (DefaultButtonText, WHITE),
        (DestructiveFill, [190, 30, 30, 255]),
        (DestructiveText, WHITE),
        (SwitchTrack, gray(150)),
        (SwitchThumb, gray(24)),
        (TextBackground, gray(32)),
        (TextFieldBorder, gray(160)),
        (SelectedTextBackground, [26, 74, 140, 255]),
        (SelectedText, WHITE),
        (SelectedControlFill, [23, 116, 218, 255]),
        (SelectedControlText, WHITE),
        (SecondarySelectedControlFill, gray(90)),
        (SecondarySelectedControlText, WHITE),
        (KeyboardFocusIndicator, [102, 178, 255, 255]),
        (Separator, gray(140)),
        (Header, WHITE),
        (ToolTip, gray(24)),
        (ToolTipText, WHITE),
    ],
    states: StateColors {
        emphasized_border: WHITE,
        pressed_default_fill: [12, 92, 180, 255],
        pressed_destructive_fill: [160, 20, 20, 255],
        selected_switch_track: [80, 166, 255, 255],
        pressed_switch_track: gray(190),
        pressed_selected_switch_track: [120, 186, 255, 255],
        disabled_switch_track: gray(90),
        disabled_selected_switch_track: [54, 82, 112, 255],
        disabled_switch_thumb: gray(170),
    },
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        adapter, Appearance, Color, ColorRole, ColorTokens, InteractionVariant, StateVariant,
    };

    fn luminance(c: Color) -> f32 {
        let lin = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
    }

    fn contrast(a: Color, b: Color) -> f32 {
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    #[test]
    fn built_in_palettes_are_opaque_in_every_state() {
        for appearance in Appearance::ALL {
            let colors = ColorTokens::neutral(appearance);
            for ((role, state), c) in &colors.map {
                assert_eq!(c.a, 1.0, "{appearance:?} {role:?} {state:?}");
            }
        }
    }

    #[test]
    fn built_in_text_and_boundaries_reach_their_ratios() {
        use ColorRole::*;
        const TEXT: f32 = 4.5;
        const BOUNDARY: f32 = 3.0;
        let text_pairs = [
            (Text, WindowBackground),
            (Text, TextBackground),
            (SecondaryText, WindowBackground),
            (PlaceholderText, TextBackground),
            (ControlText, ControlFill),
            (DefaultButtonText, DefaultButtonFill),
            (DestructiveText, DestructiveFill),
            (SelectedText, SelectedTextBackground),
            (SelectedControlText, SelectedControlFill),
            (SecondarySelectedControlText, SecondarySelectedControlFill),
            (Header, WindowBackground),
            (ToolTipText, ToolTip),
        ];
        let boundary_pairs = [
            (ControlBorder, WindowBackground),
            (TextFieldBorder, WindowBackground),
            (TextFieldBorder, TextBackground),
            (KeyboardFocusIndicator, WindowBackground),
            (KeyboardFocusIndicator, TextBackground),
        ];
        let high_contrast_boundary_pairs = [
            (SwitchTrack, WindowBackground),
            (SwitchThumb, SwitchTrack),
            (Separator, WindowBackground),
        ];
        for appearance in Appearance::ALL {
            let colors = ColorTokens::neutral(appearance);
            for state in StateVariant::ALL {
                let disabled = state.interaction == InteractionVariant::Disabled;
                let get = |role| colors.get(role, state).unwrap();
                let check = |(fg, bg): (ColorRole, ColorRole), min: f32| {
                    let ratio = contrast(get(fg), get(bg));
                    assert!(
                        ratio >= min,
                        "{appearance:?} {state:?} {fg:?} on {bg:?}: {ratio:.2} < {min}"
                    );
                };
                for pair in text_pairs {
                    check(pair, TEXT);
                }
                for pair in boundary_pairs {
                    if !disabled {
                        check(pair, BOUNDARY);
                    }
                }
                for pair in high_contrast_boundary_pairs {
                    if !disabled && appearance.is_high_contrast() {
                        check(pair, BOUNDARY);
                    }
                }
            }
        }
    }

    #[test]
    fn neutral_keeps_the_cupertino_colours_it_does_not_raise_for_contrast() {
        use ColorRole::*;
        let tokens = cupertino_theme::Theme::load().unwrap().tokens;
        let over = |c: Color, under: Color| {
            let [r, g, b, a] = c.to_rgba8();
            let u = under.to_rgba8();
            let mix = |v: u8, w: u8| {
                ((f32::from(v) * f32::from(a) + f32::from(w) * f32::from(255 - a)) / 255.0).round()
                    as u8
            };
            Color::from_rgba8([mix(r, u[0]), mix(g, u[1]), mix(b, u[2]), 255])
        };
        for (appearance, neutral) in [(Appearance::Light, LIGHT), (Appearance::Dark, DARK)] {
            let cupertino = adapter::palette(&tokens, appearance).unwrap();
            let neutral = neutral.colors();
            let normal = |role: ColorRole| cupertino.normal[role as usize].1;
            let control = over(normal(ControlFill), normal(WindowBackground));
            let surface = |role| match role {
                TextBackground | TextFieldBorder | PlaceholderText | SelectedTextBackground => {
                    normal(TextBackground)
                }
                ControlBorder => control,
                DisabledControlBorder => normal(DisabledControlFill),
                _ => normal(WindowBackground),
            };
            let raised_for_contrast = [
                SecondaryText,
                PlaceholderText,
                Header,
                ControlText,
                ControlBorder,
                TextFieldBorder,
                DisabledControlText,
                DefaultButtonFill,
                SelectedControlFill,
                DestructiveFill,
                KeyboardFocusIndicator,
            ];
            for (role, c) in cupertino.normal {
                if raised_for_contrast.contains(&role) {
                    continue;
                }
                assert_eq!(
                    neutral.normal[role as usize].1,
                    over(c, surface(role)),
                    "{appearance:?} {role:?}"
                );
            }
            let (c, n) = (cupertino.states, neutral.states);
            assert_eq!(
                n.pressed_switch_track,
                over(c.pressed_switch_track, normal(WindowBackground))
            );
        }
    }
}
