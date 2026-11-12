//! Live settings: the server watches `compositor.json`, `keyboard.conf`, `power.conf`, `data-control.conf` and
//! `desktop-control.conf` (`cupertino_defaults::watch`) and
//! applies what changed while it runs. Each change re-reads only the changed file, with the last values read as the last
//! good ones (a line or key that is wrong keeps its value and is logged), and applies only the keys whose value differs
//! from the last read, so a setting changed at run time another way (the `toggle-appearance` key action, the
//! accessibility shortcuts) stays until its own key changes in the file. A key removed from the file goes back to its
//! default, or to the value of the system file `/etc/cupertino/compositor.json` under it: that file is watched too, the target
//! of a change is always the defaults, then the system file, then the user's file, as at start, and a key is applied when
//! either file changed it.
//!
//! | File | Applied at once | At the next start |
//! |---|---|---|
//! | `compositor.json` | appearance, tracking speeds, acceleration, scroll direction, cursor size, colours and shake, minimize effect, closing animation, hot corner size and dwell, corner radius, scroll bars, key bindings, terminals; `dev_folder` of the system file, read again through its root-only check, for each shell process's next start | `decorations` (new windows only), `menu_bar_height`, `show_menu_bar`, `shell`, `screenshot` (privileged processes: read at start) |
//! | `keyboard.conf` | every key: repeat, caps lock, filters, rewrite, accent picker, input sources and layouts (the keymap is rebuilt) | — |
//! | `power.conf` | every key (the countdown in progress continues with the new timers) | — |
//! | `/etc/cupertino/accessibility.conf` | its keys, for an account without an `accessibility.conf` of its own and the login screen (`crate::accessibility`) | — |
//! | `data-control.conf` | the allowed applications; a client no longer allowed is disconnected (`crate::data_control`) | — |
//! | `desktop-control.conf` | each app's remembered desktop control and capture answers; a run whose app lost a service it uses is revoked (`crate::control`) | — |

use std::path::PathBuf;

use cupertino_defaults::compositor::CompositorSettings;
use cupertino_defaults::keyboard::KeyboardSettings as KeyboardConf;
use cupertino_defaults::power::PowerSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use smithay::reexports::calloop::channel;

use crate::config::{Config, SettingsFiles};
use crate::state::Cupertino;

/// Where the watched files are; `None` when neither `XDG_CONFIG_HOME` nor `HOME` names a settings directory.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SettingsPaths {
    pub compositor: Option<PathBuf>,
    /// The system `compositor.json` under the user's, when there is a place for one ([`crate::config::system_config_path`]).
    pub system: Option<PathBuf>,
    pub keyboard: Option<PathBuf>,
    pub power: Option<PathBuf>,
    /// The machine's accessibility defaults, when `/etc/cupertino` exists.
    pub system_accessibility: Option<PathBuf>,
    /// The applications allowed to use data control (`crate::data_control`).
    pub data_control: Option<PathBuf>,
    /// Each app's remembered desktop control and capture answers (`crate::control`).
    pub desktop_control: Option<PathBuf>,
}

impl SettingsPaths {
    /// The paths of the environment (`$CUPERTINO_SERVER_CONFIG`, `$CUPERTINO_KEYBOARD_CONFIG`, `$CUPERTINO_POWER_CONF`,
    /// `$CUPERTINO_DATA_CONTROL_CONF`, `$CUPERTINO_DESKTOP_CONTROL_CONF`, else the settings directory).
    pub fn from_env() -> Self {
        Self {
            compositor: CompositorSettings::path(),
            system: crate::config::system_config_path(std::env::var_os(crate::config::CONFIG_ENV)),
            keyboard: KeyboardConf::path(),
            power: PowerSettings::path(),
            system_accessibility: {
                let path =
                    std::path::Path::new(cupertino_defaults::accessibility::SYSTEM_ACCESSIBILITY);
                path.parent()
                    .is_some_and(std::path::Path::is_dir)
                    .then(|| path.to_path_buf())
            },
            data_control: cupertino_defaults::data_control::DataControlConf::path(),
            desktop_control: cupertino_defaults::desktop_control::DesktopControlConf::path(),
        }
    }
}

/// The values last read from the files and the watch on them.
#[derive(Default)]
pub struct LiveSettings {
    pub files: SettingsFiles,
    pub paths: SettingsPaths,
    /// How many file changes were applied (logged, and what tests wait for).
    pub reloads: u64,
    /// `appearance.conf`, whose tint of windows and title bar double click the server follows.
    appearance: Option<PathBuf>,
    watcher: Option<ConfigWatcher>,
}

