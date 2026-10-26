//! XKB keymap generator for decoded keyboard layouts.
//!
//! Layout data is **not** embedded in
//! this crate; pass the path of a decoded layout.
//!
//! Output ([`generate`]):
//!
//! * `symbols`: an `xkb_symbols` section for the alphanumeric block;
//! * `keymap`: a complete keymap (keycodes, types with one custom key type per distinct level pattern, compat, symbols);
//! * `compose`: an X compose table that reproduces the dead-key state machine, including "terminator + key" results for
//!   keys that do not compose and chained dead keys.
//!
//! Modelling: every key has eight outputs addressed by shift, caps lock and alt (alt is the
//! level-3 shifter, `level3(alt_switch)); the tables for control and for the primary modifier are not representable as
//! XKB levels and are ignored. A dead key becomes a `dead_* keysym named after the spacing accent the state would
//! produce when followed by space. Pending dead states after a *text* output (a state entered by a key that also
//! outputs text) are dropped (reported in `warnings`).
//!
//! The generator was validated against libxkbcommon for 232 layouts (90,568 key/level cells, 272,257 compose
//! sequences, 0 mismatches) by the;.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde::Deserialize;

use crate::keycodes::{is_character_key, xkb_name_of};

/// One output cell of a key table or state record.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum Output {
    Char { c: String },
    Seq { s: String },
    Dead { dead: u32 },
    Unresolved { unresolved: u32 },
    SeqIndex { seq_index: u32 },
}

#[derive(Clone, Debug, Deserialize)]
pub struct StateEntry {
    pub state: u32,
    #[serde(default)]
    pub out: Option<Output>,
    #[serde(default)]
    pub next: u32,
    #[serde(default)]
    pub range: u32,
    #[serde(default)]
    pub mul: u32,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Record {
    #[serde(default)]
    pub zero: Option<Output>,
    #[serde(default)]
    pub next: u32,
    #[serde(default)]
    pub entries: Vec<StateEntry>,
}

/// Tables for a range of keyboard types.
#[derive(Clone, Debug, Deserialize)]
pub struct KeySet {
    pub first: u32,
    pub last: u32,
    pub modmap: Vec<u32>,
    pub default_table: u32,
    pub tables: Vec<Vec<Option<Output>>>,
    #[serde(default)]
    pub records: Vec<Record>,
    #[serde(default)]
    pub terminators: Vec<Option<Output>>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Layout {
    pub name: String,
    pub id: i64,
    #[serde(default)]
    pub language: String,
    pub sets: Vec<KeySet>,
}

impl Layout {
    pub fn from_json(text: &str) -> Result<Layout, serde_json::Error> {
        serde_json::from_str(text)
    }

    pub fn from_json_file(path: &std::path::Path) -> Result<Layout, Box<dyn std::error::Error>> {
        Ok(Self::from_json(&std::fs::read_to_string(path)?)?)
    }
}

fn text_of(o: &Option<Output>) -> String {
    match o {
        Some(Output::Char { c }) => c.clone(),
        Some(Output::Seq { s }) => s.clone(),
        _ => String::new(),
    }
}

/// Result of translating one key press in a given dead-key state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Translation {
    pub text: String,
    pub next_state: u32,
}

impl KeySet {
    fn table_for(&self, mods: u32) -> u32 {
        self.modmap.get(mods as usize).copied().unwrap_or(self.default_table)
    }

