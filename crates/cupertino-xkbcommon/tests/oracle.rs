//! The crate against recorded answers of the real libxkbcommon 1.13 (`tests/fixtures`).
//!
//! The fixtures were written by a small program on the C-binding crate: for each layout, every key's keysyms at
//! levels 1 and 2 of every group, the modifier and LED state and the text of probe keys after Shift, Ctrl, Alt, AltGr,
//! Caps Lock, Num Lock and Super sequences (`update_key`), and after `update_mask` calls; plus the keysym helpers.
//! This file regenerates the same text with this crate and compares it with the recording line by line.
//!
//! The keymaps are compiled from the xkeyboard-config data on the machine, and what they contain (the order of the
//! virtual modifiers, key tables) changes between its releases, in libxkbcommon as in this crate. The recordings are
//! only comparable with the release they were made with, `tests/fixtures/xkb-data.txt`; any other data fails every
//! test with a message naming both versions. `XKB_CONFIG_ROOT` points the tests at another copy of the data, which is
//! how CI gets the recorded release on a distribution that ships another one.

use std::fmt::Write;
use std::path::{Path, PathBuf};
use xkbcommon::xkb;

const EVDEV: u32 = 8;
/// Keys whose recording differs for a known reason, left out of the comparison: the port reports an empty key (248)
/// with one empty layout where libxkbcommon 1.13 reports none.
const KNOWN_DIFFERENT_KEYS: [u32; 1] = [248];
const MODS: [&str; 7] = ["Shift", "Lock", "Control", "Mod1", "Mod2", "Mod4", "Mod5"];
/// Probe keys after every step: label and evdev code.
const PROBE: [(&str, u32); 6] = [
    ("a", 30),
    ("q", 16),
    ("apostrophe", 40),
    ("2", 3),
    ("grave", 41),
    ("semicolon", 39),
];

fn syms(s: &[xkb::Keysym]) -> String {
    if s.is_empty() {
        return "-".into();
    }
    s.iter()
        .map(|k| xkb::keysym_get_name(*k))
        .collect::<Vec<_>>()
        .join(",")
}

/// The parts of the data a compile reads, relative to the data root.
const DATA_PARTS: [&str; 5] = ["compat", "keycodes", "symbols", "types", "rules/evdev"];
const DEFAULT_DATA_ROOT: &str = "/usr/share/X11/xkb";
/// Line of `xkb-data.txt` for the release the recordings were made with.
const RECORDED_MARK: &str = "recorded";

fn data_files(path: &Path, out: &mut Vec<PathBuf>) {
    if path.is_dir() {
        let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
            .expect("the data directory is readable")
            .map(|e| e.expect("a directory entry").path())
            .collect();
        entries.sort();
        for entry in entries {
            data_files(&entry, out);
        }
    } else {
        out.push(path.to_path_buf());
    }
}

