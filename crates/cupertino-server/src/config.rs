//! Compositor configuration: window style, shell geometry, key bindings and spawn defaults.
//!
//! Every value that is not is approximate or unknown and therefore configurable. An
//! optional JSON file (`$CUPERTINO_SERVER_CONFIG`, else
//! `$XDG_CONFIG_HOME/cupertino/compositor.json`) overrides the defaults; unknown keys are ignored.

use std::path::PathBuf;

use crate::input::{Action, KeyBindings};
use cupertino_defaults::compositor::{AppearanceName, CompositorSettings, MinimizeEffectName};
use cupertino_theme::Appearance;

/// Environment variable naming the configuration file.
pub use cupertino_defaults::compositor::COMPOSITOR_CONF_ENV as CONFIG_ENV;

/// A shell process the server starts and keeps alive (see [`crate::supervisor`]).
#[derive(Clone, Debug, PartialEq)]
pub struct ShellProcess {
    /// Name in log lines.
    pub name: String,
    /// Program and arguments; never empty.
    pub command: Vec<String>,
}

/// One window shadow preset: Gaussian σ and y offset in points, density as peak alpha.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowPreset {
    pub sigma: f64,
    pub offset_y: f64,
    pub density: f64,
}

/// Window shadows.: standard active σ 20 / offset 18 /
/// density 0.40, inactive σ 12 / offset 8 / density 0.25, dark appearance active density 0.70.
/// The dark inactive density is not part of the approximate as the light ratio
/// (0.25 / 0.40) applied to the dark active density.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowStyle {
    pub active: ShadowPreset,
    pub inactive: ShadowPreset,
    pub dark_active_density: f64,
    pub dark_inactive_density: f64,
    /// Remove the shadow underneath the window shape so translucent windows stay clean.
    /// approximate design choice.
    pub cutout: bool,
}

impl Default for ShadowStyle {
    fn default() -> Self {
        Self {
            active: ShadowPreset {
                sigma: 20.0,
                offset_y: 18.0,
                density: 0.40,
            },
            inactive: ShadowPreset {
                sigma: 12.0,
                offset_y: 8.0,
                density: 0.25,
            },
            dark_active_density: 0.70,
            dark_inactive_density: 0.70 * (0.25 / 0.40),
            cutout: true,
        }
    }
}

impl ShadowStyle {
    /// Preset for a window state and appearance.
    pub fn preset(&self, active: bool, dark: bool) -> ShadowPreset {
        let mut p = if active { self.active } else { self.inactive };
        if dark {
            p.density = if active {
                self.dark_active_density
            } else {
                self.dark_inactive_density
            };
        }
        p
    }
}

/// Space the compositor reserves around a server-side decorated client surface (title bar and
/// borders). Zero by default; the decoration component sets it (unknown until its metrics exist).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Insets {
    pub top: f64,
    pub left: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Insets {
    pub fn horizontal(&self) -> f64 {
        self.left + self.right
    }
    pub fn vertical(&self) -> f64 {
        self.top + self.bottom
    }
}

/// Window corner style. radius for regular windows: 12 pt; the continuous-corner expansion comes from `cupertino-geometry`
/// (approximate).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowStyle {
    pub corner_radius: f64,
    pub continuous_corners: bool,
    pub shadow: ShadowStyle,
    /// Apply shadow and corner mask to clients that draw their own decorations.
    pub style_client_decorated: bool,
    /// Insets around the surface of server-side decorated windows (integration point for the
    /// decoration component: window frame = surface geometry + insets).
    pub ssd_insets: Insets,
    /// Draw the built-in server-side decorations (`cupertino-decorations`) for clients that
    /// accept them; their insets then replace `ssd_insets`.
    pub decorations: bool,
}

impl Default for WindowStyle {
    fn default() -> Self {
        Self {
            corner_radius: 12.0,
            continuous_corners: true,
            shadow: ShadowStyle::default(),
            style_client_decorated: true,
            ssd_insets: Insets::default(),
            decorations: true,
        }
    }
}

