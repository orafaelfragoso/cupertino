//! Notification data model: what a client sends (`NotifyInput`) and what the server keeps
//! (`Notification`).

use crate::markup::{self, Span};
use tiny_skia::Pixmap;

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
    pub fn from_dbus(width: i32, height: i32, rowstride: i32, has_alpha: bool, bits_per_sample: i32, channels: i32, data: &[u8]) -> Option<ImageData> {
        if width <= 0 || height <= 0 || bits_per_sample != 8 || !(channels == 3 || channels == 4) || (channels == 4) != has_alpha {
            return None;
        }
        let (w, h, stride, ch) = (width as usize, height as usize, rowstride as usize, channels as usize);
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
        Some(ImageData { width: width as u32, height: height as u32, rgba })
    }

    pub fn to_pixmap(&self) -> Option<Pixmap> {
        let mut pm = Pixmap::new(self.width, self.height)?;
        for (dst, src) in pm.pixels_mut().iter_mut().zip(self.rgba.chunks_exact(4)) {
            *dst = tiny_skia::ColorU8::from_rgba(src[0], src[1], src[2], src[3]).premultiply();
        }
        Some(pm)
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
        let actions = input.actions.chunks_exact(2).map(|p| Action { key: p[0].clone(), label: p[1].clone() }).collect();
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
        assert_eq!(img.rgba, vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]);
        let pm = img.to_pixmap().unwrap();
        assert_eq!((pm.width(), pm.height()), (2, 2));
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
    fn premultiplication_of_alpha_pixels() {
        let img = ImageData { width: 1, height: 1, rgba: vec![255, 0, 0, 128] };
        let pm = img.to_pixmap().unwrap();
        let p = pm.pixels()[0];
        assert_eq!(p.alpha(), 128);
        assert!((p.red() as i32 - 128).abs() <= 1);
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
    fn input_conversion() {
        let mut inp = NotifyInput::new("App", "Hi", "<b>x</b>").with_action("default", "Open").with_action("reply", "Reply");
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
