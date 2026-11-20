# cupertino-server-input

Compositor input primitives (`core` layer, D25). Depends on no other server crate.

## Modules

| Module | Contents |
|---|---|
| `input` | Key bindings (`KeyBindings`, `Chord`, `Mods`, `Action`), action translation, switcher key handling, and `KeyRouter` |
| `kbd` | Keyboard settings (`KeyboardSettings`, `AccentSettings`), XKB/decoded input sources (`InputSources`, `SourceSpec`), caps-lock debounce (`CapsDebounce`), and HUD sizing |
| `kbd_filters` | Accessibility key filters state (`KeyFilters`), `SuperTap`, `RepeatTarget`, and XKB modifier-mask rewrite helpers (`ModIndices`, `rewrite_masks`, `key_for_sym`) |
| `kbd_ui` | Accent-picker state machine (`AccentUi`, `AccentPopup`, `AccentRoute`, `TextOp`) and overlay layout helpers (`source_code`, `accent_layout`, `has_input_source_bindings`) |
| `input_method` | In-process `zwp_input_method_v2` client bridge thread (`ImBridge`, `ImEvent`) |
| `localed` | `org.freedesktop.locale1` system keyboard layout reader (`SystemLayout`) |
| `overlay_types` | Shared overlay key and shortcut-notice value types (`OverlayKey`, `ShortcutNotice`, `is_modifier_sym`) |
| `hung` | Hung-client ping liveness model (`Liveness`, `Pings`, `PING_INTERVAL`, `PONG_TIMEOUT`) |
| `pointer_constraints` | Pointer constraint state and region confinement math (`PointerConstraints`, `Constrained`, `confine`, `origin_of`) |
| `shortcut_inhibit` | Keyboard shortcut inhibit state and sanitized notice-text helpers (`ShortcutInhibit`, `notice_name`, `notice_text`, `chord_label`) |
| `global_shortcuts` | Portal global-shortcut modifier conversion and reserved-chord calculation (`modifier_bits`, `modifiers_of`, `is_modifier_key`, `reserved`) |
| `events` | Pointer acceleration (`PointerDevice`, `PointerAccelConfig`, `PointerAccelSettings`), mouse identity (`MouseIdentity`, `MouseEntry`), `AxisInput`, `AxisCarry`, `PendingDrag`, and button constants |
