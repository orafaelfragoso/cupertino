# cupertino-layers

Checks that the workspace crates follow the layer rules below. A crate's layer is declared in its manifest,
`[package.metadata.cupertino] layer = "<layer>"`; in this tree crates live in `crates/<crate>`. Layer: `tools`.

## Usage

```sh
cargo run -p cupertino-layers                     # the check, from anywhere inside the repository
cargo run -p cupertino-layers -- --graph          # DOT graph of the declared layers
cargo run -p cupertino-layers -- --write-remaining  # write remaining.txt from the problems found
cargo run -p cupertino-layers -- --write-clippy [CRATE...]  # write the clippy.toml of these shell and apps crates
```

The binary runs `cargo metadata`, without the dependencies and then resolved (`--all-features --locked --offline`),
reads the sources of the pure layers and the `clippy.toml` of the `shell` and `apps` crates, and prints one line per problem to stderr. `cupertino dev layers` runs the same
checks from the library and reports them in the `cupertino` command's forms. The check's last line is a summary:

```
91 crates, 390 internal dependencies, 234 listed, 0 problems
```

It exits with failure on any problem. It is one of the checks to run before finishing a change; `cargo xtask ci` runs
it in the profiles that have `layers = true`.

## The rules

All rules are constants in `src/lib.rs`; nothing is read from a configuration file.

`RULES` is the layer table, from the bottom. A crate depends only on the layers its row allows, in normal, build and
dev dependencies. Every crate may also have `testing` crates as dev-dependencies, and only dev-dependencies point at a
`testing` crate.

| Layer | May depend on |
|---|---|
| `foundation` | `foundation` |
| `graphics` | `foundation`, `graphics` |
| `models` | `foundation`, `models` |
| `system` | `foundation`, `system` |
| `platform` | `foundation`, `platform` |
| `protocol` | `foundation`, `protocol` |
| `transport` | `foundation`, `system`, `platform`, `protocol`, `transport` |
| `render` | `foundation`, `graphics`, `system`, `platform`, `render` |
| `ui` | `foundation`, `graphics`, `system`, `protocol`, `transport`, `ui` |
| `shellkit` | `foundation`, `system`, `protocol`, `transport`, `ui` |
| `core` | `foundation`, `graphics`, `models`, `system`, `platform`, `protocol`, `render`, `core` |
| `services` | `foundation`, `models`, `system`, `platform`, `protocol`, `transport`, `services` |
| `shell` | `foundation`, `models`, `system`, `protocol`, `ui`, `shellkit`, `shell` |
| `apps` | `foundation`, `models`, `system`, `protocol`, `ui` |
| `tools` | every layer except `core`, `shell`, `services`, `apps`, `testing` |
| `testing` | every layer |

The further rules, each a check with its own problem kind:

- `reaches_up`: no crate reaches a crate of a higher layer (later in the table, `testing` aside) through a chain of two
  or more dependencies: its own dependencies of every kind, then their normal and build dependencies. So no layer
  reaches up through `tools` or `testing` either.
- `impure_source`: the Rust files of `foundation` and `models` crates (`PURE_LAYERS`), except the crate's own `tests/`
  and `target/`, the build script and its `build/` directory, use none of `IMPURE` (`std::fs`, `std::net`,
  `std::process`, `std::env`, `std::thread`, by path or in a `use std::{…}` group; `std::os::unix::net` counts as
  `std::net`), no `unsafe` and no FFI (`extern` blocks and ABIs, `no_mangle`, `export_name`, `link`, `link_name`).
  Comments and literals do not count, and a `tests` or `target` directory below the crate's root is checked like any
  other. One problem per file and item.
- `starts_thread`: the Rust files in the `src/` of `system` crates (`THREADLESS_LAYERS`) start no threads: no path
  through `std::thread` to `spawn`, `Builder` or `scope`, whole, in a `use std::{…}` group or as `thread::…` after
  `use std::thread`. The line names the call: `std::thread::spawn`, `std::thread::Builder::spawn` or
  `std::thread::scope`. A `system` crate offers blocking functions; its callers run them as tasks. Tests, examples,
  benches and the build script are not checked. One problem per file and call.