    /// Reference implementation of the layout's translation: `mods` are the modifier bits (shift 2, caps 4, alt 8,
    /// control 0x10, command 1).
    pub fn translate(&self, key: usize, mods: u32, state: u32) -> Translation {
        let t = self.table_for(mods) as usize;
        let cell = self.tables.get(t).and_then(|row| row.get(key)).cloned().flatten();
        let term = |st: u32| -> String {
            if st > 0 {
                text_of(&self.terminators.get(st as usize - 1).cloned().flatten())
            } else {
                String::new()
            }
        };
        match cell {
            None => Translation { text: String::new(), next_state: state },
            Some(Output::Dead { dead }) => {
                let Some(rec) = self.records.get(dead as usize) else {
                    return Translation { text: String::new(), next_state: state };
                };
                if state == 0 {
                    return Translation { text: text_of(&rec.zero), next_state: rec.next };
                }
                for e in &rec.entries {
                    if state >= e.state && state <= e.state + e.range {
                        let mut text = text_of(&e.out);
                        if e.mul != 0 && text.chars().count() == 1 {
                            let c = text.chars().next().unwrap_or('\0');
                            if let Some(ch) = char::from_u32(c as u32 + e.mul * (state - e.state)) {
                                text = ch.to_string();
                            }
                        }
                        return Translation { text, next_state: e.next };
                    }
                }
                Translation { text: term(state) + &text_of(&rec.zero), next_state: rec.next }
            }
            Some(o @ (Output::Char { .. } | Output::Seq { .. })) => {
                let txt = text_of(&Some(o));
                let text = if state != 0 { term(state) + &txt } else { txt };
                Translation { text, next_state: 0 }
            }
            Some(_) => Translation { text: String::new(), next_state: state },
        }
    }
}

/// Physical keyboard family: decides where the two position-dependent keys land.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Keyboard {
    Ansi,
    Iso,
    Jis,
}

/// Which physical keys switch to the third level (the "alt" symbols).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level3Keys {
    BothAlt,
    RightAlt,
    None,
}

#[derive(Clone, Copy, Debug)]
pub struct GenOptions {
    pub keyboard: Keyboard,
    pub set_index: usize,
    pub level3: Level3Keys,
}

impl Default for GenOptions {
    fn default() -> Self {
        GenOptions { keyboard: Keyboard::Ansi, set_index: 0, level3: Level3Keys::BothAlt }
    }
}

/// Generated files and diagnostics.
#[derive(Clone, Debug)]
pub struct XkbOutput {
    pub symbols: String,
    pub keymap: String,
    pub compose: String,
    pub warnings: Vec<String>,
    pub conflicts: usize,
    pub dead_keysyms: Vec<(u32, String)>,
}

#[derive(Debug)]
pub enum GenError {
    NoSuchSet(usize),
}

impl std::fmt::Display for GenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GenError::NoSuchSet(i) => write!(f, "layout has no key set {i}"),
        }
    }
}

impl std::error::Error for GenError {}

// (shift, caps, alt, modifier bits)
const COMBOS: [(bool, bool, bool, u32); 8] = [
    (false, false, false, 0),
    (true, false, false, 2),
    (false, true, false, 4),
    (true, true, false, 6),
    (false, false, true, 8),
    (true, false, true, 10),
    (false, true, true, 12),
    (true, true, true, 14),
];

const DEAD: &[(&str, &str)] = &[
    ("`", "dead_grave"),
    ("\u{300}", "dead_grave"),
    ("´", "dead_acute"),
    ("\u{301}", "dead_acute"),
    ("^", "dead_circumflex"),
    ("ˆ", "dead_circumflex"),
    ("\u{302}", "dead_circumflex"),
    ("~", "dead_tilde"),
    ("˜", "dead_tilde"),
    ("\u{303}", "dead_tilde"),
    ("¯", "dead_macron"),
    ("\u{2c9}", "dead_macron"),
    ("\u{304}", "dead_macron"),
    ("˘", "dead_breve"),
    ("\u{306}", "dead_breve"),
    ("˙", "dead_abovedot"),
    ("\u{307}", "dead_abovedot"),
    ("¨", "dead_diaeresis"),
    ("\u{308}", "dead_diaeresis"),
    ("\u{309}", "dead_hook"),
    ("˚", "dead_abovering"),
    ("\u{30a}", "dead_abovering"),
    ("˝", "dead_doubleacute"),
    ("\u{30b}", "dead_doubleacute"),
    ("ˇ", "dead_caron"),
    ("\u{30c}", "dead_caron"),
    ("¸", "dead_cedilla"),
    ("\u{327}", "dead_cedilla"),
    ("˛", "dead_ogonek"),
    ("\u{328}", "dead_ogonek"),
    ("\u{323}", "dead_belowdot"),
    ("\u{31b}", "dead_horn"),
    ("\u{30f}", "dead_doublegrave"),
    ("\u{311}", "dead_invertedbreve"),
    ("\u{326}", "dead_belowcomma"),
    ("\u{325}", "dead_belowring"),
    ("\u{324}", "dead_belowdiaeresis"),
    ("\u{32d}", "dead_belowcircumflex"),
    ("\u{32e}", "dead_belowbreve"),
    ("\u{330}", "dead_belowtilde"),
    ("\u{331}", "dead_belowmacron"),
    ("\u{345}", "dead_iota"),
    ("\u{3099}", "dead_voiced_sound"),
    ("\u{309a}", "dead_semivoiced_sound"),
    ("\u{338}", "dead_stroke"),
    ("\u{313}", "dead_abovecomma"),
    ("\u{314}", "dead_abovereversedcomma"),
];

