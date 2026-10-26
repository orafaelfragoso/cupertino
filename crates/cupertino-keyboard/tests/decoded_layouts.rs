//! Runs the generator over locally decoded layouts when they are present (they are proprietary and not part of the
//! repository): set `CUPERTINO_LAYOUT_DIR` or keep the layout.

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use cupertino_keyboard::{generate, GenOptions, Keyboard, Layout};

fn dir() -> Option<PathBuf> {
    std::env::var_os("CUPERTINO_LAYOUT_DIR").map(PathBuf::from).filter(|d| d.is_dir())
}

fn compiles(keymap: &str) -> Option<bool> {
    let mut child = Command::new("xkbcli")
        .arg("compile-keymap")
        .arg("--keymap")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(keymap.as_bytes()).ok()?;
    Some(child.wait().ok()?.success())
}

#[test]
fn common_layouts_generate_and_compile() {
    let Some(d) = dir() else { return };
    for file in ["U_S_0.json", "ABC_252.json", "British_2.json", "German_3.json", "French_1.json", "Spanish_8.json", "Dvorak_16300.json", "Colemak_12825.json"] {
        let p = d.join(file);
        if !p.exists() {
            continue;
        }
        let layout = Layout::from_json_file(&p).unwrap();
        for kb in [Keyboard::Ansi, Keyboard::Iso] {
            let out = generate(&layout, &GenOptions { keyboard: kb, ..GenOptions::default() }).unwrap();
            assert!(out.keymap.contains("xkb_symbols"), "{file}");
            assert!(!out.compose.is_empty(), "{file}");
            if let Some(ok) = compiles(&out.keymap) {
                assert!(ok, "{file} {kb:?} does not compile");
            }
        }
    }
}

#[test]
fn dead_acute_composes_in_the_main_layout() {
    let Some(d) = dir() else { return };
    let p = d.join("U_S_0.json");
    if !p.exists() {
        return;
    }
    let out = generate(&Layout::from_json_file(&p).unwrap(), &GenOptions::default()).unwrap();
    assert!(out.compose.contains("<dead_acute> <e> : \"é\"\n"));
    assert!(out.compose.contains("<dead_acute> <E> : \"É\"\n"));
    assert!(out.dead_keysyms.iter().any(|(_, n)| n == "dead_grave"));
}
