# cupertino-kit

CupertinoKit, the Cupertino UI toolkit: the one crate applications depend on. It re-exports the `cupertino-ui-*` crates
as modules and has a `prelude`. MPL-2.0.

`app::run` and `app::options` start on the Cupertino theme from the installed theme data, and on
`ThemeTokens::neutral` when there is none or it is unusable (one line on stderr says why), so an application runs on any
compositor. The `examples` test starts the Counter that way and compares it with `tests/goldens/counter-neutral.txt`.

`ci/capture.sh kit/controls` and `kit/icons` draw the row 167a controls sheet and a grid of system symbols (`--direction rtl` mirrors them)
headless, in light and dark at any scale (`examples/kit_capture.rs`, `crates/cupertino-capture`).
