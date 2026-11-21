//! Keyboard integration on the compositor state (`cupertino-keyboard`): `wl_keyboard.repeat_info`, the
//! XKB layouts behind the input sources, the input-source switch, the caps-lock activation
//! delay and the accent picker. Their panels are drawn by the on-screen displays ([`crate::keyboard_panels`]).
//!
//! * **Repeat**: the advertised rate and delay come from `keyboard.conf` (defaults 500 ms /
//!   83 ms, 12 Hz; 420 ms / 420 ms while slow keys are on). Clients do the repeating;
//!   the compositor repeats only keys it consumes (`kbd_filters`).
//! * **Input sources**: `input_sources = us, de, ...` in `keyboard.conf` become the XKB layouts of the
//!   seat keyboard; a `decoded:<path> source gets a generated keymap that replaces the seat keymap while
//!   it is current. `Ctrl+Space` / `Ctrl+Alt+Space` (reference assignments) open the switch in
//!   most-recently-used order, further presses and the Up / Down arrows step, releasing the modifiers
//!   commits, `Escape` cancels.
//! * **Caps lock**: the press reaches the keyboard state only when the key is still down after the delay
//!   (75 ms); with hold-to-switch a tap switches the input source and a 500 ms hold
//!   turns caps lock on.
//! * **Accent picker**: the model (`AccentPicker`: opens on the first repeat of a held letter, `1`-`9`,
//!   `Tab`/arrows, `Return`, `Escape`, a pick of a candidate on its panel) runs here; committing text needs
//!   a `zwp_text_input_v3` / `zwp_input_method_v2` bridge, see [`TextOp`] and the README.
//! * **The shortcut notice**: the pill that says an application holds the shortcuts or the pointer, which the server
//!   draws because it must show when no component runs.

use cupertino_geometry::{CornerStyle, Radii, Rect as GRect};
use cupertino_keyboard::switcher::{Step, SwitchEvent};
use cupertino_server_render::decorations::{Draw, DrawList};
use smithay::backend::input::KeyState;
use smithay::input::keyboard::{keysyms, Keycode, Layout, XkbConfig};
use smithay::utils::Logical;

use crate::input::Mods;
use crate::kbd::CapsStep;
use crate::overlays::OverlayKey;
use crate::render::image::debug_sig;
use cupertino_scene::SceneKind;

use crate::render::compose::{list_piece, Piece};
use crate::render::scene::SceneParams;
use crate::state::Cupertino;

pub use cupertino_server_input::kbd_ui::{
    has_input_source_bindings, AccentPopup, AccentRoute, AccentUi, TextOp,
};

/// Font size of the shortcut notice in points (approximate).
pub const FONT_PT: f32 = 13.0;

type Rgba = [f32; 4];

/// The draw of `r` (points) filled in `color` with continuous corners of `radius`, at `scale`.
fn rounded(r: GRect, radius: f64, scale: f64, color: Rgba) -> Option<Draw> {
    let path = cupertino_server_render::decorations::shape::rounded_path(
        r,
        Radii::uniform(radius),
        CornerStyle::continuous(),
        scale,
    )?;
    Some(Draw::Path {
        path,
        stroke: None,
        color,
    })
}

impl Cupertino {
    // ---- startup ---------------------------------------------------------------------------

    /// Apply `keyboard.conf` to the seat: repeat info for clients, the layouts of the input sources (the system
    /// layouts as groups of one keymap, the generated keymaps of the `decoded:` sources).
    pub fn apply_keyboard_settings(&mut self) {
        self.apply_keyboard_settings_with_decoded(None);
    }

    /// Like [`Self::apply_keyboard_settings`], taking `decoded:` layouts already read off the compositor loop when
    /// `preloaded` is `Some` so no layout file is read on the loop thread.
    pub fn apply_keyboard_settings_with_decoded(
        &mut self,
        preloaded: Option<std::collections::HashMap<String, crate::kbd::DecodedSource>>,
    ) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        let info = self.config.keyboard.repeat_info();
        keyboard.change_repeat_info(info.rate, info.delay);
        // layouts that do not compile leave the keymap as it was (logged)
        self.set_system_keymap();
        self.keep_release_chord_typeable();
        match preloaded {
            Some(decoded) => {
                self.overlays.inputs.decoded = decoded;
            }
            None => {
                let (kb, level3) = (
                    self.config.keyboard.layout_keyboard,
                    self.config.keyboard.decoded_level3(),
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
            }
        }
        // the first source is the current one
        if let Some(first) = self.overlays.inputs.current().map(str::to_string) {
            if self.overlays.inputs.decoded.contains_key(&first) {
                self.activate_input_source(&first);
            }
        }
    }