impl Cupertino {
    /// Watch the settings files at `paths`, `files` being what was read from them at start. A watch that cannot be set
    /// up is logged: the files then apply at the next start only.
    pub fn start_settings_watch(&mut self, files: SettingsFiles, paths: SettingsPaths) {
        self.live.files = files;
        self.live.paths = paths.clone();
        self.live.appearance = cupertino_defaults::appearance::AppearanceSettings::path();
        self.apply_shell_options();
        self.apply_titlebar_double_click();
        if let Some(path) = &paths.data_control {
            self.reload_data_control(path);
        }
        if let Some(path) = &paths.desktop_control {
            self.reload_desktop_control(path);
        }
        let watched: Vec<PathBuf> = [
            paths.compositor,
            paths.system,
            paths.keyboard,
            paths.power,
            paths.system_accessibility,
            paths.data_control,
            paths.desktop_control,
            self.live.appearance.clone(),
        ]
        .into_iter()
        .flatten()
        .collect();
        if watched.is_empty() {
            tracing::warn!("no settings directory (neither XDG_CONFIG_HOME nor HOME is set): settings apply at the next start only");
            return;
        }
        let (events, rx) = channel::channel::<WatchEvent>();
        if let Err(e) = self.loop_handle.insert_source(rx, |event, _, state| {
            if let channel::Event::Msg(event) = event {
                state.on_settings_event(event);
            }
        }) {
            tracing::warn!(error = %e, "cannot watch the settings files: they apply at the next start only");
            return;
        }
        let watcher = ConfigWatcher::new(watched, move |event| {
            // the receiver is gone only while the event loop shuts down, when no change can be applied anyway
            let _ = events.send(event);
        });
        match watcher {
            Ok(w) => self.live.watcher = Some(w),
            Err(e) => {
                tracing::warn!(error = %e, "cannot watch the settings files: they apply at the next start only")
            }
        }
    }

    fn on_settings_event(&mut self, event: WatchEvent) {
        let paths = match event {
            WatchEvent::Error(e) => {
                tracing::warn!(error = %e, "watching the settings files failed: later changes may apply at the next start only");
                return;
            }
            WatchEvent::Changed(paths) => paths,
        };
        for path in paths {
            if self.live.paths.compositor.as_ref() == Some(&path) {
                self.reload_compositor(&path, false);
            } else if self.live.paths.system.as_ref() == Some(&path) {
                self.reload_compositor(&path, true);
            } else if self.live.paths.keyboard.as_ref() == Some(&path) {
                self.reload_keyboard(&path);
            } else if self.live.paths.power.as_ref() == Some(&path) {
                self.reload_power(&path);
            } else if self.live.paths.system_accessibility.as_ref() == Some(&path) {
                // a file that is gone, or refused (logged), leaves the machine's defaults off
                let system =
                    crate::accessibility::read_system_accessibility(&path).unwrap_or_default();
                self.set_system_accessibility(system);
                self.live.reloads += 1;
            } else if self.live.paths.data_control.as_ref() == Some(&path) {
                self.reload_data_control(&path);
                self.live.reloads += 1;
            } else if self.live.paths.desktop_control.as_ref() == Some(&path) {
                self.reload_desktop_control(&path);
                self.live.reloads += 1;
            } else if self.live.appearance.as_ref() == Some(&path) {
                self.reload_appearance_settings();
                self.live.reloads += 1;
            }
        }
    }

    /// Follow `appearance.conf` again: windows take a tint of the wallpaper or not, and a double click on a title bar
    /// zooms, minimises or does nothing.
    fn reload_appearance_settings(&mut self) {
        let settings = cupertino_defaults::appearance::AppearanceSettings::current();
        tracing::info!(
            tint_windows = settings.tint_windows,
            double_click = ?settings.titlebar_double_click,
            "appearance settings reloaded"
        );
        self.config.apply_appearance_settings(&settings);
        self.apply_titlebar_double_click();
        self.request_redraw();
    }

    /// Hand the title bar's double click action to the decorations.
    pub fn apply_titlebar_double_click(&mut self) {
        use cupertino_decorations::DoubleClickAction;
        use cupertino_defaults::appearance::TitlebarAction;
        self.decor
            .set_double_click(match self.config.titlebar_double_click {
                TitlebarAction::Zoom => DoubleClickAction::Zoom,
                TitlebarAction::Minimize => DoubleClickAction::Minimize,
                TitlebarAction::None => DoubleClickAction::None,
            });
    }

