# cupertino-modifiers

Modifier keys: the logical roles a shortcut is written in, the physical keys that carry them, and the profile that says
which key carries which role. MPL-2.0, foundation layer, no dependencies.

A shortcut is written once, in logical modifiers (`Primary+Q`); the keyboard reports physical keys; the active `Profile`
translates. Every profile is a permutation, so each role has exactly one key and every chord can be typed under every
profile.

Used by `cupertino-keyboard` (its chords, the key rewrite and the physical state it reads), `cupertino-text-field`, the
settings window (its chords), the server and the menu bar, and `cupertino-menu-model` and `cupertino-ui-menus` (the
chords of the menus). The profile that maps physical keys to roles is `Profile` here: the system shortcuts of `classic-pc`
are a table of their own and do not depend on it.

## Public API

| Item | Contents |
|---|---|
| `Modifier`, `Mods` | the logical roles (Shift, Primary, Ctrl, Alt, Fn, in the order a chord shows them) and a set of them. `Mods` prints as `Primary+Ctrl+Alt+Shift+Fn` (`none` when empty) |
| `Physical`, `RawMods` | the physical keys (Shift, Ctrl, Alt, Super, Fn) and a set of them that are down |
| `Profile` | `cupertino()`, `classic_pc()`, `by_name`; `translate(RawMods) -> Mods`, `role(Physical)`, `physical_for(Modifier)`, `display(Modifier)` (the key's name in a menu), `alt_selects_symbols` |
| `ModifierMap` | what a caller that only formats or parses chords needs of a profile: the key of a role and the role of a key. `Profile` implements it |

## Profiles

| Physical key | `cupertino` | `classic-pc` |
|---|---|---|
| Super | Primary | Ctrl |
| Ctrl | Ctrl | Primary |
| Alt | Alt | Alt |
| Shift | Shift | Shift |
| Fn | Fn | Fn |

`classic-pc` is a full swap of Ctrl and Super: a chord that needs Primary and Ctrl together (Lock, Full screen, the
tiles) is typed with Ctrl and Super.

## Tests

```
cargo test -p cupertino-modifiers
```
