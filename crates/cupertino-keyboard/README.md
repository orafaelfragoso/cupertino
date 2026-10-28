# cupertino-keyboard

Keyboard model for the Cupertino compositor. Pure logic, with no Smithay, Wayland, xkbcommon or windowing dependency
(only `serde` / `serde_json` for the layout and accent data files). The compositor feeds events and timestamps
(milliseconds) and acts on the returned values.

## Public API

| Module | Contents |
|---|---|
| `repeat` | `RepeatConfig` (initial delay / interval; from preference units of 1/60 s or from nanoseconds with the filter's clamp), `RepeatInfo` for `wl_keyboard.repeat_info`, `SliderTable`, `KeyRepeater` (single repeating key), `hid_usage_repeats` / `evdev_repeats`, `CapsLockDelay`, caps-lock hold limits |
| `rewrite` | `decide`: the pure table that turns a Super or Alt chord into the Ctrl chord (or Home / End) for a third-party client, given the client class, `text_input_active` and the physical modifiers; never touches Ctrl. `pressed_for` is its inverse outside a text input: the chord the person presses for the chord a client of a class receives (Super+key for a third-party Ctrl chord of a printable key or Return, Super+C / V for a terminal's Shift+Ctrl+C / V; anything else, a first-party client and `classic-pc` unchanged), which the menu bar shows for an application's accelerators; a test runs every chord of a representative set through both |
| `modifiers` | `symbol_selectors` (the profile that maps physical keys to roles is `cupertino_modifiers::Profile`), `FnUsage`, `GlobeTap` (a tap is a release within the threshold with no other key in between: `other_key`) |
| `keys` | `Key`, `NamedKey`, `Chord` with parser (`Ctrl+Alt+Left`); the logical `Mods` and physical `RawMods` are `cupertino-modifiers`' |
| `binding` | `BindingTable::standard` (100 rows, 75 `TextAction`s: emacs-style control chords, alt+arrow word moves, primary+arrow line / document moves, shift = extend selection), chord sequences, user file `~/.config/cupertino/keybindings.conf`, `Resolver` |
| `shortcuts` | `ShortcutSet` over the 80 built-in desktop assignments (app switcher, window cycling, screenshots, launcher, input-source switching, overview, workspaces, window tiling …) with neutral `SystemAction` names, enable / rebind / override text |
| `accent` | `AccentPicker`: opens on the first auto-repeat of a held character key (not with ctrl / primary, not for digits), focus with Tab / arrows, `1..9` direct selection, Return commit, Escape cancel with restore, preview text; `AccentTable` loader (JSON file, nothing embedded); `layout_popup` |
| `switcher` | `Switcher`: usage-ordered source list, HUD open / step / commit-on-modifier-release / cancel, immediate switch without modifiers, `press_menu` (the "next source in the menu" shortcut walks the menu order), HUD constants (0.3 s fade, 24 pt radius, 40 pt rows …) |
| `xkb` | `generate(&Layout, &GenOptions)` → `xkb_symbols`, a complete keymap with per-pattern key types, and an XCompose table reproducing the dead-key state machine; ANSI / ISO / JIS positions; level-3 key choice |
| `keycodes` | layout key number ↔ evdev ↔ XKB name table |
| `slow_keys` | `SlowKeys`: a press is delivered once it has been held for the acceptance delay (default 250 ms), an early release drops both events, another key while one is pending is rejected; `deadline` / `due` for the timer |
| `sticky_keys` | `StickyKeys`: per-modifier state (idle, pressed, latched, locked, chorded), a tap latches for the next key, a second tap locks, a third unlocks; five Shift releases within 30 s toggle the feature when the shortcut is on; the releases owed on toggle and after a latched key |
| `mouse_keys` | `MouseKeys`: keypad directions move the pointer (one point on the press, then a 50 ms ramp after the initial delay, default 1 s, up to the maximum speed, default 11), keypad 5 clicks, 0 holds, `.` releases; five Alt presses within 30 s toggle it when the shortcut is on; `ramp` |

## Usage

```rust
use cupertino_keyboard::*;

// 1. key repeat → wl_keyboard.repeat_info (send after the keymap, also on configuration change)
let cfg = RepeatConfig::from_pref_units(25.0, 6.0);     // preference units of 1/60 s
let RepeatInfo { rate, delay } = cfg.repeat_info();      // wl_keyboard.repeat_info(rate, delay)

// 2. keymap → wl_keyboard.keymap (xkb v1 text), compose table → ~/.XCompose include
let layout = Layout::from_json_file(std::path::Path::new("keyboard/layouts/U_S_0.json"))?;
let out = generate(&layout, &GenOptions { keyboard: Keyboard::Iso, ..Default::default() })?;
// out.keymap → memfd for wl_keyboard.keymap; out.compose → compose table for clients

// 3. shortcuts: translate the physical modifier state, then look the chord up
let profile = cupertino_modifiers::Profile::cupertino();
let mods = profile.translate(RawMods::SUPER | RawMods::SHIFT);          // Primary+Shift
let shortcuts = ShortcutSet::reference_defaults();
if let Some(action) = shortcuts.lookup(&Chord::new(mods, Key::Char('4'))) { /* run the action */ }

// 3b. text editing: standard table + ~/.config/cupertino/keybindings.conf
let (bindings, _errors) = BindingTable::load_default();
let mut resolver = Resolver::new(bindings);
let _ = resolver.feed(Chord::parse("Alt+Left").unwrap());      // Actions([MoveWordLeft])

// 4. input-source HUD (hot key with modifiers: HUD stays until they are released)
let mut sw = Switcher::new();
sw.set_sources(vec!["abc".into(), "fr".into(), "de".into()]);
sw.set_current("abc");
let ev = sw.press(Step::Next, true);      // ShowHud { order, selection }
let ev = sw.modifiers_released();         // Commit { id }
```

## Layouts and generated tables

Decoded layouts are proprietary and are not part of the crate.

- The tests use the small hand-authored `tests/test_layout.json`.
- `tests/decoded_layouts.rs` also runs over local decoded layouts when `CUPERTINO_LAYOUT_DIR` (default
  ) exists. It compiles the result with `xkbcli` when that is installed.
- `src/binding_table.rs` and `src/shortcut_table.rs` are generated from the verified tables by.

## How it works

### Compositor integration

`cupertino-server` integrates this crate (see its README, *Keyboard*):

| Crate item | Used for |
|---|---|
| `RepeatConfig` | `wl_keyboard.repeat_info` (500 ms / 83 ms → 12 Hz, `keyboard.conf` overrides) |
| `ShortcutSet::reference_defaults | merged into the `cupertino-input` shortcut map (the map wins on a taken chord, conflicts are reported) |
| `Switcher` + HUD | input sources (`Ctrl+Space`, `Ctrl+Alt+Space`) switching XKB groups |
| `CapsLockDelay` | a compositor-level 75 ms debounce, or the hold that switches the input source |
| `AccentPicker` | a popup and an in-process `zwp_input_method_v2` bridge to `zwp_text_input_v3` |
| `SlowKeys` → caps delay → `StickyKeys` → `MouseKeys` | the filter chain on device keys |
| `GlobeTap` | the fn key's action |
| `KeyRepeater` | one instance for the repeats the compositor consumes itself (switcher, HUD and popup navigation, the accent trigger) |
| `xkb` generator | `decoded:<path> input sources |

`rewrite::decide` is wired for third-party clients (§10). Not wired: `Resolver` / `BindingTable`, which
the first-party clients use themselves. Clients keep repeating from `repeat_info`.

### Wayland hooks

- **`wl_keyboard`.** Advertise `RepeatConfig::repeat_info. The compositor never generates repeats for clients, and uses
  `KeyRepeater` only for repeats it consumes (shortcuts, nested sessions, the accent trigger). Apply `CapsLockDelay::press_counts(held_ms) before updating the xkb state,
  because libinput has no caps-lock delay. `GlobeTap` handles the globe / fn key, and `FnUsage` selects the action.
- **Keymap.** `generate returns a complete keymap string. The alt keys are the level-3 shift
  (`Level3Keys::{BothAlt, RightAlt, None}), so alt-symbols (for example dead accents, ligatures) are reachable. Layout switching is a new keymap or xkb group.
  Install `out.compose` as the client compose table (for example an include of `~/.XCompose`), because the clients'
  compose handling completes dead keys.
- **`zwp_input_method_v2` / `zwp_text_input_v3`** (accent picker). Run `AccentPicker` in an input-method client that holds
  a keyboard grab, and feed `KeyDown { repeat: true }` for the repeats it generates itself (the grab carries `repeat_info`
  but no repeat events). Show the candidates in an input popup surface positioned with `layout_popup` from the text-input
  cursor rectangle, and the `AccentEvent::Focus.preview` with `set_preedit_string`. Finish with `commit_string`
  (`AccentEvent::Commit`) or clear the preedit (`AccentEvent::Cancel { restore }).
- **Text-editing chords** are toolkit business on Wayland. `Resolver` serves first-party clients; third-party clients get
  the key-event rewrite of `rewrite::decide` (§10).

## Tests

```
cargo test -p cupertino-keyboard
cargo clippy -p cupertino-keyboard --all-targets
```
