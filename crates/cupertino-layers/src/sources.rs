//! Scans of a crate's Rust sources, read as tokens with comments and literals set apart: the purity rule of the
//! `foundation` and `models` layers, and the thread rule of the `system` layer.

use std::path::Path;

/// A token of Rust source, as far as the scans need one.
#[derive(Debug, PartialEq, Eq)]
enum Token {
    Ident(String),
    /// A string, byte string or C string literal, raw or not.
    Str,
    /// A character or byte literal.
    Char,
    /// One punctuation character; `::` is two.
    Punct(char),
}

/// The tokens of `src`, without whitespace, comments and lifetimes.
fn tokens(src: &str) -> Vec<Token> {
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let at = |i: usize| chars.get(i).copied().unwrap_or('\0');
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
        } else if c == '/' && at(i + 1) == '/' {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
        } else if c == '/' && at(i + 1) == '*' {
            let mut depth = 0;
            while i < chars.len() {
                if chars[i] == '/' && at(i + 1) == '*' {
                    depth += 1;
                    i += 2;
                } else if chars[i] == '*' && at(i + 1) == '/' {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else if c == '"' {
            i = quoted(&chars, i + 1, '"');
            out.push(Token::Str);
        } else if c == '\'' {
            if at(i + 1) == '\\' || at(i + 2) == '\'' {
                i = quoted(&chars, i + 1, '\'');
                out.push(Token::Char);
            } else {
                // a lifetime or a label: the quote and the name after it
                i += 1;
                while i < chars.len() && is_ident(chars[i]) {
                    i += 1;
                }
            }
        } else if is_ident(c) {
            let start = i;
            while i < chars.len() && is_ident(chars[i]) {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            let prefix = matches!(word.as_str(), "r" | "br" | "cr");
            if prefix && (at(i) == '"' || at(i) == '#' && raw_string_follows(&chars, i)) {
                i = raw_string(&chars, i);
                out.push(Token::Str);
            } else if word == "r" && at(i) == '#' {
                // a raw identifier, `r#type`
                let start = i + 1;
                i = start;
                while i < chars.len() && is_ident(chars[i]) {
                    i += 1;
                }
                out.push(Token::Ident(chars[start..i].iter().collect()));
            } else if matches!(word.as_str(), "b" | "c") && at(i) == '"' {
                i = quoted(&chars, i + 1, '"');
                out.push(Token::Str);
            } else if word == "b" && at(i) == '\'' {
                i = quoted(&chars, i + 1, '\'');
                out.push(Token::Char);
            } else {
                out.push(Token::Ident(word));
            }
        } else {
            out.push(Token::Punct(c));
            i += 1;
        }
    }
    out
}

fn is_ident(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// The index after the literal that starts at `i`, just after its opening `close` character.
fn quoted(chars: &[char], mut i: usize, close: char) -> usize {
    while i < chars.len() {
        match chars[i] {
            '\\' => i += 2,
            c if c == close => return i + 1,
            _ => i += 1,
        }
    }
    i
}

/// Whether the `#` characters at `i` open a raw string.
fn raw_string_follows(chars: &[char], mut i: usize) -> bool {
    while chars.get(i) == Some(&'#') {
        i += 1;
    }
    chars.get(i) == Some(&'"')
}

/// The index after the raw string whose hashes or opening quote are at `i`.
fn raw_string(chars: &[char], mut i: usize) -> usize {
    let mut hashes = 0;
    while chars.get(i) == Some(&'#') {
        hashes += 1;
        i += 1;
    }
    i += 1;
    while i < chars.len() {
        if chars[i] == '"' && (1..=hashes).all(|k| chars.get(i + k) == Some(&'#')) {
            return i + 1 + hashes;
        }
        i += 1;
    }
    i
}

fn punct(tokens: &[Token], i: usize, c: char) -> bool {
    tokens.get(i) == Some(&Token::Punct(c))
}

fn ident(tokens: &[Token], i: usize) -> Option<&str> {
    match tokens.get(i) {
        Some(Token::Ident(word)) => Some(word),
        _ => None,
    }
}

/// Appends to `out` the paths that start at token `i`, as their segments, with every `use` group expanded:
/// `std::{fs, os::unix::net}` is `std::fs` and `std::os::unix::net`. Returns the index after them.
fn paths<'a>(
    tokens: &'a [Token],
    mut i: usize,
    mut prefix: Vec<&'a str>,
    out: &mut Vec<Vec<&'a str>>,
) -> usize {
    loop {
        match tokens.get(i) {
            Some(Token::Ident(word)) => {
                prefix.push(word);
                i += 1;
                if punct(tokens, i, ':') && punct(tokens, i + 1, ':') {
                    i += 2;
                } else {
                    out.push(prefix);
                    return i;
                }
            }
            Some(Token::Punct('{')) => {
                i += 1;
                loop {
                    match tokens.get(i) {
                        None => return i,
                        Some(Token::Punct('}')) => return i + 1,
                        // a renamed item: the name after `as` is no path
                        Some(Token::Ident(word)) if word == "as" => i += 2,
                        Some(Token::Ident(_)) | Some(Token::Punct('{')) => {
                            i = paths(tokens, i, prefix.clone(), out)
                        }
                        _ => i += 1,
                    }
                }
            }
            _ => {
                out.push(prefix);
                return i;
            }
        }
    }
}

/// The standard library module a path names, with `std::os::unix::net` counted as `std::net`.
fn std_module<'a>(path: &[&'a str]) -> Option<&'a str> {
    if path.first() != Some(&"std") {
        return None;
    }
    if path.get(1..4) == Some(&["os", "unix", "net"]) {
        return Some("net");
    }
    path.get(1).copied()
}

