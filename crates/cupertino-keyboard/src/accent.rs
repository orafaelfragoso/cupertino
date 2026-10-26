//! Accent picker (hold-a-key alternatives).
//!
//! Behaviour from the reference implementation:
//!
//! * the picker opens on the **first auto-repeat event** of a held character key, i.e. after the initial
//!   key-repeat delay; there is no separate hold timer;
//! * it does not open when control or the primary modifier is down, for decimal digits, for characters without
//!   alternatives, or when the feature is disabled globally or for the application;
//! * once open: Return commits the focused candidate, Escape cancels (restoring the original character), Tab and
//!   Left / Right move the focus, keys 1..=9 commit the n-th candidate directly; the focused candidate is shown as
//!   marked text by default; the base character is not part of the candidate list;
//! * candidate lists come from per-keyboard data files (`Roman-Accent-<char> keys with a space separated list).
//!   No list is embedded in this crate; [`AccentTable`] is filled from a JSON file or by the caller.
//!
//! Popup geometry could not be (unknown); [`PopupMetrics`] are configurable placeholders
//! (approximate).
//!
//! Wayland: run the picker in an input-method client (`zwp_input_method_v2`): grab the keyboard to observe key and
//! repeat events (the grab delivers `repeat_info` but no repeats - generate them with
//! [`crate::repeat::KeyRepeater`] and feed `repeat: true` to [`AccentPicker::key_down`]), show candidates in an
//! `input_popup_surface`, preview with `set_preedit_string`, finish with `commit_string`.

use std::collections::HashMap;

use serde::Deserialize;

use crate::keys::Mods;

/// Where the source data places the alternatives relative to the key ("right" / "left" / "strictly-left").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Direction {
    #[default]
    Right,
    Left,
    StrictlyLeft,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccentEntry {
    pub direction: Direction,
    /// All strings of the source list, base character first when present.
    pub strings: Vec<String>,
}

#[derive(Deserialize)]
struct RawEntry {
    direction: Option<String>,
    strings: Vec<String>,
}

/// Base character → alternatives.
#[derive(Clone, Debug, Default)]
pub struct AccentTable {
    map: HashMap<char, AccentEntry>,
}

impl AccentTable {
    pub fn new() -> AccentTable {
        AccentTable::default()
    }

    pub fn insert(&mut self, base: char, direction: Direction, strings: Vec<String>) {
        self.map.insert(base, AccentEntry { direction, strings });
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    pub fn entry(&self, base: char) -> Option<&AccentEntry> {
        self.map.get(&base)
    }

    /// Loads the merged table of the named keyboards from the JSON produced by the
    /// (`{ "<keyboard>": { "<char>": { "direction": "right", "strings": [...] } } }). Later names override earlier ones;
    /// pass e.g. `["default", "en"].
    pub fn from_json(text: &str, keyboards: &[&str]) -> Result<AccentTable, serde_json::Error> {
        let all: HashMap<String, HashMap<String, RawEntry>> = serde_json::from_str(text)?;
        let mut t = AccentTable::new();
        for kb in keyboards {
            if let Some(m) = all.get(*kb) {
                for (k, v) in m {
                    let mut it = k.chars();
                    if let (Some(c), None) = (it.next(), it.next()) {
                        let dir = match v.direction.as_deref() {
                            Some("left") => Direction::Left,
                            Some("strictly-left") => Direction::StrictlyLeft,
                            _ => Direction::Right,
                        };
                        t.insert(c, dir, v.strings.iter().filter(|s| !s.is_empty()).cloned().collect());
                    }
                }
            }
        }
        Ok(t)
    }
}

/// Picker settings.
#[derive(Clone, Debug)]
pub struct AccentConfig {
    /// Global switch (default on).
    pub enabled: bool,
    /// Application identifiers for which the picker is off (the design turns it off by default for a few
    /// terminal / editor applications).
    pub disabled_apps: Vec<String>,
    /// Remove the base character from the candidate list (default true in the reference implementation).
    pub remove_base: bool,
    /// Show the focused candidate as preview text (default true).
    pub preview_focused: bool,
    /// Mark the first candidate when the popup opens (default false).
    pub mark_on_open: bool,
    /// Wrap the focus at the ends (not determined, approximate: false).
    pub wrap: bool,
}

impl Default for AccentConfig {
    fn default() -> Self {
        AccentConfig { enabled: true, disabled_apps: Vec::new(), remove_base: true, preview_focused: true, mark_on_open: false, wrap: false }
    }
}

/// A key press as seen by the picker.
#[derive(Clone, Debug)]
pub struct KeyDown {
    /// Character produced by the key (before composition), if any.
    pub ch: Option<char>,
    pub key: PickerKey,
    pub repeat: bool,
    pub mods: Mods,
}

/// Non-character keys the picker reacts to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PickerKey {
    Char,
    Return,
    Escape,
    Tab,
    Left,
    Right,
    Other,
}

