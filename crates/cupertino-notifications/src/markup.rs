//! Body markup of the notification specification: `<b>`, `<i>`, `<u>`, `<a href>` and `<img>`.
//!
//! Any other tag is dropped and its text kept. `<img>` is replaced by its `alt` text (images in
//! bodies are not rendered). Malformed input never fails: a `<` that does not start a tag is
//! literal text, unclosed tags end at the end of the body and stray closing tags are ignored.

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Span {
    pub text: String,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub link: Option<String>,
}

impl Span {
    fn same_style(&self, o: &Span) -> bool {
        self.bold == o.bold
            && self.italic == o.italic
            && self.underline == o.underline
            && self.link == o.link
    }
}

pub fn decode_entities(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let decoded = rest.find(';').filter(|&e| e <= 10).and_then(|e| {
            let name = &rest[1..e];
            let ch = match name {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ => name
                    .strip_prefix("#x")
                    .or_else(|| name.strip_prefix("#X"))
                    .and_then(|h| u32::from_str_radix(h, 16).ok())
                    .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                    .and_then(char::from_u32),
            };
            ch.map(|c| (c, e + 1))
        });
        match decoded {
            Some((c, used)) => {
                out.push(c);
                rest = &rest[used..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

struct Tag<'a> {
    name: String,
    closing: bool,
    attrs: &'a str,
}

fn parse_tag(inner: &str) -> Option<Tag<'_>> {
    let (closing, body) = match inner.strip_prefix('/') {
        Some(b) => (true, b),
        None => (false, inner),
    };
    let body = body.strip_suffix('/').unwrap_or(body);
    let name_end = body.find(|c: char| c.is_whitespace()).unwrap_or(body.len());
    let name = &body[..name_end];
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    Some(Tag {
        name: name.to_ascii_lowercase(),
        closing,
        attrs: &body[name_end..],
    })
}

fn attribute(attrs: &str, key: &str) -> Option<String> {
    let mut rest = attrs;
    while let Some(eq) = rest.find('=') {
        let name = rest[..eq].trim();
        let after = rest[eq + 1..].trim_start();
        let (value, tail) = match after.chars().next()? {
            q @ ('"' | '\'') => {
                let end = after[1..].find(q)?;
                (&after[1..1 + end], &after[end + 2..])
            }
            _ => {
                let end = after.find(char::is_whitespace).unwrap_or(after.len());
                (&after[..end], &after[end..])
            }
        };
        if name.eq_ignore_ascii_case(key) {
            return Some(decode_entities(value));
        }
        rest = tail;
    }
    None
}

pub fn parse(body: &str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let (mut bold, mut italic, mut underline) = (0u32, 0u32, 0u32);
    let mut links: Vec<String> = Vec::new();
    let push = |text: String,
                bold: u32,
                italic: u32,
                underline: u32,
                link: Option<&String>,
                spans: &mut Vec<Span>| {
        if text.is_empty() {
            return;
        }
        let s = Span {
            text,
            bold: bold > 0,
            italic: italic > 0,
            underline: underline > 0 || link.is_some(),
            link: link.cloned(),
        };
        match spans.last_mut() {
            Some(last) if last.same_style(&s) => last.text.push_str(&s.text),
            _ => spans.push(s),
        }
    };
    let mut rest = body;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            push(
                decode_entities(rest),
                bold,
                italic,
                underline,
                links.last(),
                &mut spans,
            );
            break;
        };
        push(
            decode_entities(&rest[..lt]),
            bold,
            italic,
            underline,
            links.last(),
            &mut spans,
        );
        rest = &rest[lt..];
        let tag = rest
            .find('>')
            .and_then(|gt| parse_tag(&rest[1..gt]).map(|t| (t, gt)));
        let Some((tag, gt)) = tag else {
            push(
                "<".into(),
                bold,
                italic,
                underline,
                links.last(),
                &mut spans,
            );
            rest = &rest[1..];
            continue;
        };
        match (tag.name.as_str(), tag.closing) {
            ("b", false) => bold += 1,
            ("b", true) => bold = bold.saturating_sub(1),
            ("i", false) => italic += 1,
            ("i", true) => italic = italic.saturating_sub(1),
            ("u", false) => underline += 1,
            ("u", true) => underline = underline.saturating_sub(1),
            ("a", false) => links.push(attribute(tag.attrs, "href").unwrap_or_default()),
            ("a", true) => {
                links.pop();
            }
            ("img", false) => {
                let alt = attribute(tag.attrs, "alt").unwrap_or_default();
                push(alt, bold, italic, underline, links.last(), &mut spans);
            }
            ("br", false) => push(
                "\n".into(),
                bold,
                italic,
                underline,
                links.last(),
                &mut spans,
            ),
            _ => {}
        }
        rest = &rest[gt + 1..];
    }
    spans
}

pub fn strip(body: &str) -> String {
    parse(body).into_iter().map(|s| s.text).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plain(s: &str) -> Span {
        Span {
            text: s.into(),
            ..Default::default()
        }
    }

    #[test]
    fn plain_text_is_one_span() {
        assert_eq!(parse("hello world"), vec![plain("hello world")]);
        assert!(parse("").is_empty());
    }

    #[test]
    fn nested_styles() {
        let s = parse("a <b>bold <i>both</i></b> c");
        assert_eq!(s.len(), 4);
        assert!(s[1].bold && !s[1].italic && s[1].text == "bold ");
        assert!(s[2].bold && s[2].italic && s[2].text == "both");
        assert_eq!(s[3], plain(" c"));
    }

    #[test]
    fn links_are_underlined_and_keep_href() {
        let s = parse("see <a href=\"https://example.org/?a=1&amp;b=2\">here</a>.");
        assert_eq!(s[1].link.as_deref(), Some("https://example.org/?a=1&b=2"));
        assert!(s[1].underline);
        assert_eq!(strip("see <a href='x'>here</a>."), "see here.");
    }

    #[test]
    fn images_become_alt_text_and_unknown_tags_vanish() {
        assert_eq!(strip("x<img src=\"a.png\" alt=\"[pic]\"/>y"), "x[pic]y");
        assert_eq!(
            strip("<span color=\"red\">hi</span> <blink>there</blink>"),
            "hi there"
        );
    }

    #[test]
    fn entities() {
        assert_eq!(
            decode_entities("&lt;b&gt; &amp; &#65;&#x42; &quot;q&quot; &apos;"),
            "<b> & AB \"q\" '"
        );
        assert_eq!(
            decode_entities("fish & chips &unknown; &"),
            "fish & chips &unknown; &"
        );
        assert_eq!(strip("1 &lt; 2"), "1 < 2");
        assert_eq!(escape("<a & b>"), "&lt;a &amp; b&gt;");
    }

    #[test]
    fn malformed_input_never_panics() {
        assert_eq!(strip("a < b"), "a < b");
        assert_eq!(strip("<b>unclosed"), "unclosed");
        assert_eq!(strip("stray</b></i> text"), "stray text");
        assert_eq!(strip("<<b>x</b>"), "<x");
        assert_eq!(strip("<>"), "<>");
        assert_eq!(strip("tail <b"), "tail <b");
        let _ = parse("<a href=\"unterminated>x</a>");
    }

    #[test]
    fn line_breaks_survive() {
        assert_eq!(strip("one\ntwo<br/>three"), "one\ntwo\nthree");
    }
}
