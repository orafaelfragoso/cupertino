//! Accent-picker model, text-edit queue, and keyboard overlay layout helpers.

use std::collections::HashSet;

use cupertino_geometry::Rect as GRect;
use cupertino_keyboard::accent::{
    layout_popup, AccentEvent, AccentPicker, AccentTable, KeyDown, PickerKey, PopupLayout,
    PopupMetrics, Rect as KRect,
};
use cupertino_keyboard::modifiers::Mods as KMods;
use smithay::input::keyboard::keysyms;

use crate::input::{Action, KeyBindings};
use crate::kbd::AccentSettings;
use crate::overlay_types::{is_modifier_sym, OverlayKey};

/// A text edit the accent picker asks the focused client to make. The edits are meant for
/// `zwp_text_input_v3` (`delete_surrounding_text`, `commit_string`, `preedit_string`, `done`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextOp {
    /// Replace the last `delete_before` characters by a preview shown as marked text.
    Preedit { delete_before: usize, text: String },
    /// Insert `text`, first deleting `delete_before` characters (the base character typed before the picker opened)
    /// when no preview replaced it yet.
    Commit { delete_before: usize, text: String },
    /// Remove the marked text and put `text` back.
    Restore { text: String },
}

/// What the accent picker does with a key event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AccentRoute {
    /// Not for the picker.
    Pass,
    /// Taken by the open popup (or a swallowed release); `repeats` for the navigation keys.
    Consumed { repeats: bool },
    /// A plain letter with alternatives went down in an application with an active text input: it is typed, and
    /// the compositor's first repeat of the key opens the picker.
    Armed(char),
}

/// The open accent popup.
#[derive(Clone, Debug, PartialEq)]
pub struct AccentPopup {
    pub base: char,
    pub candidates: Vec<String>,
    pub focused: Option<usize>,
    /// Anchor in global logical coordinates (the text cursor; the window centre when unknown, approximate).
    pub anchor: GRect,
}

pub struct AccentUi {
    pub picker: AccentPicker,
    pub popup: Option<AccentPopup>,
    /// Physical keys whose release is swallowed: held letters whose release the application got with the press.
    pub swallow_release: HashSet<u32>,
    /// A preview replaced the base character already.
    pub preview_active: bool,
    /// Edits waiting for a text-input bridge.
    pub outbox: Vec<TextOp>,
    /// A bridge to the focused client's text input exists; without one the picker does not open on key repeat.
    pub bridge: bool,
    pub settings: AccentSettings,
    has_table: bool,
}

