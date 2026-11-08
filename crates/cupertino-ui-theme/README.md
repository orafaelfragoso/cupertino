# cupertino-ui-theme

CupertinoKit theme tokens: semantic colours per `ColorRole` and `StateVariant`, control metrics, type scale, focus
ring and motion. MPL-2.0. Reuses `Color`, `Appearance`, `ControlSize` and `DesignTokens` from `cupertino-theme`.

## Colours

A theme is one colour per role plus a few state colours (emphasized border, pressed button fills, switch tracks and
the disabled thumb). Resolution expands them into every role in all 16 states, so `ColorTokens::get` never falls back
to a normal colour for a selected or disabled control. Selection applies first, then hover or press; disabled overrides
both.

- `ColorTokens::cupertino(tokens, appearance)`: light and dark name tokens of `data/theme/tokens.json` in that
  appearance only. A missing token is an error, never a colour from another appearance.
- `ColorTokens::neutral(appearance)`: built in, for any compositor, with no theme data, pack or desktop service. Every
  colour is opaque. Light and dark are the Cupertino colours flattened onto the surface they are drawn on (a test keeps
  them in step with the token data), except where that falls short: text is raised to 4.5:1 and control and field
  borders and the focus ring to 3:1 against their surface in every enabled state, with disabled text readable at
  4.5:1 too. The accent and red fills are darker, so white text on them reaches 4.5:1.
- High contrast is built in and used by both: opaque, text at least 4.5:1 and borders, tracks, thumbs, separators and
  the focus ring at least 3:1 against their surface in every enabled state (tested per resolved pair). Hover and press
  turn borders black (light) or white (dark). The dark switch thumb is dark, so it stands out on both tracks, and it
  stands 3:1 against the control border and the panel track, so a slider's knob shows against its ring and its track.

Slider tracks. The empty part of a slider's track has a role of its own (`SliderTrack`), so it is not the switch
track; its disabled colour is the disabled switch track's. It and a panel's level slider track (`PanelTrack`) stand 3:1
or more against the surface they sit on in every palette, light and dark, as is, with Increase Contrast and with
Reduce Transparency, in every enabled state: `SliderTrack` against the window and a text background (a mid grey, 128,
in light and dark; 118 and 150 in high contrast), `PanelTrack` against a module, the group fill over the window
background (118 in light, 145 in dark; 110 and 165 in high contrast). The thumb colour, which is a slider's knob and a
panel slider's fill, stands 3:1 against both tracks. `tests/slider-track.rs` checks the resolved colours.

The overlay scroller thumb (`ScrollerThumb`) is translucent in Cupertino light and dark and opaque in the built-in
palettes, at 3:1 or more against the window there. The dialog scrim (`DialogScrim`) is the one colour that is
translucent in every palette, since it dims what it covers: 20 % black in light, 40 % in dark, 60 % in high-contrast
dark.

Links (`Link`) take the theme's link token in light and dark and reach 4.5:1 against the window and text backgrounds in
the built-in palettes. A page indicator dot for a page not shown (`PageDot`) is text at 20 % in light and dark and an
opaque grey at 3:1 or more in the built-in palettes; the shown page's dot is `Text`.

`ThemeTokens::surface` gives a sidebar, menu or popover the material to ask the compositor for (`Sidebar`, or `Menu`
for menus and popovers, `LoginWindowControl` for a heads-up display, `AppGrid` for the full-screen application grid and
`Switcher` for the application switcher's panel) and the opaque window background to paint where there is none. With
reduced transparency or in high contrast it asks for no material.

Approximate: every state colour, the disabled fills and borders, the opaque thumb, the dark white role text and the
dark tooltip in the Cupertino light and dark palettes; all neutral and high-contrast values.

Accepted exceptions: the Cupertino light and dark palettes keep their look below 4.5:1 for text and 3:1 for boundaries
in places (white on the accent and red fills, secondary, placeholder and light header text, control, field and switch
outlines, separators, the light focus ring at 2.49:1, link text on the light window background at 4.45:1). High contrast
and neutral are the accessible palettes. Switch tracks and separators of the neutral palettes keep the Cupertino look
below 3:1. The dark switch track is a fifth of white so it stands apart (1.75:1 or more) from the window, the text
background and the control background behind it, and the switch thumb stands 3:1 against it (tested). Slider tracks
are no exception (above).

## Theme tokens

`ThemeTokens::cupertino(theme, preferences)` is the Cupertino theme: the colours above for the preferred appearance;
button and field heights (20 pt) and the switch (38 × 22 pt) from the regular control metrics table of `cupertino-theme`
(not from the theme data); body (13 pt) and caption (11 pt) sizes and the font family from the theme's type scale; the
reduce-motion preference. With reduced transparency, light and dark take the opaque neutral colours, which also raise
text, borders and the focus ring to their contrast ratios; high contrast is opaque already. Approximate: button inset 10
pt and radius 5 pt, field inset 6 pt and radius 5 pt, switch thumb 18 pt with a 2 pt inset, 24 × 24 pt minimum target,
section 12 pt and title 15 pt semibold, a 2 pt focus ring 2 pt outside the chrome with 3 pt added radius, and 500 ms
caret phases.

`ThemeTokens::neutral(preferences)` is the theme without theme data: the neutral colours on the same metrics, body 13 pt
and caption 11 pt (the theme data's sizes), and no font family, so the application's UI font draws the text.
