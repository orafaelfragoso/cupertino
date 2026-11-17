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
        (Separator, Token("separator")),
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
        (TertiaryText, Token("tertiary-label")),
        // black at 40 %: a sidebar's labels and symbols in a window that is not active
        (InactiveSidebarText, Rgba([0, 0, 0, 102])),
        // black at 3 % over the white window background
        (InactiveTitlebar, Rgba([247, 247, 247, 255])),
        (AlternatingRow, Token("alternating-row")),
    ],
    states: StateColors {
        emphasized_border: Rgba([0, 0, 0, 96]),
        pressed_default_fill: Rgba([0, 100, 225, 255]),
        pressed_destructive_fill: Rgba([214, 47, 50, 255]),
        selected_switch_track: Token("control-accent-blue"),
        pressed_switch_track: Rgba([0, 0, 0, 51]),
        pressed_selected_switch_track: Rgba([0, 100, 225, 255]),
        disabled_switch_track: Rgba([220, 220, 220, 255]),
        disabled_selected_switch_track: Rgba([153, 202, 255, 255]),
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
        (KeyboardFocusIndicator, Token("keyboard-focus-indicator")),
        (Separator, Token("separator")),
        (Header, Token("header")),
        (ToolTip, Token("tool-tip")),
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
        (TertiaryText, Token("tertiary-label")),
        // white at 35 %: a sidebar's labels and symbols in a window that is not active, as grey over the tinted material
        // as the secondary label reads there
        (InactiveSidebarText, Rgba([255, 255, 255, 89])),
        // white at 3 % over the dark window background
        (InactiveTitlebar, Rgba([37, 37, 37, 255])),
        (AlternatingRow, Token("alternating-row")),
    ],
    states: StateColors {
        emphasized_border: Rgba([255, 255, 255, 96]),
        pressed_default_fill: Rgba([0, 89, 209, 255]),
        pressed_destructive_fill: Rgba([219, 57, 59, 255]),
        selected_switch_track: Token("control-accent-blue"),
        pressed_switch_track: Rgba([255, 255, 255, 51]),
        pressed_selected_switch_track: Rgba([0, 89, 209, 255]),
        disabled_switch_track: Rgba([70, 70, 70, 255]),
        disabled_selected_switch_track: Rgba([20, 62, 109, 255]),
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
