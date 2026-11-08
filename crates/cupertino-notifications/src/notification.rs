//! Notification data model: what a client sends (`NotifyInput`) and what the server keeps
//! (`Notification`).

use crate::markup::{self, Span};
use std::borrow::Cow;
use unicode_segmentation::UnicodeSegmentation;

/// Longest application name kept, in characters; a longer one is cut and ends in an ellipsis.
pub const MAX_APP_NAME_CHARS: usize = 64;
/// Longest summary kept, in characters; a longer one is cut and ends in an ellipsis.
pub const MAX_SUMMARY_CHARS: usize = 256;
/// Longest body kept, in characters, markup included; a longer one is cut and ends in an ellipsis. The cards lay out
/// still less of it (`view::MAX_BODY_BYTES`).
pub const MAX_BODY_CHARS: usize = 8192;
/// What ends a text that was cut.
pub const ELLIPSIS: char = '\u{2026}';
/// Longest `desktop-entry` hint kept, in bytes (a file name's bound); a longer one is dropped.
pub const MAX_DESKTOP_ENTRY_BYTES: usize = 255;

/// `text` with at most `max` characters: a longer one keeps the whole user-perceived characters (grapheme clusters)
/// that fit in its first `max - 1` and ends in [`ELLIPSIS`], so a cut never splits a letter from its marks, an emoji
/// sequence or a flag.
pub fn cap(text: &str, max: usize) -> Cow<'_, str> {
    if text.char_indices().nth(max).is_none() {
        return Cow::Borrowed(text);
    }
    let room = max.saturating_sub(1);
    let (mut end, mut used) = (0, 0);
    for (at, cluster) in text.grapheme_indices(true) {
        let chars = cluster.chars().count();
        if used + chars > room {
            break;
        }
        used += chars;
        end = at + cluster.len();
    }
    let mut out = text[..end].trim_end().to_owned();
    if max > 0 {
        out.push(ELLIPSIS);
    }
    Cow::Owned(out)
}

/// The longest entity the body markup decodes, from `&` to `;` (`markup::decode_entities`).
const MAX_ENTITY_BYTES: usize = 10;

/// [`cap`] of a body in markup: a cut that falls inside a tag or an entity drops what is left of it, so the parser
/// never shows half a tag or entity as text.
pub fn cap_markup(text: &str, max: usize) -> Cow<'_, str> {
    let Cow::Owned(cut) = cap(text, max) else {
        return Cow::Borrowed(text);
    };
    let kept = cut.strip_suffix(ELLIPSIS).unwrap_or(&cut);
    let open_tag = kept.rfind('<').filter(|&i| !kept[i..].contains('>'));
    let open_entity = kept
        .rfind('&')
        .filter(|&i| !kept[i..].contains(';') && kept.len() - i <= MAX_ENTITY_BYTES);
    let Some(end) = open_tag.into_iter().chain(open_entity).min() else {
        return Cow::Owned(cut);
    };
    let mut out = kept[..end].trim_end().to_owned();
    out.push(ELLIPSIS);
    Cow::Owned(out)
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Urgency {
    Low = 0,
    #[default]
    Normal = 1,
    Critical = 2,
}

impl Urgency {
    pub fn from_byte(b: u8) -> Urgency {
        match b {
            0 => Urgency::Low,
            2.. => Urgency::Critical,
            _ => Urgency::Normal,
        }
    }
}

/// Reason codes of the `NotificationClosed` signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CloseReason {
    Expired = 1,
    Dismissed = 2,
    Closed = 3,
    Undefined = 4,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    pub key: String,
    pub label: String,
}

/// Key of the action invoked by clicking the banner itself (it has no button of its own).
pub const DEFAULT_ACTION_KEY: &str = "default";