    /// Hand the menu bar's auto-hide, the tiling options and the material density of the configuration to the parts
    /// that act on them.
    pub fn apply_shell_options(&mut self) {
        use cupertino_wm::menu_bar::AutoHide;
        let now = self.now();
        let policy = if self.config.shell.menu_bar_auto_hide {
            AutoHide::Always
        } else {
            AutoHide::FullscreenOnly
        };
        let events = self.menu_bars.set.set_auto_hide(policy, now);
        let outputs: Vec<_> = events
            .into_iter()
            .filter_map(|event| match event {
                cupertino_wm::menu_bar::BarEvent::UsableAreaChanged { output } => Some(output),
                _ => None,
            })
            .collect();
        for output in outputs {
            self.refresh_usable_area(output);
        }
        let snap = &mut self.wm.config_mut().snap;
        snap.edge_drag_tiling = self.config.edge_tiling;
        snap.top_edge_tiling = self.config.edge_tiling;
        snap.tiled_margin = if self.config.tile_margins {
            cupertino_wm::SnapConfig::default().tiled_margin
        } else {
            0.0
        };
    }

    /// Re-read one layer of `compositor.json`, the system file or the user's, and apply what changed in the result.
    fn reload_compositor(&mut self, path: &std::path::Path, system: bool) {
        let layers = |files: &SettingsFiles| {
            if system {
                files.system.clone()
            } else {
                files.compositor.clone()
            }
        };
        let before = layers(&self.live.files);
        let (after, problems) = match CompositorSettings::load(path, Some(&before)) {
            Ok(read) => read,
            Err(e) => {
                // as at start, a system file that is broken is an error: it holds the shell list of the whole desktop
                if system {
                    tracing::error!(path = %path.display(), error = %e, "cannot read the changed system configuration: the running settings stay");
                } else {
                    tracing::warn!(path = %path.display(), error = %e, "cannot read the changed configuration: the running settings stay");
                }
                return;
            }
        };
        for p in problems {
            if system {
                tracing::error!(path = %path.display(), key = p.key, error = %p.message, "invalid system configuration: the key keeps its value");
            } else {
                tracing::warn!(path = %path.display(), key = p.key, error = %p.message, "invalid configuration: the key keeps its value");
            }
        }
        let (system_before, user_before) = (
            self.live.files.system.clone(),
            self.live.files.compositor.clone(),
        );
        if system {
            self.live.files.system = after;
        } else {
            self.live.files.compositor = after;
        }
        let (system_after, user_after) = (
            self.live.files.system.clone(),
            self.live.files.compositor.clone(),
        );
        self.apply_compositor_layers((&system_before, &user_before), (&system_after, &user_after));
        if system && system_before.dev_folder != system_after.dev_folder {
            self.reload_dev_folder(path);
        }
        self.live.reloads += 1;
        tracing::info!(path = %path.display(), "configuration reloaded");
    }

    /// Read the development folder of the system file at `path` again, through the check that only root could have
    /// written the file, and give it to the supervisor for each shell process's next start. A file that fails the check
    /// leaves the folder as it was.
    fn reload_dev_folder(&mut self, path: &std::path::Path) {
        let declared = match crate::config::read_system_file(path) {
            Ok(text) => text.and_then(|text| CompositorSettings::parse(&text, None).0.dev_folder),
            Err(e) => {
                tracing::error!(path = %path.display(), error = %e, "the development folder of the changed system configuration is refused: it stays as it was");
                return;
            }
        };
        let folder = declared.map(|f| crate::config::DevFolder {
            path: f.path.into(),
            components: f.components,
        });
        if folder.is_none() {
            tracing::info!("the development folder is gone: every shell process starts its installed program from its next start");
        }
        self.supervisor.set_dev_folder(folder.as_ref());
        self.config.dev_folder = folder;
    }

    /// Apply the keys whose value differs between `before` and `after`, the values the user's `compositor.json` held
    /// over the system file's.
    pub fn apply_compositor_change(
        &mut self,
        before: &CompositorSettings,
        after: &CompositorSettings,
    ) {
        let system = self.live.files.system.clone();
        self.apply_compositor_layers((&system, before), (&system, after));
    }

