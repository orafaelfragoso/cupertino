//! The Cupertino theme's light and dark palettes, named by design token. A token resolves in its own appearance only;
//! one that is missing is an error, never a colour borrowed from another appearance.

use crate::palette::{Palette, StateColors};
use crate::ColorRole::*;
use crate::{Appearance, Color, DesignTokens};
use cupertino_theme::{Error, Result};

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Source {
    Token(&'static str),
    Rgba([u8; 4]),
}

use Source::{Rgba, Token};

const WHITE: Source = Rgba([255, 255, 255, 255]);

pub(crate) const LIGHT: Palette<Source> = Palette {
    normal: [
        (WindowBackground, Token("window-background")),
        (Text, Token("text")),
        (SecondaryText, Token("secondary-label")),
        (PlaceholderText, Token("placeholder-text")),
        (ControlFill, Token("control")),
        (ControlBorder, Token("container-border")),
        (ControlText, Token("control-text")),
        (DisabledControlFill, Rgba([242, 242, 242, 255])),
        (DisabledControlBorder, Rgba([0, 0, 0, 32])),
        (DisabledControlText, Token("disabled-control-text")),
        (DefaultButtonFill, Token("control-accent-blue")),
        (DefaultButtonText, Token("alternate-selected-control-text")),
        (DestructiveFill, Token("system-red")),
        (DestructiveText, Token("alternate-selected-control-text")),
        (SwitchTrack, Token("system-fill")),
        (SwitchThumb, WHITE),
        (TextBackground, Token("text-background")),
        (TextFieldBorder, Token("container-border")),
        (SelectedTextBackground, Token("selected-text-background")),
        (SelectedText, Token("text")),
        (SelectedControlFill, Token("control-accent-blue")),
        (
            SelectedControlText,
            Token("alternate-selected-control-text"),
        ),
        (
            SecondarySelectedControlFill,
            Token("secondary-selected-control"),
        ),
        (SecondarySelectedControlText, Token("control-text")),
        (KeyboardFocusIndicator, Token("keyboard-focus-indicator")),
        (Separator, Token("grid")),
        (Header, Token("header")),
        (ToolTip, Token("tool-tip")),
        (ToolTipText, Token("tool-tip-text")),
    ],
    states: StateColors {
        emphasized_border: Rgba([0, 0, 0, 96]),
        pressed_default_fill: Rgba([0, 104, 218, 255]),
        pressed_destructive_fill: Rgba([215, 45, 38, 255]),
        selected_switch_track: Token("control-accent-blue"),
        pressed_switch_track: Rgba([0, 0, 0, 51]),
        pressed_selected_switch_track: Rgba([0, 104, 218, 255]),
        disabled_switch_track: Rgba([220, 220, 220, 255]),
        disabled_selected_switch_track: Rgba([166, 198, 232, 255]),
        disabled_switch_thumb: Rgba([248, 248, 248, 255]),
    },
};

pub(crate) const DARK: Palette<Source> = Palette {
    normal: [
        (WindowBackground, Token("window-background")),
        (Text, Token("text")),
        (SecondaryText, Token("secondary-label")),
        (PlaceholderText, Token("placeholder-text")),
        (ControlFill, Token("control")),
        (ControlBorder, Token("container-border")),
        (ControlText, Token("control-text")),
        (DisabledControlFill, Rgba([62, 62, 62, 255])),
        (DisabledControlBorder, Rgba([255, 255, 255, 32])),
        (DisabledControlText, Token("disabled-control-text")),
        (DefaultButtonFill, Token("control-accent-blue")),
        (DefaultButtonText, WHITE),
        (DestructiveFill, Token("system-red")),
        (DestructiveText, WHITE),
        (SwitchTrack, Token("system-fill")),
        (SwitchThumb, WHITE),
        (TextBackground, Token("text-background")),
        (TextFieldBorder, Token("container-border")),
        (SelectedTextBackground, Token("selected-text-background")),
        (SelectedText, Token("text")),
        (SelectedControlFill, Token("control-accent-blue")),
        (SelectedControlText, WHITE),
        (
            SecondarySelectedControlFill,
            Token("secondary-selected-control"),
        ),
        (SecondarySelectedControlText, Token("control-text")),
        // The focus indicator token's colour at full alpha.
        (KeyboardFocusIndicator, Rgba([37, 173, 255, 255])),
        (Separator, Token("grid")),
        (Header, Token("header")),
        (ToolTip, Rgba([40, 40, 40, 255])),
        (ToolTipText, Token("tool-tip-text")),
    ],
    states: StateColors {
        emphasized_border: Rgba([255, 255, 255, 96]),
        pressed_default_fill: Rgba([0, 104, 218, 255]),
        pressed_destructive_fill: Rgba([220, 55, 46, 255]),
        selected_switch_track: Token("control-accent-blue"),
        pressed_switch_track: Rgba([255, 255, 255, 51]),
        pressed_selected_switch_track: Rgba([0, 104, 218, 255]),
        disabled_switch_track: Rgba([70, 70, 70, 255]),
        disabled_selected_switch_track: Rgba([54, 82, 112, 255]),
        disabled_switch_thumb: Rgba([180, 180, 180, 255]),
    },
};

pub(crate) fn palette(tokens: &DesignTokens, appearance: Appearance) -> Result<Palette<Color>> {
    let spec = if appearance.is_dark() { DARK } else { LIGHT };
    let table = tokens.table(appearance);
    spec.try_map(|source| match source {
        Token(name) => table
            .get(name)
            .ok_or_else(|| Error::NotFound(format!("colour {name} in {}", appearance.as_str()))),
        Rgba(c) => Ok(Color::from_rgba8(c)),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ColorTokens, StateVariant};

    fn tokens() -> DesignTokens {
        cupertino_theme::Theme::load().unwrap().tokens
    }

    #[test]
    fn roles_take_their_tokens_from_the_named_appearance() {
        let tokens = tokens();
        for appearance in [Appearance::Light, Appearance::Dark] {
            let colors = ColorTokens::cupertino(&tokens, appearance).unwrap();
            let token = |name| tokens.table(appearance).get(name).unwrap();
            let get = |role| colors.get(role, StateVariant::NORMAL).unwrap();
            assert_eq!(get(ControlFill), token("control"));
            assert_eq!(get(ControlBorder), token("container-border"));
            assert_eq!(get(DefaultButtonFill), token("control-accent-blue"));
        }
    }

    #[test]
    fn the_dark_focus_ring_is_opaque() {
        let colors = ColorTokens::cupertino(&tokens(), Appearance::Dark).unwrap();
        let ring = colors
            .get(KeyboardFocusIndicator, StateVariant::NORMAL)
            .unwrap();
        assert_eq!(ring.to_rgba8(), [37, 173, 255, 255]);
    }

    #[test]
    fn a_missing_token_is_an_error_not_a_fallback() {
        let json =
            std::fs::read_to_string(cupertino_theme::spec_dir().join("tokens.json")).unwrap();
        let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
        value["appearances"]["dark"]
            .as_object_mut()
            .unwrap()
            .remove("control");
        let tokens = DesignTokens::from_str(&value.to_string()).unwrap();
        let err = ColorTokens::cupertino(&tokens, Appearance::Dark).unwrap_err();
        assert!(err.to_string().contains("control in dark"), "{err}");
        assert!(ColorTokens::cupertino(&tokens, Appearance::Light).is_ok());
    }
}