- `components_apart`: a `shell` crate never depends on another `shell` crate, except `SHELL_HOST` (`cupertino-shell`)
  on the components; `SEPARATE_PROGRAMS` (`cupertino-login`, `cupertino-prompts`, `cupertino-screenshot`) are not
  components.
- `window_system`: `WINDOW_SYSTEM` lists crates that never reach other crates through any chain of normal
  dependencies, external crates included (a name ending in `*` stands for every crate it starts):
  `cupertino-wayland-client` never reaches `cupertino-protocols` or the wlr, misc and plasma protocol crates;
  `cupertino-app-protocols` never reaches `cupertino-protocols`; `cupertino-kit` and `cupertino-ui-*` never reach
  `cupertino-protocols` or `cupertino-shell-client`; `cupertino-ui-render` never reaches `cupertino-ui-wayland` or
  `cupertino-wayland-client`.
- `facade`: `FACADES`. A `shell` or `apps` crate never depends on the toolkit's crates below `cupertino-kit`
  (`cupertino-ui-*`, `cupertino-theme`, `cupertino-animation`, `cupertino-raster-math`, `cupertino-gpu`,
  `cupertino-wayland-client`, `cupertino-app-protocols`), nor on `cupertino-shell-client` or `cupertino-protocols`,
  which `shell` crates reach through `cupertino-kit-shell`.
- `external_crate`: `EXTERNAL_CRATES` says which layers may depend directly on `smithay` and `smithay-*` (`core`), the
  image crates (`tiny-skia`, `resvg`, `usvg`, `image`, `png`), the Wayland client crates, `xkbcommon`,
  `wayland-server`, `zbus`, and the system-call crates (`libc`, `rustix`, `nix`, `inotify`, `notify`), and in which
  layers they are allowed as dev-dependencies only. Other external crates are not restricted.
- `undeclared_licence`: every crate declares a `license` that is a licence expression, or a `license-file`.
- `disallowed_methods`: every `shell` and `apps` crate holds the generated `clippy.toml` (below).

