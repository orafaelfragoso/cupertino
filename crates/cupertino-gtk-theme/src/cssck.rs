//! A small CSS tokenizer and structural parser, enough to check that generated GTK style sheets
//! are well formed (balanced blocks, terminated strings and comments, `name: value;`
//! declarations, `@define-color name value;` and `@import`) and to read them back in tests.
//!
//! It is intentionally not a complete CSS implementation: it understands the subset that GTK
//! style sheets use and rejects anything structurally broken.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Token {
    Ident(String),
    AtKeyword(String),
    Hash(String),
    Str(String),
    Number(f64, String),
    Percentage(f64),
    Function(String),
    Colon,
    Semicolon,
    Comma,
    LBrace,
    RBrace,
    LParen,
    RParen,
    Delim(char),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CssError {
    pub message: String,
    pub line: usize,
}

impl std::fmt::Display for CssError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for CssError {}

fn is_ident_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_' || c == '-' || !c.is_ascii()
}

fn is_ident_char(c: char) -> bool {
    is_ident_start(c) || c.is_ascii_digit()
}

/// Tokenize `css`, skipping whitespace and comments.
pub fn tokenize(css: &str) -> Result<Vec<(Token, usize)>, CssError> {
    Ok(lex(css)?.into_iter().map(|(t, l, _)| (t, l)).collect())
}

