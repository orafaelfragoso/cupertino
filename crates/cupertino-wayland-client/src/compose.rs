//! Compose sequences and dead keys: the X compose table of the user's locale (`XCOMPOSEFILE`, else
//! `$XDG_CONFIG_HOME/XCompose` or `~/.XCompose`, else the locale's file under `XLOCALEDIR` or `/usr/share/X11/locale`,
//! found through `compose.dir`), and the state that walks it one key press at a time.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use xkbcommon::xkb;

const SYSTEM_LOCALE_DIR: &str = "/usr/share/X11/locale";
/// How deep `include` lines may nest; deeper ones are skipped, which also ends include cycles.
const MAX_INCLUDE_DEPTH: usize = 5;

#[derive(Debug, Default)]
struct Node {
    next: HashMap<u32, usize>,
    result: Option<char>,
}

/// The sequences of a compose file, as a trie of keysyms.
#[derive(Debug)]
pub(crate) struct ComposeTable {
    nodes: Vec<Node>,
}

/// Where the files a table reads come from: the environment of the process, or fixed values in tests.
#[derive(Clone, Debug, Default)]
pub(crate) struct ComposeEnv {
    pub locale: String,
    pub locale_dir: PathBuf,
    pub home: Option<PathBuf>,
    pub config_home: Option<PathBuf>,
    pub compose_file: Option<PathBuf>,
}

impl ComposeEnv {
    pub(crate) fn from_process() -> Self {
        let var = |name: &str| std::env::var(name).ok().filter(|v| !v.is_empty());
        let locale = ["LC_ALL", "LC_CTYPE", "LANG"]
            .into_iter()
            .find_map(var)
            .unwrap_or_else(|| "C".into());
        let home = var("HOME").map(PathBuf::from);
        Self {
            locale,
            locale_dir: var("XLOCALEDIR").map_or_else(|| SYSTEM_LOCALE_DIR.into(), PathBuf::from),
            config_home: var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| home.as_ref().map(|h| h.join(".config"))),
            home,
            compose_file: var("XCOMPOSEFILE").map(PathBuf::from),
        }
    }

    /// The system compose file of the locale, by `locale.alias` and `compose.dir`.
    fn locale_file(&self) -> Option<PathBuf> {
        let read = |name: &str| std::fs::read_to_string(self.locale_dir.join(name)).ok();
        let entries = |text: String| -> Vec<(String, String)> {
            text.lines()
                .filter(|line| !line.trim_start().starts_with('#'))
                .filter_map(|line| {
                    let mut words = line.split_whitespace();
                    Some((
                        words.next()?.trim_end_matches(':').to_string(),
                        words.next()?.to_string(),
                    ))
                })
                .collect()
        };
        let name = read("locale.alias")
            .map(entries)
            .and_then(|aliases| {
                aliases
                    .into_iter()
                    .find(|(alias, _)| *alias == self.locale)
                    .map(|(_, name)| name)
            })
            .unwrap_or_else(|| self.locale.clone());
        entries(read("compose.dir")?)
            .into_iter()
            .find(|(_, locale)| *locale == name)
            .map(|(file, _)| self.locale_dir.join(file))
    }

    /// The file the table starts from: the user's when there is one, else the locale's.
    fn start_file(&self) -> Option<PathBuf> {
        if let Some(file) = &self.compose_file {
            return Some(file.clone());
        }
        let user = [
            self.config_home.as_ref().map(|c| c.join("XCompose")),
            self.home.as_ref().map(|h| h.join(".XCompose")),
        ];
        user.into_iter()
            .flatten()
            .find(|path| path.is_file())
            .or_else(|| self.locale_file())
    }
}

impl ComposeTable {
    /// The table of `env`; `None` when no compose file is found or none of its lines is a sequence.
    pub(crate) fn load(env: &ComposeEnv) -> Option<Self> {
        let mut table = Self {
            nodes: vec![Node::default()],
        };
        table.read_file(&env.start_file()?, env, 0);
        (!table.nodes[0].next.is_empty()).then_some(table)
    }

