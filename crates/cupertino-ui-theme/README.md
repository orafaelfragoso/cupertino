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
  borders and the focus ring to 3:1 against their surface in every enabled state. Disabled text recedes, as the
  token data's does: grey 192 on white, [86,87,91] on dark, at least 1.4:1 against its surface and 2.5:1 apart from
  enabled text. The accent and red fills are darker, so white text on them reaches 4.5:1.
- High contrast is built in and used by both: opaque, text at least 4.5:1 and borders, tracks, thumbs, separators and
  the focus ring at least 3:1 against their surface in every enabled state (tested per resolved pair). Disabled text
  reads at 3:1 there (grey 150 in dark) and still stands 2.5:1 apart from enabled text. The backgrounds keep the
  normal ones: a white window in light, [33,34,39] and a text background of 23 in dark. Hover and press
  turn borders black (light) or white (dark). The dark switch thumb is dark, so it stands out on both tracks, and it
  stands 3:1 against the control border and the panel track, so a slider's knob shows against its ring and its track.

Slider tracks. The empty part of a slider's track has a role of its own (`SliderTrack`), so it is not the switch
track; its disabled colour is the disabled switch track's. It and a panel's level slider track (`PanelTrack`) stand 3:1
or more against the surface they sit on in every palette, light and dark, as is, with Increase Contrast and with
Reduce Transparency, in every enabled state: `SliderTrack` against the window and a text background (a mid grey, 128,
in light and dark; 118 and 150 in high contrast), `PanelTrack` against a module, the group fill over the window
background (118 in light, 145 in dark; 110 and 165 in high contrast). The thumb colour, which is a slider's knob and a
panel slider's fill, stands 3:1 against both tracks. A disabled `PanelTrack` has its own colour, the enabled track faded towards the module (185 in light, 95 in dark; 160 and 110 in high contrast): still apart from the module, fainter than the enabled track. `tests/slider-track.rs` checks the resolved colours.

Menus, popovers and the Dock (`Surface::Menu`, `Surface::Popover`, `Surface::Dock`) are glass (`GLASS_MATERIAL`, `CLEAR_GLASS_MATERIAL`): `ThemeTokens::surface` keeps asking for it with Reduce Transparency and in high contrast, where the compositor draws the glass recipe of those options; the Dock asks for its own `Dock` body with Reduce Transparency. The application grid (`Surface::AppGrid`) keeps its `AppGrid` material too, whose recipe has variants of its own for those options, and a window's background drawn by the compositor (`Surface::WindowBackground`, `WindowBackground`) is opaque, so it is kept as well. The Dock's running dot (`Surface::DockIndicator`) asks for `RunningIndicator`, which lightens what lies behind it, in the dark appearance only (not in high contrast); elsewhere it asks for nothing and the dot paints itself. The other surfaces ask for no material with Reduce Transparency or in high contrast and paint their opaque fill.