/// Raw image of an `image-data` hint, converted to straight-alpha RGBA8.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl ImageData {
    /// Convert the `(iiibiiay)` structure of the specification. Only 8 bits per sample with 3
    /// (RGB) or 4 (RGBA) channels are accepted; anything else, or an inconsistent buffer, is `None`.
    pub fn from_dbus(
        width: i32,
        height: i32,
        rowstride: i32,
        has_alpha: bool,
        bits_per_sample: i32,
        channels: i32,
        data: &[u8],
    ) -> Option<ImageData> {
        if width <= 0
            || height <= 0
            || bits_per_sample != 8
            || !(channels == 3 || channels == 4)
            || (channels == 4) != has_alpha
        {
            return None;
        }
        let (w, h, stride, ch) = (
            width as usize,
            height as usize,
            rowstride as usize,
            channels as usize,
        );
        if rowstride <= 0 || stride < w * ch || w > 4096 || h > 4096 {
            return None;
        }
        if data.len() < stride * (h - 1) + w * ch {
            return None;
        }
        let mut rgba = Vec::with_capacity(w * h * 4);
        for y in 0..h {
            let row = &data[y * stride..y * stride + w * ch];
            for px in row.chunks_exact(ch) {
                rgba.extend_from_slice(&px[..3]);
                rgba.push(if ch == 4 { px[3] } else { 255 });
            }
        }
        Some(ImageData {
            width: width as u32,
            height: height as u32,
            rgba,
        })
    }
}

/// `expire_timeout` of a `Notify` call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Expire {
    /// −1: the server decides (by urgency).
    Default,
    /// 0: never expires.
    Never,
    Millis(u32),
}

