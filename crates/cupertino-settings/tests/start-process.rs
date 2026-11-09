//! ST-20 for the process: missing assets never stop the start. Without any asset pack, or with a pack that lacks a
//! required symbol, the binary reports what is missing and goes on until it needs the display it does not have. It runs
//! on a private session bus (it claims `org.cupertino.Settings1` first).
#![cfg(feature = "app")]

mod common;

use std::process::{Command, Output};

/// Run the binary on `bus` with no display, no user asset pack and no host icon theme (`XDG_DATA_HOME`, `XDG_DATA_DIRS` and `HOME` are `data`) and the asset pack `assets`
/// alone (`None`: no pack at all, not even one installed on the machine).
fn start(
    bus: &common::PrivateBus,
    data: &std::path::Path,
    assets: Option<&std::path::Path>,
) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cupertino-settings"));
    command
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("XDG_DATA_HOME", data)
        .env("XDG_DATA_DIRS", data)
        .env("HOME", data)
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("CUPERTINO_ASSET_DIR")
        .env(
            "CUPERTINO_ASSET_SEARCH_PATH",
            assets.unwrap_or(std::path::Path::new("")),
        );
    command.output().expect("the binary runs")
}

#[test]
fn without_any_asset_pack_the_start_reaches_the_display() {
    let Some(bus) = common::bus() else { return };
    let data = tempfile::tempdir().unwrap();
    let out = start(&bus, data.path(), None);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("no asset pack provides the icon")
            && stderr.contains("cannot connect to the display server"),
        "{stderr}"
    );
}

#[test]
fn a_pack_without_a_required_symbol_names_it_and_the_start_goes_on() {
    let Some(bus) = common::bus() else { return };
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("manifest.json"),
        r#"{"version": 2, "name": "no-symbols"}"#,
    )
    .unwrap();
    let data = tempfile::tempdir().unwrap();
    let out = start(&bus, data.path(), Some(dir.path()));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("`symbol.trackpad`")
            && stderr.contains("cannot connect to the display server"),
        "{stderr}"
    );
}
