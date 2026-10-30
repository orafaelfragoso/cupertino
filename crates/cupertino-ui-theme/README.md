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
  turn borders black (light) or white (dark). The dark switch thumb is dark, so it stands out on both tracks.

Approximate: every state colour, the disabled fills and borders, the opaque thumb, the dark white role text and the
dark tooltip in the Cupertino light and dark palettes; all neutral and high-contrast values.

Accepted exceptions: the Cupertino light and dark palettes keep their look below 4.5:1 for text and 3:1 for boundaries
in places (white on the accent and red fills, secondary, placeholder and light header text, control, field and switch
outlines, separators, the light focus ring at 2.49:1). High contrast and neutral are the accessible palettes. Switch
tracks and separators of the neutral palettes keep the Cupertino look below 3:1.

## Theme tokens

`ThemeTokens::cupertino(theme, preferences)` is the Cupertino theme: the colours above for the preferred appearance;
button and field heights (20 pt) and the switch (38 × 22 pt) from the theme's regular control metrics; body (13 pt) and
caption (11 pt) sizes and the font family from its type scale; the reduce-motion preference. Approximate: button inset
10 pt and radius 5 pt, field inset 6 pt and radius 5 pt, switch thumb 18 pt with a 2 pt inset, 24 × 24 pt minimum
target, section 12 pt and title 15 pt semibold, a 2 pt focus ring 2 pt outside the chrome with 3 pt added radius, and
500 ms caret phases.
