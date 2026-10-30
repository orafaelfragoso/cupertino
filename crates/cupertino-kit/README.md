# cupertino-kit

CupertinoKit, the Cupertino UI toolkit: the one crate applications depend on. It re-exports the `cupertino-ui-*` crates
as modules and has a `prelude`. MPL-2.0.

`ci/capture.sh kit/controls` and `kit/icons` draw the row 167a controls sheet and a grid of system symbols (`--direction rtl` mirrors them)
headless, in light and dark at any scale (`examples/kit_capture.rs`, `crates/cupertino-capture`).
