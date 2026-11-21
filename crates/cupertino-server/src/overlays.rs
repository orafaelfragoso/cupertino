//! The keyboard's overlays: the input sources and the accent picker the server runs (their panels are drawn by the
//! on-screen displays, [`crate::keyboard_panels`]), and the notice that an application holds the shortcuts, which
//! the server draws.

use cupertino_assets::{AssetError, Assets, FontRole};
use cupertino_server_render::decorations::text::TextRenderer;
use cupertino_server_render::decorations::Draw;

use crate::config::Config;
use crate::kbd::InputSources;
use crate::kbd_ui::AccentUi;
use crate::render::image::ImageCache;

pub use cupertino_server_input::overlay_types::{is_modifier_sym, OverlayKey, ShortcutNotice};

/// Text drawing for the shortcut notice (the `ui` font of the asset directory).
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
    /// The objects of the component that draws the keyboard's panels, and what they were sent.
    pub panels: crate::keyboard_panels::KeyboardPanels,
    pub images: ImageCache,
    /// The image wallpapers (`render/wallpaper.rs`).
    pub wallpaper: crate::render::wallpaper::Wallpapers,
    pub text: OverlayText,
    /// Scripted input behaves as if the shortcut's modifiers were still held (the switch stays open).
    pub hold_modifiers: bool,
    /// The notice that an application holds the shortcuts or the pointer (`crate::shortcut_inhibit`).
    pub shortcut_notice: Option<ShortcutNotice>,
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
            panels: crate::keyboard_panels::KeyboardPanels::default(),
            images: ImageCache::default(),
            wallpaper: crate::render::wallpaper::Wallpapers::new(assets),
            text: OverlayText::new(typography, assets)?,
            hold_modifiers: false,
            shortcut_notice: None,
        })
    }

    /// Any overlay that needs frames at `now`.
    pub fn wants_tick(&self) -> bool {
        self.accent.is_open() || self.wallpaper.pending() || self.shortcut_notice.is_some()
    }
}

impl crate::state::Cupertino {
    /// Per-frame work of the overlays: the timed keyboard filters (slow keys, mouse keys, the compositor's key repeat,
    /// which also opens the accent picker).
    pub fn tick_overlays(&mut self) {
        self.tick_key_filters();
    }
}
