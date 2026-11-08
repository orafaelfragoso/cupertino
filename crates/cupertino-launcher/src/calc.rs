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

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn val(s: &str) -> f64 {
        evaluate(s)
            .unwrap_or_else(|| panic!("{s:?} should evaluate"))
            .value
    }

    fn text(s: &str) -> String {
        evaluate(s)
            .unwrap_or_else(|| panic!("{s:?} should evaluate"))
            .text
    }

    #[test]
    fn basic_arithmetic_and_precedence() {
        assert_eq!(val("1+2*3"), 7.0);
        assert_eq!(val("(1+2)*3"), 9.0);
        assert_eq!(val("10/4"), 2.5);
        assert_eq!(val("2 - 3 - 4"), -5.0);
        assert_eq!(val("8/4/2"), 1.0);
        assert_eq!(val("7 % 4"), 3.0);
        assert_eq!(val("2*(3+4)*5"), 70.0);
    }

    #[test]
    fn power_is_right_associative_and_binds_tighter_than_unary_minus() {
        assert_eq!(val("2^3^2"), 512.0);
        assert_eq!(val("-2^2"), -4.0);
        assert_eq!(val("2**10"), 1024.0);
        assert_eq!(val("2^-1"), 0.5);
        assert_eq!(val("(-2)^2"), 4.0);
    }

    #[test]
    fn unary_operators_and_signs() {
        assert_eq!(val("-5+3"), -2.0);
        assert_eq!(val("--5 + 0"), 5.0);
        assert_eq!(val("3*-2"), -6.0);
        assert_eq!(val("+4 + 1"), 5.0);
    }

    #[test]
    fn percent_postfix_and_modulo() {
        assert_eq!(val("50%*2"), 1.0);
        assert!((val("200*15%") - 30.0).abs() < 1e-12);
        assert_eq!(val("10 % 3 + 1"), 2.0);
        assert!((val("5%") - 0.05).abs() < 1e-12);
        // a sign after the percent sign is the next operator, not the start of a divisor
        assert!((val("50% + 10") - 10.5).abs() < 1e-12);
        assert!((val("10%-5") + 4.9).abs() < 1e-12);
        assert!((val("50%*-2") + 1.0).abs() < 1e-12);
        assert_eq!(val("7 % (0-2)"), 1.0);
    }

    #[test]
    fn unicode_operators() {
        assert_eq!(val("6×7"), 42.0);
        assert_eq!(val("84÷2"), 42.0);
        assert_eq!(val("5−3"), 2.0);
    }

    #[test]
    fn numbers_with_separators_and_exponents() {
        assert_eq!(val("1_000 + 1"), 1001.0);
        assert_eq!(val("1e3+1"), 1001.0);
        assert_eq!(val("1.5e-1*10"), 1.5);
        assert_eq!(val(".5+.5"), 1.0);
        assert!(
            evaluate("2e").is_none(),
            "implicit multiplication is not supported"
        );
    }

    #[test]
    fn constants_and_functions() {
        assert!((val("pi*2") - std::f64::consts::TAU).abs() < 1e-12);
        assert!((val("2*π") - std::f64::consts::TAU).abs() < 1e-12);
        assert_eq!(val("sqrt(16)+0"), 4.0);
        assert_eq!(val("abs(-3)*1"), 3.0);
        assert!((val("ln(e)+0") - 1.0).abs() < 1e-12);
        assert_eq!(val("log(1000)+0"), 3.0);
        assert_eq!(val("log2(8)+0"), 3.0);
        assert!((val("sin(pi/2)") - 1.0).abs() < 1e-12);
        assert_eq!(val("floor(2.7)+ceil(2.1)+round(2.5)"), 8.0);
        assert_eq!(val("SQRT(9)+1"), 4.0, "case-insensitive");
    }

    #[test]
    fn only_arithmetic_is_answered() {
        for s in [
            "",
            "42",
            "-5",
            "3.14",
            "firefox",
            "hello world",
            "sqrt",
            "pi pi",
            "12 pears",
            "(",
            ")",
            "1 +",
            "+",
            "* 3",
            "()",
        ] {
            assert!(evaluate(s).is_none(), "{s:?} must not produce a result");
        }
        assert!(evaluate("pi").is_some(), "a constant is arithmetic enough");
        assert!(evaluate("e").is_some());
    }

    #[test]
    fn invalid_arithmetic_is_rejected() {
        for s in [
            "1/0",
            "5 % 0",
            "sqrt(-1)",
            "2^10000",
            "1+2)",
            "(1+2",
            "foo(3)",
            "ln(0)*0+0",
            "1 2",
            "1++",
            "3 * * 4",
            "log(-1)+0",
            "2(3)",
        ] {
            assert!(evaluate(s).is_none(), "{s:?}");
        }
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let deep = format!("{}1{}+1", "(".repeat(500), ")".repeat(500));
        assert!(evaluate(&deep).is_none());
        let ok = format!("{}1{}+1", "(".repeat(20), ")".repeat(20));
        assert_eq!(val(&ok), 2.0);
        let unary = format!("{}1+1", "-".repeat(500));
        assert!(evaluate(&unary).is_none());
        assert!(evaluate(&"1+".repeat(150)).is_none());
        let long = "1+".repeat(80) + "1";
        assert_eq!(val(&long), 81.0);
    }

    #[test]
    fn leading_equals_is_accepted() {
        assert_eq!(val("=1+1"), 2.0);
        assert_eq!(evaluate("= 2*3").unwrap().expression, "2*3");
    }

    #[test]
    fn formatting() {
        assert_eq!(text("1/3"), "0.333333333333");
        assert_eq!(text("2/3"), "0.666666666667");
        assert_eq!(text("0.1+0.2"), "0.3");
        assert_eq!(text("10/4"), "2.5");
        assert_eq!(text("100*100"), "10000");
        assert_eq!(text("1e20*10"), "1e21");
        assert_eq!(text("1/1e8"), "1e-8");
        assert_eq!(text("2^0.5"), "1.41421356237");
        assert_eq!(text("123456789*1000"), "123456789000");
        assert_eq!(text("-1.50*1"), "-1.5");
        assert_eq!(format_number(0.0), "0");
        assert_eq!(format_number(-0.0), "0");
        assert_eq!(format_number(1234.5), "1234.5");
        assert_eq!(format_number(1.5e-7), "1.5e-7");
        assert_eq!(format_number(-2.5e22), "-2.5e22");
    }

    #[test]
    fn division_by_zero_is_no_result() {
        for s in [
            "1/0",
            "0/0",
            "5 ÷ 0",
            "1/(2-2)",
            "7 % (3-3)",
            "1/0*0",
            "-1/0",
        ] {
            assert!(evaluate(s).is_none(), "{s:?}");
        }
        assert_eq!(val("0/5"), 0.0);
    }

    #[test]
    fn huge_numbers_stay_finite_or_give_no_result() {
        assert_eq!(text("99999999999999999999*1"), "1e20");
        assert_eq!(text("1e300*1e8"), "1e308");
        assert_eq!(text("2^1000*1"), "1.07150860719e301");
        for s in [
            "1e308*10",
            "2^1024",
            "10^400",
            "exp(1000)",
            "1e309+1",
            "-1e308*10",
        ] {
            assert!(evaluate(s).is_none(), "{s:?} overflows");
        }
        assert_eq!(text("1/1e300"), "1e-300");
        assert!(evaluate("1e-320*1").is_some(), "subnormals still show");
    }

    #[test]
    fn a_number_is_any_arithmetic_or_a_bare_number() {
        assert_eq!(number("42"), Some(42.0));
        assert_eq!(number("-3.5"), Some(-3.5));
        assert_eq!(number(" 2*(1+2) "), Some(6.0));
        assert_eq!(number("pi"), Some(std::f64::consts::PI));
        for s in ["", "abc", "1/0", "2^2000", "1 2", "(1", &"1+".repeat(150)] {
            assert_eq!(number(s), None, "{s:?}");
        }
    }

    /// Pieces the fuzz tests put together: numbers at the edges of the float range, every operator and name the
    /// calculator and the converter read, separators, and characters they do not (a decimal comma, a factorial, NUL,
    /// a combining accent, letters whose lower case changes length, an emoji).
    pub(crate) const FUZZ_PIECES: &[&str] = &[
        "0",
        "1",
        "7",
        "9",
        "12.5",
        ".",
        "_",
        "e",
        "E",
        "1e308",
        "1e-320",
        "1e999",
        "9e9999999999",
        "pi",
        "π",
        "τ",
        "tau",
        "sqrt(",
        "ln(",
        "log(",
        "log2(",
        "exp(",
        "abs(",
        "sin(",
        "(",
        ")",
        "+",
        "-",
        "−",
        "–",
        "*",
        "**",
        "×",
        "·",
        "/",
        "÷",
        "^",
        "%",
        "!",
        ",",
        " ",
        "\t",
        "=",
        " in ",
        " to ",
        " as ",
        "->",
        "→",
        "km",
        "mi",
        "°c",
        "f",
        "k",
        "K",
        "ft",
        "\"",
        "'",
        "m²",
        "fl oz",
        "\u{130}",
        "\u{1e9e}",
        "\u{212a}",
        "é",
        "ß",
        "😀",
        "\0",
        "\u{301}",
    ];

    /// A small deterministic generator, so a failure names its seed and repeats.
    pub(crate) struct Fuzz(pub u64);

    impl Fuzz {
        pub(crate) fn next(&mut self) -> u64 {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            self.0
        }

        pub(crate) fn below(&mut self, n: usize) -> usize {
            (self.next() % n as u64) as usize
        }

        /// Up to `pieces` pieces, now and then a character from anywhere in Unicode instead.
        pub(crate) fn input(&mut self, pieces: usize) -> String {
            let mut s = String::new();
            for _ in 0..self.below(pieces + 1) {
                if self.below(16) == 0 {
                    s.extend(char::from_u32(self.next() as u32 % 0x11_0000));
                } else {
                    s.push_str(FUZZ_PIECES[self.below(FUZZ_PIECES.len())]);
                }
            }
            s
        }
    }

    /// Whatever the input, the calculator returns, and a value it gives is finite and reads as a number.
    fn check(input: &str) {
        if let Some(c) = evaluate(input) {
            assert!(c.value.is_finite(), "{input:?} gave {}", c.value);
            assert!(
                !c.text.is_empty() && !c.text.contains("inf") && !c.text.contains("NaN"),
                "{input:?} shows {:?}",
                c.text
            );
        }
        if let Some(v) = number(input) {
            assert!(v.is_finite(), "{input:?} gave {v}");
        }
    }

    #[test]
    fn fuzzed_input_never_panics_and_never_shows_a_non_finite_value() {
        let mut fuzz = Fuzz(0x9e37_79b9_7f4a_7c15);
        for _ in 0..20_000 {
            check(&fuzz.input(24));
        }
        // past the input limit, and nesting of every kind far past the depth limit
        for n in [63, 64, 65, 100, 199, 200, 201, 10_000] {
            for input in [
                format!("{}1{}", "(".repeat(n), ")".repeat(n)),
                format!("{}1", "-".repeat(n)),
                format!("2{}", "^2".repeat(n)),
                format!("{}1{}", "sqrt(".repeat(n), ")".repeat(n)),
                format!("1{}", "%".repeat(n)),
                "9".repeat(n) + "*1",
                "1+".repeat(n) + "1",
            ] {
                check(&input);
            }
        }
        // the float range's edges
        for input in [
            "0^0",
            "0^-1",
            "(-8)^(1/3)",
            "1e308+1e308",
            "-1e308-1e308",
            "1e-320/1e10",
            "0*1e999",
            "1e999-1e999",
            "exp(709)",
            "exp(710)",
            "ln(0)",
            "tan(pi/2)",
            "asin(2)",
            "5!",
            "1,5+1",
            "1.5e+308*10",
            "2^-1074",
            "2^-1075",
            "10^308",
            "10^309",
        ] {
            check(input);
        }
        assert_eq!(val("0^0"), 1.0);
    }

    #[test]
    fn garbage_never_panics() {
        for s in [
            "((((", "1e", "e1", "..", "1..2", "1.2.3+1", "😀+1", "1e+", "%%%", "^^", "1_", "__",
        ] {
            let _ = evaluate(s);
        }
    }
}
