//! Calculator for simple arithmetic typed into the launcher.
//!
//! Grammar (highest precedence first): numbers (`12`, `3.5`, `.5`, `1e3`, `1_000`), constants
//! (`pi`, `e`, `tau`), parentheses, function calls (`sqrt sin cos tan asin acos atan ln log log2
//! exp abs floor ceil round`), postfix percent (`50%` = 0.5), unary `+`/`−`, right-associative
//! power `^` (`**` also), multiplication and division (`* / × ÷`), modulo (`%` followed by a
//! number, a name or a parenthesis; before a sign it is percent, so `50% + 10` is 10.5), and addition and subtraction (`+ − –`). Trigonometry works in radians.
//!
//! [`evaluate`] only answers for input that is arithmetic rather than a word or a plain number:
//! it must contain at least one operator, a function call, a constant or a parenthesis. Division by
//! zero, overflow and non-finite results yield `None`, as does nesting deeper than 64 levels.

#[derive(Clone, Debug, PartialEq)]
pub struct Calculation {
    pub expression: String,
    pub value: f64,
    /// `value` formatted for display (and for the clipboard).
    pub text: String,
}

#[derive(Clone, Debug, PartialEq)]
enum Tok {
    Num(f64),
    Ident(String),
    Plus,
    Minus,
    Star,
    Slash,
    Caret,
    Percent,
    LParen,
    RParen,
}

const MAX_DEPTH: usize = 64;

fn lex(s: &str) -> Option<Vec<Tok>> {
    let c: Vec<char> = s.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < c.len() {
        let ch = c[i];
        match ch {
            ' ' | '\t' => i += 1,
            '0'..='9' | '.' => {
                let start = i;
                let mut seen_e = false;
                while i < c.len() {
                    let d = c[i];
                    if d.is_ascii_digit() || d == '.' || d == '_' {
                        i += 1;
                    } else if (d == 'e' || d == 'E') && !seen_e && i > start {
                        let mut j = i + 1;
                        if j < c.len() && (c[j] == '+' || c[j] == '-') {
                            j += 1;
                        }
                        if j < c.len() && c[j].is_ascii_digit() {
                            seen_e = true;
                            i = j;
                        } else {
                            break;
                        }
                    } else {
                        break;
                    }
                }
                let text: String = c[start..i].iter().filter(|&&d| d != '_').collect();
                out.push(Tok::Num(text.parse().ok()?));
            }
            'a'..='z' | 'A'..='Z' | 'π' | 'τ' => {
                let start = i;
                while i < c.len() && (c[i].is_ascii_alphanumeric() || c[i] == 'π' || c[i] == 'τ')
                {
                    i += 1;
                }
                out.push(Tok::Ident(
                    c[start..i].iter().collect::<String>().to_lowercase(),
                ));
            }
            '+' => {
                out.push(Tok::Plus);
                i += 1;
            }
            '-' | '−' | '–' => {
                out.push(Tok::Minus);
                i += 1;
            }
            '*' | '×' | '·' => {
                if ch == '*' && c.get(i + 1) == Some(&'*') {
                    out.push(Tok::Caret);
                    i += 2;
                } else {
                    out.push(Tok::Star);
                    i += 1;
                }
            }
            '/' | '÷' => {
                out.push(Tok::Slash);
                i += 1;
            }
            '^' => {
                out.push(Tok::Caret);
                i += 1;
            }
            '%' => {
                out.push(Tok::Percent);
                i += 1;
            }
            '(' => {
                out.push(Tok::LParen);
                i += 1;
            }
            ')' => {
                out.push(Tok::RParen);
                i += 1;
            }
            _ => return None,
        }
    }
    Some(out)
}

struct Parser {
    t: Vec<Tok>,
    p: usize,
    depth: usize,
}

/// Whether a `%` before `t` is modulo rather than percent: it is followed by a number, a name or a parenthesis. A sign
/// after it is the next operator, so `50% + 10` adds to a percentage.
fn starts_operand(t: Option<&Tok>) -> bool {
    matches!(t, Some(Tok::Num(_) | Tok::Ident(_) | Tok::LParen))
}

impl Parser {
    fn peek(&self) -> Option<&Tok> {
        self.t.get(self.p)
    }

    fn enter(&mut self) -> Option<()> {
        self.depth += 1;
        (self.depth <= MAX_DEPTH).then_some(())
    }

    fn expr(&mut self) -> Option<f64> {
        self.enter()?;
        let mut v = self.term()?;
        loop {
            match self.peek() {
                Some(Tok::Plus) => {
                    self.p += 1;
                    v += self.term()?;
                }
                Some(Tok::Minus) => {
                    self.p += 1;
                    v -= self.term()?;
                }
                _ => break,
            }
        }
        self.depth -= 1;
        Some(v)
    }