/// Shell geometry (approximate unless noted; menu bar height from `cupertino-theme`).
#[derive(Clone, Debug, PartialEq)]
pub struct ShellConfig {
    pub menu_bar_height: f64,
    pub overview_dim: f64,
    /// Reserve the menu bar's height in the usable area (off: windows use the whole output).
    pub show_menu_bar: bool,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            menu_bar_height: cupertino_theme::ShapeTokens::default().menu_bar_height,
            overview_dim: 0.35,
            show_menu_bar: true,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Config {
    pub appearance: Appearance,
    pub window: WindowStyle,
    pub shell: ShellConfig,
    /// Shell processes supervised by the server (`shell` in the JSON file); none by default.
    pub shell_processes: Vec<ShellProcess>,
    pub keybindings: KeyBindings,
    pub effects: crate::fx::EffectsConfig,
    /// Hot-corner detection: all four corners arm (the binding's action is a placeholder; the dock
    /// maps a corner to its action), `size` and `dwell` from the file.: the dwell
    /// defaults to 0.0 s.
    pub hot_corners: cupertino_input::HotCornerConfig,
    /// Terminal candidates tried in order for the spawn action.
    pub terminals: Vec<String>,
    /// Program and leading arguments of the screenshot process (`screenshot` in the JSON file), started by the
    /// screenshot shortcuts with the mode appended ([`crate::capture::launch`]); never empty.
    pub screenshot_command: Vec<String>,
    /// Cascade step of new windows without a position, right and down (points). 29 pt
    /// (the design's horizontal and vertical cascade amounts).
    pub cascade_step: f64,
    /// Fraction of the usable area a client without a size preference opens at.
    pub default_window_fraction: f64,
    /// Key repeat, modifier profile, input sources, caps-lock delay, accent picker (`keyboard.conf`).
    pub keyboard: crate::kbd::KeyboardSettings,
    /// Tracking speed, acceleration and scroll direction.
    pub pointer: PointerSettings,
    /// Dim, display sleep, the password delay and system sleep (`power.conf`).
    pub power: cupertino_defaults::power::PowerSettings,
}

/// Pointer settings.
#[derive(Clone, Debug, PartialEq)]
pub struct PointerSettings {
    /// Mouse tracking speed, 0…3 (stops: `cupertino_input::TRACKING_SPEED_STOPS`).
    /// default 0.6875.
    pub mouse_speed: f64,
    /// Touchpad tracking speed, 0…3. default 0.6875.
    pub trackpad_speed: f64,
    /// Resolution the user stored per mouse, by `cupertino_defaults::compositor::mouse_dpi_key`; a mouse without an
    /// entry uses the hardware database's, else 1000.
    pub mouse_dpi: std::collections::BTreeMap<String, u32>,
    /// Speed-dependent mouse acceleration (default on); off gives a constant gain by `mouse_speed`, per
    /// count of a 400 dpi mouse and scaled by the real resolution (a negative speed passes counts through).
    pub mouse_acceleration: bool,
    /// Content follows the fingers (default on).
    pub natural_scrolling: bool,
    /// Cursor size as a scale of the theme's nominal size, 1…4 (range, the
    /// fresh 1.0 is derived from the plist default of the size key). It scales the
    /// cursors the server draws and the `XCURSOR_SIZE` of the clients it starts.
    pub cursor_scale: f64,
    /// Fill and outline of the server-drawn cursors (defaults black / white).
    pub cursor_colors: crate::cursor::CursorColors,
    /// Shaking the pointer grows the cursor; default on: the design only has
    /// a disable key.
    pub shake_to_locate: bool,
}

impl PointerSettings {
    /// Apply the pointer keys `f` sets.
    pub fn apply_file(&mut self, f: &CompositorSettings) {
        if let Some(v) = f.mouse_speed {
            self.mouse_speed = v;
        }
        if let Some(v) = f.trackpad_speed {
            self.trackpad_speed = v;
        }
        if let Some(m) = &f.mouse_dpi {
            self.mouse_dpi = m.clone();
        }
        if let Some(b) = f.mouse_acceleration {
            self.mouse_acceleration = b;
        }
        if let Some(b) = f.natural_scrolling {
            self.natural_scrolling = b;
        }
        if let Some(s) = f.cursor_size {
            // JSON has no NaN, so the clamp cannot panic
            self.cursor_scale = cupertino_input::clamp_cursor_scale(s);
        }
        if let Some(c) = f.cursor_fill {
            self.cursor_colors.fill = c;
        }
        if let Some(c) = f.cursor_outline {
            self.cursor_colors.outline = c;
        }
        if let Some(b) = f.shake_to_locate {
            self.shake_to_locate = b;
        }
    }
}

impl Default for PointerSettings {
    fn default() -> Self {
        let speed = cupertino_input::DEFAULT_TRACKING_SPEED;
        Self {
            mouse_speed: speed,
            trackpad_speed: speed,
            mouse_dpi: std::collections::BTreeMap::new(),
            mouse_acceleration: cupertino_input::DEFAULT_ACCELERATION,
            natural_scrolling: true,
            cursor_scale: cupertino_input::CURSOR_SCALE_MIN,
            cursor_colors: crate::cursor::CursorColors::DEFAULT,
            shake_to_locate: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            appearance: Appearance::Light,
            window: WindowStyle::default(),
            shell: ShellConfig::default(),
            shell_processes: Vec::new(),
            keybindings: KeyBindings::from_shortcuts(&crate::kbd::default_shortcut_map().0),
            effects: crate::fx::EffectsConfig::default(),
            hot_corners: default_hot_corners(),
            terminals: ["foot", "alacritty", "kitty", "wezterm", "xterm"]
                .map(String::from)
                .to_vec(),
            screenshot_command: vec!["cupertino-screenshot".to_string()],
            cascade_step: 29.0,
            default_window_fraction: 0.6,
            keyboard: crate::kbd::KeyboardSettings::default(),
            pointer: PointerSettings::default(),
            power: cupertino_defaults::power::PowerSettings::default(),
        }
    }
}

/// Path of the optional shortcut file: `$CUPERTINO_SHORTCUTS`, else
/// `$XDG_CONFIG_HOME/cupertino/shortcuts.conf` (default `~/.config`).
pub fn shortcuts_path() -> Option<PathBuf> {
    cupertino_defaults::config_file("CUPERTINO_SHORTCUTS", "shortcuts.conf")
}

/// The default shortcut map with the lines of the shortcut file applied; invalid lines are
/// reported and skipped.
pub fn shortcut_map() -> cupertino_input::ShortcutMap {
    let (mut map, report) = crate::kbd::default_shortcut_map();
    for (chord, table, kept) in &report.conflicts {
        tracing::warn!(
            chord,
            table,
            kept,
            "reference shortcut table row dropped: chord already bound"
        );
    }
    if let Some(path) = shortcuts_path() {
        if let Ok(text) = std::fs::read_to_string(&path) {
            for (line, err) in map.apply_config(&text) {
                tracing::warn!(path = %path.display(), line, error = %err, "ignoring invalid shortcut");
            }
        }
    }
    map
}

/// Every corner armed, so the recognizer reports each one; which action a corner runs is the
/// dock's setting (`dock.conf`), so the binding's action is never read.
fn default_hot_corners() -> cupertino_input::HotCornerConfig {
    let mut cfg = cupertino_input::HotCornerConfig {
        dwell: 0.0,
        ..cupertino_input::HotCornerConfig::default()
    };
    for corner in cupertino_input::Corner::ALL {
        cfg.set(
            corner,
            Some(cupertino_input::CornerBinding::new(
                cupertino_input::CornerAction::Overview,
            )),
        );
    }
    cfg
}

/// The power settings with `power.conf` applied; invalid lines are reported and keep the defaults, and an unreadable
/// file leaves the defaults (reported).
fn load_power() -> cupertino_defaults::power::PowerSettings {
    use cupertino_defaults::power::PowerSettings;
    let Some(path) = PowerSettings::path() else {
        return PowerSettings::default();
    };
    match PowerSettings::load(&path, None) {
        Ok((settings, problems)) => {
            for p in problems {
                tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "ignoring invalid power setting");
            }
            settings
        }
        Err(e) => {
            tracing::warn!(path = %path.display(), error = %e, "cannot read the power settings: the defaults apply");
            PowerSettings::default()
        }
    }
}