    fn read_file(&mut self, path: &Path, env: &ComposeEnv, depth: usize) {
        let Ok(text) = std::fs::read_to_string(path) else {
            return;
        };
        for line in text.lines() {
            let line = line.trim();
            if let Some(rest) = line.strip_prefix("include") {
                if depth < MAX_INCLUDE_DEPTH {
                    if let Some(included) = quoted(rest.trim()).and_then(|p| expand(&p, env)) {
                        self.read_file(&included, env, depth + 1);
                    }
                }
            } else if let Some((keys, result)) = sequence(line) {
                self.insert(&keys, result);
            }
        }
    }

    /// Add a sequence; a later one replaces an earlier one it shares a prefix with, as in libxkbcommon.
    fn insert(&mut self, keys: &[u32], result: char) {
        let mut at = 0;
        for (index, key) in keys.iter().enumerate() {
            let last = index + 1 == keys.len();
            at = match self.nodes[at].next.get(key) {
                Some(&next) => next,
                None => {
                    self.nodes.push(Node::default());
                    let next = self.nodes.len() - 1;
                    self.nodes[at].next.insert(*key, next);
                    next
                }
            };
            if last {
                self.nodes[at] = Node {
                    next: HashMap::new(),
                    result: Some(result),
                };
            } else {
                self.nodes[at].result = None;
            }
        }
    }
}

/// The text of a `"…"` string at the start of `s`, with its escapes resolved.
fn quoted(s: &str) -> Option<String> {
    let mut chars = s.strip_prefix('"')?.chars();
    let mut out = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                'r' => out.push('\r'),
                't' => out.push('\t'),
                'x' | 'X' => {
                    let digits: String = chars
                        .clone()
                        .take_while(char::is_ascii_hexdigit)
                        .take(2)
                        .collect();
                    chars.nth(digits.len().checked_sub(1)?);
                    out.push(char::from_u32(u32::from_str_radix(&digits, 16).ok()?)?);
                }
                c @ '0'..='7' => {
                    let rest: String = chars
                        .clone()
                        .take_while(|c| ('0'..='7').contains(c))
                        .take(2)
                        .collect();
                    for _ in 0..rest.len() {
                        chars.next();
                    }
                    out.push(char::from_u32(
                        u32::from_str_radix(&format!("{c}{rest}"), 8).ok()?,
                    )?);
                }
                c => out.push(c),
            },
            c => out.push(c),
        }
    }
}

/// An include path with `%%`, `%H` (home), `%L` (the locale's compose file) and `%S` (the system locale directory).
fn expand(path: &str, env: &ComposeEnv) -> Option<PathBuf> {
    let mut out = String::new();
    let mut chars = path.chars();
    while let Some(c) = chars.next() {
        if c != '%' {
            out.push(c);
            continue;
        }
        match chars.next()? {
            '%' => out.push('%'),
            'H' => out.push_str(env.home.as_ref()?.to_str()?),
            'L' => out.push_str(env.locale_file()?.to_str()?),
            'S' => out.push_str(env.locale_dir.to_str()?),
            _ => return None,
        }
    }
    Some(PathBuf::from(out))
}