    fn term(&mut self) -> Option<f64> {
        let mut v = self.unary()?;
        loop {
            match self.peek() {
                Some(Tok::Star) => {
                    self.p += 1;
                    v *= self.unary()?;
                }
                Some(Tok::Slash) => {
                    self.p += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return None;
                    }
                    v /= d;
                }
                Some(Tok::Percent) if starts_operand(self.t.get(self.p + 1)) => {
                    self.p += 1;
                    let d = self.unary()?;
                    if d == 0.0 {
                        return None;
                    }
                    v %= d;
                }
                _ => break,
            }
        }
        Some(v)
    }

    fn unary(&mut self) -> Option<f64> {
        match self.peek() {
            Some(Tok::Minus) => {
                self.p += 1;
                self.enter()?;
                let v = self.unary()?;
                self.depth -= 1;
                Some(-v)
            }
            Some(Tok::Plus) => {
                self.p += 1;
                self.enter()?;
                let v = self.unary()?;
                self.depth -= 1;
                Some(v)
            }
            _ => self.power(),
        }
    }

    fn power(&mut self) -> Option<f64> {
        let base = self.postfix()?;
        if self.peek() == Some(&Tok::Caret) {
            self.p += 1;
            self.enter()?;
            let exp = self.unary()?;
            self.depth -= 1;
            return Some(base.powf(exp));
        }
        Some(base)
    }

    fn postfix(&mut self) -> Option<f64> {
        let mut v = self.atom()?;
        while self.peek() == Some(&Tok::Percent) && !starts_operand(self.t.get(self.p + 1)) {
            self.p += 1;
            v /= 100.0;
        }
        Some(v)
    }

    fn atom(&mut self) -> Option<f64> {
        let tok = self.t.get(self.p)?.clone();
        self.p += 1;
        match tok {
            Tok::Num(n) => Some(n),
            Tok::LParen => {
                let v = self.expr()?;
                (self.t.get(self.p) == Some(&Tok::RParen)).then(|| {
                    self.p += 1;
                    v
                })
            }
            Tok::Ident(name) => {
                if self.peek() == Some(&Tok::LParen) {
                    self.p += 1;
                    let arg = self.expr()?;
                    if self.t.get(self.p) != Some(&Tok::RParen) {
                        return None;
                    }
                    self.p += 1;
                    function(&name, arg)
                } else {
                    constant(&name)
                }
            }
            _ => None,
        }
    }
}

fn constant(name: &str) -> Option<f64> {
    Some(match name {
        "pi" | "π" => std::f64::consts::PI,
        "tau" | "τ" => std::f64::consts::TAU,
        "e" => std::f64::consts::E,
        _ => return None,
    })
}

fn function(name: &str, x: f64) -> Option<f64> {
    Some(match name {
        "sqrt" => {
            if x < 0.0 {
                return None;
            }
            x.sqrt()
        }
        "sin" => x.sin(),
        "cos" => x.cos(),
        "tan" => x.tan(),
        "asin" => x.asin(),
        "acos" => x.acos(),
        "atan" => x.atan(),
        "ln" => x.ln(),
        "log" => x.log10(),
        "log2" => x.log2(),
        "exp" => x.exp(),
        "abs" => x.abs(),
        "floor" => x.floor(),
        "ceil" => x.ceil(),
        "round" => x.round(),
        _ => return None,
    })
}

/// Whether `s` is worth evaluating: arithmetic, not a bare number or a word.
fn looks_like_math(toks: &[Tok]) -> bool {
    let has_value = toks
        .iter()
        .any(|t| matches!(t, Tok::Num(_) | Tok::Ident(_)));
    let structural = toks.iter().enumerate().any(|(i, t)| match t {
        Tok::Plus | Tok::Star | Tok::Slash | Tok::Caret | Tok::LParen => true,
        Tok::Minus | Tok::Percent => i > 0,
        Tok::Ident(n) => constant(n).is_some() || matches!(toks.get(i + 1), Some(Tok::LParen)),
        _ => false,
    });
    has_value && structural
}

/// The longest input evaluated, in bytes.
const MAX_INPUT: usize = 200;

/// The value of the whole of `toks`, `None` when they are not valid arithmetic or the value is not finite.
fn value_of(toks: Vec<Tok>) -> Option<f64> {
    let mut p = Parser {
        t: toks,
        p: 0,
        depth: 0,
    };
    let value = p.expr()?;
    (p.p == p.t.len() && value.is_finite()).then_some(value)
}

/// The value of `input` as a number or any arithmetic, a bare number included (`12`, `-3.5`, `2*(1+2)`): the quantity
/// of a conversion. `None` for anything else, and on the same failures as [`evaluate`].
pub fn number(input: &str) -> Option<f64> {
    let input = input.trim();
    if input.is_empty() || input.len() > MAX_INPUT {
        return None;
    }
    value_of(lex(input)?)
}

/// Evaluate `input`; `None` when it is not (valid) arithmetic.
pub fn evaluate(input: &str) -> Option<Calculation> {
    let input = input.trim().trim_start_matches('=').trim();
    if input.is_empty() || input.len() > MAX_INPUT {
        return None;
    }
    let toks = lex(input)?;
    if !looks_like_math(&toks) {
        return None;
    }
    let value = value_of(toks)?;
    Some(Calculation {
        expression: input.to_string(),
        value,
        text: format_number(value),
    })
}

/// Display format: up to 12 significant digits, no trailing zeros, scientific notation for very
/// large or small magnitudes.
pub fn format_number(v: f64) -> String {
    if v == 0.0 {
        return "0".into();
    }
    let a = v.abs();
    if !(1e-6..1e15).contains(&a) {
        let s = format!("{v:.11e}");
        let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
        let mant = if mant.contains('.') {
            mant.trim_end_matches('0').trim_end_matches('.')
        } else {
            mant
        };
        return format!("{mant}e{exp}");
    }
    let decimals = (11 - a.log10().floor() as i32).clamp(0, 20) as usize;
    let s = format!("{v:.decimals$}");
    let s = if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        s
    };
    if s == "-0" {
        "0".into()
    } else {
        s
    }
}