/// What the purity rule finds in `src`, each once and in the order of `impure`: `std::<module>` for a module of
/// `impure` named by a path or a `use std::{…}` group (`std::os::unix::net` is `std::net`), `unsafe`, and `ffi` for
/// an `extern` block or ABI or a linking attribute.
pub(crate) fn impurities(src: &str, impure: &[&'static str]) -> Vec<&'static str> {
    let tokens = tokens(src);
    let mut found = Vec::new();
    for i in 0..tokens.len() {
        match ident(&tokens, i) {
            Some("std") if punct(&tokens, i + 1, ':') && punct(&tokens, i + 2, ':') => {
                let mut named = Vec::new();
                paths(&tokens, i, Vec::new(), &mut named);
                for module in named.iter().filter_map(|path| std_module(path)) {
                    if let Some(item) = impure
                        .iter()
                        .find(|m| m.strip_prefix("std::") == Some(module))
                    {
                        found.push(*item);
                    }
                }
            }
            Some("unsafe") => found.push("unsafe"),
            Some("extern")
                if matches!(
                    tokens.get(i + 1),
                    Some(Token::Str) | Some(Token::Punct('{'))
                ) =>
            {
                found.push("ffi")
            }
            Some("no_mangle" | "export_name" | "link" | "link_name")
                if punct(&tokens, i.wrapping_sub(1), '[')
                    || punct(&tokens, i.wrapping_sub(1), '(') =>
            {
                found.push("ffi")
            }
            _ => {}
        }
    }
    let order: Vec<&'static str> = impure.iter().copied().chain(["unsafe", "ffi"]).collect();
    order
        .into_iter()
        .filter(|item| found.contains(item))
        .collect()
}

/// The calls that start a thread: the item of `std::thread` a path reaches, and the call the thread rule names.
const THREAD_STARTS: [(&str, &str); 3] = [
    ("spawn", "std::thread::spawn"),
    ("Builder", "std::thread::Builder::spawn"),
    ("scope", "std::thread::scope"),
];

/// The calls of `src` that start a thread, each once and in the order of [`THREAD_STARTS`]: a path through
/// `std::thread` to `spawn`, `Builder` or `scope`, whole, in a `use std::{…}` group, or as `thread::…` after a
/// `use std::thread`.
pub(crate) fn thread_starts(src: &str) -> Vec<&'static str> {
    let tokens = tokens(src);
    let mut named = Vec::new();
    for i in 0..tokens.len() {
        let path_start = punct(&tokens, i + 1, ':') && punct(&tokens, i + 2, ':');
        match ident(&tokens, i) {
            Some("std") if path_start => {
                paths(&tokens, i, Vec::new(), &mut named);
            }
            // `thread::spawn` after `use std::thread`, never the inside of a longer path
            Some("thread") if path_start && !punct(&tokens, i.wrapping_sub(1), ':') => {
                paths(&tokens, i, vec!["std"], &mut named);
            }
            _ => {}
        }
    }
    THREAD_STARTS
        .iter()
        .filter(|(item, _)| {
            named
                .iter()
                .any(|path| path.get(..3) == Some(&["std", "thread", item]))
        })
        .map(|(_, call)| *call)
        .collect()
}

/// Whether `src` references `cupertino_kit_shell::hosting` (by path or `use cupertino_kit_shell::{..., hosting}`).
pub(crate) fn uses_kit_shell_hosting(src: &str) -> bool {
    let tokens = tokens(src);
    let mut named = Vec::new();
    for i in 0..tokens.len() {
        let path_start = punct(&tokens, i + 1, ':') && punct(&tokens, i + 2, ':');
        if ident(&tokens, i) == Some("cupertino_kit_shell") && path_start {
            paths(&tokens, i, Vec::new(), &mut named);
        }
    }
    named
        .iter()
        .any(|path| path.get(..2) == Some(&["cupertino_kit_shell", "hosting"]))
}

/// The Rust files under `dir`, as paths relative to `base` with `/` separators, sorted. Hidden directories are left
/// out, and so are the `target` and `tests` directories, the build script and its `build` directory of modules at
/// `base`; a `tests` or `target` directory deeper down is scanned like any other.
pub(crate) fn rust_files(base: &Path, dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or_default();
        if path.is_dir() {
            if name.starts_with('.') || dir == base && matches!(name, "target" | "tests" | "build")
            {
                continue;
            }
            out.extend(rust_files(base, &path));
        } else if name.ends_with(".rs") && !(dir == base && name == "build.rs") {
            let relative = path.strip_prefix(base).unwrap_or(&path);
            let parts: Vec<_> = relative.iter().map(|p| p.to_string_lossy()).collect();
            out.push(parts.join("/"));
        }
    }
    out
}