/// The keysyms and the one character of a line `[modifiers] <a> <b> … : "text" [keysym]  # comment`; `None` for any
/// other line, and for a result that is not exactly one character.
fn sequence(line: &str) -> Option<(Vec<u32>, char)> {
    let (left, right) = line.split_once(':')?;
    let mut keys = Vec::new();
    let mut rest = left;
    while let Some(start) = rest.find('<') {
        let end = rest[start..].find('>')? + start;
        let sym = xkb::keysym_from_name(&rest[start + 1..end], xkb::KEYSYM_NO_FLAGS);
        if sym.raw() == 0 {
            return None;
        }
        keys.push(sym.raw());
        rest = &rest[end + 1..];
    }
    if keys.is_empty() {
        return None;
    }
    let right = right.trim();
    let text = quoted(right).filter(|t| !t.is_empty()).or_else(|| {
        let name = right
            .split(|c: char| c.is_whitespace() || c == '#')
            .next()?;
        let sym = xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS);
        Some(xkb::keysym_to_utf8(sym)).filter(|t| !t.is_empty())
    })?;
    let mut chars = text.chars();
    let result = chars.next()?;
    chars.next().is_none().then_some((keys, result))
}

/// What a key press does to a compose sequence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Composed {
    /// No sequence: the key types what it types.
    Passed,
    /// The key goes on with a sequence and types nothing yet.
    Composing,
    /// The key ends a sequence and types its character.
    Done(char),
    /// The key does not go on with the sequence: the sequence ends, and the key types nothing.
    Cancelled,
}

/// Where a sequence is in its table.
#[derive(Debug, Default)]
pub(crate) struct ComposeState {
    at: Option<usize>,
    /// The keysyms of the sequence typed so far, modifiers left out.
    typed: Vec<u32>,
}