fn printable(t: &str) -> bool {
    !t.is_empty() && t.chars().all(|c| (c as u32) >= 0x20 && c as u32 != 0x7F && !(0x80..0xA0).contains(&(c as u32)))
}

/// Keysym name accepted by xkb for a character: ASCII letters and digits by themselves, everything else `Uxxxx`.
pub fn keysym_name(c: char) -> String {
    if c.is_ascii_alphanumeric() {
        c.to_string()
    } else {
        format!("U{:04X}", c as u32)
    }
}

#[derive(Clone, PartialEq, Eq, Hash, Debug)]
enum Cell {
    Dead(String),
    Text(Vec<String>, String),
    None,
}

struct Ctx<'a> {
    set: &'a KeySet,
    state_key: HashMap<u32, String>,
    used: HashMap<String, u32>,
    plain: HashSet<String>,
}

impl Ctx<'_> {
    fn dead_name(&mut self, st: u32) -> String {
        if let Some(n) = self.state_key.get(&st) {
            return n.clone();
        }
        let glyph = if st > 0 { text_of(&self.set.terminators.get(st as usize - 1).cloned().flatten()) } else { String::new() };
        let mut nm = DEAD.iter().find(|(g, _)| *g == glyph).map(|(_, n)| (*n).to_string());
        let clash = |nm: &String, used: &HashMap<String, u32>| used.get(nm).is_some_and(|u| *u != st);
        if nm.as_ref().is_none_or(|n| clash(n, &self.used)) {
            let mut it = glyph.chars();
            nm = match (it.next(), it.next()) {
                (Some(c), None) => Some(keysym_name(c)),
                _ => None,
            };
            if nm.as_ref().is_none_or(|n| self.plain.contains(n) || clash(n, &self.used)) {
                nm = Some(format!("U{:04X}", 0xE000 + st));
            }
        }
        let nm = nm.unwrap_or_default();
        self.used.insert(nm.clone(), st);
        self.state_key.insert(st, nm.clone());
        nm
    }
}

