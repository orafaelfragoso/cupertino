//! Keyboard integration on the compositor state (`cupertino-keyboard`): `wl_keyboard.repeat_info`, the
//! XKB layouts behind the input sources, the input-source switcher HUD, the caps-lock activation
//! delay and the accent picker.
//!
//! * **Repeat**: the advertised rate and delay come from `keyboard.conf` (defaults 500 ms /
//!   83 ms, 12 Hz; 420 ms / 420 ms while slow keys are on). Clients do the repeating;
//!   the compositor repeats only keys it consumes (`kbd_filters`).
//! * **Input sources**: `input_sources = us, de, ...` in `keyboard.conf` become the XKB layouts of the
//!   seat keyboard; a `decoded:<path> source gets a generated keymap that replaces the seat keymap while
//!   it is current. `Ctrl+Space` / `Ctrl+Alt+Space` (reference assignments) open the HUD in
//!   most-recently-used order, further presses and the Up / Down arrows step, releasing the modifiers
//!   commits, `Escape` cancels. HUD metrics: radius 24, minimum width 167, padding 10, rows of
//!   40, white tint at 0.35, 0.3 s fade; the material, the placement (screen centre) and the
//!   source icons are approximate / unknown.
//! * **Caps lock**: the press reaches the keyboard state only when the key is still down after the delay
//!   (75 ms); with hold-to-switch a tap switches the input source and a 500 ms hold
//!   turns caps lock on.
//! * **Accent picker**: the model (`AccentPicker`: opens on the first repeat of a held letter, `1`-`9`,
//!   `Tab`/arrows, `Return`, `Escape`, a click on a candidate) and the popup are implemented
//!   here; committing text needs a `zwp_text_input_v3` / `zwp_input_method_v2` bridge, see [`TextOp`] and
//!   the README.

use std::collections::HashSet;

use cupertino_geometry::{CornerStyle, Radii, Rect as GRect};
use cupertino_keyboard::accent::{
    layout_popup, AccentEvent, AccentPicker, AccentTable, KeyDown, PickerKey, PopupMetrics,
    Rect as KRect,
};
use cupertino_keyboard::switcher::{self, Step};
use cupertino_modifiers::Mods as KMods;
use smithay::backend::input::KeyState;
use smithay::backend::renderer::gles::GlesRenderer;
use smithay::input::keyboard::{keysyms, Keycode, Layout, XkbConfig};
use smithay::utils::{Logical, Rectangle};
use tiny_skia::{FillRule, Paint, Pixmap, Transform};

use crate::input::{Action, KeyBindings, Mods};
use crate::kbd::{hud_size, AccentSettings, CapsStep, HudView};
use crate::overlays::OverlayKey;
use crate::render::image::{debug_sig, image_element};
use crate::render::scene::{material_layer, rrect, shadow_with, SceneParams, WindowPlan};
use crate::render::shader_rect::ElementKey;
use crate::render::{CupertinoElement, LayerStack, SceneLayer};
use crate::state::Cupertino;

/// Font size of HUD rows and accent cells in points (approximate).
pub const FONT_PT: f32 = 13.0;
/// Accent candidate size in points (approximate).
pub const ACCENT_FONT_PT: f32 = 18.0;
/// Edge of the source icon in the HUD (approximate, icon artwork unknown).
pub const HUD_ICON_PT: f64 = 24.0;
/// Material behind the HUD and the accent popup (unknown, the menu material is used).
const MATERIAL: &str = "Menu";

type Rgba = [f32; 4];

fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint {
        anti_alias: true,
        ..Paint::default()
    };
    p.set_color(
        tiny_skia::Color::from_rgba(
            c[0].clamp(0.0, 1.0),
            c[1].clamp(0.0, 1.0),
            c[2].clamp(0.0, 1.0),
            c[3].clamp(0.0, 1.0),
        )
        .unwrap_or(tiny_skia::Color::TRANSPARENT),
    );
    p
}

fn fill_rounded(pm: &mut Pixmap, r: GRect, radius: f64, scale: f64, color: Rgba) {
    if let Some(path) = cupertino_decorations::shape::rounded_path(
        r,
        Radii::uniform(radius),
        CornerStyle::continuous(),
        scale,
    ) {
        pm.fill_path(
            &path,
            &paint(color),
            FillRule::Winding,
            Transform::identity(),
            None,
        );
    }
}

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

