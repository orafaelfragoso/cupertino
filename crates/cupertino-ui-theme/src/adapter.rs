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
        (ScrollerThumb, Rgba([0, 0, 0, 128])),
        (DialogScrim, Rgba([0, 0, 0, 51])),
        (Link, Token("link")),
        (PageDot, Rgba([0, 0, 0, 51])),
        (GroupFill, Token("secondary-system-fill")),
        (SwitchThumbGlyph, Rgba([115, 115, 115, 255])),
        // A panel's level slider track: a mid grey, 3:1 or more against the module behind it.
        (PanelTrack, Rgba([118, 118, 118, 255])),
        (PanelTrackBorder, Rgba([0, 0, 0, 26])),
        (QuaternaryLabel, Token("quaternary-label")),
        // The empty part of a slider's track: a mid grey, 3:1 or more against the window behind it.
        (SliderTrack, Rgba([128, 128, 128, 255])),
        // A sidebar's neutral selection: black at about a tenth over the sidebar's material.
        (SidebarSelection, Rgba([0, 0, 0, 25])),
    ],
    states: StateColors {
        emphasized_border: Rgba([0, 0, 0, 96]),
        pressed_default_fill: Rgba([42, 100, 214, 255]),
        pressed_destructive_fill: Rgba([215, 45, 38, 255]),
        selected_switch_track: Token("control-accent-blue"),
        pressed_switch_track: Rgba([0, 0, 0, 51]),
        pressed_selected_switch_track: Rgba([42, 100, 214, 255]),
        disabled_switch_track: Rgba([220, 220, 220, 255]),
        disabled_selected_switch_track: Rgba([166, 198, 232, 255]),
        disabled_switch_thumb: Rgba([248, 248, 248, 255]),
        disabled_panel_track: Rgba([185, 185, 185, 255]),
        // about half the active selection, as in the dark appearance
        inactive_sidebar_selection: Rgba([0, 0, 0, 13]),
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
        // An off switch's track: white at 20 %, so the track stands apart from the window and from a text background
        // behind it.
        (SwitchTrack, Rgba([255, 255, 255, 51])),
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
        (ScrollerThumb, Rgba([255, 255, 255, 128])),
        (DialogScrim, Rgba([0, 0, 0, 102])),
        (Link, Token("link")),
        (PageDot, Rgba([255, 255, 255, 51])),
        (GroupFill, Token("secondary-system-fill")),
        (SwitchThumbGlyph, Rgba([115, 115, 115, 255])),
        // A panel's level slider track: a light grey, 3:1 or more against the module behind it and from the white fill.
        (PanelTrack, Rgba([145, 145, 145, 255])),
        (PanelTrackBorder, Rgba([0, 0, 0, 0])),
        (QuaternaryLabel, Token("quaternary-label")),
        // The empty part of a slider's track: a mid grey, 3:1 or more against the window and the text background.
        (SliderTrack, Rgba([128, 128, 128, 255])),
        // A sidebar's neutral selection: white at about an eighth over the sidebar's material, which lifts each
        // channel of the material evenly, so its tint shows through.
        (SidebarSelection, Rgba([255, 255, 255, 34])),
    ],
    states: StateColors {
        emphasized_border: Rgba([255, 255, 255, 96]),
        pressed_default_fill: Rgba([42, 100, 214, 255]),
        pressed_destructive_fill: Rgba([220, 55, 46, 255]),
        selected_switch_track: Token("control-accent-blue"),
        pressed_switch_track: Rgba([255, 255, 255, 51]),
        pressed_selected_switch_track: Rgba([42, 100, 214, 255]),
        disabled_switch_track: Rgba([70, 70, 70, 255]),
        disabled_selected_switch_track: Rgba([54, 82, 112, 255]),
        disabled_switch_thumb: Rgba([180, 180, 180, 255]),
        disabled_panel_track: Rgba([95, 95, 95, 255]),
        // about half the active one
        inactive_sidebar_selection: Rgba([255, 255, 255, 16]),
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
    fn links_take_the_link_token_and_page_dots_a_faint_text_colour() {
        let tokens = tokens();
        for (appearance, dot) in [
            (Appearance::Light, [0, 0, 0, 51]),
            (Appearance::Dark, [255, 255, 255, 51]),
        ] {
            let colors = ColorTokens::cupertino(&tokens, appearance).unwrap();
            let get = |role| colors.get(role, StateVariant::NORMAL).unwrap();
            assert_eq!(get(Link), tokens.table(appearance).get("link").unwrap());
            assert_eq!(get(PageDot).to_rgba8(), dot);
        }
    }

    #[test]
    fn the_scroller_thumb_and_the_dialog_scrim_are_translucent_in_light_and_dark() {
        let tokens = tokens();
        for (appearance, thumb, scrim) in [
            (Appearance::Light, [0, 0, 0, 128], [0, 0, 0, 51]),
            (Appearance::Dark, [255, 255, 255, 128], [0, 0, 0, 102]),
        ] {
            let colors = ColorTokens::cupertino(&tokens, appearance).unwrap();
            let get = |role| colors.get(role, StateVariant::NORMAL).unwrap().to_rgba8();
            assert_eq!(get(ScrollerThumb), thumb);
            assert_eq!(get(DialogScrim), scrim);
        }
    }

    /// The WCAG contrast ratio of two opaque colours.
    fn contrast(a: Color, b: Color) -> f32 {
        let luminance = |c: Color| {
            let lin = |v: f32| {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b)
        };
        let (x, y) = (luminance(a), luminance(b));
        (x.max(y) + 0.05) / (x.min(y) + 0.05)
    }

    /// `top` drawn over the opaque `bottom`.
    fn over(top: Color, bottom: Color) -> Color {
        let mix = |t: f32, b: f32| t * top.a + b * (1.0 - top.a);
        Color {
            r: mix(top.r, bottom.r),
            g: mix(top.g, bottom.g),
            b: mix(top.b, bottom.b),
            a: 1.0,
        }
    }

    #[test]
    fn the_dark_switch_track_stands_apart_from_the_backgrounds_behind_it() {
        /// The least contrast of an off switch's track with the window and with a text background, in dark.
        const DARK_TRACK: f32 = 1.75;
        let tokens = tokens();
        let colors = ColorTokens::cupertino(&tokens, Appearance::Dark).unwrap();
        let get = |role| colors.get(role, StateVariant::NORMAL).unwrap();
        let control_background = tokens
            .table(Appearance::Dark)
            .get("control-background")
            .unwrap();
        for background in [
            get(WindowBackground),
            get(TextBackground),
            control_background,
        ] {
            let background = over(background, Color::from_rgba8([0, 0, 0, 255]));
            let track = over(get(SwitchTrack), background);
            let ratio = contrast(track, background);
            assert!(
                ratio >= DARK_TRACK,
                "track {track:?} on {background:?}: {ratio:.2} < {DARK_TRACK}"
            );
            // the knob and the filled part stay far apart from the empty track
            assert!(contrast(get(SwitchThumb), track) >= 3.0);
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