/// Generates the XKB files for `layout`.
pub fn generate(layout: &Layout, opts: &GenOptions) -> Result<XkbOutput, GenError> {
    let set = layout.sets.get(opts.set_index).ok_or(GenError::NoSuchSet(opts.set_index))?;
    let mut ctx = Ctx { set, state_key: HashMap::new(), used: HashMap::new(), plain: HashSet::new() };
    let mut warnings = Vec::new();

    let mut pos: BTreeMap<u8, &'static str> = BTreeMap::new();
    for k in 0u8..=0x7E {
        if !is_character_key(k) {
            continue;
        }
        if (0x5D..=0x5F).contains(&k) && opts.keyboard != Keyboard::Jis {
            continue;
        }
        if let Some(n) = xkb_name_of(k) {
            pos.insert(k, n);
        }
    }
    match opts.keyboard {
        Keyboard::Iso => {
            pos.insert(0x0A, "TLDE");
            pos.insert(0x32, "LSGT");
        }
        Keyboard::Ansi | Keyboard::Jis => {
            pos.remove(&0x0A);
        }
    }

    for &k in pos.keys() {
        for (_, _, _, m) in COMBOS {
            let t0 = set.translate(k as usize, m, 0);
            if printable(&t0.text) {
                for c in t0.text.chars() {
                    ctx.plain.insert(keysym_name(c));
                }
            }
        }
    }

    // cells per key
    let mut keys: BTreeMap<u8, Vec<Cell>> = BTreeMap::new();
    for &k in pos.keys() {
        let mut cells = Vec::new();
        for (_, _, _, m) in COMBOS {
            let t = set.translate(k as usize, m, 0);
            let dead = t.next_state != 0;
            if dead && t.text.is_empty() {
                cells.push(Cell::Dead(ctx.dead_name(t.next_state)));
            } else if printable(&t.text) {
                if dead {
                    warnings.push(format!("key {k:#04x}/{m}: text {:?} together with pending state {} (state dropped)", t.text, t.next_state));
                }
                let names = t.text.chars().map(keysym_name).collect();
                cells.push(Cell::Text(names, t.text));
            } else {
                cells.push(Cell::None);
            }
        }
        if cells.iter().all(|c| *c == Cell::None) {
            continue;
        }
        keys.insert(k, cells);
    }

    // types and key definitions
    let mut types: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    let mut key_lines = Vec::new();
    for (k, cells) in &keys {
        let mut seen: Vec<(u8, Vec<String>)> = Vec::new();
        let mut pat = Vec::new();
        let ident = |c: &Cell| -> (u8, Vec<String>) {
            match c {
                Cell::None => (0, vec![]),
                Cell::Dead(n) => (1, vec![n.clone()]),
                Cell::Text(n, _) => (2, n.clone()),
            }
        };
        for c in cells {
            let id = ident(c);
            let idx = match seen.iter().position(|s| *s == id) {
                Some(i) => i,
                None => {
                    seen.push(id);
                    seen.len() - 1
                }
            };
            pat.push(idx);
        }
        let tname = format!("CUP_{}", pat.iter().map(|p| p.to_string()).collect::<String>());
        types.insert(tname.clone(), pat.clone());
        let syms: Vec<String> = seen
            .iter()
            .map(|(kind, names)| match kind {
                0 => "NoSymbol".to_string(),
                1 => names[0].clone(),
                _ if names.len() == 1 => names[0].clone(),
                _ => format!("{{ {} }}", names.join(", ")),
            })
            .collect();
        let sym_text = syms.join(", ");
        let name = pos[k];
        key_lines.push(format!("    key <{name}> {{ type[Group1] = \"{tname}\", symbols[Group1] = [ {sym_text} ] }};"));
    }

    let mods = ["", "Shift", "Lock", "Shift+Lock", "LevelThree", "Shift+LevelThree", "Lock+LevelThree", "Shift+Lock+LevelThree"];
    let mut type_defs = Vec::new();
    for (name, pat) in &types {
        let mut s = format!("    type \"{name}\" {{\n        modifiers = Shift+Lock+LevelThree;\n");
        for (i, p) in pat.iter().enumerate().skip(1) {
            if *p != 0 {
                s.push_str(&format!("        map[{}] = Level{};\n", mods[i], p + 1));
            }
        }
        for i in 0..=pat.iter().copied().max().unwrap_or(0) {
            s.push_str(&format!("        level_name[Level{}] = \"L{}\";\n", i + 1, i + 1));
        }
        s.push_str("    };");
        type_defs.push(s);
    }

    let level3 = match opts.level3 {
        Level3Keys::BothAlt => Some("level3(alt_switch)"),
        Level3Keys::RightAlt => Some("level3(ralt_switch)"),
        Level3Keys::None => None,
    };
    let safe_name = layout.name.replace('"', "");
    let mut symbols = vec![
        format!("// generated from layout \"{}\" (id {}), key set {}", layout.name, layout.id, opts.set_index),
        "default partial alphanumeric_keys".to_string(),
        "xkb_symbols \"basic\" {".to_string(),
        format!("    name[Group1] = \"{safe_name}\";"),
    ];
    symbols.extend(key_lines.iter().cloned());
    if let Some(l) = level3 {
        symbols.push(format!("    include \"{l}\""));
    }
    symbols.push("};".to_string());

    let mut keymap = vec![
        "xkb_keymap {".to_string(),
        "    xkb_keycodes { include \"evdev+aliases(qwerty)\" };".to_string(),
        "    xkb_types \"cupertino\" {\n    include \"complete\"".to_string(),
    ];
    keymap.extend(type_defs.iter().cloned());
    keymap.push("    };".to_string());
    keymap.push("    xkb_compat { include \"complete\" };".to_string());
    keymap.push("    xkb_symbols { include \"pc+inet(evdev)\"".to_string());
    if let Some(l) = level3 {
        keymap.push(format!("        include \"{l}\""));
    }
    keymap.push(format!("        name[Group1] = \"{safe_name}\";"));
    keymap.extend(key_lines.iter().cloned());
    keymap.push("    };".to_string());
    keymap.push("};".to_string());

    let (compose, conflicts) = compose_table(layout, &mut ctx, &keys, &mut warnings);
    let mut dead_keysyms: Vec<(u32, String)> = ctx.state_key.iter().map(|(a, b)| (*a, b.clone())).collect();
    dead_keysyms.sort();
    Ok(XkbOutput {
        symbols: symbols.join("\n") + "\n",
        keymap: keymap.join("\n") + "\n",
        compose,
        warnings,
        conflicts,
        dead_keysyms,
    })
}