impl AccentUi {
    pub fn new(settings: &AccentSettings) -> Self {
        let table = settings
            .table
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).map_err(|e| tracing::warn!(path = %p.display(), error = %e, "accent table not read")).ok())
            .and_then(|text| {
                let kb: Vec<&str> = settings.keyboards.iter().map(String::as_str).collect();
                AccentTable::from_json(&text, &kb).map_err(|e| tracing::warn!(error = %e, "accent table is not valid")).ok()
            })
            .unwrap_or_default();
        Self::with_table(settings, table)
    }

    pub fn with_table(settings: &AccentSettings, table: AccentTable) -> Self {
        let cfg = cupertino_keyboard::AccentConfig {
            enabled: settings.enabled,
            disabled_apps: settings.disabled_apps.clone(),
            ..Default::default()
        };
        let has_table = !table.is_empty();
        Self {
            has_table,
            picker: AccentPicker::new(cfg, table),
            popup: None,
            swallow_release: HashSet::new(),
            preview_active: false,
            outbox: Vec::new(),
            bridge: false,
            settings: settings.clone(),
        }
    }

    pub fn is_open(&self) -> bool {
        self.popup.is_some()
    }

    /// An accent table was loaded (without one the picker has nothing to offer).
    pub fn has_table(&self) -> bool {
        self.has_table
    }

    /// Open the picker for `base` as if its key had produced its first repeat (the table decides whether there
    /// are candidates). Returns false when there is nothing to show.
    pub fn open_for(&mut self, base: char, anchor: GRect, app_id: &str) -> bool {
        let ev = self.picker.key_down(
            &KeyDown {
                ch: Some(base),
                key: PickerKey::Char,
                repeat: true,
                mods: KMods::NONE,
            },
            app_id,
        );
        self.apply(ev, anchor)
    }

    fn apply(&mut self, ev: AccentEvent, anchor: GRect) -> bool {
        match ev {
            AccentEvent::Show {
                base,
                candidates,
                focused,
            } => {
                self.popup = Some(AccentPopup {
                    base,
                    candidates,
                    focused,
                    anchor,
                });
                self.preview_active = false;
                if let Some(i) = focused {
                    let text = self
                        .popup
                        .as_ref()
                        .and_then(|p| p.candidates.get(i).cloned())
                        .unwrap_or_default();
                    self.preview_active = true;
                    self.outbox.push(TextOp::Preedit {
                        delete_before: 1,
                        text,
                    });
                }
                true
            }
            AccentEvent::Focus { index, preview } => {
                if let Some(p) = self.popup.as_mut() {
                    p.focused = Some(index);
                }
                if let Some(text) = preview {
                    let delete_before = if self.preview_active { 0 } else { 1 };
                    self.preview_active = true;
                    self.outbox.push(TextOp::Preedit {
                        delete_before,
                        text,
                    });
                }
                true
            }
            AccentEvent::Commit { text } => {
                let delete_before = if self.preview_active { 0 } else { 1 };
                self.outbox.push(TextOp::Commit {
                    delete_before,
                    text,
                });
                self.close();
                true
            }
            AccentEvent::Cancel { restore } => {
                if let (Some(text), true) = (restore, self.preview_active) {
                    self.outbox.push(TextOp::Restore { text });
                }
                self.close();
                true
            }
            AccentEvent::Consumed => true,
            AccentEvent::PassThrough => false,
        }
    }

    pub fn close(&mut self) {
        self.popup = None;
        self.preview_active = false;
    }

    /// A key press while the picker is open. True when the picker consumed it; a key it does not know closes the
    /// popup and is passed on.
    pub fn key(&mut self, key: &OverlayKey, app_id: &str) -> bool {
        let Some(anchor) = self.popup.as_ref().map(|p| p.anchor) else {
            return false;
        };
        let pk = match key.sym {
            keysyms::KEY_Escape => PickerKey::Escape,
            keysyms::KEY_Return | keysyms::KEY_KP_Enter => PickerKey::Return,
            keysyms::KEY_Tab | keysyms::KEY_ISO_Left_Tab => PickerKey::Tab,
            keysyms::KEY_Left => PickerKey::Left,
            keysyms::KEY_Right => PickerKey::Right,
            s if is_modifier_sym(s) => return false,
            _ if key.text.is_some_and(|c| !c.is_control()) => PickerKey::Char,
            _ => PickerKey::Other,
        };
        let shift_tab =
            key.sym == keysyms::KEY_ISO_Left_Tab || (key.sym == keysyms::KEY_Tab && key.mods.shift);
        let ev = if shift_tab {
            // shift+tab walks backwards like the left arrow
            self.picker.key_down(
                &KeyDown {
                    ch: None,
                    key: PickerKey::Left,
                    repeat: false,
                    mods: KMods::NONE,
                },
                app_id,
            )
        } else {
            self.picker.key_down(
                &KeyDown {
                    ch: key.text,
                    key: pk,
                    repeat: false,
                    mods: KMods::NONE,
                },
                app_id,
            )
        };
        self.apply(ev, anchor)
    }

    /// A click on candidate `index`.
    pub fn click(&mut self, index: usize) {
        let Some(anchor) = self.popup.as_ref().map(|p| p.anchor) else {
            return;
        };
        let ev = self.picker.click(index);
        self.apply(ev, anchor);
    }

    /// Close without inserting (focus lost, pointer left the client).
    pub fn dismiss(&mut self) {
        if self.popup.is_none() {
            return;
        }
        let anchor = self
            .popup
            .as_ref()
            .map(|p| p.anchor)
            .unwrap_or(GRect::new(0.0, 0.0, 0.0, 0.0));
        let ev = self.picker.dismiss();
        self.apply(ev, anchor);
        self.close();
    }
}

/// Two-letter code of a source id for its placeholder icon (`fr(nodeadkeys)` → `FR`).
pub fn source_code(id: &str) -> String {
    id.split('(')
        .next()
        .unwrap_or(id)
        .chars()
        .take(2)
        .collect::<String>()
        .to_uppercase()
}

/// Popup geometry for the open picker on an output (output-local logical coordinates).
pub fn accent_layout(popup: &AccentPopup, output: GRect, top_inset: f64) -> PopupLayout {
    let anchor = KRect {
        x: popup.anchor.x() - output.x(),
        y: popup.anchor.y() - output.y(),
        w: popup.anchor.width(),
        h: popup.anchor.height(),
    };
    // the popup stays below the menu bar
    let screen = KRect {
        x: 0.0,
        y: top_inset,
        w: output.width(),
        h: (output.height() - top_inset).max(1.0),
    };
    layout_popup(
        popup.candidates.len().min(9),
        anchor,
        screen,
        &PopupMetrics::default(),
    )
}

/// Does `bindings` bind the input-source pair? (diagnostics)
pub fn has_input_source_bindings(bindings: &KeyBindings) -> bool {
    bindings
        .entries()
        .iter()
        .any(|(_, a)| matches!(a, Action::InputSourceNext | Action::InputSourcePrevious))
}