/// What the settings files held when they were last read: the start of the next change's comparison
/// (`crate::live`).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SettingsFiles {
    pub compositor: CompositorSettings,
    pub keyboard: cupertino_defaults::keyboard::KeyboardSettings,
    pub power: cupertino_defaults::power::PowerSettings,
}

impl Config {
    /// Path of the configuration file, if one is configured or present.
    pub fn path() -> Option<PathBuf> {
        if let Some(p) = std::env::var_os(CONFIG_ENV).filter(|p| !p.is_empty()) {
            return Some(PathBuf::from(p));
        }
        cupertino_defaults::config_file(CONFIG_ENV, "compositor.json").filter(|p| p.is_file())
    }

    /// Defaults overridden by the settings files, and what the files held. Problems are reported and their keys keep
    /// the defaults.
    pub fn load_with_files() -> (Self, SettingsFiles) {
        let (keyboard, keyboard_file) = crate::kbd::KeyboardSettings::load();
        let power = load_power();
        let mut cfg = Config {
            keybindings: KeyBindings::from_shortcuts(&shortcut_map()),
            keyboard,
            power,
            ..Config::default()
        };
        let mut compositor = CompositorSettings::default();
        if let Some(path) = Self::path() {
            match CompositorSettings::load(&path, None) {
                Ok((file, problems)) => {
                    for p in problems {
                        tracing::warn!(path = %path.display(), key = p.key, error = %p.message, "ignoring invalid configuration");
                    }
                    for problem in cfg.apply_file(&file) {
                        tracing::warn!(path = %path.display(), error = %problem, "ignoring invalid configuration");
                    }
                    compositor = file;
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot read configuration")
                }
            }
        }
        (
            cfg,
            SettingsFiles {
                compositor,
                keyboard: keyboard_file,
                power,
            },
        )
    }

