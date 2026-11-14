//! Shell overlays that are drawn above the windows and take keyboard or pointer input: the
//! input-source HUD and the accent popup. This module holds their shared state ([`Overlays`]) and the
//! helpers they have in common (key events, the text renderer, the theme icon adapter); the behaviour
//! lives in `kbd_ui`.

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_decorations::text::TextRenderer;
use cupertino_decorations::Draw;
use smithay::input::keyboard::keysyms;

use crate::config::Config;
use crate::input::Mods;
use crate::kbd::InputSources;
use crate::kbd_ui::AccentUi;
use crate::render::image::ImageCache;

/// A key press the overlays interpret: the keysym of the key without modifiers (letters lower case),
/// the character the modifiers produce, and the physical modifiers (Super is `logo`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayKey {
    /// Raw xkb keycode (identifies the key for repeat and release).
    pub code: u32,
    pub sym: u32,
    pub text: Option<char>,
    pub mods: Mods,
}

/// Text drawing for the HUD and the accent popup (the `ui` font of the asset directory).
pub struct OverlayText {
    text: TextRenderer,
    cap_height: f32,
}

impl OverlayText {
    pub fn new(
        typography: &cupertino_theme::Typography,
        assets: &Assets,
    ) -> Result<Self, AssetError> {
        Ok(Self {
            text: TextRenderer::new(assets, FontRole::Ui)?,
            cap_height: (typography.metrics.cap_height / typography.metrics.units_per_em) as f32,
        })
    }

    /// Output scale of the surface the next calls draw for (selects the font's optical size from the point size).
    pub fn set_scale(&mut self, scale: f32) {
        self.text.set_scale(scale);
    }

    /// Width in device pixels.
    pub fn width(&mut self, text: &str, px: f32, weight: u16) -> f32 {
        let w = self.text.effective_weight(weight);
        self.text.measure(text, px, w, 0.0)
    }

    /// The draw of `text` vertically centred on `mid_y` (device pixels) using the cap height, its left edge at `x`.
    #[allow(clippy::too_many_arguments)]
    pub fn centered(
        &mut self,
        text: &str,
        x: f32,
        mid_y: f32,
        px: f32,
        weight: u16,
        color: [f32; 4],
    ) -> Draw {
        let w = self.text.effective_weight(weight);
        let baseline = mid_y + self.cap_height * px / 2.0;
        Draw::Glyphs {
            glyphs: self.text.place(text, x, baseline, px, w, 0.0, color),
            color,
        }
    }

    pub fn fit(&mut self, text: &str, px: f32, weight: u16, max_px: f32) -> String {
        let w = self.text.effective_weight(weight);
        self.text.fit(text, px, w, 0.0, max_px).0
    }
}

/// State of all overlays.
pub struct Overlays {
    pub inputs: InputSources,
    pub accent: AccentUi,
    pub images: ImageCache,
    /// The image wallpapers (`render/wallpaper.rs`).
    pub wallpaper: crate::render::wallpaper::Wallpapers,
    pub text: OverlayText,
    /// Scripted input behaves as if the shortcut's modifiers were still held (HUD stays open).
    pub hold_modifiers: bool,
    /// The notice that an application holds the shortcuts or the pointer (`crate::shortcut_inhibit`).
    pub shortcut_notice: Option<ShortcutNotice>,
}

/// What the notice that an application holds the shortcuts or the pointer says, and where and until when it shows.
#[derive(Clone, Debug, PartialEq)]
pub struct ShortcutNotice {
    pub text: String,
    /// What shows instead of `text` on an output too narrow for it: the sentence that names the release chord.
    pub clause: String,
    /// What shows instead of `clause` below the supported output width: the release chord alone.
    pub short: String,
    pub output: cupertino_wm::OutputId,
    /// The toplevel surface whose inhibitor or pointer constraint the notice is for; `None` for a notice no deactivation
    /// takes down.
    pub owner: Option<smithay::reexports::wayland_server::backend::ObjectId>,
    pub until: f64,
}

impl Overlays {
    pub fn new(
        config: &Config,
        typography: &cupertino_theme::Typography,
        assets: &Assets,
    ) -> Result<Self, AssetError> {
        Ok(Self {
            inputs: InputSources::new(config.keyboard.sources.clone()),
            accent: AccentUi::new(&config.keyboard.accent),
            images: ImageCache::default(),
            wallpaper: crate::render::wallpaper::Wallpapers::new(assets),
            text: OverlayText::new(typography, assets)?,
            hold_modifiers: false,
            shortcut_notice: None,
        })
    }

    /// Any overlay that needs frames at `now`.
    pub fn wants_tick(&self) -> bool {
        self.inputs.wants_tick()
            || self.accent.is_open()
            || self.wallpaper.pending()
            || self.shortcut_notice.is_some()
    }
}

/// Keysyms that are modifiers (their presses carry no text and are ignored by the overlays' repeat).
pub fn is_modifier_sym(sym: u32) -> bool {
    matches!(
        sym,
        keysyms::KEY_Shift_L
            | keysyms::KEY_Shift_R
            | keysyms::KEY_Control_L
            | keysyms::KEY_Control_R
            | keysyms::KEY_Alt_L
            | keysyms::KEY_Alt_R
            | keysyms::KEY_Super_L
            | keysyms::KEY_Super_R
            | keysyms::KEY_Meta_L
            | keysyms::KEY_Meta_R
            | keysyms::KEY_Caps_Lock
            | keysyms::KEY_ISO_Level3_Shift
            | keysyms::KEY_Num_Lock
    )
}

impl crate::state::Cupertino {
    /// Per-frame work of the overlays: HUD fade, and the timed keyboard filters (slow keys, mouse keys, the
    /// compositor's key repeat, which also opens the accent picker).
    pub fn tick_overlays(&mut self, now: f64) {
        self.overlays.inputs.tick(now);
        self.tick_key_filters();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifier_keys_are_recognised() {
        assert!(is_modifier_sym(keysyms::KEY_Super_L));
        assert!(is_modifier_sym(keysyms::KEY_Caps_Lock));
        assert!(!is_modifier_sym(keysyms::KEY_a));
    }
}