/// Tokens with their line and whether whitespace or a comment preceded them.
fn lex(css: &str) -> Result<Vec<(Token, usize, bool)>, CssError> {
    let c: Vec<char> = css.chars().collect();
    let mut out: Vec<(Token, usize)> = Vec::new();
    let mut ws: Vec<bool> = Vec::new();
    let mut pending_ws = false;
    let mut i = 0;
    let mut line = 1;
    let err = |message: &str, line: usize| CssError { message: message.to_string(), line };
    while i < c.len() {
        let ch = c[i];
        let before = out.len();
        match ch {
            '\n' => {
                line += 1;
                i += 1;
            }
            ' ' | '\t' | '\r' => i += 1,
            '/' if c.get(i + 1) == Some(&'*') => {
                let start = line;
                i += 2;
                loop {
                    match (c.get(i), c.get(i + 1)) {
                        (Some('*'), Some('/')) => {
                            i += 2;
                            break;
                        }
                        (Some('\n'), _) => {
                            line += 1;
                            i += 1;
                        }
                        (Some(_), _) => i += 1,
                        (None, _) => return Err(err("unterminated comment", start)),
                    }
                }
            }
            '"' | '\'' => {
                let q = ch;
                let start = line;
                i += 1;
                let mut s = String::new();
                loop {
                    match c.get(i) {
                        Some('\\') if i + 1 < c.len() => {
                            s.push(c[i + 1]);
                            i += 2;
                        }
                        Some(&x) if x == q => {
                            i += 1;
                            break;
                        }
                        Some('\n') | None => return Err(err("unterminated string", start)),
                        Some(&x) => {
                            s.push(x);
                            i += 1;
                        }
                    }
                }
                out.push((Token::Str(s), line));
            }
            '{' => {
                out.push((Token::LBrace, line));
                i += 1;
            }
            '}' => {
                out.push((Token::RBrace, line));
                i += 1;
            }
            '(' => {
                out.push((Token::LParen, line));
                i += 1;
            }
            ')' => {
                out.push((Token::RParen, line));
                i += 1;
            }
            ':' => {
                out.push((Token::Colon, line));
                i += 1;
            }
            ';' => {
                out.push((Token::Semicolon, line));
                i += 1;
            }
            ',' => {
                out.push((Token::Comma, line));
                i += 1;
            }
            '#' => {
                let s = i + 1;
                i += 1;
                while i < c.len() && is_ident_char(c[i]) {
                    i += 1;
                }
                if i == s {
                    return Err(err("empty hash", line));
                }
                out.push((Token::Hash(c[s..i].iter().collect()), line));
            }
            '@' => {
                let s = i + 1;
                i += 1;
                while i < c.len() && is_ident_char(c[i]) {
                    i += 1;
                }
                if i == s {
                    return Err(err("empty at-keyword", line));
                }
                out.push((Token::AtKeyword(c[s..i].iter().collect()), line));
            }
            d if d.is_ascii_digit() || (d == '.' && c.get(i + 1).is_some_and(|x| x.is_ascii_digit())) || ((d == '-' || d == '+') && c.get(i + 1).is_some_and(|x| x.is_ascii_digit() || *x == '.')) => {
                let s = i;
                i += 1;
                while i < c.len() && (c[i].is_ascii_digit() || c[i] == '.') {
                    i += 1;
                }
                let num: f64 = c[s..i].iter().collect::<String>().parse().map_err(|_| err("bad number", line))?;
                if c.get(i) == Some(&'%') {
                    i += 1;
                    out.push((Token::Percentage(num), line));
                } else {
                    let u = i;
                    while i < c.len() && c[i].is_ascii_alphabetic() {
                        i += 1;
                    }
                    out.push((Token::Number(num, c[u..i].iter().collect()), line));
                }
            }
            d if is_ident_start(d) => {
                let s = i;
                while i < c.len() && is_ident_char(c[i]) {
                    i += 1;
                }
                let name: String = c[s..i].iter().collect();
                if c.get(i) == Some(&'(') {
                    i += 1;
                    out.push((Token::Function(name), line));
                } else {
                    out.push((Token::Ident(name), line));
                }
            }
            other => {
                out.push((Token::Delim(other), line));
                i += 1;
            }
        }
        if out.len() > before {
            ws.push(pending_ws);
            pending_ws = false;
        } else {
            pending_ws = true;
        }
    }
    Ok(out.into_iter().zip(ws).map(|((t, l), w)| (t, l, w)).collect())
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Rule {
    pub selector: String,
    pub declarations: Vec<(String, String)>,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stylesheet {
    pub defines: BTreeMap<String, String>,
    pub imports: Vec<String>,
    pub rules: Vec<Rule>,
}

impl Stylesheet {
    /// All declarations named `property` in rules whose selector list contains `selector` verbatim.
    pub fn values(&self, selector: &str, property: &str) -> Vec<&str> {
        self.rules
            .iter()
            .filter(|r| r.selector.split(',').any(|s| s.trim() == selector))
            .flat_map(|r| r.declarations.iter().filter(|(p, _)| p == property).map(|(_, v)| v.as_str()))
            .collect()
    }

    pub fn has_selector(&self, selector: &str) -> bool {
        self.rules.iter().any(|r| r.selector.split(',').any(|s| s.trim() == selector))
    }

    /// Names of every `@name` colour reference inside rule values.
    pub fn referenced_colors(&self) -> Vec<String> {
        let mut v = Vec::new();
        for r in &self.rules {
            for (_, value) in &r.declarations {
                let b: Vec<char> = value.chars().collect();
                let mut i = 0;
                while i < b.len() {
                    if b[i] == '@' {
                        let s = i + 1;
                        i += 1;
                        while i < b.len() && is_ident_char(b[i]) {
                            i += 1;
                        }
                        v.push(b[s..i].iter().collect());
                    } else {
                        i += 1;
                    }
                }
            }
        }
        v
    }
}

fn render(tokens: &[(Token, usize, bool)]) -> String {
    let mut s = String::new();
    for (i, (t, _, ws)) in tokens.iter().enumerate() {
        let piece = match t {
            Token::Ident(x) => x.clone(),
            Token::AtKeyword(x) => format!("@{x}"),
            Token::Hash(x) => format!("#{x}"),
            Token::Str(x) => format!("\"{x}\""),
            Token::Number(n, u) => format!("{n}{u}"),
            Token::Percentage(n) => format!("{n}%"),
            Token::Function(x) => format!("{x}("),
            Token::Colon => ":".into(),
            Token::Semicolon => ";".into(),
            Token::Comma => ",".into(),
            Token::LBrace => "{".into(),
            Token::RBrace => "}".into(),
            Token::LParen => "(".into(),
            Token::RParen => ")".into(),
            Token::Delim(c) => c.to_string(),
        };
        if i > 0 && *ws {
            s.push(' ');
        }
        s.push_str(&piece);
    }
    s
}

/// Tokenize and parse the structure; returns the first problem found.
pub fn parse(css: &str) -> Result<Stylesheet, CssError> {
    let tokens = lex(css)?;
    let mut sheet = Stylesheet::default();
    let mut i = 0;
    let err = |message: String, line: usize| CssError { message, line };
    let mut depth_check = 0i32;
    for (t, line, _) in &tokens {
        match t {
            Token::LBrace | Token::LParen | Token::Function(_) => depth_check += 1,
            Token::RBrace | Token::RParen => {
                depth_check -= 1;
                if depth_check < 0 {
                    return Err(err("unbalanced closing bracket".into(), *line));
                }
            }
            _ => {}
        }
    }
    if depth_check != 0 {
        return Err(err("unclosed block or parenthesis".into(), tokens.last().map_or(1, |t| t.1)));
    }
    while i < tokens.len() {
        let (tok, line, _) = &tokens[i];
        if let Token::AtKeyword(name) = tok {
            let end = tokens[i..].iter().position(|(t, _, _)| matches!(t, Token::Semicolon | Token::LBrace)).map(|p| p + i).ok_or_else(|| err(format!("@{name} is not terminated"), *line))?;
            if matches!(tokens[end].0, Token::LBrace) {
                return Err(err(format!("block at-rule @{name} is not supported"), *line));
            }
            let body = &tokens[i + 1..end];
            match name.as_str() {
                "define-color" => {
                    let Some((Token::Ident(n), _, _)) = body.first() else { return Err(err("@define-color needs a name".into(), *line)) };
                    if body.len() < 2 {
                        return Err(err(format!("@define-color {n} has no value"), *line));
                    }
                    sheet.defines.insert(n.clone(), render(&body[1..]));
                }
                "import" => match body.first() {
                    Some((Token::Str(s), _, _)) => sheet.imports.push(s.clone()),
                    Some((Token::Function(f), _, _)) if f == "url" => match body.get(1) {
                        Some((Token::Str(s), _, _)) => sheet.imports.push(s.clone()),
                        _ => return Err(err("@import url() needs a string".into(), *line)),
                    },
                    _ => return Err(err("@import needs a string or url()".into(), *line)),
                },
                other => return Err(err(format!("unsupported at-rule @{other}"), *line)),
            }
            i = end + 1;
            continue;
        }
        let open = tokens[i..].iter().position(|(t, _, _)| matches!(t, Token::LBrace | Token::Semicolon | Token::RBrace)).map(|p| p + i);
        let Some(open) = open else { return Err(err("selector without a block".into(), *line)) };
        if !matches!(tokens[open].0, Token::LBrace) {
            return Err(err("expected `{` after selector".into(), tokens[open].1));
        }
        if open == i {
            return Err(err("empty selector".into(), *line));
        }
        let selector = render(&tokens[i..open]);
        let mut j = open + 1;
        let mut rule = Rule { selector, declarations: Vec::new() };
        loop {
            let Some((t, l, _)) = tokens.get(j) else { return Err(err("unclosed block".into(), *line)) };
            match t {
                Token::RBrace => {
                    j += 1;
                    break;
                }
                Token::Semicolon => j += 1,
                Token::Ident(prop) => {
                    if !matches!(tokens.get(j + 1).map(|t| &t.0), Some(Token::Colon)) {
                        return Err(err(format!("declaration `{prop}` has no colon"), *l));
                    }
                    let mut k = j + 2;
                    let mut depth = 0i32;
                    while let Some((t, _, _)) = tokens.get(k) {
                        match t {
                            Token::LParen | Token::Function(_) => depth += 1,
                            Token::RParen => depth -= 1,
                            Token::Semicolon if depth == 0 => break,
                            Token::RBrace if depth == 0 => break,
                            Token::LBrace => return Err(err("nested blocks are not supported".into(), *l)),
                            _ => {}
                        }
                        k += 1;
                    }
                    if k == j + 2 {
                        return Err(err(format!("declaration `{prop}` has no value"), *l));
                    }
                    rule.declarations.push((prop.clone(), render(&tokens[j + 2..k])));
                    j = k;
                }
                other => return Err(err(format!("unexpected token {other:?} in a declaration block"), *l)),
            }
        }
        sheet.rules.push(rule);
        i = j;
    }
    Ok(sheet)
}

/// Parse a colour value (`#rgb`, `#rrggbb`, `rgb()`, `rgba()`) to RGBA8.
pub fn parse_color(value: &str) -> Option<[u8; 4]> {
    let v = value.trim();
    if let Some(h) = v.strip_prefix('#') {
        let h: Vec<u8> = h.chars().map(|c| c.to_digit(16).map(|d| d as u8)).collect::<Option<_>>()?;
        return match h.len() {
            3 => Some([h[0] * 17, h[1] * 17, h[2] * 17, 255]),
            6 => Some([h[0] * 16 + h[1], h[2] * 16 + h[3], h[4] * 16 + h[5], 255]),
            _ => None,
        };
    }
    let inner = v.strip_prefix("rgba(").or_else(|| v.strip_prefix("rgb("))?.strip_suffix(')')?;
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() < 3 || parts.len() > 4 {
        return None;
    }
    let ch = |s: &str| s.parse::<f64>().ok().filter(|v| (0.0..=255.0).contains(v)).map(|v| v.round() as u8);
    let a = match parts.get(3) {
        Some(a) => (a.parse::<f64>().ok().filter(|v| (0.0..=1.0).contains(v))? * 255.0).round() as u8,
        None => 255,
    };
    Some([ch(parts[0])?, ch(parts[1])?, ch(parts[2])?, a])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizer_recognises_the_gtk_subset() {
        let t = tokenize("@define-color a #ff0000; /* c */ b > c:hover { min-height: 12px; color: alpha(@a, 0.5); w: 50%; s: \"x\"; }").unwrap();
        let kinds: Vec<&Token> = t.iter().map(|(t, _)| t).collect();
        assert!(kinds.contains(&&Token::AtKeyword("define-color".into())));
        assert!(kinds.contains(&&Token::Hash("ff0000".into())));
        assert!(kinds.contains(&&Token::Number(12.0, "px".into())));
        assert!(kinds.contains(&&Token::Function("alpha".into())));
        assert!(kinds.contains(&&Token::Percentage(50.0)));
        assert!(kinds.contains(&&Token::Str("x".into())));
        assert!(kinds.contains(&&Token::Delim('>')));
    }

    #[test]
    fn tokenizer_rejects_broken_input() {
        assert!(tokenize("/* open").is_err());
        assert!(tokenize("a { b: \"open; }").is_err());
        assert_eq!(tokenize("a\n\n/* x").unwrap_err().line, 3);
    }

    #[test]
    fn parses_defines_imports_and_rules() {
        let s = parse("@import url(\"resource:///x.css\");\n@define-color accent_color #007aff;\n@define-color shade rgba(0, 0, 0, 0.07);\nbutton, .flat { min-height: 18px; padding: 0 8px; color: alpha(@accent_color, .5) }\nentry:focus { outline: 3px solid @accent_color; }").unwrap();
        assert_eq!(s.imports, vec!["resource:///x.css"]);
        assert_eq!(s.defines["accent_color"], "#007aff");
        assert_eq!(s.defines["shade"], "rgba(0, 0, 0, 0.07)");
        assert_eq!(s.rules.len(), 2);
        assert!(s.has_selector("button") && s.has_selector(".flat") && s.has_selector("entry:focus"));
        assert_eq!(s.values("button", "min-height"), vec!["18px"]);
        assert_eq!(s.values("button", "padding"), vec!["0 8px"]);
        assert_eq!(s.referenced_colors(), vec!["accent_color", "accent_color"]);
    }

    #[test]
    fn rejects_structural_errors() {
        for bad in [
            "a { b: c;",
            "a { b: c; } }",
            "a { b c; }",
            "a { b: ; }",
            "@define-color x;",
            "@define-color ;",
            "a { b: { } }",
            "{ b: c; }",
            "a b",
            "@media screen { a { b: c; } }",
            "@import;",
            "a { b: calc(1px; }",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} must be rejected");
        }
    }

    #[test]
    fn colour_parsing() {
        assert_eq!(parse_color("#ff8000"), Some([255, 128, 0, 255]));
        assert_eq!(parse_color("#fff"), Some([255, 255, 255, 255]));
        assert_eq!(parse_color("rgba(0, 0, 0, 0.5)"), Some([0, 0, 0, 128]));
        assert_eq!(parse_color("rgb(1, 2, 3)"), Some([1, 2, 3, 255]));
        assert_eq!(parse_color("rgba(300, 0, 0, 1)"), None);
        assert_eq!(parse_color("#ggg"), None);
        assert_eq!(parse_color("red"), None);
        assert_eq!(parse_color("rgba(0, 0, 0, 2)"), None);
    }

    #[test]
    fn empty_and_comment_only_sheets_are_valid() {
        assert_eq!(parse("").unwrap(), Stylesheet::default());
        assert_eq!(parse("/* nothing */").unwrap(), Stylesheet::default());
    }
}