/// What the accent picker does with a key event ([`Cupertino::accent_filter`]).
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

    fn close(&mut self) {
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
            s if crate::overlays::is_modifier_sym(s) => return false,
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

impl Cupertino {
    // ---- startup ---------------------------------------------------------------------------

    /// Apply `keyboard.conf` to the seat: repeat info for clients, the layouts of the input sources (the system
    /// layouts as groups of one keymap, the generated keymaps of the `decoded:` sources).
    pub fn apply_keyboard_settings(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let info = self.config.keyboard.repeat_info();
        keyboard.change_repeat_info(info.rate, info.delay);
        self.set_system_keymap();
        let (kb, level3) = (
            self.config.keyboard.layout_keyboard,
            self.config.keyboard.layout_level3,
        );
        for spec in self.config.keyboard.sources.clone() {
            let Some(path) = spec.decoded.as_ref() else {
                continue;
            };
            match crate::kbd::generate_decoded(path, kb, level3) {
                Ok(source) => {
                    tracing::info!(
                        source = spec.id,
                        layout = source.label,
                        "decoded layout loaded"
                    );
                    self.overlays.inputs.decoded.insert(spec.id.clone(), source);
                }
                Err(e) => {
                    tracing::error!(source = spec.id, error = %e, "decoded layout not loaded; the source cannot be selected")
                }
            }
        }
        // the first source is the current one
        if let Some(first) = self.overlays.inputs.current().map(str::to_string) {
            if self.overlays.inputs.decoded.contains_key(&first) {
                self.activate_input_source(&first);
            }
        }
    }

    /// The system XKB layouts of the input sources as the seat keymap.
    fn set_system_keymap(&mut self) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let (layout, variant) = self.config.keyboard.xkb_names();
        if layout.is_empty() {
            return;
        }
        let cfg = XkbConfig {
            layout: &layout,
            variant: &variant,
            ..XkbConfig::default()
        };
        if let Err(e) = keyboard.set_xkb_config(self, cfg) {
            tracing::warn!(error = ?e, layout, variant, "the configured input sources do not form a keymap");
        }
    }

    // ---- input sources -----------------------------------------------------------------------

    fn source_label(&mut self, id: &str) -> String {
        if let Some(d) = self.overlays.inputs.decoded.get(id) {
            return d.label.clone();
        }
        let Some(idx) = self.overlays.inputs.group_of(id) else {
            return id.to_string();
        };
        let Some(keyboard) = self.seat.get_keyboard() else {
            return id.to_string();
        };
        let name = keyboard.with_xkb_state(self, |ctx| {
            let xkb = ctx.xkb().lock().unwrap();
            let n = xkb.layout_name(Layout(idx as u32)).to_string();
            n
        });
        if name.is_empty() {
            id.to_string()
        } else {
            name
        }
    }

    /// A shortcut of the input-source pair was pressed.
    pub fn input_source_step(&mut self, menu: bool, step: Step) {
        if !self.overlays.inputs.enabled() {
            tracing::info!(
                "input source shortcut ignored: fewer than two input_sources in keyboard.conf"
            );
            return;
        }
        let mods = self.current_mods();
        let held = self.overlays.hold_modifiers || mods.ctrl || mods.alt || mods.logo;
        let now = self.now();
        let ev = self.overlays.inputs.press(menu, step, held);
        // labels are looked up lazily for the sources the HUD lists
        let ids = self.overlays.inputs.switcher.historical_order();
        let labels: std::collections::HashMap<String, String> = ids
            .iter()
            .map(|id| (id.clone(), self.source_label(id)))
            .collect();
        if let Some(id) = self.overlays.inputs.apply(ev, now, |id| {
            labels.get(id).cloned().unwrap_or_else(|| id.to_string())
        }) {
            self.activate_input_source(&id);
        }
        self.request_redraw();
    }

    /// The modifier state changed: releasing the shortcut's modifiers commits the displayed source.
    pub fn input_sources_modifiers(&mut self, mods: Mods) {
        if !self.overlays.inputs.hud_open()
            || self.overlays.hold_modifiers
            || mods.ctrl
            || mods.alt
            || mods.logo
        {
            return;
        }
        let now = self.now();
        let ev = self.overlays.inputs.switcher.modifiers_released();
        if let Some(id) = self.overlays.inputs.apply(ev, now, |id| id.to_string()) {
            self.activate_input_source(&id);
        }
        self.request_redraw();
    }

    /// `Escape`, `Return` and the arrows while the HUD is open. True when the key was consumed. Up and Down move the
    /// selection through the list like further presses of the shortcut (approximate: the design HUD's own key
    /// handling beyond Escape is not known).
    pub fn input_sources_key(&mut self, sym: u32) -> bool {
        if !self.overlays.inputs.hud_open() {
            return false;
        }
        let now = self.now();
        let ev = match sym {
            keysyms::KEY_Escape => self.overlays.inputs.switcher.escape(),
            keysyms::KEY_Return | keysyms::KEY_KP_Enter => self.overlays.inputs.switcher.commit(),
            keysyms::KEY_Up => self.overlays.inputs.switcher.press(Step::Previous, true),
            keysyms::KEY_Down => self.overlays.inputs.switcher.press(Step::Next, true),
            _ => return false,
        };
        if let Some(id) = self.overlays.inputs.apply(ev, now, |id| id.to_string()) {
            self.activate_input_source(&id);
        }
        self.request_redraw();
        true
    }

    fn activate_input_source(&mut self, id: &str) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if let Some(source) = self.overlays.inputs.decoded.get(id) {
            let keymap = source.keymap.clone();
            match keyboard.set_keymap_from_string(self, keymap) {
                Ok(()) => {
                    self.overlays.inputs.decoded_active = true;
                    tracing::info!(source = id, "input source (decoded layout)");
                }
                Err(e) => {
                    tracing::error!(source = id, error = ?e, "the generated keymap was rejected")
                }
            }
            return;
        }
        let Some(idx) = self.overlays.inputs.group_of(id) else {
            tracing::error!(
                source = id,
                "input source without a keymap (a decoded layout that did not load)"
            );
            return;
        };
        if self.overlays.inputs.decoded_active {
            self.set_system_keymap();
            self.overlays.inputs.decoded_active = false;
        }
        keyboard.with_xkb_state(self, |mut ctx| ctx.set_layout(Layout(idx as u32)));
        tracing::info!(source = id, group = idx, "input source");
    }

    /// Index of the active xkb group (tests, status display).
    pub fn active_xkb_group(&mut self) -> u32 {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return 0;
        };
        keyboard.with_xkb_state(self, |ctx| ctx.xkb().lock().unwrap().active_layout().0)
    }

    // ---- caps lock ---------------------------------------------------------------------------

    /// Caps-lock debounce for a key event. True when the event was held back or dropped. With hold-to-switch a
    /// tap switches to the previously used input source (like `Ctrl+Space`, committed at once).
    pub fn caps_gate(&mut self, code: Keycode, state: KeyState, time_ms: u32) -> bool {
        // evdev KEY_CAPSLOCK (58) plus the xkb offset of 8
        if code.raw() != 66 || !self.caps.active() {
            return false;
        }
        let now = u64::from(time_ms);
        match state {
            KeyState::Pressed => match self.caps.press(now) {
                CapsStep::Hold => {
                    self.caps_press_time = time_ms;
                    let delay = std::time::Duration::from_millis(self.caps.delay_ms());
                    use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
                    let timer = self.loop_handle.insert_source(
                        Timer::from_duration(delay),
                        move |_, _, st| {
                            st.caps_deliver_due();
                            TimeoutAction::Drop
                        },
                    );
                    // without the timer a held-back press would never arrive: deliver it now, and
                    // forget it so its release is delivered too
                    if let Err(e) = timer {
                        self.caps.cancel();
                        tracing::error!(error = %e, "caps-lock delay timer not inserted; the press is delivered without the delay");
                        return false;
                    }
                    true
                }
                CapsStep::Deliver | CapsStep::Drop | CapsStep::SwitchSource => false,
            },
            KeyState::Released => match self.caps.release(now) {
                CapsStep::Deliver => false,
                CapsStep::Drop | CapsStep::Hold => true,
                CapsStep::SwitchSource => {
                    self.input_source_step(false, Step::Next);
                    true
                }
            },
        }
    }

    /// The caps-lock delay elapsed: deliver the held-back press if the key is still down.
    pub fn caps_deliver_due(&mut self) {
        let now = u64::from(self.caps_press_time) + self.caps.delay_ms();
        if let Some(t) = self.caps.due(now) {
            self.key_after_delays(
                Keycode::new(66),
                KeyState::Pressed,
                t as u32 + self.caps.delay_ms() as u32,
            );
        }
    }

    // ---- accent picker -----------------------------------------------------------------------

    /// Identity of the key window's application (the per-application accent switch).
    pub(crate) fn key_app_id(&self) -> String {
        self.wm
            .key_window()
            .and_then(|w| self.wm.window(w))
            .and_then(|w| self.wm.app(w.app))
            .map(|a| a.identity.clone())
            .unwrap_or_default()
    }

    /// Anchor of the popup: the text cursor reported by the application's text input, else the centre of the
    /// key window (approximate).
    fn accent_anchor(&self) -> GRect {
        if let Some(r) = self.text_cursor_global() {
            return r;
        }
        match self
            .wm
            .key_window()
            .and_then(|w| self.wm.window(w))
            .map(|w| w.geometry())
        {
            Some(g) => GRect::new(g.cx(), g.cy(), 2.0, 20.0),
            None => {
                let o = self.outputs.first().map(|o| o.rect).unwrap_or_default();
                GRect::new(o.x + o.w / 2.0, o.y + o.h / 2.0, 2.0, 20.0)
            }
        }
    }

    pub fn accent_dismiss(&mut self) {
        if self.overlays.accent.is_open() {
            self.overlays.accent.dismiss();
            self.request_redraw();
        }
    }

    /// Open the picker for `base` now (scripted input, tests; a held key opens it through [`Self::tick_accent`]).
    pub fn accent_open(&mut self, base: char) -> bool {
        let anchor = self.accent_anchor();
        let app = self.key_app_id();
        let opened = self.overlays.accent.open_for(base, anchor, &app);
        if opened {
            self.request_redraw();
        }
        opened
    }

    /// Keyboard filter for the accent picker, called for every key event that is not a modifier.
    pub fn accent_filter(&mut self, key: &OverlayKey, pressed: bool) -> AccentRoute {
        if !pressed {
            return if self.overlays.accent.swallow_release.remove(&key.code) {
                AccentRoute::Consumed { repeats: false }
            } else {
                AccentRoute::Pass
            };
        }
        if self.overlays.accent.is_open() {
            let app = self.key_app_id();
            let consumed = self.overlays.accent.key(key, &app);
            self.request_redraw();
            if !consumed {
                // the key closed the popup and goes to the client
                self.overlays.accent.close();
                return AccentRoute::Pass;
            }
            let navigation = matches!(
                key.sym,
                keysyms::KEY_Tab
                    | keysyms::KEY_ISO_Left_Tab
                    | keysyms::KEY_Left
                    | keysyms::KEY_Right
            );
            return AccentRoute::Consumed {
                repeats: navigation,
            };
        }
        // a plain character key whose character has alternatives arms the trigger (not with control or the
        // primary modifier, not for digits; Alt is excluded too because it selects the alternate symbols here)
        let plain = !key.mods.ctrl && !key.mods.alt && !key.mods.logo;
        match key.text.filter(|c| plain && !c.is_control()) {
            Some(c)
                if self.overlays.accent.bridge
                    && self.overlays.accent.picker.offers(c)
                    && self.overlays.accent.picker.allowed_for(&self.key_app_id()) =>
            {
                AccentRoute::Armed(c)
            }
            _ => AccentRoute::Pass,
        }
    }

    /// The model clock in milliseconds (the time base of the keyboard filters).
    pub(crate) fn now_ms(&self) -> u64 {
        (self.now() * 1000.0) as u64
    }

    /// First compositor repeat of a held letter with alternatives: open the picker.
    pub fn accent_trigger(&mut self, ch: char) {
        let anchor = self.accent_anchor();
        let app = self.key_app_id();
        if self.overlays.accent.open_for(ch, anchor, &app) {
            self.request_redraw();
        }
        self.flush_text_ops();
    }

    /// A pointer press while the accent popup is open: a candidate commits it (a single click commits), a
    /// press elsewhere on the popup is taken without effect, a press outside closes the popup and goes on
    /// (approximate: what the design does with an outside click is not known). True when the press is taken.
    pub fn accent_pointer_press(&mut self, pos: smithay::utils::Point<f64, Logical>) -> bool {
        let Some(popup) = self.overlays.accent.popup.clone() else {
            return false;
        };
        let centre = cupertino_geometry::Point::new(
            popup.anchor.x() + popup.anchor.width() / 2.0,
            popup.anchor.y() + popup.anchor.height() / 2.0,
        );
        let output = self
            .outputs
            .iter()
            .map(|o| GRect::new(o.rect.x, o.rect.y, o.rect.w, o.rect.h))
            .find(|r| r.contains(centre));
        let Some(output) = output else {
            self.accent_dismiss();
            return false;
        };
        let inset = if self.config.shell.show_menu_bar {
            self.config.shell.menu_bar_height
        } else {
            0.0
        };
        let layout = accent_layout(&popup, output, inset);
        let (x, y) = (pos.x - output.x(), pos.y - output.y());
        let inside = |r: &KRect| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h;
        if let Some(index) = layout.cells.iter().position(inside) {
            self.overlays.accent.click(index);
            self.flush_text_ops();
            self.request_redraw();
            return true;
        }
        if inside(&layout.rect) {
            return true;
        }
        self.accent_dismiss();
        false
    }

    /// Deliver one edit to the focused client's text input. False when there is no bridge.
    pub fn send_text_op(&mut self, op: &TextOp) -> bool {
        self.deliver_text_op(op)
    }

    /// Hand pending accent edits to the text-input bridge; without one they are only logged.
    pub fn flush_text_ops(&mut self) {
        let ops = std::mem::take(&mut self.overlays.accent.outbox);
        for op in ops {
            if !self.send_text_op(&op) {
                tracing::info!(
                    ?op,
                    "accent edit not delivered: no text-input bridge for the focused client"
                );
            }
        }
    }
}

