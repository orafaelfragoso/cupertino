//! Accessibility display settings in the server: `accessibility.conf` (`cupertino-defaults`) is
//! read at start and written on every change; the server applies the settings to its own drawing
//! and sends the four display options to the shell processes on the session object.
//!
//! * Reduce Transparency: materials use their opaque variants (`RenderResources::recipe`); glass keeps its shape and
//!   takes its reduced-transparency recipe ([`glass_mode`]): no bending at the edge, a much wider blur and a dense
//!   fill, so what lies behind shows only as soft colour.
//! * Increase Contrast: the high-contrast catalogs ([`Cupertino::appearance`]), so materials are
//!   opaque too, and the decorations take the high-contrast palette and the contrast half of the
//!   shadow table (`decor.rs`). Glass takes its increased-contrast colours (a denser fill and a firmer edge); with
//!   both options on, the reduced-transparency optics with the increased-contrast colours.
//! * Reduce Motion: the replacements of `render::motion`; glass stops bending at the edge and widens its narrowest blur.
//! * Invert, display contrast and the colour filter: a material pass over the whole output, below
//!   the pointer ([`Accessibility::display_filter`], `cupertino_glass::display_filter`).
//!
//! The machine's defaults (`/etc/cupertino/accessibility.conf`, read as every system file is, only its allowed keys) are
//! the login screen's settings, and those of an account for every key its own `accessibility.conf` does not record. The
//! person's file records only the keys they changed (`PersonalAccessibility`), so a change of the machine's file applies
//! at once to every other key.

use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::{anyhow, Result};
use cupertino_defaults::accessibility::{
    AccessibilitySettings, DisplayOption, DisplayOptions, PersonalAccessibility,
    SystemAccessibility,
};
use cupertino_effects::{TransformKind, WindowKind};
use cupertino_glass::display_filter::display_filter_ops;
use cupertino_theme::recipe::{Backdrop, Recipe};
use cupertino_theme::{Appearance, GlassMode, GlassStyle};

use crate::render::GpuRecipe;

use crate::state::Cupertino;

/// The glass recipe mode of `style` in the dark (`dark`) or light appearance under the display `options`: each option
/// selects its own recipe of the glass rather than turning it off.
pub fn glass_mode(dark: bool, style: GlassStyle, options: DisplayOptions) -> GlassMode {
    GlassMode {
        dark,
        style,
        reduce_transparency: options.reduce_transparency,
        increase_contrast: options.increase_contrast,
        reduce_motion: options.reduce_motion,
        density: 0,
    }
}

/// The settings and where they are stored.
#[derive(Debug, Default)]
pub struct Accessibility {
    /// In effect: `personal` over `system`.
    pub settings: AccessibilitySettings,
    /// `None` keeps changes in memory (offscreen runs and tests).
    store: Option<PathBuf>,
    /// The machine's defaults, from the system file.
    system: AccessibilitySettings,
    /// The keys the person changed.
    personal: PersonalAccessibility,
}

/// The machine's defaults at `path`, read with the rules of every system file (no link, owned by root, writable by no one
/// else) and only the keys it may set. `None` when there is no file; a refused or unreadable file is logged and `None`.
pub fn read_system_accessibility(path: &Path) -> Option<AccessibilitySettings> {
    let text = match crate::config::read_system_file(path) {
        Ok(text) => text?,
        Err(e) => {
            tracing::error!(path = %path.display(), error = %e, "ignoring the machine's accessibility settings");
            return None;
        }
    };
    let (system, problems) = SystemAccessibility::from_text(&text);
    for (line, problem) in problems {
        tracing::warn!(path = %path.display(), line, problem, "ignoring a machine accessibility setting");
    }
    Some(system.settings)
}

impl Accessibility {
    /// Read the settings in `path` and store every later change there. Lines that are not
    /// understood are logged and skipped; a file that cannot be read is an error.
    pub fn use_store(&mut self, path: PathBuf) -> Result<()> {
        let (personal, problems) = PersonalAccessibility::load(&path)
            .map_err(|e| anyhow!("reading {}: {e}", path.display()))?;
        for (line, problem) in problems {
            tracing::warn!(path = %path.display(), line, problem, "ignoring accessibility setting");
        }
        self.settings = personal.over(&self.system);
        self.personal = personal;
        self.store = Some(path);
        Ok(())
    }