Test placement (no test code in a crate's `src/`) is checked by the gate of `cargo xtask ci`, not here.

A crate without a declared layer, or with a layer that has no row in `RULES`, stops the check.

## The remaining-work list

Today's crates still declare today's layers and break some of the rules. `remaining.txt` lists each such problem on
one line, `<layer> <rule> <crate> …` (the layer the crate declares), sorted, under a two-line `#` header:

```
shell facade cupertino-dock -> cupertino-ui-wayland normal
platform external_crate cupertino-seat -> smithay normal
apps disallowed_methods cupertino-files clippy.toml
```

A line names only what the problem is about (crates, kind of dependency, file and item; never a line number or a
chain), so it changes only when the problem does. The check fails on a problem the file does not list
(`forbidden_edge`, `reaches_up`, …) and on a line that is no longer a problem (`stale_remaining`). After fixing a problem, or after a change that moves a crate to another layer, write it again:

```sh
cargo run -q -p cupertino-layers -- --write-remaining
```

`ENFORCED` lists the closed layers. Their problems are never excused by the file, `--write-remaining` leaves them out
and fails while they remain, and a line of an enforced layer in the file is a problem (`enforced_remaining`). It holds
`foundation`, `graphics`, `models`, `system`, `protocol`, `transport` and `testing`.

`cargo xtask ci`, in the profiles that run this check, also compares the file with its version at the merge base
(`cargo xtask remaining [<ref>]` runs only that comparison) and reports a line the base does not hold, unless the same
change removes a line with the same `<layer> <rule> <crate>` (a renamed dependency or file) or the line's rule is one
this check at the base does not have yet. During the layered refactor such a line passes; once `GROWTH_FAILS` in
`xtask/src/remaining.rs` is set, it fails, so `--write-remaining` cannot list a new problem of an existing rule.

## The clippy.toml of shell and apps crates

`DISALLOWED_METHODS` (`src/clippy.rs`) is Clippy's `disallowed-methods` for every `shell` and `apps` crate: starting
threads (`std::thread::spawn`, `std::thread::Builder::spawn` and `spawn_scoped`, `std::thread::scope` and
`std::thread::Scope::spawn`), `block_on` of the executors in use, the functions of `std::fs`, the methods of `Path`,
`File` and `DirEntry` that ask the file system (`exists`, `try_exists`, `is_file`, `is_dir`, `is_symlink`, `metadata`,
`symlink_metadata`, `read_dir`, `read_link`, `canonicalize`), and the blocking bus calls of `zbus::blocking`
(connections, proxies, names, signals, the object server), so that a UI thread has no blocking call within reach,
and `cupertino_assets::theme::load`, so that only the toolkit reads the theme data: a program names a colour of it
with `kit::theme::system_color`.
Every path resolves in the workspace's build, so Clippy prints no warning about the list itself. Files
and bus calls go through the toolkit's task API and its async bus API instead. Blocking proxies generated by zbus's
`proxy` macro are not on the list; their constructors and calls go through the listed `zbus::blocking` items.

A `shell` or `apps` crate whose `clippy.toml`, next to its manifest, is missing or differs from the list is a problem
of the rules like the others, with the line `<layer> disallowed_methods <crate> clippy.toml` in the remaining-work
list. Once a crate's banned calls are gone, `--write-clippy <crate>` writes its file and `--write-remaining` drops its
line; without crate names, `--write-clippy` writes the file of every `shell` and `apps` crate. A name that is not a
`shell` or `apps` crate fails and writes nothing.

## The graph

`--graph` prints a DOT graph with one cluster per layer. Comment lines above it give the crate count per layer and every
edge the layer table forbids, with its kind. Build and dev edges are labelled and forbidden edges drawn red.
`graph/layers.dot` is a committed snapshot. The check, without `--graph`, and `cupertino dev layers` fail when it
differs from the workspace, with the problem `stale_graph`; regenerate it after a dependency or layer change:

```sh
cargo run -q -p cupertino-layers -- --graph > crates/cupertino-layers/graph/layers.dot
```

The comparison leaves out the two count lines (`// N crates, M internal dependencies` and `// per layer: ...`), so two
branches that each add a crate do not conflict on them.

## Library

`Layer` (`ALL`, `name`, `from_name`, ordered from the bottom), `Kind`, `RULES`, `allows(rules, from, to)`,
`allows_edge(rules, edge)`, `Edge`, `edges(layers, deps)`, `check(rules, edges, facades)`,
`external_problems(layers, deps, externals)`, `reaches_up(edges)`, `crate_paths(deps, start, targets)`, `matches`,
`is_licence`, `Facade`, `External`, `Problem` (with `kind()`, a stable snake_case name, `from()`, `to()`, `layer()` and
`line()`, its line in the remaining-work file), `dot(…)`, `clippy_toml(methods)`, and the constants above.

The whole check of a checkout is `check_workspace(root)`, or `check_workspace_with(root, cargo, policy)` with the
command that runs Cargo (its environment is the caller's) and a `Policy` (`POLICY` is this repository's). It returns
`Findings`: the workspace root, each crate's layer, the edges, every problem of the rules, the lines of the
remaining-work file, the problems of those lines and of the snapshots, with `unlisted()`, `all_problems()`,
`is_clean()`, `summary()`, `dot()`, `by_layer()`, `remaining_file()`, `write_remaining()` and `write_clippy(crates)`. A Cargo
that cannot start, a `cargo metadata` that fails and a crate without a layer are a `LayersError` (`Cargo`, `Metadata`,
`Manifest`), never a panic. The binary is this call on the working directory.
