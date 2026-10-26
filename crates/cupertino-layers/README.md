# cupertino-layers

Checks that dependencies between workspace crates follow the layer rules below. A crate's layer is
declared in its manifest, `[package.metadata.cupertino] layer = "<layer>"`; in this tree crates live in
`crates/<crate>`. Layer: `tools`; the only dependency is `serde_json`.

## Usage

```sh
cargo run -p cupertino-layers        # from anywhere inside the repository
```

The binary runs `cargo metadata --no-deps`, collects every dependency between workspace crates (normal, optional
and dev), and prints one line per forbidden edge to stderr. The last line is a summary:

```
24 crates, 63 internal dependencies, 0 pending, 0 problems
```

It exits with failure on any problem; lists it among the pre-finish checks.

## Design

| Layer | May depend on |
|---|---|
| `foundation` | `foundation` |
| `models` | `foundation`, `models` |
| `platform` | `foundation`, `platform` |
| `render` | `foundation`, `platform`, `render` |
| `core`, `shell`, `services`, `apps`, `tools` | `foundation`, `models`, `platform`, `render` |

`PENDING` in `src/lib.rs` lists known violations that a planned phase removes, as `(from, to, reason)`. It is empty.
An entry that no longer matches an edge is an error, so the list only shrinks.

Library: `Layer` (`from_name`, `may_depend_on`), `Edge`, `Problem`, `check(edges, pending)`.

## Tests

```sh
cargo test -p cupertino-layers
```