/// What the caller must do after a key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AccentEvent {
    /// The key is not for the picker; handle it normally.
    PassThrough,
    /// Open the popup with these candidates (numbered from 1; only the first nine have number shortcuts).
    Show { base: char, candidates: Vec<String>, focused: Option<usize> },
    /// The focus moved; show `text` as preview when preview is enabled.
    Focus { index: usize, preview: Option<String> },
    /// Insert `text` and close.
    Commit { text: String },
    /// Close; if `restore` is set re-insert the original character (a preview was showing).
    Cancel { restore: Option<String> },
    /// The key was consumed without visible change.
    Consumed,
}

#[derive(Clone, Debug)]
struct Open {
    base: char,
    candidates: Vec<String>,
    focused: Option<usize>,
}

/// The picker state machine.
#[derive(Clone, Debug)]
pub struct AccentPicker {
    cfg: AccentConfig,
    table: AccentTable,
    open: Option<Open>,
}

impl AccentPicker {
    pub fn new(cfg: AccentConfig, table: AccentTable) -> AccentPicker {
        AccentPicker { cfg, table, open: None }
    }

    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    pub fn candidates(&self) -> &[String] {
        self.open.as_ref().map(|o| o.candidates.as_slice()).unwrap_or(&[])
    }

    pub fn focused(&self) -> Option<usize> {
        self.open.as_ref().and_then(|o| o.focused)
    }

    /// Whether the picker may start for an application.
    pub fn allowed_for(&self, app_id: &str) -> bool {
        self.cfg.enabled && !self.cfg.disabled_apps.iter().any(|a| a == app_id)
    }

    /// Whether a held `base` key would open the popup (the table has alternatives for it).
    pub fn offers(&self, base: char) -> bool {
        !base.is_numeric() && !self.candidates_for(base).is_empty()
    }

    fn candidates_for(&self, base: char) -> Vec<String> {
        let Some(e) = self.table.entry(base) else { return Vec::new() };
        let b = base.to_string();
        e.strings.iter().filter(|s| !(self.cfg.remove_base && **s == b)).cloned().collect()
    }

    fn preview(&self, o: &Open) -> Option<String> {
        if self.cfg.preview_focused {
            o.focused.and_then(|i| o.candidates.get(i).cloned())
        } else {
            None
        }
    }

    fn move_focus(&mut self, delta: i32) -> AccentEvent {
        let wrap = self.cfg.wrap;
        let Some(o) = self.open.as_mut() else { return AccentEvent::PassThrough };
        let n = o.candidates.len() as i32;
        let cur = o.focused.map(|i| i as i32);
        let mut next = match cur {
            None if delta > 0 => 0,
            None => n - 1,
            Some(c) => c + delta,
        };
        if wrap {
            next = next.rem_euclid(n);
        } else {
            next = next.clamp(0, n - 1);
        }
        o.focused = Some(next as usize);
        let o = o.clone();
        AccentEvent::Focus { index: next as usize, preview: self.preview(&o) }
    }

