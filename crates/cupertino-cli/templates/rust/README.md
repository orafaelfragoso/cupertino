# {{name}}

A CupertinoKit application in Rust, started with `cupertino sdk new --lang rust {{name}}`. It depends on the
`cupertino-kit` crate of the Cupertino release v{{version}}, taken from {{kit_origin}}.

Cargo fetches a git tag when it first builds the project. If it reports that it cannot find the tag (a release that is
not published yet) or you work offline, give the dependency in `Cargo.toml` as `{ path = "CHECKOUT/crates/cupertino-kit" }`,
or start the project with `cupertino sdk new --lang rust {{name}} --source CHECKOUT`, where CHECKOUT is a Cupertino
checkout. The keyboard handling links the system's libxkbcommon.

```sh
cargo run
```

It opens a window on the session's Wayland display: a number and two buttons that change it.

`{{name}}.desktop` is the application's desktop entry; its file name is the app id of the window. Install it with the
program to show the application in the desktop's launcher.