    /// [`Config::load_with_files`] without the files.
    pub fn load() -> Self {
        Self::load_with_files().0
    }

    /// Apply the keys `f` sets on top of the current values. Returns what the server itself refuses (an unknown key
    /// action or chord); everything else applies.
    pub fn apply_file(&mut self, f: &CompositorSettings) -> Vec<String> {
        let mut problems = Vec::new();
        if let Some(a) = f.appearance {
            self.appearance = match a {
                AppearanceName::Light => Appearance::Light,
                AppearanceName::Dark => Appearance::Dark,
                AppearanceName::LightHighContrast => Appearance::LightHighContrast,
                AppearanceName::DarkHighContrast => Appearance::DarkHighContrast,
            };
        }
        if let Some(t) = &f.terminals {
            self.terminals = t.clone();
        }
        if let Some(r) = f.corner_radius {
            self.window.corner_radius = r;
        }
        if let Some(h) = f.menu_bar_height {
            self.shell.menu_bar_height = h;
        }
        if let Some(b) = f.show_menu_bar {
            self.shell.show_menu_bar = b;
        }
        if let Some(b) = f.decorations {
            self.window.decorations = b;
        }
        if let Some(b) = f.closing_animation {
            self.effects.closing = b;
        }
        if let Some(e) = f.minimize_effect {
            self.effects.minimize = match e {
                MinimizeEffectName::Warp => Some(cupertino_effects::MinimizeKind::Warp),
                MinimizeEffectName::Funnel => Some(cupertino_effects::MinimizeKind::Funnel),
                MinimizeEffectName::Scale => Some(cupertino_effects::MinimizeKind::Scale),
                MinimizeEffectName::Plain => None,
            };
        }
        if let Some(s) = f.hot_corner_size {
            self.hot_corners.size = s;
        }
        if let Some(d) = f.hot_corner_dwell {
            self.hot_corners.dwell = d;
        }
        self.pointer.apply_file(f);
        if let Some(procs) = &f.shell {
            self.shell_processes = procs
                .iter()
                .map(|p| ShellProcess {
                    name: p.name.clone(),
                    command: p.command.clone(),
                })
                .collect();
        }
        if let Some(command) = &f.screenshot {
            self.screenshot_command = command.clone();
        }
        for (chord, action) in f.keybindings.iter().flatten() {
            match Action::from_name(action) {
                Some(a) => {
                    if let Err(e) = self.keybindings.bind_str(chord, a) {
                        problems.push(format!("keybindings: {chord}: {e}"));
                    }
                }
                None => problems.push(format!("keybindings: {chord}: unknown action {action}")),
            }
        }
        problems
    }