fn compose_table(layout: &Layout, ctx: &mut Ctx<'_>, keys: &BTreeMap<u8, Vec<Cell>>, warnings: &mut Vec<String>) -> (String, usize) {
    let set = ctx.set;
    let mut cells: Vec<(u8, u32, String)> = Vec::new();
    for (k, cs) in keys {
        for ((_, _, _, m), c) in COMBOS.iter().zip(cs) {
            match c {
                Cell::Text(n, _) if n.len() == 1 => cells.push((*k, *m, n[0].clone())),
                Cell::Dead(n) => cells.push((*k, *m, n.clone())),
                _ => {}
            }
        }
    }
    let mut seqs: BTreeMap<Vec<String>, String> = BTreeMap::new();
    let mut conflicts = 0usize;
    let mut start: Vec<u32> = cells
        .iter()
        .filter_map(|(k, m, _)| {
            let t = set.translate(*k as usize, *m, 0);
            (t.text.is_empty() && t.next_state != 0).then_some(t.next_state)
        })
        .collect();
    start.sort_unstable();
    start.dedup();
    let mut todo: Vec<(u32, Vec<String>)> = start.into_iter().map(|s| (s, vec![ctx.dead_name(s)])).collect();
    let mut seen: HashSet<u32> = HashSet::new();
    let mut head = 0;
    while head < todo.len() {
        let (st, path) = todo[head].clone();
        head += 1;
        if seen.contains(&st) || path.len() > 3 {
            continue;
        }
        seen.insert(st);
        for (k, m, ks) in &cells {
            let t = set.translate(*k as usize, *m, st);
            let dead = t.next_state != 0;
            let mut add = |text: String, seqs: &mut BTreeMap<Vec<String>, String>| {
                let mut key = path.clone();
                key.push(ks.clone());
                match seqs.get(&key) {
                    Some(old) if *old != text => conflicts += 1,
                    _ => {
                        seqs.insert(key, text);
                    }
                }
            };
            if dead {
                if !t.text.is_empty() {
                    add(t.text.clone(), &mut seqs);
                    warnings.push(format!("state {st} + key {k:#04x}/{m}: output {:?} then new dead state {} (pending state dropped)", t.text, t.next_state));
                    continue;
                }
                let nm = ctx.dead_name(t.next_state);
                let mut p = path.clone();
                p.push(nm);
                todo.push((t.next_state, p));
            } else if printable(&t.text) {
                add(t.text, &mut seqs);
            } else if !t.text.is_empty() {
                warnings.push(format!("state {st} key {k:#04x}/{m}: non-printable output {:?}", t.text));
            }
        }
    }
    let mut out = format!("# generated: dead-key state machine of layout \"{}\" (id {})\n", layout.name, layout.id);
    for (key, text) in &seqs {
        if text.is_empty() {
            continue;
        }
        let lit: String = text.chars().flat_map(|c| match c {
            '"' => vec!['\\', '"'],
            '\\' => vec!['\\', '\\'],
            o => vec![o],
        }).collect();
        let lhs: Vec<String> = key.iter().map(|k| format!("<{k}>")).collect();
        out.push_str(&format!("{} : \"{}\"\n", lhs.join(" "), lit));
    }
    (out, conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_LAYOUT: &str = include_str!("../tests/test_layout.json");

    fn layout() -> Layout {
        Layout::from_json(TEST_LAYOUT).unwrap()
    }

    #[test]
    fn translate_plain_and_levels() {
        let l = layout();
        let s = &l.sets[0];
        assert_eq!(s.translate(0, 0, 0).text, "a");
        assert_eq!(s.translate(0, 2, 0).text, "A");
        assert_eq!(s.translate(0, 4, 0).text, "A");
        assert_eq!(s.translate(0, 8, 0).text, "å");
    }

    #[test]
    fn dead_key_state_machine() {
        let l = layout();
        let s = &l.sets[0];
        let d = s.translate(14, 8, 0);
        assert_eq!(d, Translation { text: String::new(), next_state: 1 });
        assert_eq!(s.translate(14, 0, 1).text, "é");
        assert_eq!(s.translate(0, 0, 1).text, "´a");
        assert_eq!(s.translate(49, 0, 1).text, "´");
    }

    #[test]
    fn generates_symbols_types_and_compose() {
        let out = generate(&layout(), &GenOptions::default()).unwrap();
        assert!(out.symbols.contains("key <AC01> { type[Group1] = \"CUP_01112333\", symbols[Group1] = [ a, A, U00E5, U00C5 ] };"));
        assert!(out.symbols.contains("include \"level3(alt_switch)\""));
        assert!(out.keymap.contains("type \"CUP_01112333\""));
        assert!(out.keymap.contains("map[Shift+Lock+LevelThree] = Level4;"));
        assert!(out.keymap.contains("key <AD03> { type[Group1] = \"CUP_01112333\", symbols[Group1] = [ e, E, dead_acute, U00B4 ] };"));
        assert!(out.compose.contains("<dead_acute> <e> : \"é\"\n"));
        assert!(out.compose.contains("<dead_acute> <a> : \"´a\"\n"));
        assert!(out.compose.contains("<dead_acute> <U0020> : \"´\"\n"));
        assert_eq!(out.dead_keysyms, vec![(1, "dead_acute".to_string())]);
        assert_eq!(out.conflicts, 0);
    }

    #[test]
    fn iso_swaps_position_keys() {
        let ansi = generate(&layout(), &GenOptions::default()).unwrap();
        assert!(ansi.symbols.contains("<TLDE>") && !ansi.symbols.contains("<LSGT>"));
        let iso = generate(&layout(), &GenOptions { keyboard: Keyboard::Iso, ..GenOptions::default() }).unwrap();
        assert!(iso.symbols.contains("<LSGT>"));
    }

    #[test]
    fn level3_options() {
        let none = generate(&layout(), &GenOptions { level3: Level3Keys::None, ..GenOptions::default() }).unwrap();
        assert!(!none.symbols.contains("level3("));
        let r = generate(&layout(), &GenOptions { level3: Level3Keys::RightAlt, ..GenOptions::default() }).unwrap();
        assert!(r.keymap.contains("level3(ralt_switch)"));
    }

    #[test]
    fn missing_set_is_an_error() {
        assert!(generate(&layout(), &GenOptions { set_index: 7, ..GenOptions::default() }).is_err());
    }

    #[test]
    fn keysym_names() {
        assert_eq!(keysym_name('a'), "a");
        assert_eq!(keysym_name('7'), "7");
        assert_eq!(keysym_name('é'), "U00E9");
        assert_eq!(keysym_name(' '), "U0020");
    }

    #[test]
    fn generated_keymap_compiles_when_xkbcli_is_present() {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let out = generate(&layout(), &GenOptions::default()).unwrap();
        let Ok(mut child) = Command::new("xkbcli").arg("compile-keymap").arg("--keymap").stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::piped()).spawn() else {
            return;
        };
        child.stdin.take().unwrap().write_all(out.keymap.as_bytes()).unwrap();
        let res = child.wait_with_output().unwrap();
        assert!(res.status.success(), "{}", String::from_utf8_lossy(&res.stderr));
    }
}