impl Expire {
    pub fn from_dbus(v: i32) -> Expire {
        match v {
            -1 => Expire::Default,
            0 => Expire::Never,
            n if n > 0 => Expire::Millis(n as u32),
            _ => Expire::Default,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Hints {
    pub urgency: Option<Urgency>,
    pub category: Option<String>,
    pub transient: bool,
    pub resident: bool,
    pub image: Option<ImageData>,
    pub image_path: Option<String>,
    pub desktop_entry: Option<String>,
    pub suppress_sound: bool,
}

/// Arguments of `Notify`, decoded from the bus (or built directly by the host, e.g. for the
/// compositor's own messages).
#[derive(Clone, Debug, PartialEq)]
pub struct NotifyInput {
    pub app_name: String,
    pub replaces_id: u32,
    pub icon: String,
    pub summary: String,
    pub body: String,
    /// Flat list of `[key, label, key, label, ...]`; an odd trailing element is ignored.
    pub actions: Vec<String>,
    pub expire: Expire,
    pub hints: Hints,
}

impl NotifyInput {
    /// The input with its application name, summary and body cut to [`MAX_APP_NAME_CHARS`], [`MAX_SUMMARY_CHARS`] and
    /// [`MAX_BODY_CHARS`], as the bus server keeps them.
    pub fn capped(mut self) -> Self {
        for (text, max) in [
            (&mut self.app_name, MAX_APP_NAME_CHARS),
            (&mut self.summary, MAX_SUMMARY_CHARS),
        ] {
            if let Cow::Owned(cut) = cap(text, max) {
                *text = cut;
            }
        }
        if let Cow::Owned(cut) = cap_markup(&self.body, MAX_BODY_CHARS) {
            self.body = cut;
        }
        // no desktop entry id is longer than a file name; a longer hint names none, and the name stands in
        if self
            .hints
            .desktop_entry
            .as_ref()
            .is_some_and(|d| d.len() > MAX_DESKTOP_ENTRY_BYTES)
        {
            self.hints.desktop_entry = None;
        }
        self
    }

    pub fn new(app_name: &str, summary: &str, body: &str) -> Self {
        Self {
            app_name: app_name.into(),
            replaces_id: 0,
            icon: String::new(),
            summary: summary.into(),
            body: body.into(),
            actions: Vec::new(),
            expire: Expire::Default,
            hints: Hints::default(),
        }
    }

    pub fn with_action(mut self, key: &str, label: &str) -> Self {
        self.actions.push(key.into());
        self.actions.push(label.into());
        self
    }

    pub fn with_urgency(mut self, u: Urgency) -> Self {
        self.hints.urgency = Some(u);
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Notification {
    pub id: u32,
    pub app_name: String,
    pub icon: String,
    pub summary: String,
    pub body: String,
    pub spans: Vec<Span>,
    pub actions: Vec<Action>,
    pub expire: Expire,
    pub urgency: Urgency,
    pub category: Option<String>,
    pub transient: bool,
    pub resident: bool,
    pub image: Option<ImageData>,
    pub image_path: Option<String>,
    pub desktop_entry: Option<String>,
    /// Time of the latest `Notify` for this id, in the host's clock (seconds).
    pub timestamp: f64,
    /// Number of times the id was replaced.
    pub revision: u32,
}

impl Notification {
    pub fn from_input(id: u32, input: NotifyInput, now: f64) -> Notification {
        let actions = input
            .actions
            .chunks_exact(2)
            .map(|p| Action {
                key: p[0].clone(),
                label: p[1].clone(),
            })
            .collect();
        Notification {
            id,
            spans: markup::parse(&input.body),
            app_name: input.app_name,
            icon: input.icon,
            summary: input.summary,
            body: input.body,
            actions,
            expire: input.expire,
            urgency: input.hints.urgency.unwrap_or_default(),
            category: input.hints.category,
            transient: input.hints.transient,
            resident: input.hints.resident,
            image: input.hints.image,
            image_path: input.hints.image_path,
            desktop_entry: input.hints.desktop_entry,
            timestamp: now,
            revision: 0,
        }
    }

    /// Plain body text with the markup removed.
    pub fn body_text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }

    pub fn has_default_action(&self) -> bool {
        self.actions.iter().any(|a| a.key == DEFAULT_ACTION_KEY)
    }

    /// Actions that get a button (everything except the default action).
    pub fn button_actions(&self) -> impl Iterator<Item = &Action> {
        self.actions.iter().filter(|a| a.key != DEFAULT_ACTION_KEY)
    }

    /// Grouping key: the desktop entry when given (stable across clients that name themselves
    /// differently), else the application name.
    pub fn group_key(&self) -> String {
        match &self.desktop_entry {
            Some(d) if !d.is_empty() => d.trim_end_matches(".desktop").to_string(),
            _ => self.app_name.clone(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_data_converts_rgb_with_row_padding() {
        // 2x2 RGB, rowstride 8 (2 bytes padding per row)
        let data = [1, 2, 3, 4, 5, 6, 0, 0, 7, 8, 9, 10, 11, 12, 0, 0];
        let img = ImageData::from_dbus(2, 2, 8, false, 8, 3, &data).unwrap();
        assert_eq!(
            img.rgba,
            vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]
        );
    }

    #[test]
    fn image_data_rejects_inconsistent_input() {
        let d = vec![0u8; 64];
        assert!(ImageData::from_dbus(0, 2, 8, true, 8, 4, &d).is_none());
        assert!(ImageData::from_dbus(2, 2, 8, true, 16, 4, &d).is_none());
        assert!(ImageData::from_dbus(2, 2, 8, false, 8, 4, &d).is_none());
        assert!(ImageData::from_dbus(2, 2, 4, true, 8, 4, &d).is_none());
        assert!(ImageData::from_dbus(4, 4, 16, true, 8, 4, &d[..40]).is_none());
        assert!(ImageData::from_dbus(2, 2, 8, true, 8, 4, &d).is_some());
    }

    #[test]
    fn expire_and_urgency_decoding() {
        assert_eq!(Expire::from_dbus(-1), Expire::Default);
        assert_eq!(Expire::from_dbus(0), Expire::Never);
        assert_eq!(Expire::from_dbus(2500), Expire::Millis(2500));
        assert_eq!(Expire::from_dbus(-7), Expire::Default);
        assert_eq!(Urgency::from_byte(0), Urgency::Low);
        assert_eq!(Urgency::from_byte(1), Urgency::Normal);
        assert_eq!(Urgency::from_byte(9), Urgency::Critical);
    }

    #[test]
    fn a_long_text_is_cut_to_its_cap_with_an_ellipsis_and_a_short_one_kept() {
        assert_eq!(cap("Mail", 4), "Mail");
        assert_eq!(cap("Mailbox", 4), "Mai\u{2026}");
        // characters, not bytes, and no space before the ellipsis
        assert_eq!(cap("é é é é", 4), "é é\u{2026}");
        let long = NotifyInput::new(
            &"a".repeat(MAX_APP_NAME_CHARS + 10),
            &"s".repeat(MAX_SUMMARY_CHARS + 1),
            &"b".repeat(MAX_BODY_CHARS * 2),
        )
        .capped();
        for (text, max) in [
            (&long.app_name, MAX_APP_NAME_CHARS),
            (&long.summary, MAX_SUMMARY_CHARS),
            (&long.body, MAX_BODY_CHARS),
        ] {
            assert_eq!(text.chars().count(), max);
            assert!(text.ends_with(ELLIPSIS));
        }
        let short = NotifyInput::new("Mail", "Lunch", "At noon?").capped();
        assert_eq!(
            (
                short.app_name.as_str(),
                short.summary.as_str(),
                short.body.as_str()
            ),
            ("Mail", "Lunch", "At noon?")
        );
    }

    #[test]
    fn a_cut_keeps_whole_clusters_and_right_to_left_text_in_its_order() {
        // a letter and its combining mark stay together: "e\u{301}" is two characters
        assert_eq!(cap("ae\u{301}e\u{301}", 3), "a\u{2026}");
        assert_eq!(cap("ae\u{301}e\u{301}", 4), "ae\u{301}\u{2026}");
        // a family emoji (five characters joined) and a flag (two) are kept whole or dropped whole
        let family = "\u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}";
        assert_eq!(
            cap(&format!("{family}{family}"), 8),
            format!("{family}\u{2026}")
        );
        assert_eq!(cap(&format!("{family}{family}"), 5), "\u{2026}");
        let flags = "\u{1F1EB}\u{1F1F7}\u{1F1E9}\u{1F1EA}";
        assert_eq!(cap(flags, 3), "\u{1F1EB}\u{1F1F7}\u{2026}");
        assert_eq!(cap(flags, 2), "\u{2026}");
        // right-to-left text keeps its first characters in reading (logical) order
        assert_eq!(cap("שלום עולם", 5), "שלום\u{2026}");
        for text in ["ae\u{301}e\u{301}", "שלום עולם", family] {
            for max in 0..12 {
                assert!(
                    cap(text, max).chars().count() <= max.max(1),
                    "{text:?} {max}"
                );
            }
        }
    }

    #[test]
    fn a_body_cut_inside_a_tag_or_an_entity_drops_the_rest_of_it() {
        let cut = |body: &str, max| cap_markup(body, max).into_owned();
        assert_eq!(cut("Hi <b>there</b>", 6), "Hi\u{2026}");
        assert_eq!(cut("Hi <a href=\"https://x\">x</a>", 12), "Hi\u{2026}");
        assert_eq!(cut("Tom &amp; Jerry", 8), "Tom\u{2026}");
        assert_eq!(cut("<b>bold text</b> more", 9), "<b>bold\u{2026}");
        assert_eq!(cut("a < b and more", 6), "a\u{2026}");
        assert_eq!(cut("short", 9), "short");
        // what the parser makes of a cut body: no part of a tag or entity is text
        for max in 1..40 {
            let body =
                "<b>Ada</b> &quot;hi&quot; <a href=\"https://example.org\">a link</a> &#x263A; end";
            let text: String = markup::parse(&cap_markup(body, max))
                .into_iter()
                .map(|s| s.text)
                .collect();
            assert!(!text.contains(['<', '>', '&']), "{max}: {text:?}");
        }
    }

    #[test]
    fn an_overlong_desktop_entry_names_no_application() {
        let mut inp = NotifyInput::new("Mail", "Hi", "");
        inp.hints.desktop_entry = Some("a".repeat(MAX_DESKTOP_ENTRY_BYTES + 1));
        assert_eq!(inp.clone().capped().hints.desktop_entry, None);
        inp.hints.desktop_entry = Some("org.example.Mail".into());
        assert_eq!(
            inp.capped().hints.desktop_entry.as_deref(),
            Some("org.example.Mail")
        );
    }

    #[test]
    fn input_conversion() {
        let mut inp = NotifyInput::new("App", "Hi", "<b>x</b>")
            .with_action("default", "Open")
            .with_action("reply", "Reply");
        inp.actions.push("dangling".into());
        inp.hints.desktop_entry = Some("org.example.App.desktop".into());
        let n = Notification::from_input(5, inp, 1.0);
        assert_eq!(n.actions.len(), 2);
        assert!(n.has_default_action());
        assert_eq!(n.button_actions().count(), 1);
        assert_eq!(n.body_text(), "x");
        assert_eq!(n.group_key(), "org.example.App");
    }
}