    /// Apply the keys whose value differs between `before` and `after`, each the system file and the user's file: a key
    /// changes when either file changed it, and the new value is what the defaults, the system file and the user's file
    /// over it make, as at start.
    pub fn apply_compositor_layers(
        &mut self,
        before: (&CompositorSettings, &CompositorSettings),
        after: (&CompositorSettings, &CompositorSettings),
    ) {
        let (mut target, problems) =
            Config::from_layers(after.0, after.1, &self.config.keyboard.profile);
        for problem in problems {
            tracing::warn!(error = %problem, "invalid configuration: the key keeps its default");
        }
        let target = &mut target;
        macro_rules! changed {
            ($($key:ident),+) => { false $(|| before.0.$key != after.0.$key || before.1.$key != after.1.$key)+ };
        }
        if changed!(appearance) {
            self.set_appearance(target.appearance);
        }
        if changed!(terminals) {
            self.config.terminals = target.terminals.clone();
        }
        if changed!(keybindings) {
            self.config.keybindings = target.keybindings.clone();
            self.key_router.bindings = target.keybindings.clone();
            self.refresh_menu_bar_shortcuts();
        }
        if changed!(corner_radius) {
            self.config.window.corner_radius = target.window.corner_radius;
        }
        if changed!(show_scroll_bars) {
            self.config.show_scroll_bars = target.show_scroll_bars;
            self.sync_scroll_bars();
        }
        if changed!(minimize_effect, closing_animation) {
            self.config.effects = target.effects;
            self.fx.cfg = target.effects;
            self.apply_minimize_timing();
        }
        if changed!(hot_corner_size, hot_corner_dwell) {
            self.config.hot_corners = target.hot_corners.clone();
            self.hot_corners = cupertino_input::HotCorners::new(target.hot_corners.clone());
        }
        if changed!(
            mouse_speed,
            trackpad_speed,
            mouse_acceleration,
            trackpad_acceleration,
            mouse_dpi,
            natural_scrolling,
            mouse_natural_scrolling,
            cursor_size,
            cursor_fill,
            cursor_outline,
            shake_to_locate
        ) {
            let tracking = changed!(
                mouse_speed,
                trackpad_speed,
                mouse_acceleration,
                trackpad_acceleration,
                mouse_dpi
            );
            self.config.pointer = target.pointer.clone();
            if tracking {
                for device in self.pointer_devices.values_mut() {
                    device.reconfigure(&self.config.pointer);
                }
            }
            if changed!(mouse_dpi) {
                self.publish_session_settings();
            }
            self.cursor_loader
                .set_colors(self.config.pointer.cursor_colors);
        }
        if changed!(wallpaper, wallpaper_dark) {
            self.config.wallpaper = target.wallpaper.clone();
            self.request_redraw();
        }
        if changed!(
            menu_bar_auto_hide,
            edge_tiling,
            tile_margins,
            wallpaper_color,
            material_density
        ) {
            self.config.shell.menu_bar_auto_hide = target.shell.menu_bar_auto_hide;
            self.config.edge_tiling = target.edge_tiling;
            self.config.tile_margins = target.tile_margins;
            self.config.wallpaper_color = target.wallpaper_color;
            self.config.material_density = target.material_density;
            self.apply_shell_options();
            self.request_redraw();
        }
        if changed!(decorations) {
            self.config.window.decorations = target.window.decorations;
            tracing::info!("`decorations` changed: windows that open from now on follow it, open windows keep theirs");
        }
        if changed!(menu_bar_height, show_menu_bar, shell) {
            tracing::info!(
                "`menu_bar_height`, `show_menu_bar` and `shell` apply at the next start"
            );
        }
        self.request_redraw();
    }