    /// The colour pass of the settings over a whole output: the backdrop captured at full size
    /// without blur, then the colour chain. None when the display is drawn unchanged.
    pub fn display_filter(&self) -> Option<Rc<GpuRecipe>> {
        let ops = display_filter_ops(&self.settings);
        if ops.is_empty() {
            return None;
        }
        let recipe = Recipe {
            name: "DisplayFilter".into(),
            backdrop: Some(Backdrop {
                scale: 1.0,
                sigma_pt: 0.0,
                ops,
            }),
            ..Recipe::default()
        };
        Some(Rc::new(GpuRecipe::new(&recipe)))
    }

    /// The person changed the settings to `settings`: the keys that differ are theirs from now on, whether or not the
    /// file could be written.
    fn change(&mut self, settings: AccessibilitySettings) {
        self.personal.record(&self.settings, &settings);
        self.settings = settings;
        let Some(path) = &self.store else { return };
        if let Err(e) = self.personal.store(path) {
            tracing::error!(path = %path.display(), error = %e, "cannot store the accessibility settings: the change lasts until the server stops");
        }
    }
}

impl Cupertino {
    /// The display options in effect.
    pub fn display_options(&self) -> DisplayOptions {
        self.accessibility.settings.display
    }

    /// Time the window model's minimize and restore for the effect drawn: the configured one, or under Reduce Motion
    /// its stand-in (`render::motion::minimize_kind`).
    pub fn apply_minimize_timing(&mut self) {
        let mut effects = self.config.effects;
        effects.minimize =
            crate::render::motion::minimize_kind(effects.minimize, self.display_options());
        crate::state::set_minimize_timing(self.wm.config_mut(), effects);
    }

    /// The appearance the server draws with: the session's light or dark appearance, in its
    /// high-contrast catalog while Increase Contrast is on (catalog selection).
    pub fn appearance(&self) -> Appearance {
        self.config
            .appearance
            .with_high_contrast(self.display_options().increase_contrast)
    }

    /// The transform a closing window of `kind` fades out with, None when it disappears at once
    /// (`render::motion::close_transform`).
    pub fn close_transform(&self, kind: WindowKind) -> Option<TransformKind> {
        crate::render::motion::close_transform(
            kind,
            self.fx.close_kind(kind),
            self.display_options(),
        )
    }

    /// The machine's defaults are now `system` (the defaults when its file is gone): they apply at once to every key the
    /// person did not change.
    pub fn set_system_accessibility(&mut self, system: AccessibilitySettings) {
        self.accessibility.system = system;
        let settings = self.accessibility.personal.over(&system);
        if self.accessibility.settings == settings {
            return;
        }
        let before = self.display_options();
        self.accessibility.settings = settings;
        tracing::info!("the machine's accessibility settings changed");
        self.apply_accessibility(before);
    }

    /// Read `accessibility.conf` from `path`, apply it and store later changes there.
    pub fn use_accessibility_store(&mut self, path: PathBuf) -> Result<()> {
        let before = self.display_options();
        self.accessibility.use_store(path)?;
        self.apply_accessibility(before);
        Ok(())
    }

    /// Turn a display option on or off: the server draws with it from the next frame, the shell
    /// processes are told and the settings file is written. Setting the value it has changes nothing.
    pub fn set_display_option(&mut self, option: DisplayOption, on: bool) {
        if self.display_options().get(option) == on {
            return;
        }
        let before = self.display_options();
        let mut settings = self.accessibility.settings;
        settings.display.set(option, on);
        tracing::info!(option = option.key(), on, "display option changed");
        self.accessibility.change(settings);
        self.apply_accessibility(before);
    }

    pub fn toggle_display_option(&mut self, option: DisplayOption) {
        let on = !self.display_options().get(option);
        self.set_display_option(option, on);
    }

    /// Replace every accessibility setting (the colour filter included), store and apply them.
    pub fn set_accessibility(&mut self, settings: AccessibilitySettings) {
        if self.accessibility.settings == settings {
            return;
        }
        let before = self.display_options();
        self.accessibility.change(settings);
        self.apply_accessibility(before);
    }

    /// Draw with the current settings; the observers hear of the display options only when they
    /// differ from `before`, and of the steady caret when it changed.
    fn apply_accessibility(&mut self, before: DisplayOptions) {
        self.apply_minimize_timing();
        let appearance = self.appearance();
        self.decor.set_appearance(appearance);
        self.tell_display_options(before);
        self.publish_session_settings();
        self.sync_screen_reader();
        self.request_redraw();
    }
}