    pub fn is_dark(&self) -> bool {
        self.appearance.is_dark()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Apply `json` to `c`; the problems of the file and those the server finds, as text.
    fn apply(c: &mut Config, json: &str) -> Vec<String> {
        let (file, problems) = CompositorSettings::parse(json, None);
        let mut out: Vec<String> = problems
            .into_iter()
            .map(|p| format!("{}: {}", p.key, p.message))
            .collect();
        out.extend(c.apply_file(&file));
        out
    }

    #[test]
    fn shadow_presets_follow_evidence() {
        let s = ShadowStyle::default();
        assert_eq!(
            s.preset(true, false),
            ShadowPreset {
                sigma: 20.0,
                offset_y: 18.0,
                density: 0.40
            }
        );
        assert_eq!(
            s.preset(false, false),
            ShadowPreset {
                sigma: 12.0,
                offset_y: 8.0,
                density: 0.25
            }
        );
        assert_eq!(s.preset(true, true).density, 0.70);
        assert_eq!(s.preset(true, true).sigma, 20.0);
        assert!(s.preset(false, true).density < 0.70);
    }

    #[test]
    fn corner_radius_is_twelve() {
        assert_eq!(WindowStyle::default().corner_radius, 12.0);
    }

    #[test]
    fn json_overrides_apply() {
        let mut c = Config::default();
        assert_eq!(
            apply(
                &mut c,
                r#"{"appearance":"dark","corner_radius":10,"terminals":["kitty"],"keybindings":{"Super+t":"spawn-terminal"}}"#
            ),
            Vec::<String>::new()
        );
        assert!(c.is_dark());
        assert_eq!(c.window.corner_radius, 10.0);
        assert_eq!(c.terminals, vec!["kitty".to_string()]);
        assert!(!apply(&mut c, r#"{"keybindings":{"Super+t":"nope"}}"#).is_empty());
        assert_eq!(
            apply(
                &mut c,
                r#"{"minimize_effect":"funnel","decorations":false,"closing_animation":false}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.effects.minimize,
            Some(cupertino_effects::MinimizeKind::Funnel)
        );
        assert!(!c.window.decorations && !c.effects.closing);
        assert!(!apply(&mut c, r#"{"minimize_effect":"nope"}"#).is_empty());
        assert_eq!(c.hot_corners.dwell, 0.0, " default");
        assert!(
            cupertino_input::Corner::ALL
                .iter()
                .all(|c2| c.hot_corners.binding(*c2).is_some()),
            "every corner arms; the dock maps it"
        );
        assert_eq!(
            apply(&mut c, r#"{"hot_corner_dwell":0.2,"hot_corner_size":3}"#),
            Vec::<String>::new()
        );
        assert_eq!((c.hot_corners.dwell, c.hot_corners.size), (0.2, 3.0));
        assert_eq!(
            apply(
                &mut c,
                r#"{"mouse_speed":0.6875,"trackpad_speed":3,"mouse_acceleration":false,"natural_scrolling":false}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.pointer,
            PointerSettings {
                mouse_speed: 0.6875,
                trackpad_speed: 3.0,
                mouse_acceleration: false,
                natural_scrolling: false,
                ..PointerSettings::default()
            }
        );
        assert_eq!(
            apply(
                &mut c,
                r##"{"cursor_size":7,"cursor_fill":"#ff0000","cursor_outline":"#00ff0080","shake_to_locate":false}"##
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.pointer.cursor_scale, 4.0,
            "the size is clamped to the settings range"
        );
        assert_eq!(
            c.pointer.cursor_colors,
            crate::cursor::CursorColors {
                fill: [255, 0, 0, 255],
                outline: [0, 255, 0, 128]
            }
        );
        assert!(!c.pointer.shake_to_locate);
        for bad in [
            r#"{"cursor_fill":"red"}"#,
            r##"{"cursor_fill":"#12345"}"##,
            r##"{"cursor_outline":"#gg0000"}"##,
        ] {
            assert!(!apply(&mut c, bad).is_empty(), "{bad}");
        }
        assert!(!apply(&mut c, "{").is_empty());
        assert!(c.shell_processes.is_empty());
        assert_eq!(
            apply(
                &mut c,
                r#"{"shell":[{"name":"dock","command":["cupertino-dock","--bar"]}]}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.shell_processes,
            vec![ShellProcess {
                name: "dock".into(),
                command: vec!["cupertino-dock".into(), "--bar".into()]
            }]
        );
        assert!(!apply(&mut c, r#"{"shell":[{"name":"dock","command":[]}]}"#).is_empty());
        assert_eq!(
            c.screenshot_command,
            vec!["cupertino-screenshot".to_string()]
        );
        assert_eq!(
            apply(
                &mut c,
                r#"{"screenshot":["/opt/cupertino/bin/cupertino-screenshot"]}"#
            ),
            Vec::<String>::new()
        );
        assert_eq!(
            c.screenshot_command,
            vec!["/opt/cupertino/bin/cupertino-screenshot".to_string()]
        );
        assert!(!apply(&mut c, r#"{"screenshot":[]}"#).is_empty());
    }
}