    fn reload_keyboard(&mut self, path: &std::path::Path) {
        let before = self.live.files.keyboard.clone();
        let (after, problems) = match KeyboardConf::load(path, Some(&before)) {
            Ok(read) => read,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read the changed keyboard settings: the running settings stay");
                return;
            }
        };
        for p in problems {
            tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "invalid keyboard setting: the key keeps its value");
        }
        self.live.files.keyboard = after.clone();
        self.apply_keyboard_change(&before, &after);
        self.live.reloads += 1;
        tracing::info!(path = %path.display(), "keyboard settings reloaded");
    }

    /// Apply the keys whose value differs between `before` and `after`, the values `keyboard.conf` held.
    pub fn apply_keyboard_change(&mut self, before: &KeyboardConf, after: &KeyboardConf) {
        let mut target = crate::kbd::KeyboardSettings {
            system: self.config.keyboard.system.clone(),
            ..crate::kbd::KeyboardSettings::default()
        };
        target.apply_file(after);
        target.use_system_layout();
        let (kb, t) = (&mut self.config.keyboard, &target);
        macro_rules! changed {
            ($($key:ident),+) => { false $(|| before.$key != after.$key)+ };
        }
        macro_rules! take {
            ($key:ident => $($field:tt)+) => {
                if changed!($key) {
                    kb.$($field)+ = t.$($field)+.clone();
                }
            };
        }
        take!(repeat_delay_ms => repeat.initial_delay_ms);
        take!(repeat_interval_ms => repeat.interval_ms);
        take!(modifier_profile => profile);
        take!(input_sources => sources);
        take!(caps_lock_delay_ms => caps.delay_ms);
        take!(caps_lock_switch => caps_switch);
        take!(caps_lock_hold_ms => caps_hold_ms);
        take!(fn_key => fn_usage);
        take!(layout_keyboard => layout_keyboard);
        take!(layout_level3 => layout_level3);
        take!(compose_key => compose);
        take!(slow_keys => access.slow_keys);
        take!(slow_keys_delay_ms => access.slow_keys_delay_ms);
        take!(sticky_keys => access.sticky_keys);
        take!(sticky_keys_shortcut => access.sticky_keys_shortcut);
        take!(mouse_keys => access.mouse_keys);
        take!(mouse_keys_shortcut => access.mouse_keys_shortcut);
        take!(mouse_keys_delay_ms => access.mouse_keys_delay_ms);
        take!(mouse_keys_max_speed => access.mouse_keys_max_speed);
        take!(client_rewrite => client_rewrite);
        take!(rewrite_terminals => rewrite_terminals);
        take!(rewrite_off => rewrite_off);
        take!(accent_picker => accent.enabled);
        take!(accent_table => accent.table);
        take!(accent_keyboards => accent.keyboards);
        take!(accent_disabled_apps => accent.disabled_apps);
        if changed!(modifier_profile) {
            // the profile has its own shortcut table: build the bindings again, as at start
            let (rebuilt, _) = Config::from_layers(
                &self.live.files.system,
                &self.live.files.compositor,
                &self.config.keyboard.profile,
            );
            self.config.keybindings = rebuilt.keybindings.clone();
            self.key_router.bindings = rebuilt.keybindings;
            self.refresh_menu_bar_shortcuts();
        }
        if changed!(
            caps_lock_delay_ms,
            caps_lock_switch,
            caps_lock_hold_ms,
            input_sources
        ) {
            self.caps = crate::kbd::CapsDebounce::new(
                self.config.keyboard.caps,
                self.config.keyboard.caps_switch_hold(),
            );
        }
        if changed!(
            slow_keys,
            slow_keys_delay_ms,
            sticky_keys,
            sticky_keys_shortcut,
            mouse_keys,
            mouse_keys_shortcut,
            mouse_keys_delay_ms,
            mouse_keys_max_speed
        ) {
            self.key_filters = crate::kbd_filters::KeyFilters::new(&self.config.keyboard);
        }
        if changed!(input_sources, layout_keyboard, layout_level3, compose_key) {
            self.overlays.inputs =
                crate::kbd::InputSources::new(self.config.keyboard.sources.clone());
            // the repeat too, and the keymap of the new sources
            self.apply_keyboard_settings();
        } else if changed!(repeat_delay_ms, repeat_interval_ms, slow_keys) {
            if let Some(keyboard) = self.seat.get_keyboard() {
                let info = self.config.keyboard.repeat_info();
                keyboard.change_repeat_info(info.rate, info.delay);
            }
        }
    }

    fn reload_power(&mut self, path: &std::path::Path) {
        let before = self.live.files.power;
        let (after, problems) = match PowerSettings::load(path, Some(&before)) {
            Ok(read) => read,
            Err(e) => {
                tracing::warn!(path = %path.display(), error = %e, "cannot read the changed power settings: the running settings stay");
                return;
            }
        };
        for p in problems {
            tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "invalid power setting: the key keeps its value");
        }
        self.live.files.power = after;
        if after != before {
            self.config.power = after;
            self.power.set_settings(after);
        }
        self.live.reloads += 1;
        tracing::info!(path = %path.display(), "power settings reloaded");
    }
}