`ThemeTokens::on_glass(tone)` is the theme of what a window draws on glass whose face the compositor measured as `tone`
(`None` until it did, and for shapes too large to follow what lies behind them): the neutral colours of `Text`,
`ControlText` and `Header`, `SecondaryText` and `PlaceholderText` take `glass_foreground` (the foreground matrix of the
tone, or the appearance's) at the vibrant levels of `vibrant_level` (1, 0.75, 0.7), or more where the label was
stronger; an opaque grey label of the high-contrast and reduced-transparency themes counts as the appearance's
foreground at the opacity of how far it lies towards it; a disabled label keeps its own strength, and a selected label (drawn on its selection's fill), accents and fills keep their colours. The same holds in
every accessibility mode.

The overlay scroller thumb (`ScrollerThumb`) is translucent in Cupertino light and dark and opaque in the built-in
palettes, at 3:1 or more against the window there. The dialog scrim (`DialogScrim`) is the one colour that is
translucent in every palette, since it dims what it covers: 20 % black in light, 40 % in dark, 60 % in high-contrast
dark.

Selection. The selected fills of every control (a sidebar's, a list's, a table's or an outline's selected row in an
active window, a menu's highlighted item, the default button, an on switch) are the theme's accent token itself, on a
window and on glass: no role lightens, fades, saturates or tints it (`tests/selection-roles.rs`). A sidebar's neutral
selection (`SidebarSelection`) is a grey laid over the sidebar's material, so it lifts each of the material's channels
evenly and keeps its hue: white at 34/255 in dark (each channel of a navy material up by about 29), black at 25/255 in
light. A file browser's places draw their selected row with it in an active window, and every sidebar draws it for its
selected row in a window that is not active, where it resolves fainter: white at 16/255 in dark, black at 13/255 in
light. The built-in palettes flatten it onto the window background, opaque, with an active sidebar's labels at 4.5:1 on it;
an inactive window's secondary labels read at 4:1 or better on its fainter selection (the neutral light one, 224, a
little darker than the opaque sidebar it lies on, so a little under 4.5:1 there, as on the sidebar itself).

Inactive windows. Three roles carry what an inactive window's chrome recedes to: `InactiveSidebarText`, a sidebar's
labels, symbols and headings (white at 89/255 in dark, black at 102/255 in light, so the sidebar's tint still shows
around them), `TertiaryText`, the toolbar's title and glyphs (the tertiary label token), and `InactiveTitlebar`, the
solid band under an inactive window's toolbar row ([38,39,44] dark, 247 light). The built-in palettes give opaque
greys that read at 4:1 or better on the sidebar's inactive selection and 4.5:1 on the band.

Links (`Link`) take the theme's link token in light and dark and reach 4.5:1 against the window and text backgrounds in
the built-in palettes. A page indicator dot for a page not shown (`PageDot`) is text at 20 % in light and dark and an
opaque grey at 3:1 or more in the built-in palettes; the shown page's dot is `Text`.

`ThemeTokens::surface` gives each surface the material to ask the compositor for (`SidebarGlass` for a sidebar, `MenuGlass` for menus,
`DenseGlass` for popovers, `Glass` for the Dock, `LoginWindowControl` for a heads-up display, `AppGrid` for the application grid's panel,
`SwitcherGlass` for the application switcher's panel, `RunningIndicator` for the Dock's running dot, `WindowBackground`
for a window's background and `TextBackground` for the colour behind a form's groups) and the opaque fill to paint
where there is none (the text background colour for `TextBackground`, the window background for the others). With reduced transparency or
in high contrast it asks for no material, except for the surfaces above that keep theirs.

## Material inventory

Every component of the toolkit and of the shell that lies on a material the compositor draws, with the call that asks
for it and what becomes of it with Reduce Transparency and with Increase Contrast. A `Glass::…` call or a `GlassStyle`
is glass of that style (`cupertino-theme`'s `GlassParams`); a `Surface::…` is the theme surface whose material
`ThemeTokens::surface` names; a quoted name is a recipe of `materials.json`.

- **kept**: the component asks for the same material and the compositor draws that material's recipe of the option. For
  glass that is, with Reduce Transparency, a wide blur under a nearly opaque face with nothing bent at the edge, and
  with Increase Contrast the same optics under the contrast face; for a recipe of `materials.json`, its opaque variant
  and the high-contrast catalog's.
- **kept in dark**: kept in the dark appearance, not drawn in the light one.
- **opaque**: the component asks for no material and paints the theme's opaque window colour in the same shape.
- **painted**: the component asks for no material and paints an opaque body of its own.

| Component | Drawn in | Draws | Reduce Transparency | Increase Contrast |
|---|---|---|---|---|
| Menu (pop-up, context and bar menus) | `crates/cupertino-ui-controls/src/menu.rs` | `Surface::Menu` | kept | kept |
| Menu with a pointer | `crates/cupertino-ui-controls/src/menu.rs` | `Glass::menu()` | kept | kept |
| Popover | `crates/cupertino-ui-controls/src/popover.rs` | `Surface::Popover` | kept | kept |
| Popover with a pointer | `crates/cupertino-ui-controls/src/popover.rs` | `Glass::dense()` | kept | kept |
| Alert | `crates/cupertino-ui-controls/src/dialog.rs` | `Glass::dense().radius(ALERT_RADIUS)` | painted | painted |
| Toolbar controls | `crates/cupertino-ui-controls/src/glass.rs` | `Glass::toolbar()` | kept | kept |
| Glass button | `crates/cupertino-ui-controls/src/glass.rs` | `Glass::regular()` | kept | kept |
| A callout's pointer | `crates/cupertino-ui-controls/src/callout.rs` | `cx.glass(self.glass.capsule())` | kept | kept |
| Sidebar panel | `crates/cupertino-ui-controls/src/glass.rs` | `Surface::Sidebar` | opaque | kept |
| One-panel window (About, portal prompts) | `crates/cupertino-ui-controls/src/shell.rs` | `Surface::Sidebar` | opaque | kept |
| Window content pane | `crates/cupertino-ui-controls/src/shell.rs` | `Surface::WindowBackground` | kept | kept |
| Form pane | `crates/cupertino-ui-controls/src/form.rs` | `Surface::WindowBackground` | kept | kept |
| Scrolled navigation bar's band | `crates/cupertino-ui-controls/src/navigation.rs` | `Surface::ToolbarBand` | opaque | opaque |
| Menu bar strip | `crates/cupertino-menubar/src/app.rs` | `set_material("MenuBar")` | kept | kept |
| Menu bar menus and prompt | `crates/cupertino-menubar/src/view.rs` | `Surface::Menu` | kept | kept |
| Dock bar | `crates/cupertino-dock/src/view/bar.rs` | `Surface::DockBar` | kept | kept |
| Dock stack panel (the grid and its pointer, dense glass) | `crates/cupertino-dock/src/view/stack.rs` | `cx.glass(panel_glass(` | kept | kept |
| Dock tile name and menu | `crates/cupertino-dock/src/view/label.rs` | `Surface::Tooltip` | opaque | kept |
| Dock running indicator | `crates/cupertino-dock/src/view/bar.rs` | `Surface::DockIndicator` | kept in dark | opaque |
| Application switcher | `crates/cupertino-dock/src/switcher_kit.rs` | `Surface::Switcher` | opaque | kept |
| Control panel tiles | `crates/cupertino-controlcenter/src/view.rs` | `Glass::module()` | kept | kept |
| Control panel level tracks | `crates/cupertino-controlcenter/src/view.rs` | `Glass::track()` | kept | kept |
| Notification cards (banners and the centre) | `crates/cupertino-notifications/src/view.rs` | `Glass::card()` | kept | kept |
| Launcher panel | `crates/cupertino-launcher/src/view.rs` | `Glass::large_panel()` | kept | kept |
| Volume and brightness overlay | `crates/cupertino-osd/src/view.rs` | `Surface::Hud` | opaque | kept |
| Screenshot toolbar | `crates/cupertino-screenshot/src/view.rs` | `Surface::Hud` | opaque | kept |
| Quick preview panel | `crates/cupertino-files/src/kit/peek.rs` | `Glass::dense()` | kept | kept |
| Log out, restart, shut down, power button, interrupt and consent panels | `crates/cupertino-login/src/view/widgets.rs` | `Glass::regular()` | kept | kept |
| Authentication and keyring prompts | `crates/cupertino-polkit-agent/src/render.rs` | `then_some(PANEL_MATERIAL)` | opaque | opaque |
| Portal prompts (sharing, open with, consent) | `crates/cupertino-portal/src/dialog.rs` | `PANEL_WINDOW_BACKGROUND` | opaque | kept |
| Input-source switcher | `crates/cupertino-server/src/kbd_ui.rs` | `GlassStyle::Regular` | kept | kept |
| Accent popup | `crates/cupertino-server/src/kbd_ui.rs` | `SceneKind::AccentMaterial` | kept | kept |

Solid by design, so not in the table: a sheet, a tooltip, the Force Quit window and the pills of a fanned stack.
`tests/material-inventory.rs` reads this table: every row's file still holds the row's call, a row that draws a theme
surface says what `ThemeTokens::surface` does with each option in light and dark, and every theme surface is in a row.

Approximate: every state colour, the disabled fills and borders, the opaque thumb, the dark white role text and the
dark tooltip in the Cupertino light and dark palettes; all neutral and high-contrast values.

Accepted exceptions: the Cupertino light and dark palettes keep their look below 4.5:1 for text and 3:1 for boundaries
in places (white on the accent and red fills, secondary, placeholder and light header text, control, field and switch
outlines, separators, the light focus ring at 2.49:1, link text on the light window background at 4.45:1). High contrast
and neutral are the accessible palettes. Switch tracks and separators of the neutral palettes keep the Cupertino look
below 3:1. The dark switch track is a fifth of white so it stands apart (1.75:1 or more) from the window, the text
background and the control background behind it, and the switch thumb stands 3:1 against it (tested). Slider tracks
are no exception (above).

`ColorTokens::with_accent` re-derives the selected fill, its pressed shade, the disabled selected switch track and the selected-text background from an accent colour and an optional highlight colour, so one set of tokens serves every accent.


## Theme tokens

`ThemeTokens::cupertino(theme, preferences)` is the Cupertino theme: the colours above for the preferred appearance;
the regular, non-compact row of `cupertino-theme`'s control size table (`ControlMetrics::regular`: buttons and fields
24 pt with a 6 pt corner, button inset 13.5 pt, field inset 6 pt, the switch 54 × 24 pt with a 20 pt thumb 2 pt in);
body (13 pt) and caption (11 pt) sizes and the font family from the theme's type scale; the reduce-motion preference.
`ControlMetrics::at_size(size, compact)` gives the metrics at another size and setting, keeping the minimum target, and
`ControlMetrics::geometry(family)` the geometry of any family at the metrics' size. With reduced transparency, light and
dark take the opaque neutral colours, which also raise text, borders and the focus ring to their contrast ratios; high
contrast is opaque already. Approximate: 24 × 24 pt minimum target, section 12 pt and title 15 pt semibold, a 2 pt
focus ring 2 pt outside the chrome with 3 pt added radius, and 500 ms caret phases. The accent and the selection are
a softer, less saturated blue than the system blue.

`ThemeTokens::neutral(preferences)` is the theme without theme data: the neutral colours on the same metrics, body 13 pt
and caption 11 pt (the theme data's sizes), and no font family, so the application's UI font draws the text.
