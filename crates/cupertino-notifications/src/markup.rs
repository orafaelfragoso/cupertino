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