impl ComposeState {
    /// Feed the keysym of a pressed key. Modifier keys leave a sequence where it is.
    pub(crate) fn feed(&mut self, table: &ComposeTable, keysym: u32) -> Composed {
        if xkb::Keysym::new(keysym).is_modifier_key() {
            return if self.at.is_some() {
                Composed::Composing
            } else {
                Composed::Passed
            };
        }
        let from = self.at.take().unwrap_or(0);
        let typed = std::mem::take(&mut self.typed);
        match table.nodes[from].next.get(&keysym) {
            Some(&next) => match table.nodes[next].result {
                Some(result) => Composed::Done(result),
                None => {
                    self.at = Some(next);
                    self.typed = typed;
                    self.typed.push(keysym);
                    Composed::Composing
                }
            },
            None if from == 0 => Composed::Passed,
            None => Composed::Cancelled,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.at = None;
        self.typed.clear();
    }

    /// The keysyms of the sequence being typed; empty outside one.
    pub(crate) fn pending(&self) -> &[u32] {
        &self.typed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(name: &str) -> u32 {
        xkb::keysym_from_name(name, xkb::KEYSYM_NO_FLAGS).raw()
    }

    struct Files(tempfile::TempDir);

    impl Files {
        fn new(files: &[(&str, &str)]) -> Self {
            let dir = tempfile::tempdir().unwrap();
            for (name, text) in files {
                let path = dir.path().join(name);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, text).unwrap();
            }
            Self(dir)
        }

        fn env(&self, locale: &str) -> ComposeEnv {
            ComposeEnv {
                locale: locale.into(),
                locale_dir: self.0.path().join("locale"),
                home: Some(self.0.path().join("home")),
                config_home: Some(self.0.path().join("home/.config")),
                compose_file: None,
            }
        }
    }

    fn typed(table: &ComposeTable, keys: &[&str]) -> Vec<Composed> {
        let mut state = ComposeState::default();
        keys.iter().map(|k| state.feed(table, sym(k))).collect()
    }

    const LOCALE: [(&str, &str); 4] = [
        (
            "locale/compose.dir",
            "# file locale\nen_US.UTF-8/Compose:\t\ten_US.UTF-8\npt_BR.UTF-8/Compose\t\tpt_BR.UTF-8\n",
        ),
        ("locale/locale.alias", "pt_BR\t\tpt_BR.UTF-8\n"),
        (
            "locale/en_US.UTF-8/Compose",
            "<dead_tilde> <a> : \"\u{e3}\" atilde # LATIN SMALL LETTER A WITH TILDE\n\
             <Multi_key> <comma> <c> : \"\u{e7}\" ccedilla\n\
             <dead_acute> <c> : \"\u{107}\" U0107\n\
             <Multi_key> <o> <c> : \"\\251\"\n",
        ),
        (
            "locale/pt_BR.UTF-8/Compose",
            "include \"%S/en_US.UTF-8/Compose\"\n<dead_acute> <c> : \"\u{e7}\" ccedilla\n",
        ),
    ];

    #[test]
    fn the_locale_file_includes_the_base_and_overrides_its_sequences() {
        let files = Files::new(&LOCALE);
        let table = ComposeTable::load(&files.env("pt_BR")).unwrap();
        assert_eq!(
            typed(&table, &["dead_tilde", "a"]),
            [Composed::Composing, Composed::Done('ã')]
        );
        assert_eq!(
            typed(&table, &["Multi_key", "Shift_L", "comma", "c"]),
            [
                Composed::Composing,
                Composed::Composing,
                Composed::Composing,
                Composed::Done('ç')
            ]
        );
        assert_eq!(
            typed(&table, &["dead_acute", "c"])[1],
            Composed::Done('ç'),
            "pt_BR's own"
        );
        assert_eq!(
            typed(&table, &["Multi_key", "o", "c"])[2],
            Composed::Done('©'),
            "an octal escape"
        );
        let en = ComposeTable::load(&files.env("en_US.UTF-8")).unwrap();
        assert_eq!(typed(&en, &["dead_acute", "c"])[1], Composed::Done('ć'));
    }

    #[test]
    fn a_key_outside_the_sequence_cancels_it_and_the_next_key_types() {
        let files = Files::new(&LOCALE);
        let table = ComposeTable::load(&files.env("pt_BR")).unwrap();
        assert_eq!(
            typed(
                &table,
                &["dead_tilde", "x", "x", "Multi_key", "Escape", "a"]
            ),
            [
                Composed::Composing,
                Composed::Cancelled,
                Composed::Passed,
                Composed::Composing,
                Composed::Cancelled,
                Composed::Passed
            ]
        );
    }

    #[test]
    fn the_users_file_replaces_the_locales_and_can_include_it() {
        let mut files = LOCALE.to_vec();
        files.push((
            "home/.XCompose",
            "include \"%L\"\n<Multi_key> <comma> <c> : \"\u{c7}\"\n<dead_tilde> <dead_tilde> <a> : \"\u{e2}\"\n",
        ));
        let files = Files::new(&files);
        let table = ComposeTable::load(&files.env("pt_BR")).unwrap();
        assert_eq!(
            typed(&table, &["Multi_key", "comma", "c"])[2],
            Composed::Done('Ç')
        );
        assert_eq!(typed(&table, &["dead_acute", "c"])[1], Composed::Done('ç'));
        assert_eq!(
            typed(&table, &["dead_tilde", "a"]),
            [Composed::Composing, Composed::Done('ã')]
        );
        let mut env = files.env("pt_BR");
        env.compose_file = Some(files.0.path().join("locale/en_US.UTF-8/Compose"));
        let chosen = ComposeTable::load(&env).unwrap();
        assert_eq!(
            typed(&chosen, &["dead_acute", "c"])[1],
            Composed::Done('ć'),
            "XCOMPOSEFILE wins"
        );
    }

    #[test]
    fn an_include_cycle_ends_and_a_locale_without_a_file_has_no_table() {
        let files = Files::new(&[
            ("locale/compose.dir", "loop/Compose x.UTF-8\n"),
            (
                "locale/loop/Compose",
                "include \"%S/loop/Compose\"\n<dead_grave> <a> : \"\u{e0}\"\n",
            ),
        ]);
        let table = ComposeTable::load(&files.env("x.UTF-8")).unwrap();
        assert_eq!(typed(&table, &["dead_grave", "a"])[1], Composed::Done('à'));
        assert!(ComposeTable::load(&files.env("nowhere")).is_none());
    }
}