/// FNV-1a over the relative names and contents of [`DATA_PARTS`]: equal for equal releases, wherever they are installed.
fn data_fingerprint(root: &Path) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut feed = |bytes: &[u8]| {
        for b in bytes {
            hash = (hash ^ u64::from(*b)).wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for part in DATA_PARTS {
        let mut files = Vec::new();
        data_files(&root.join(part), &mut files);
        for file in files {
            feed(
                file.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .as_bytes(),
            );
            feed(&std::fs::read(&file).expect("a data file is readable"));
        }
    }
    format!("{hash:016x}")
}

/// Fails unless the data the keymaps are compiled from is the release the recordings were made with.
fn check_data_version() {
    let root = std::env::var("XKB_CONFIG_ROOT").unwrap_or_else(|_| DEFAULT_DATA_ROOT.into());
    let found = data_fingerprint(Path::new(&root));
    let mut recorded = None;
    let mut known = None;
    for line in include_str!("fixtures/xkb-data.txt").lines() {
        let mut fields = line.split_whitespace();
        let (Some(fingerprint), Some(version)) = (fields.next(), fields.next()) else {
            continue;
        };
        if fields.next() == Some(RECORDED_MARK) {
            recorded = Some(version);
        }
        if fingerprint == found {
            known = Some(version);
        }
    }
    let recorded = recorded.expect("xkb-data.txt marks the recorded release");
    assert_eq!(
        known,
        Some(recorded),
        "the recordings were made with xkeyboard-config {recorded}, but the data in {root} is {} (fingerprint {found}); \
         install xkeyboard-config {recorded} or point XKB_CONFIG_ROOT at a copy of it",
        known.map_or("an unlisted release".to_string(), |v| format!(
            "xkeyboard-config {v}"
        )),
    );
}

fn compile(layout: &str, variant: &str) -> xkb::Keymap {
    check_data_version();
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    xkb::Keymap::new_from_names(
        &context,
        "evdev",
        "pc105",
        layout,
        variant,
        None,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("the layout compiles")
}

fn dump_keys(km: &xkb::Keymap, out: &mut String) {
    writeln!(out, "mods {}", km.mods().collect::<Vec<_>>().join(",")).unwrap();
    writeln!(
        out,
        "layouts {}",
        km.layouts().collect::<Vec<_>>().join(",")
    )
    .unwrap();
    writeln!(out, "leds {}", km.leds().collect::<Vec<_>>().join(",")).unwrap();
    let (min, max) = (km.min_keycode().raw(), km.max_keycode().raw().min(255));
    writeln!(out, "keycodes {min}..={max}").unwrap();
    for kc in min..=max {
        let key = xkb::Keycode::new(kc);
        let Some(name) = km.key_get_name(key) else {
            continue;
        };
        if KNOWN_DIFFERENT_KEYS.contains(&kc) {
            continue;
        }
        let layouts = km.num_layouts_for_key(key);
        let mut line = format!(
            "key {kc} {name} repeats={} layouts={layouts}",
            km.key_repeats(key)
        );
        for layout in 0..layouts {
            let levels = km.num_levels_for_key(key, layout);
            write!(line, " | g{layout} levels={levels}").unwrap();
            for level in 0..levels.min(2) {
                write!(
                    line,
                    " l{level}={}",
                    syms(km.key_get_syms_by_level(key, layout, level))
                )
                .unwrap();
            }
        }
        writeln!(out, "{line}").unwrap();
    }
}

fn state_line(st: &xkb::State, changed: u32, out: &mut String) {
    let mut line = format!(
        "changed={changed:#x} dep={:#x} lat={:#x} lock={:#x} eff={:#x} grp={}",
        st.serialize_mods(xkb::STATE_MODS_DEPRESSED),
        st.serialize_mods(xkb::STATE_MODS_LATCHED),
        st.serialize_mods(xkb::STATE_MODS_LOCKED),
        st.serialize_mods(xkb::STATE_MODS_EFFECTIVE),
        st.serialize_layout(xkb::STATE_LAYOUT_EFFECTIVE),
    );
    let active: Vec<_> = MODS
        .iter()
        .filter(|m| st.mod_name_is_active(**m, xkb::STATE_MODS_EFFECTIVE))
        .copied()
        .collect();
    write!(line, " active=[{}]", active.join(",")).unwrap();
    let leds: Vec<_> = [xkb::LED_NAME_CAPS, xkb::LED_NAME_NUM, xkb::LED_NAME_SCROLL]
        .into_iter()
        .filter(|l| st.led_name_is_active(*l))
        .collect();
    write!(line, " leds=[{}]", leds.join(",")).unwrap();
    for (name, code) in PROBE {
        let kc = xkb::Keycode::new(code + EVDEV);
        write!(
            line,
            " {name}={}/{:?}/one={}",
            syms(st.key_get_syms(kc)),
            st.key_get_utf8(kc),
            xkb::keysym_get_name(st.key_get_one_sym(kc))
        )
        .unwrap();
    }
    writeln!(out, "{line}").unwrap();
}

/// Label and the (evdev code, pressed) events of each recorded sequence.
fn sequences() -> Vec<(&'static str, Vec<(u32, bool)>)> {
    let tap = |code| vec![(code, true), (code, false)];
    let mut caps = tap(58);
    caps.extend([
        (30, true),
        (30, false),
        (42, true),
        (30, true),
        (30, false),
        (42, false),
    ]);
    caps.extend(tap(58));
    caps.extend([(30, true), (30, false)]);
    let mut num = tap(69);
    num.extend([(79, true), (79, false)]);
    num.extend(tap(69));
    num.extend([(79, true), (79, false)]);
    vec![
        (
            "shift",
            vec![(42, true), (30, true), (30, false), (42, false)],
        ),
        (
            "ctrl",
            vec![(29, true), (30, true), (30, false), (29, false)],
        ),
        (
            "alt",
            vec![(56, true), (30, true), (30, false), (56, false)],
        ),
        (
            "altgr",
            vec![
                (100, true),
                (16, true),
                (16, false),
                (40, true),
                (40, false),
                (100, false),
            ],
        ),
        (
            "ctrl-alt",
            vec![
                (29, true),
                (56, true),
                (30, true),
                (30, false),
                (56, false),
                (29, false),
            ],
        ),
        (
            "super",
            vec![(125, true), (30, true), (30, false), (125, false)],
        ),
        ("capslock", caps),
        ("numlock", num),
        (
            "shift-altgr",
            vec![
                (42, true),
                (100, true),
                (16, true),
                (16, false),
                (100, false),
                (42, false),
            ],
        ),
    ]
}

fn dump_sequences(km: &xkb::Keymap, out: &mut String) {
    for (label, events) in sequences() {
        let mut st = xkb::State::new(km);
        writeln!(out, "seq {label}").unwrap();
        write!(out, "  init ").unwrap();
        state_line(&st, 0, out);
        for (code, down) in events {
            let direction = if down {
                xkb::KeyDirection::Down
            } else {
                xkb::KeyDirection::Up
            };
            let changed = st.update_key(xkb::Keycode::new(code + EVDEV), direction);
            write!(out, "  {} {code} ", if down { "down" } else { "up" }).unwrap();
            state_line(&st, changed, out);
        }
    }
    let mut st = xkb::State::new(km);
    writeln!(out, "mask").unwrap();
    for (depressed, latched, locked, group) in [
        (1u32, 0u32, 0u32, 0u32),
        (0, 0, 2, 0),
        (4, 0, 0, 0),
        (0, 0, 0, 1),
        (0x80, 0, 0, 0),
    ] {
        let changed = st.update_mask(depressed, latched, locked, 0, 0, group);
        write!(
            out,
            "  mask {depressed:#x},{latched:#x},{locked:#x},{group} "
        )
        .unwrap();
        state_line(&st, changed, out);
    }
}

fn dump(km: &xkb::Keymap) -> String {
    let mut out = String::new();
    dump_keys(km, &mut out);
    dump_sequences(km, &mut out);
    out
}

/// Fail with the lines that differ (at most 10), which a diff of the whole text would bury.
fn assert_same(actual: &str, expected: &str, what: &str) {
    let differing: Vec<String> = actual
        .lines()
        .zip(expected.lines())
        .enumerate()
        .filter(|(_, (a, e))| a != e)
        .take(10)
        .map(|(n, (a, e))| format!("line {}:\n  ours:   {a}\n  libxkb: {e}", n + 1))
        .collect();
    assert!(
        differing.is_empty(),
        "{what} differs from libxkbcommon:\n{}",
        differing.join("\n")
    );
    assert_eq!(
        actual.lines().count(),
        expected.lines().count(),
        "{what}: line count differs"
    );
}

fn check_layout(layout: &str, variant: &str, recorded: &str) {
    let skipped: Vec<String> = KNOWN_DIFFERENT_KEYS
        .iter()
        .map(|kc| format!("key {kc} "))
        .collect();
    let expected: String = recorded
        .lines()
        .filter(|line| !skipped.iter().any(|prefix| line.starts_with(prefix)))
        .map(|line| format!("{line}\n"))
        .collect();
    let expected = expected.as_str();
    let keymap = compile(layout, variant);
    assert_same(&dump(&keymap), expected, &format!("{layout}({variant})"));

    // the text we serialise parses back into a keymap that answers the same
    let text = keymap.get_as_string(xkb::KEYMAP_FORMAT_TEXT_V1);
    let context = xkb::Context::new(xkb::CONTEXT_NO_FLAGS);
    let reparsed = xkb::Keymap::new_from_string(
        &context,
        text,
        xkb::KEYMAP_FORMAT_TEXT_V1,
        xkb::KEYMAP_COMPILE_NO_FLAGS,
    )
    .expect("the serialised keymap parses");
    assert_same(
        &dump(&reparsed),
        expected,
        &format!("{layout}({variant}) after get_as_string"),
    );
}

#[test]
fn us_matches_libxkbcommon() {
    check_layout("us", "", include_str!("fixtures/us.txt"));
}

#[test]
fn us_intl_matches_libxkbcommon() {
    check_layout("us", "intl", include_str!("fixtures/us-intl.txt"));
}

#[test]
fn de_matches_libxkbcommon() {
    check_layout("de", "", include_str!("fixtures/de.txt"));
}

#[test]
fn br_matches_libxkbcommon() {
    check_layout("br", "", include_str!("fixtures/br.txt"));
}

#[test]
fn keysym_helpers_match_libxkbcommon() {
    const NAMES: [&str; 14] = [
        "a",
        "A",
        "Return",
        "return",
        "KP_1",
        "dead_acute",
        "EuroSign",
        "Greek_alpha",
        "U20AC",
        "0x1000100",
        "Nosuchkeysym",
        "",
        "space",
        "ISO_Level3_Shift",
    ];
    const SYMS: [u32; 18] = [
        0x61,
        0x41,
        0xff0d,
        0xff08,
        0xff09,
        0xff1b,
        0xffff,
        0xffb1,
        0xfe51,
        0x20ac,
        0x7e1,
        0x6c1,
        0x1000100,
        0x110000,
        0x0100_20ac,
        0xffe1,
        0,
        0x2000_0000,
    ];
    const CODES: [u32; 10] = [
        0x61, 0x20, 0xe9, 0x20ac, 0x1f600, 0x110000, 0xd800, 0x7f, 0x1b, 0x3b1,
    ];
    let mut out = String::new();
    for name in NAMES {
        let exact = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
        let icase = xkb::keysym_from_name(name, xkb::KEYSYM_CASE_INSENSITIVE);
        writeln!(
            out,
            "name {name:?} -> {:#x} icase {:#x}",
            exact.raw(),
            icase.raw()
        )
        .unwrap();
    }
    for value in SYMS {
        let sym = xkb::Keysym::new(value);
        writeln!(
            out,
            "sym {value:#x} -> name {:?} utf32 {:#x} utf8 {:?}",
            xkb::keysym_get_name(sym),
            xkb::keysym_to_utf32(sym),
            xkb::keysym_to_utf8(sym)
        )
        .unwrap();
    }
    for code in CODES {
        writeln!(
            out,
            "ucs {code:#x} -> {:#x}",
            xkb::utf32_to_keysym(code).raw()
        )
        .unwrap();
    }
    assert_same(&out, include_str!("fixtures/keysyms.txt"), "keysym helpers");
}