// ---- scene -------------------------------------------------------------------------------------

/// Colours of the HUD and popup contents. approximate.
struct Palette {
    text: Rgba,
    /// Fill of the highlighted row and the text on it.
    row_highlight: Rgba,
    row_highlight_text: Rgba,
}

fn palette(dark: bool) -> Palette {
    let row_highlight = [0.04, 0.45, 1.0, 1.0];
    let row_highlight_text = [1.0, 1.0, 1.0, 1.0];
    if dark {
        Palette {
            text: [0.97, 0.97, 0.98, 0.95],
            row_highlight,
            row_highlight_text,
        }
    } else {
        Palette {
            text: [0.05, 0.05, 0.06, 0.92],
            row_highlight,
            row_highlight_text,
        }
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

fn render_hud(
    view: &HudView,
    size: (f64, f64),
    scale: f64,
    dark: bool,
    text: &mut crate::overlays::OverlayText,
) -> Option<(Pixmap, [u32; 4])> {
    let (w, h) = (
        (size.0 * scale).round().max(1.0) as u32,
        (size.1 * scale).round().max(1.0) as u32,
    );
    let mut pm = Pixmap::new(w, h)?;
    let pal = palette(dark);
    let pad = switcher::HUD_PADDING;
    for (i, label) in view.labels.iter().enumerate() {
        let y = pad + i as f64 * switcher::HUD_ROW_HEIGHT;
        let row = GRect::new(pad, y, size.0 - 2.0 * pad, switcher::HUD_ROW_HEIGHT);
        let hot = i == view.selection;
        if hot {
            fill_rounded(
                &mut pm,
                row,
                switcher::HUD_BACKGROUND_PATTERN_RADIUS,
                scale,
                pal.row_highlight,
            );
        }
        let color = if hot {
            pal.row_highlight_text
        } else {
            pal.text
        };
        // placeholder source icon: a rounded square with the layout code
        let icon = GRect::new(
            row.x() + 10.0,
            y + (switcher::HUD_ROW_HEIGHT - HUD_ICON_PT) / 2.0,
            HUD_ICON_PT,
            HUD_ICON_PT,
        );
        let icon_fill = if hot {
            [1.0, 1.0, 1.0, 0.28]
        } else {
            [color[0], color[1], color[2], 0.14]
        };
        fill_rounded(&mut pm, icon, 6.0, scale, icon_fill);
        let code = view
            .order
            .get(i)
            .map(|id| source_code(id))
            .unwrap_or_default();
        let px = (10.0 * scale) as f32;
        let cw = text.width(&code, px, 600);
        text.draw_centered(
            &mut pm,
            &code,
            ((icon.x() + icon.width() / 2.0) * scale) as f32 - cw / 2.0,
            ((icon.y() + icon.height() / 2.0) * scale) as f32,
            px,
            600,
            color,
        );
        let tx = icon.x() + HUD_ICON_PT + 10.0;
        let avail = (row.x() + row.width() - 10.0 - tx) * scale;
        let px = FONT_PT * scale as f32;
        let fitted = text.fit(label, px, 400, avail as f32);
        text.draw_centered(
            &mut pm,
            &fitted,
            (tx * scale) as f32,
            ((y + switcher::HUD_ROW_HEIGHT / 2.0) * scale) as f32,
            px,
            400,
            color,
        );
    }
    Some((pm, [0; 4]))
}

/// The input-source HUD (centre of the primary output while it is open or fading).
pub fn input_hud(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    stack: &mut LayerStack<CupertinoElement>,
) {
    if p.wm.outputs().first().map(|o| o.id) != Some(p.output_id) {
        return;
    }
    let Some(view) = p.overlays.inputs.hud.clone() else {
        return;
    };
    let alpha = view.alpha(p.now) as f32;
    if alpha <= 0.002 || view.labels.is_empty() {
        return;
    }
    let (scale, dark) = (p.scale, p.config.is_dark());
    let px = FONT_PT * scale as f32;
    let overlays = &mut *p.overlays;
    let text = &mut overlays.text;
    text.set_scale(scale as f32);
    let (w, h) = hud_size(&view.labels, |l| f64::from(text.width(l, px, 400)) / scale);
    // the placement is unknown: centred
    let (x, y) = (
        ((p.rect.w - w) / 2.0).round(),
        ((p.rect.h - h) / 2.0).round(),
    );
    let sig = debug_sig(&(
        &view.order,
        &view.labels,
        view.selection,
        (w.to_bits(), h.to_bits()),
        scale.to_bits(),
        dark,
    ));
    let cut = overlays.images.get(("input-hud", 0), sig, || {
        render_hud(&view, (w, h), scale, dark, text)
    });
    let mut els: Vec<CupertinoElement> = Vec::new();
    if let Some((buf, px, _)) = cut {
        if let Some(e) = image_element(renderer, &buf, px, x, y, scale, alpha) {
            els.push(e);
        }
    }
    let area = Rectangle::<f64, Logical>::new((x, y).into(), (w, h).into());
    // white tint of the panel (white at 0.35) over the material
    let (white, tint_alpha) = switcher::HUD_TINT_WHITE_ALPHA;
    let tint = [white as f32, white as f32, white as f32, tint_alpha as f32];
    els.push(
        rrect(
            p,
            ElementKey::new("input-hud-tint", 0),
            area,
            switcher::HUD_CORNER_RADIUS,
            true,
            tint,
            None,
            alpha,
        )
        .into(),
    );
    els.extend(material_layer(
        p,
        ElementKey::new("input-hud-material", 0),
        MATERIAL,
        area,
        switcher::HUD_CORNER_RADIUS,
        alpha,
    ));
    let params = p.decor.popup_shadow_params();
    let plan = WindowPlan {
        visual: cupertino_wm::Rect::new(p.rect.x + x, p.rect.y + y, w, h),
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        shadow: 1.0,
        exact: true,
        corner: None,
    };
    if let Some(s) = shadow_with(
        p,
        ElementKey::new("input-hud-shadow", 0),
        &params,
        &plan,
        switcher::HUD_CORNER_RADIUS,
        f64::from(alpha),
    ) {
        els.push(s.into());
    }
    stack.extend(SceneLayer::InputHud, els);
}

/// Popup geometry for the open picker on an output (output-local logical coordinates).
pub fn accent_layout(
    popup: &AccentPopup,
    output: GRect,
    top_inset: f64,
) -> cupertino_keyboard::accent::PopupLayout {
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

fn render_accent(
    popup: &AccentPopup,
    layout: &cupertino_keyboard::accent::PopupLayout,
    scale: f64,
    dark: bool,
    text: &mut crate::overlays::OverlayText,
) -> Option<(Pixmap, [u32; 4])> {
    let r = layout.rect;
    let mut pm = Pixmap::new(
        (r.w * scale).round().max(1.0) as u32,
        (r.h * scale).round().max(1.0) as u32,
    )?;
    let pal = palette(dark);
    for (i, cell) in layout.cells.iter().enumerate() {
        let Some(cand) = popup.candidates.get(i) else {
            break;
        };
        let c = GRect::new(cell.x - r.x, cell.y - r.y, cell.w, cell.h);
        let hot = popup.focused == Some(i);
        if hot {
            fill_rounded(&mut pm, c, 8.0, scale, pal.row_highlight);
        }
        let color = if hot {
            pal.row_highlight_text
        } else {
            pal.text
        };
        let px = ACCENT_FONT_PT * scale as f32;
        let w = text.width(cand, px, 400);
        text.draw_centered(
            &mut pm,
            cand,
            ((c.x() + c.width() / 2.0) * scale) as f32 - w / 2.0,
            ((c.y() + c.height() * 0.42) * scale) as f32,
            px,
            400,
            color,
        );
        // the number key that picks the candidate directly (1 to 9)
        let num = (i + 1).to_string();
        let npx = 9.0 * scale as f32;
        let nw = text.width(&num, npx, 400);
        let dim = [color[0], color[1], color[2], color[3] * 0.6];
        text.draw_centered(
            &mut pm,
            &num,
            ((c.x() + c.width() / 2.0) * scale) as f32 - nw / 2.0,
            ((c.y() + c.height() - 8.0) * scale) as f32,
            npx,
            400,
            dim,
        );
    }
    Some((pm, [0; 4]))
}

/// The accent popup next to its anchor.
pub fn accent_popup(
    renderer: &mut GlesRenderer,
    p: &mut SceneParams<'_>,
    stack: &mut LayerStack<CupertinoElement>,
) {
    let Some(popup) = p.overlays.accent.popup.clone() else {
        return;
    };
    let anchor_center = (
        popup.anchor.x() + popup.anchor.width() / 2.0,
        popup.anchor.y() + popup.anchor.height() / 2.0,
    );
    let rect = GRect::new(p.rect.x, p.rect.y, p.rect.w, p.rect.h);
    if !rect.contains(cupertino_geometry::Point::new(
        anchor_center.0,
        anchor_center.1,
    )) {
        return;
    }
    let layout = accent_layout(
        &popup,
        rect,
        if p.config.shell.show_menu_bar {
            p.config.shell.menu_bar_height
        } else {
            0.0
        },
    );
    let (scale, dark) = (p.scale, p.config.is_dark());
    let overlays = &mut *p.overlays;
    let text = &mut overlays.text;
    text.set_scale(scale as f32);
    let sig = debug_sig(&(
        &popup.candidates,
        popup.focused,
        (
            layout.rect.x.to_bits(),
            layout.rect.y.to_bits(),
            layout.rect.w.to_bits(),
        ),
        scale.to_bits(),
        dark,
    ));
    let cut = overlays.images.get(("accent", 0), sig, || {
        render_accent(&popup, &layout, scale, dark, text)
    });
    let r = layout.rect;
    let mut els: Vec<CupertinoElement> = Vec::new();
    if let Some((buf, px, _)) = cut {
        if let Some(e) = image_element(renderer, &buf, px, r.x, r.y, scale, 1.0) {
            els.push(e);
        }
    }
    let area = Rectangle::<f64, Logical>::new((r.x, r.y).into(), (r.w, r.h).into());
    els.extend(material_layer(
        p,
        ElementKey::new("accent-material", 0),
        MATERIAL,
        area,
        12.0,
        1.0,
    ));
    let params = p.decor.popup_shadow_params();
    let plan = WindowPlan {
        visual: cupertino_wm::Rect::new(p.rect.x + r.x, p.rect.y + r.y, r.w, r.h),
        scale_x: 1.0,
        scale_y: 1.0,
        opacity: 1.0,
        shadow: 1.0,
        exact: true,
        corner: None,
    };
    if let Some(s) = shadow_with(
        p,
        ElementKey::new("accent-shadow", 0),
        &params,
        &plan,
        12.0,
        1.0,
    ) {
        els.push(s.into());
    }
    stack.extend(SceneLayer::AccentPopup, els);
}

/// Does `bindings` bind the input-source pair? (diagnostics)
pub fn has_input_source_bindings(bindings: &KeyBindings) -> bool {
    bindings
        .entries()
        .iter()
        .any(|(_, a)| matches!(a, Action::InputSourceNext | Action::InputSourcePrevious))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> AccentTable {
        let mut t = AccentTable::new();
        t.insert(
            'e',
            cupertino_keyboard::accent::Direction::Right,
            vec!["e".into(), "è".into(), "é".into(), "ê".into(), "ë".into()],
        );
        t
    }

    fn ui() -> AccentUi {
        AccentUi::with_table(&AccentSettings::default(), table())
    }

    fn key(sym: u32, text: Option<char>) -> OverlayKey {
        OverlayKey {
            code: 0,
            sym,
            text,
            mods: Mods::NONE,
        }
    }

    fn anchor() -> GRect {
        GRect::new(100.0, 100.0, 2.0, 20.0)
    }

    #[test]
    fn opens_on_the_first_repeat_with_the_alternatives_without_the_base() {
        let mut a = ui();
        assert!(a.open_for('e', anchor(), "app"));
        let p = a.popup.clone().unwrap();
        assert_eq!(p.candidates, vec!["è", "é", "ê", "ë"]);
        assert_eq!(p.focused, None);
        assert!(a.outbox.is_empty());
        assert!(
            !ui().open_for('z', anchor(), "app"),
            "no alternatives, no popup"
        );
        assert!(!ui().open_for('1', anchor(), "app"));
    }

    #[test]
    fn number_key_commits_replacing_the_typed_base() {
        let mut a = ui();
        a.open_for('e', anchor(), "app");
        assert!(a.key(&key(keysyms::KEY_2, Some('2')), "app"));
        assert_eq!(
            a.outbox,
            vec![TextOp::Commit {
                delete_before: 1,
                text: "é".into()
            }]
        );
        assert!(!a.is_open());
    }

    #[test]
    fn focus_previews_then_return_commits_without_a_second_delete() {
        let mut a = ui();
        a.open_for('e', anchor(), "app");
        assert!(a.key(&key(keysyms::KEY_Tab, None), "app"));
        assert!(a.key(&key(keysyms::KEY_Right, None), "app"));
        assert_eq!(a.popup.as_ref().unwrap().focused, Some(1));
        assert!(a.key(&key(keysyms::KEY_Return, None), "app"));
        assert_eq!(
            a.outbox,
            vec![
                TextOp::Preedit {
                    delete_before: 1,
                    text: "è".into()
                },
                TextOp::Preedit {
                    delete_before: 0,
                    text: "é".into()
                },
                TextOp::Commit {
                    delete_before: 0,
                    text: "é".into()
                },
            ]
        );
    }

    #[test]
    fn escape_restores_only_when_a_preview_was_showing() {
        let mut a = ui();
        a.open_for('e', anchor(), "app");
        assert!(a.key(&key(keysyms::KEY_Escape, None), "app"));
        assert!(a.outbox.is_empty() && !a.is_open());
        a.open_for('e', anchor(), "app");
        a.key(&key(keysyms::KEY_Tab, None), "app");
        a.key(&key(keysyms::KEY_Escape, None), "app");
        assert_eq!(a.outbox.last(), Some(&TextOp::Restore { text: "e".into() }));
    }

    #[test]
    fn other_keys_close_the_popup_and_pass_through() {
        let mut a = ui();
        a.open_for('e', anchor(), "app");
        assert!(!a.key(&key(keysyms::KEY_x, Some('x')), "app"));
    }

    #[test]
    fn disabled_applications_never_open_it() {
        let s = AccentSettings {
            disabled_apps: vec!["foot".into()],
            ..Default::default()
        };
        let mut a = AccentUi::with_table(&s, table());
        assert!(!a.open_for('e', anchor(), "foot"));
        assert!(a.open_for('e', anchor(), "other"));
    }

    #[test]
    fn click_commits_and_dismiss_closes() {
        let mut a = ui();
        a.open_for('e', anchor(), "app");
        a.click(0);
        assert_eq!(
            a.outbox,
            vec![TextOp::Commit {
                delete_before: 1,
                text: "è".into()
            }]
        );
        a.open_for('e', anchor(), "app");
        a.dismiss();
        assert!(!a.is_open());
        assert_eq!(
            a.outbox.len(),
            1,
            "dismissing without a preview inserts nothing"
        );
    }

    #[test]
    fn popup_sits_above_the_anchor_inside_the_screen() {
        let mut a = ui();
        a.open_for('e', GRect::new(500.0, 400.0, 2.0, 20.0), "app");
        let l = accent_layout(
            a.popup.as_ref().unwrap(),
            GRect::new(0.0, 0.0, 1000.0, 700.0),
            24.0,
        );
        assert!(l.above);
        assert_eq!(l.cells.len(), 4);
        assert!(l.rect.y + l.rect.h < 400.0);
        // no room above the menu bar edge: below the anchor, never under the bar
        let mut b = ui();
        b.open_for('e', GRect::new(500.0, 40.0, 2.0, 20.0), "app");
        let l = accent_layout(
            b.popup.as_ref().unwrap(),
            GRect::new(0.0, 0.0, 1000.0, 700.0),
            24.0,
        );
        assert!(!l.above && l.rect.y >= 24.0);
    }

    #[test]
    fn source_codes() {
        assert_eq!(source_code("fr(nodeadkeys)"), "FR");
        assert_eq!(source_code("us"), "US");
    }
}