    /// The system XKB layouts of the input sources as the seat keymap. False when they do not compile: the seat keeps
    /// the keymap it had, the last one that compiled, and the failure is logged.
    fn set_system_keymap(&mut self) -> bool {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return false;
        };
        let (layout, variant) = self.config.keyboard.xkb_names();
        let options = self.config.keyboard.xkb_options();
        if layout.is_empty() && options.is_empty() {
            return true;
        }
        // the names are C strings
        if [&layout, &variant, &options]
            .iter()
            .any(|n| n.contains('\0'))
        {
            tracing::error!(
                layout,
                variant,
                options,
                "the configured input sources hold a NUL and form no keymap; the last working keymap stays"
            );
            return false;
        }
        let cfg = XkbConfig {
            layout: &layout,
            variant: &variant,
            options: (!options.is_empty()).then(|| options.clone()),
            ..XkbConfig::default()
        };
        match keyboard.set_xkb_config(self, cfg) {
            Ok(()) => true,
            Err(e) => {
                tracing::error!(error = ?e, layout, variant, options, "the configured input sources do not compile; the last working keymap stays");
                false
            }
        }
    }

    /// Take the layout localed holds: its layouts become the input sources unless `keyboard.conf` sets some, and its
    /// xkb options apply.
    pub fn apply_system_layout(&mut self, system: crate::localed::SystemLayout) {
        tracing::info!(
            layout = system.layout,
            variant = system.variant,
            options = system.options,
            "system keyboard layout"
        );
        if system
            .options
            .split(',')
            .any(|o| o.trim().starts_with("grp:"))
        {
            tracing::info!(
                options = system.options,
                "the system's layout switching keys (grp:) are left out: the input sources switch the layouts"
            );
        }
        let keyboard = &mut self.config.keyboard;
        keyboard.system = system;
        if self.live.files.keyboard.input_sources.is_none() {
            keyboard.sources.clear();
        }
        keyboard.use_system_layout();
        let decoded = std::mem::take(&mut self.overlays.inputs.decoded);
        self.overlays.inputs = crate::kbd::InputSources::new(self.config.keyboard.sources.clone());
        self.caps = crate::kbd::CapsDebounce::new(
            self.config.keyboard.caps,
            self.config.keyboard.caps_switch_hold(),
        );
        self.apply_keyboard_settings_with_decoded(Some(decoded));
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
        let ev = self.overlays.inputs.press(menu, step, held);
        self.run_switch_event(ev);
    }

    /// Run `ev` on the input sources: the source it commits becomes the keymap, then the switcher's panel is sent the
    /// list or the close.
    pub(crate) fn run_switch_event(&mut self, ev: SwitchEvent) {
        let was_open = self.overlays.inputs.switch_open();
        // labels are looked up lazily for the sources the panel lists
        let labels: std::collections::HashMap<String, String> = match &ev {
            SwitchEvent::ShowHud { order, .. } => order
                .iter()
                .map(|id| (id.clone(), self.source_label(id)))
                .collect(),
            _ => std::collections::HashMap::new(),
        };
        if let Some(id) = self.overlays.inputs.apply(ev, |id| {
            labels.get(id).cloned().unwrap_or_else(|| id.to_string())
        }) {
            self.activate_input_source(&id);
        }
        self.sources_changed(was_open);
    }

    /// The modifier state changed: releasing the shortcut's modifiers commits the displayed source.
    pub fn input_sources_modifiers(&mut self, mods: Mods) {
        if !self.overlays.inputs.switch_open()
            || self.overlays.hold_modifiers
            || mods.ctrl
            || mods.alt
            || mods.logo
        {
            return;
        }
        let ev = self.overlays.inputs.switcher.modifiers_released();
        self.run_switch_event(ev);
    }

    /// `Escape`, `Return` and the arrows while a switch is open. True when the key was consumed. Up and Down move the
    /// selection through the list like further presses of the shortcut (approximate: the design switcher's own key
    /// handling beyond Escape is not known).
    pub fn input_sources_key(&mut self, sym: u32) -> bool {
        if !self.overlays.inputs.switch_open() {
            return false;
        }
        let ev = match sym {
            keysyms::KEY_Escape => self.overlays.inputs.switcher.escape(),
            keysyms::KEY_Return | keysyms::KEY_KP_Enter => self.overlays.inputs.switcher.commit(),
            keysyms::KEY_Up => self.overlays.inputs.switcher.press(Step::Previous, true),
            keysyms::KEY_Down => self.overlays.inputs.switcher.press(Step::Next, true),
            _ => return false,
        };
        self.run_switch_event(ev);
        true
    }

    fn activate_input_source(&mut self, id: &str) {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return;
        };
        if let Some(source) = self.overlays.inputs.decoded.get(id) {
            let keymap = source.keymap.clone();
            // the keymap text is a C string
            if keymap.contains('\0') {
                tracing::error!(
                    source = id,
                    "the generated keymap holds a NUL; the last working keymap stays"
                );
                return;
            }
            match keyboard.set_keymap_from_string(self, keymap) {
                Ok(()) => {
                    self.overlays.inputs.decoded_active = true;
                    self.keep_release_chord_typeable();
                    tracing::info!(source = id, "input source (decoded layout)");
                }
                Err(e) => {
                    tracing::error!(source = id, error = ?e, "the generated keymap does not compile; the last working keymap stays")
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
        // the decoded keymap stays when the system layouts do not compile: its groups are not theirs
        if self.overlays.inputs.decoded_active {
            if !self.set_system_keymap() {
                return;
            }
            self.overlays.inputs.decoded_active = false;
        }
        keyboard.with_xkb_state(self, |mut ctx| ctx.set_layout(Layout(idx as u32)));
        self.keep_release_chord_typeable();
        tracing::info!(source = id, group = idx, "input source");
    }

    /// Index of the active xkb group (tests, status display).
    pub fn active_xkb_group(&mut self) -> u32 {
        let Some(keyboard) = self.seat.get_keyboard() else {
            return 0;
        };
        keyboard.with_xkb_state(self, |ctx| ctx.xkb().lock().unwrap().active_layout().0)
    }

    /// The first-level keysym of the evdev key `evdev` in the layout `layout` of the seat keymap.
    pub fn xkb_first_keysym(&mut self, evdev: u32, layout: u32) -> Option<u32> {
        let keyboard = self.seat.get_keyboard()?;
        // SAFETY: the keymap reference only lives for this lookup, inside the lock
        keyboard.with_xkb_state(self, |ctx| {
            unsafe { ctx.xkb().lock().unwrap().keymap() }
                .key_get_syms_by_level(Keycode::new(evdev + 8), layout, 0)
                .first()
                .map(|sym| sym.raw())
        })
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
                let o = self
                    .active_output()
                    .and_then(|id| self.output_entry(id))
                    .map(|o| o.rect)
                    .unwrap_or_default();
                GRect::new(o.x + o.w / 2.0, o.y + o.h / 2.0, 2.0, 20.0)
            }
        }
    }

    /// Close the picker as cancelled: the letter is put back where a preview replaced it, and its panel is told.
    pub fn accent_dismiss(&mut self) {
        if self.overlays.accent.is_open() {
            self.overlays.accent.dismiss();
            self.flush_text_ops();
        }
        self.sync_accent_panel();
    }

    /// Open the picker for `base` now, as the first repeat of its held key would ([`Self::accent_trigger`]): scripted
    /// input and tests. False when it does not open: no candidates, no panel to show it, no output at the anchor.
    pub fn accent_open(&mut self, base: char) -> bool {
        let anchor = self.accent_anchor();
        if !self.panels_can_open_picker() || !self.picker_has_output(anchor) {
            return false;
        }
        let app = self.key_app_id();
        let opened = self.overlays.accent.open_for(base, anchor, &app);
        self.flush_text_ops();
        self.sync_accent_panel();
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
            if !consumed {
                // the key closed the picker and goes to the client
                self.overlays.accent.close();
            }
            self.sync_accent_panel();
            if !consumed {
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
                    && self.overlays.accent.picker.allowed_for(&self.key_app_id())
                    && self.panels_can_open_picker() =>
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

    /// First compositor repeat of a held letter with alternatives: open the picker, unless its panel cannot show it
    /// (the panels' component went since the letter was armed, or no output holds the anchor).
    pub fn accent_trigger(&mut self, ch: char) {
        let anchor = self.accent_anchor();
        if self.panels_can_open_picker() && self.picker_has_output(anchor) {
            let app = self.key_app_id();
            self.overlays.accent.open_for(ch, anchor, &app);
        }
        self.flush_text_ops();
        self.sync_accent_panel();
    }

    /// A pointer press while the picker is open: anywhere but on the picker's panel it closes the picker as
    /// cancelled. The press goes on to what it hit either way; a candidate's panel reports its pick itself.
    pub fn accent_pointer_press(&mut self, pos: smithay::utils::Point<f64, Logical>) {
        if !self.overlays.accent.is_open() {
            return;
        }
        let on_panel = self
            .surface_under(pos)
            .is_some_and(|(surface, _)| self.is_accents_panel(&surface));
        if !on_panel {
            self.accent_dismiss();
        }
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

/// The colour of the notice's text in the dark and the light appearance (approximate).
fn notice_text(dark: bool) -> Rgba {
    if dark {
        [0.97, 0.97, 0.98, 0.95]
    } else {
        [0.05, 0.05, 0.06, 0.92]
    }
}

/// Padding of the notice that an application holds the shortcuts, in points: horizontal and vertical.
const NOTICE_PADDING: (f64, f64) = (18.0, 11.0);
/// Distance of that notice from the top of the output, in points.
const NOTICE_TOP: f64 = 48.0;

/// How the notice is set: the text, its size in points, and the padding at its left and right in points.
#[derive(Clone, Debug, PartialEq)]
struct NoticeSetting {
    text: String,
    pt: f32,
    pad: f64,
}

fn render_notice(
    setting: &NoticeSetting,
    size: (f64, f64),
    scale: f64,
    dark: bool,
    renderer: &mut crate::overlays::OverlayText,
) -> Option<DrawList> {
    let mut list = DrawList::new(
        (size.0 * scale).round().max(1.0) as u32,
        (size.1 * scale).round().max(1.0) as u32,
    );
    let fill = if dark {
        [0.16, 0.16, 0.18, 0.92]
    } else {
        [0.97, 0.97, 0.98, 0.94]
    };
    list.draws.extend(rounded(
        GRect::new(0.0, 0.0, size.0, size.1),
        size.1 / 2.0,
        scale,
        fill,
    ));
    list.draws.push(renderer.centered(
        &setting.text,
        (setting.pad * scale) as f32,
        (size.1 / 2.0 * scale) as f32,
        setting.pt * scale as f32,
        400,
        notice_text(dark),
    ));
    Some(list)
}

/// The narrowest output (in logical points) the notice is made for. No display is narrower. From this width up, at any
/// scale: (a) the sentence that names the release chord ("Press <chord> to get your shortcuts back.") is shown whole
/// for chords up to the tested length (`Super+Escape`, `Ctrl+Alt+Escape`), in a slightly smaller type when the output is
/// narrow, and the application's name goes when it does not fit; (b) for a longer chord the chord itself is shown whole,
/// alone, in the smaller type: every chord of the four modifiers and a key the table names fits in
/// [`NOTICE_CHORD_BUDGET`] (the widest, `Ctrl+Alt+Shift+Super+XF86MonBrightnessDown`, is 262 points). The chord is what
/// matters, the sentence around it is a courtesy. Below this width nothing is guaranteed: the notice shows the chord
/// alone, cut at the output's edge.
pub const NOTICE_MIN_OUTPUT_WIDTH: f64 = 320.0;
/// The room (in points) the chord alone gets at the narrowest supported output: the output less a side margin and the
/// compact padding on each side.
pub const NOTICE_CHORD_BUDGET: f64 =
    NOTICE_MIN_OUTPUT_WIDTH - 2.0 * NOTICE_SIDE_MARGIN - 2.0 * NOTICE_COMPACT.1;
/// The least room, in points, the text of the notice gets, whatever the output: a narrower output cuts the notice at its edge.
const NOTICE_MIN_TEXT_ROOM: f64 = 40.0;
/// Distance of the notice from each side of the output, in points.
const NOTICE_SIDE_MARGIN: f64 = 8.0;
/// The type size and the horizontal padding of the notice on a narrow output, in points.
const NOTICE_COMPACT: (f32, f64) = (11.5, 12.0);

/// How the notice is set on an output `output_width` points wide at `scale`: the whole text when it fits between the
/// margins, else the sentence that names the release chord, in the normal type and then in the compact one, else the
/// chord alone in the compact type, cut at the right when even that does not fit. The chord is in all of them.
fn notice_setting(
    notice: &crate::overlays::ShortcutNotice,
    output_width: f64,
    scale: f64,
    text: &mut crate::overlays::OverlayText,
) -> NoticeSetting {
    let (compact_pt, compact_pad) = NOTICE_COMPACT;
    let room =
        |pad: f64| (output_width - 2.0 * NOTICE_SIDE_MARGIN - 2.0 * pad).max(NOTICE_MIN_TEXT_ROOM);
    for (candidate, pt, pad) in [
        (&notice.text, FONT_PT, NOTICE_PADDING.0),
        (&notice.clause, FONT_PT, NOTICE_PADDING.0),
        (&notice.clause, compact_pt, compact_pad),
    ] {
        if f64::from(text.width(candidate, pt * scale as f32, 400)) / scale <= room(pad) {
            return NoticeSetting {
                text: candidate.clone(),
                pt,
                pad,
            };
        }
    }
    NoticeSetting {
        text: text.fit(
            &notice.short,
            compact_pt * scale as f32,
            400,
            (room(compact_pad) * scale) as f32,
        ),
        pt: compact_pt,
        pad: compact_pad,
    }
}

/// The size and the position of the notice (in points) for a text `text_width` wide set with `setting` on an output
/// `output_width` wide: as wide as the text needs, never wider than the output less a margin on each side, centred. The
/// text starts at the left padding. From [`NOTICE_MIN_OUTPUT_WIDTH`] up what is shown fits the pill: (a) the chord
/// sentence whole for chords up to the tested length, (b) the chord itself whole for a longer one whose label is at most
/// [`NOTICE_CHORD_BUDGET`] wide; below it nothing is guaranteed.
fn notice_layout(
    text_width: f64,
    setting: &NoticeSetting,
    output_width: f64,
) -> ((f64, f64), (f64, f64)) {
    let wanted = text_width + 2.0 * setting.pad;
    // an output narrower than the supported minimum is cut by its own edge: the pill keeps its size and its left edge
    let width = if output_width >= NOTICE_MIN_OUTPUT_WIDTH {
        wanted.min(output_width - 2.0 * NOTICE_SIDE_MARGIN)
    } else {
        wanted
    }
    .max(1.0);
    let size = (width, f64::from(setting.pt) + 2.0 * NOTICE_PADDING.1);
    let x = ((output_width - width) / 2.0).round().max(0.0);
    (size, (x, NOTICE_TOP))
}

/// The notice that an application holds the shortcuts: a pill at the top of the output of its window, above everything
/// but the session overlays. It is drawn on the compositor-overlay layer, which a lock-only frame omits.
pub fn shortcut_notice(p: &mut SceneParams<'_>) -> Vec<Piece> {
    let Some(notice) = p.overlays.shortcut_notice.clone() else {
        return Vec::new();
    };
    if notice.output != p.output_id || p.now >= notice.until {
        return Vec::new();
    }
    let (scale, dark) = (p.scale, p.config.is_dark());
    let overlays = &mut *p.overlays;
    let text = &mut overlays.text;
    text.set_scale(scale as f32);
    let setting = notice_setting(&notice, p.rect.w, scale, text);
    let text_width = f64::from(text.width(&setting.text, setting.pt * scale as f32, 400)) / scale;
    let (size, (x, y)) = notice_layout(text_width, &setting, p.rect.w);
    let sig = debug_sig(&(
        &setting,
        (size.0.to_bits(), size.1.to_bits()),
        scale.to_bits(),
        dark,
    ));
    let cut = overlays.images.get(("shortcut-notice", 0), sig, || {
        render_notice(&setting, size, scale, dark, text)
    });
    cut.map(|(image, px)| {
        vec![list_piece(
            &image,
            SceneKind::ShortcutNotice,
            px,
            x,
            y,
            scale,
            1.0,
        )]
    })
    .unwrap_or_default()
}