    /// Feeds a key press. `app_id` is used for the per-application switch.
    pub fn key_down(&mut self, ev: &KeyDown, app_id: &str) -> AccentEvent {
        if self.open.is_none() {
            return self.try_open(ev, app_id);
        }
        match ev.key {
            PickerKey::Escape => {
                let o = self.open.take().unwrap();
                AccentEvent::Cancel { restore: o.focused.map(|_| o.base.to_string()) }
            }
            PickerKey::Return => {
                let o = self.open.take().unwrap();
                match o.focused.and_then(|i| o.candidates.get(i).cloned()) {
                    Some(text) => AccentEvent::Commit { text },
                    None => AccentEvent::Cancel { restore: None },
                }
            }
            PickerKey::Tab | PickerKey::Right => self.move_focus(1),
            PickerKey::Left => self.move_focus(-1),
            PickerKey::Char => {
                if let Some(d) = ev.ch.and_then(|c| c.to_digit(10)) {
                    let o = self.open.as_ref().unwrap();
                    if (1..=9).contains(&d) {
                        if let Some(text) = o.candidates.get(d as usize - 1).cloned() {
                            self.open = None;
                            return AccentEvent::Commit { text };
                        }
                        return AccentEvent::Consumed;
                    }
                }
                self.close_passthrough()
            }
            PickerKey::Other => self.close_passthrough(),
        }
    }

    fn close_passthrough(&mut self) -> AccentEvent {
        self.open = None;
        AccentEvent::PassThrough
    }

    fn try_open(&mut self, ev: &KeyDown, app_id: &str) -> AccentEvent {
        if !self.allowed_for(app_id) || !ev.repeat || ev.key != PickerKey::Char {
            return AccentEvent::PassThrough;
        }
        if ev.mods.contains(Mods::CTRL) || ev.mods.contains(Mods::PRIMARY) {
            return AccentEvent::PassThrough;
        }
        let Some(ch) = ev.ch else { return AccentEvent::PassThrough };
        if ch.is_numeric() {
            return AccentEvent::PassThrough;
        }
        let candidates = self.candidates_for(ch);
        if candidates.is_empty() {
            return AccentEvent::PassThrough;
        }
        let focused = if self.cfg.mark_on_open { Some(0) } else { None };
        self.open = Some(Open { base: ch, candidates: candidates.clone(), focused });
        AccentEvent::Show { base: ch, candidates, focused }
    }

    /// Mouse selection of candidate `index` (a single click commits).
    pub fn click(&mut self, index: usize) -> AccentEvent {
        match self.open.take() {
            Some(o) => match o.candidates.get(index).cloned() {
                Some(text) => AccentEvent::Commit { text },
                None => AccentEvent::Cancel { restore: None },
            },
            None => AccentEvent::PassThrough,
        }
    }

    /// Closes without inserting (focus lost, pointer left the client ...).
    pub fn dismiss(&mut self) -> AccentEvent {
        match self.open.take() {
            Some(o) => AccentEvent::Cancel { restore: o.focused.map(|_| o.base.to_string()) },
            None => AccentEvent::PassThrough,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

/// Popup geometry parameters (placeholders, approximate).
#[derive(Clone, Copy, Debug)]
pub struct PopupMetrics {
    pub cell_w: f64,
    pub cell_h: f64,
    pub padding: f64,
    pub gap_to_anchor: f64,
    pub screen_margin: f64,
}

impl Default for PopupMetrics {
    fn default() -> Self {
        PopupMetrics { cell_w: 36.0, cell_h: 44.0, padding: 6.0, gap_to_anchor: 4.0, screen_margin: 8.0 }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct PopupLayout {
    pub rect: Rect,
    pub cells: Vec<Rect>,
    /// `true` when placed above the anchor.
    pub above: bool,
}

/// Single-row layout centred on the anchor (the character being edited), flipped below when there is no room above and
/// shifted to stay inside the screen. A row wider than the screen is truncated to the cells that fit.
pub fn layout_popup(count: usize, anchor: Rect, screen: Rect, m: &PopupMetrics) -> PopupLayout {
    let max_cells = (((screen.w - 2.0 * m.screen_margin - 2.0 * m.padding) / m.cell_w).floor().max(1.0)) as usize;
    let n = count.min(max_cells).max(1);
    let w = n as f64 * m.cell_w + 2.0 * m.padding;
    let h = m.cell_h + 2.0 * m.padding;
    let mut x = anchor.x + anchor.w / 2.0 - w / 2.0;
    x = x.clamp(screen.x + m.screen_margin, (screen.x + screen.w - m.screen_margin - w).max(screen.x + m.screen_margin));
    let above_y = anchor.y - m.gap_to_anchor - h;
    let (y, above) = if above_y >= screen.y + m.screen_margin {
        (above_y, true)
    } else {
        (anchor.y + anchor.h + m.gap_to_anchor, false)
    };
    let cells = (0..n)
        .map(|i| Rect { x: x + m.padding + i as f64 * m.cell_w, y: y + m.padding, w: m.cell_w, h: m.cell_h })
        .collect();
    PopupLayout { rect: Rect { x, y, w, h }, cells, above }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> AccentTable {
        let mut t = AccentTable::new();
        t.insert('e', Direction::Right, ["e", "è", "é", "ê", "ë"].iter().map(|s| s.to_string()).collect());
        t.insert('1', Direction::Right, vec!["1".into(), "¹".into()]);
        t
    }

    fn down(ch: char, repeat: bool, mods: Mods) -> KeyDown {
        KeyDown { ch: Some(ch), key: PickerKey::Char, repeat, mods }
    }

    fn key(k: PickerKey) -> KeyDown {
        KeyDown { ch: None, key: k, repeat: false, mods: Mods::NONE }
    }

    #[test]
    fn opens_only_on_repeat() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        assert_eq!(p.key_down(&down('e', false, Mods::NONE), "app"), AccentEvent::PassThrough);
        match p.key_down(&down('e', true, Mods::NONE), "app") {
            AccentEvent::Show { base, candidates, focused } => {
                assert_eq!(base, 'e');
                assert_eq!(candidates, vec!["è", "é", "ê", "ë"]);
                assert_eq!(focused, None);
            }
            other => panic!("{other:?}"),
        }
        assert!(p.is_open());
    }

    #[test]
    fn blocked_cases() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        assert_eq!(p.key_down(&down('e', true, Mods::CTRL), "a"), AccentEvent::PassThrough);
        assert_eq!(p.key_down(&down('e', true, Mods::PRIMARY), "a"), AccentEvent::PassThrough);
        assert_eq!(p.key_down(&down('1', true, Mods::NONE), "a"), AccentEvent::PassThrough);
        assert_eq!(p.key_down(&down('q', true, Mods::NONE), "a"), AccentEvent::PassThrough);
        assert!(p.key_down(&down('e', true, Mods::SHIFT), "a") != AccentEvent::PassThrough);
        let cfg = AccentConfig { disabled_apps: vec!["term".into()], ..AccentConfig::default() };
        let mut p = AccentPicker::new(cfg, table());
        assert_eq!(p.key_down(&down('e', true, Mods::NONE), "term"), AccentEvent::PassThrough);
        assert!(p.key_down(&down('e', true, Mods::NONE), "other") != AccentEvent::PassThrough);
        let mut off = AccentPicker::new(AccentConfig { enabled: false, ..AccentConfig::default() }, table());
        assert_eq!(off.key_down(&down('e', true, Mods::NONE), "a"), AccentEvent::PassThrough);
    }

    #[test]
    fn navigation_and_commit() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.key_down(&key(PickerKey::Right), "a"), AccentEvent::Focus { index: 0, preview: Some("è".into()) });
        assert_eq!(p.key_down(&key(PickerKey::Tab), "a"), AccentEvent::Focus { index: 1, preview: Some("é".into()) });
        assert_eq!(p.key_down(&key(PickerKey::Left), "a"), AccentEvent::Focus { index: 0, preview: Some("è".into()) });
        assert_eq!(p.key_down(&key(PickerKey::Left), "a"), AccentEvent::Focus { index: 0, preview: Some("è".into()) });
        assert_eq!(p.key_down(&key(PickerKey::Return), "a"), AccentEvent::Commit { text: "è".into() });
        assert!(!p.is_open());
    }

    #[test]
    fn number_keys_commit_directly() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.key_down(&down('3', false, Mods::NONE), "a"), AccentEvent::Commit { text: "ê".into() });
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.key_down(&down('9', false, Mods::NONE), "a"), AccentEvent::Consumed);
        assert!(p.is_open());
    }

    #[test]
    fn escape_restores_only_after_preview() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.key_down(&key(PickerKey::Escape), "a"), AccentEvent::Cancel { restore: None });
        p.key_down(&down('e', true, Mods::NONE), "a");
        p.key_down(&key(PickerKey::Tab), "a");
        assert_eq!(p.key_down(&key(PickerKey::Escape), "a"), AccentEvent::Cancel { restore: Some("e".into()) });
    }

    #[test]
    fn mark_on_open_and_wrap_options() {
        let cfg = AccentConfig { mark_on_open: true, wrap: true, ..AccentConfig::default() };
        let mut p = AccentPicker::new(cfg, table());
        match p.key_down(&down('e', true, Mods::NONE), "a") {
            AccentEvent::Show { focused, .. } => assert_eq!(focused, Some(0)),
            other => panic!("{other:?}"),
        }
        assert_eq!(p.key_down(&key(PickerKey::Left), "a"), AccentEvent::Focus { index: 3, preview: Some("ë".into()) });
    }

    #[test]
    fn other_keys_close_and_pass_through() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.key_down(&down('x', false, Mods::NONE), "a"), AccentEvent::PassThrough);
        assert!(!p.is_open());
    }

    #[test]
    fn click_and_dismiss() {
        let mut p = AccentPicker::new(AccentConfig::default(), table());
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.click(2), AccentEvent::Commit { text: "ê".into() });
        p.key_down(&down('e', true, Mods::NONE), "a");
        assert_eq!(p.dismiss(), AccentEvent::Cancel { restore: None });
        assert_eq!(p.dismiss(), AccentEvent::PassThrough);
    }

    #[test]
    fn json_loading_merges_keyboards() {
        let json = r#"{"default":{"e":{"direction":"right","strings":["e","x"]}},"en":{"e":{"direction":"left","strings":["e","é"]},"a":{"strings":["a","á"]}}}"#;
        let t = AccentTable::from_json(json, &["default", "en"]).unwrap();
        assert_eq!(t.len(), 2);
        assert_eq!(t.entry('e').unwrap().strings, vec!["e", "é"]);
        assert_eq!(t.entry('e').unwrap().direction, Direction::Left);
        let t2 = AccentTable::from_json(json, &["default"]).unwrap();
        assert_eq!(t2.entry('e').unwrap().strings, vec!["e", "x"]);
    }

    #[test]
    fn popup_layout_clamps_and_flips() {
        let m = PopupMetrics::default();
        let screen = Rect { x: 0.0, y: 0.0, w: 800.0, h: 600.0 };
        let anchor = Rect { x: 400.0, y: 300.0, w: 10.0, h: 20.0 };
        let l = layout_popup(4, anchor, screen, &m);
        assert!(l.above);
        assert_eq!(l.cells.len(), 4);
        assert!((l.rect.x + l.rect.w / 2.0 - 405.0).abs() < 1e-9);
        let edge = layout_popup(4, Rect { x: 2.0, y: 300.0, w: 10.0, h: 20.0 }, screen, &m);
        assert_eq!(edge.rect.x, m.screen_margin);
        let top = layout_popup(4, Rect { x: 400.0, y: 10.0, w: 10.0, h: 20.0 }, screen, &m);
        assert!(!top.above);
        assert!(top.rect.y >= 30.0);
        let narrow = layout_popup(40, anchor, Rect { x: 0.0, y: 0.0, w: 200.0, h: 600.0 }, &m);
        assert!(narrow.cells.len() < 40 && narrow.rect.x + narrow.rect.w <= 200.0 - m.screen_margin + 1e-9);
    }
}
