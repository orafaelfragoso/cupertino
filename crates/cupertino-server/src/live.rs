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
//! | `compositor.json` | appearance, tracking speeds, acceleration, scroll direction, cursor size, colours and shake, minimize effect, closing animation, hot corner size and dwell, corner radius, scroll bars, key bindings, terminals, the fractional scale mode; `dev_folder` of the system file, read again through its root-only check, for each shell process's next start | `decorations` (new windows only), `menu_bar_height`, `show_menu_bar`, `shell`, `screenshot` (privileged processes: read at start) |
//! | `keyboard.conf` | every key: repeat, caps lock, filters, rewrite, accent picker, input sources and layouts (the keymap is rebuilt) | — |
//! | `power.conf` | every key (the countdown in progress continues with the new timers) | — |
//! | `/etc/cupertino/accessibility.conf` | its keys, for an account without an `accessibility.conf` of its own and the login screen (`crate::accessibility`) | — |
//! | `data-control.conf` | the allowed applications; a client no longer allowed is disconnected (`crate::data_control`) | — |
//! | `desktop-control.conf` | each app's remembered desktop control and capture answers; a run whose app lost a service it uses is revoked (`crate::control`) | — |

use std::collections::HashMap;
use std::os::fd::AsFd;
use std::path::PathBuf;
use std::time::Instant;

use cupertino_defaults::accessibility::AccessibilitySettings;
use cupertino_defaults::appearance::AppearanceSettings;
use cupertino_defaults::compositor::CompositorSettings;
use cupertino_defaults::data_control::{DataControlConf, Entry as DataControlEntry};
use cupertino_defaults::desktop_control::DesktopControlConf;
use cupertino_defaults::keyboard::KeyboardSettings as KeyboardConf;
use cupertino_defaults::power::PowerSettings;
use cupertino_defaults::watch::{ConfigWatcher, WatchEvent};
use cupertino_keyboard::AccentTable;
use smithay::reexports::calloop::channel::{self, Channel, Event as ChannelEvent, Sender};
use smithay::reexports::calloop::generic::Generic;
use smithay::reexports::calloop::timer::{TimeoutAction, Timer};
use smithay::reexports::calloop::{Interest, Mode, PostAction};

use crate::config::{Config, DevFolder, SettingsFiles};
use crate::kbd::DecodedSource;
use crate::process::{Alive, Lookups};
use crate::state::Cupertino;

/// How many concurrent off-loop read threads may be alive per watched file kind (so a stalled mount
/// cannot leak unbounded threads while a transient slow read does not block a newer change).
const SETTINGS_READ_LIMIT: usize = 2;

/// The watched settings and policy files, one slot each for ordering and concurrency limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
enum ReloadKind {
    UserCompositor = 0,
    SystemCompositor = 1,
    Keyboard = 2,
    Power = 3,
    SystemAccessibility = 4,
    DataControl = 5,
    DesktopControl = 6,
    Appearance = 7,
}

impl ReloadKind {
    const COUNT: usize = 8;

    fn idx(self) -> usize {
        self as usize
    }
}

/// Result of an off-loop settings or policy file read, returned to the compositor loop over a channel.
struct ReloadDone {
    kind: ReloadKind,
    generation: u64,
    payload: ReloadPayload,
}

enum ReloadPayload {
    Compositor {
        path: PathBuf,
        system: bool,
        after: Option<CompositorSettings>,
        /// Verified `dev_folder` from root-only system file check (when `system` is true).
        dev_folder: Option<Result<Option<DevFolder>, String>>,
    },
    Keyboard {
        path: PathBuf,
        after: Option<KeyboardConf>,
        decoded: Option<HashMap<String, DecodedSource>>,
        accent_table: Option<AccentTable>,
    },
    Power {
        path: PathBuf,
        after: Option<PowerSettings>,
    },
    SystemAccessibility {
        settings: AccessibilitySettings,
    },
    DataControl {
        allowed: Vec<DataControlEntry>,
    },
    DesktopControl {
        store: DesktopControlConf,
    },
    Appearance {
        settings: AppearanceSettings,
    },
}

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
    /// Whether a timer waits for the burst of file changes under way to be quiet.
    quiet_timer: bool,
    /// Channel back to the compositor loop for off-loop settings and policy reads.
    reloaded: Option<Sender<ReloadDone>>,
    /// Per-file-kind monotonic generation so slower older reads never overwrite newer ones.
    generations: [u64; ReloadKind::COUNT],
    /// Per-file-kind active reader count, bounded by [`SETTINGS_READ_LIMIT`].
    readers: [Alive; ReloadKind::COUNT],
}

impl Cupertino {
    /// Watch the settings files at `paths`, `files` being what was read from them at start. A watch that cannot be set
    /// up is logged: the files then apply at the next start only. All post-startup reads and policy verifications run
    /// off the compositor loop; policy files grant no privileges while their read is pending or fails.
    pub fn start_settings_watch(&mut self, files: SettingsFiles, paths: SettingsPaths) {
        self.live.files = files;
        self.live.paths = paths.clone();
        self.live.appearance = AppearanceSettings::path();
        self.apply_shell_options();
        self.apply_titlebar_double_click();
        if let Some(path) = &paths.desktop_control {
            self.set_desktop_control_store_path(path.clone());
        }
        let (tx, rx): (Sender<ReloadDone>, Channel<ReloadDone>) = channel::channel();
        match self.loop_handle.insert_source(rx, |event, _, state| {
            if let ChannelEvent::Msg(done) = event {
                state.apply_reload_done(done);
            }
        }) {
            Ok(_) => self.live.reloaded = Some(tx),
            Err(e) => {
                tracing::warn!(error = %e, "cannot register the settings reload channel: live reloads disabled");
                return;
            }
        }
        // Start initial policy verification off the loop; privileges remain empty until verified.
        if let Some(path) = paths.data_control.clone() {
            self.revoke_pending_policy(ReloadKind::DataControl);
            self.dispatch_reload(ReloadKind::DataControl, path);
        }
        if let Some(path) = paths.desktop_control.clone() {
            self.revoke_pending_policy(ReloadKind::DesktopControl);
            self.dispatch_reload(ReloadKind::DesktopControl, path);
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
        let watcher = match ConfigWatcher::new(watched) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(error = %e, "cannot watch the settings files: they apply at the next start only");
                return;
            }
        };
        // the loop waits on a descriptor of the watch's own; it drains kernel events without reading files
        let readable = watcher.as_fd().try_clone_to_owned().and_then(|fd| {
            self.loop_handle
                .insert_source(
                    Generic::new(fd, Interest::READ, Mode::Level),
                    |_, _, state| {
                        state.read_settings();
                        Ok(PostAction::Continue)
                    },
                )
                .map_err(|e| std::io::Error::other(e.to_string()))
        });
        match readable {
            Ok(_) => {
                self.live.watcher = Some(watcher);
                // drain any kernel events that arrived while setting up the watch
                self.read_settings();
            }
            Err(e) => {
                tracing::warn!(error = %e, "cannot watch the settings files: they apply at the next start only")
            }
        }
    }

    /// Map a watched path to its [`ReloadKind`].
    fn reload_kind_for(&self, path: &std::path::Path) -> Option<ReloadKind> {
        if self.live.paths.compositor.as_deref() == Some(path) {
            Some(ReloadKind::UserCompositor)
        } else if self.live.paths.system.as_deref() == Some(path) {
            Some(ReloadKind::SystemCompositor)
        } else if self.live.paths.keyboard.as_deref() == Some(path) {
            Some(ReloadKind::Keyboard)
        } else if self.live.paths.power.as_deref() == Some(path) {
            Some(ReloadKind::Power)
        } else if self.live.paths.system_accessibility.as_deref() == Some(path) {
            Some(ReloadKind::SystemAccessibility)
        } else if self.live.paths.data_control.as_deref() == Some(path) {
            Some(ReloadKind::DataControl)
        } else if self.live.paths.desktop_control.as_deref() == Some(path) {
            Some(ReloadKind::DesktopControl)
        } else if self.live.appearance.as_deref() == Some(path) {
            Some(ReloadKind::Appearance)
        } else {
            None
        }
    }

    /// Revoke privileges granted by a policy file as soon as a change is observed, so an
    /// unverifiable or stalled policy file never grants privileges while its read is pending.
    fn revoke_pending_policy(&mut self, kind: ReloadKind) {
        match kind {
            ReloadKind::DataControl => self.set_data_control_allowed(Vec::new()),
            ReloadKind::DesktopControl => {
                self.apply_desktop_control_store(DesktopControlConf::default());
            }
            ReloadKind::SystemAccessibility => {
                self.set_system_accessibility(AccessibilitySettings::default());
            }
            ReloadKind::SystemCompositor => {
                self.supervisor.set_dev_folder(None);
                self.config.dev_folder = None;
            }
            ReloadKind::UserCompositor
            | ReloadKind::Keyboard
            | ReloadKind::Power
            | ReloadKind::Appearance => {}
        }
    }

    /// Drain kernel watch events without reading any watched file on the compositor loop, revoke
    /// pending policy privileges immediately, and dispatch quiet bursts to off-loop reader threads.
    fn read_settings(&mut self) {
        let Some(watcher) = self.live.watcher.as_mut() else {
            return;
        };
        let polled = watcher.poll_touched(Instant::now());
        let quiet_at = watcher.quiet_at();
        for failure in polled.failures {
            if let WatchEvent::Error(e) = failure {
                tracing::warn!(error = %e, "watching the settings files failed: later changes may apply at the next start only");
            }
        }
        for path in polled.immediate {
            if let Some(kind) = self.reload_kind_for(&path) {
                self.revoke_pending_policy(kind);
            }
        }
        for path in polled.quiet {
            if let Some(kind) = self.reload_kind_for(&path) {
                self.dispatch_reload(kind, path);
            }
        }
        if let Some(at) = quiet_at.filter(|_| !self.live.quiet_timer) {
            let armed = self
                .loop_handle
                .insert_source(Timer::from_deadline(at), |_, _, state| {
                    state.live.quiet_timer = false;
                    state.read_settings();
                    TimeoutAction::Drop
                });
            match armed {
                Ok(_) => self.live.quiet_timer = true,
                Err(e) => {
                    tracing::warn!(error = %e, "cannot wait for the settings files to be quiet: a change applies with the next one")
                }
            }
        }
    }

    /// Spawn an off-loop reader for `kind` at `path` with a fresh generation number.
    fn dispatch_reload(&mut self, kind: ReloadKind, path: PathBuf) {
        let Some(tx) = self.live.reloaded.clone() else {
            return;
        };
        let slot = kind.idx();
        self.live.generations[slot] = self.live.generations[slot].wrapping_add(1);
        let generation = self.live.generations[slot];
        let owner = &self.live.readers[slot];
        let system_before = self.live.files.system.clone();
        let user_before = self.live.files.compositor.clone();
        let keyboard_before = self.live.files.keyboard.clone();
        let keyboard_system = self.config.keyboard.system.clone();
        let power_before = self.live.files.power;
        let started = Lookups::shared().spawn(Some((owner, SETTINGS_READ_LIMIT)), move || {
            let payload = read_off_loop(
                kind,
                path,
                system_before,
                user_before,
                keyboard_before,
                keyboard_system,
                power_before,
            );
            let _ = tx.send(ReloadDone {
                kind,
                generation,
                payload,
            });
        });
        if !started {
            tracing::warn!(?kind, "no thread available to read changed settings file");
            self.revoke_pending_policy(kind);
        }
    }

    /// Apply the result of an off-loop read on the compositor loop, discarding stale generations.
    fn apply_reload_done(&mut self, done: ReloadDone) {
        if done.generation != self.live.generations[done.kind.idx()] {
            return;
        }
        match done.payload {
            ReloadPayload::Compositor {
                path,
                system,
                after,
                dev_folder,
            } => {
                if let Some(after) = after {
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
                    self.apply_compositor_layers(
                        (&system_before, &user_before),
                        (&system_after, &user_after),
                    );
                    self.live.reloads += 1;
                    tracing::info!(path = %path.display(), "configuration reloaded");
                }
                if system {
                    match dev_folder {
                        Some(Ok(folder)) => {
                            if folder.is_none() {
                                tracing::info!("the development folder is gone: every shell process starts its installed program from its next start");
                            }
                            self.supervisor.set_dev_folder(folder.as_ref());
                            self.config.dev_folder = folder;
                        }
                        Some(Err(e)) => {
                            tracing::error!(path = %path.display(), error = %e, "the development folder of the changed system configuration is refused");
                            self.supervisor.set_dev_folder(None);
                            self.config.dev_folder = None;
                        }
                        None => {
                            self.supervisor.set_dev_folder(None);
                            self.config.dev_folder = None;
                        }
                    }
                }
            }
            ReloadPayload::Keyboard {
                path,
                after,
                decoded,
                accent_table,
            } => {
                let Some(after) = after else {
                    return;
                };
                let before = self.live.files.keyboard.clone();
                self.live.files.keyboard = after.clone();
                self.apply_keyboard_change_preloaded(&before, &after, decoded, accent_table);
                self.live.reloads += 1;
                tracing::info!(path = %path.display(), "keyboard settings reloaded");
            }
            ReloadPayload::Power { path, after } => {
                let Some(after) = after else {
                    return;
                };
                let before = self.live.files.power;
                self.live.files.power = after;
                if after != before {
                    self.config.power = after;
                    self.power.set_settings(after);
                }
                self.live.reloads += 1;
                tracing::info!(path = %path.display(), "power settings reloaded");
            }
            ReloadPayload::SystemAccessibility { settings } => {
                self.set_system_accessibility(settings);
                self.live.reloads += 1;
            }
            ReloadPayload::DataControl { allowed } => {
                self.set_data_control_allowed(allowed);
                self.live.reloads += 1;
            }
            ReloadPayload::DesktopControl { store } => {
                self.apply_desktop_control_store(store);
                self.live.reloads += 1;
            }
            ReloadPayload::Appearance { settings } => {
                tracing::info!(
                    tint_windows = settings.tint_windows,
                    double_click = ?settings.titlebar_double_click,
                    "appearance settings reloaded"
                );
                self.config.apply_appearance_settings(&settings);
                self.apply_titlebar_double_click();
                self.request_redraw();
                self.live.reloads += 1;
            }
        }
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
            self.refresh_global_shortcut_reserved();
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
        if changed!(fractional_scale_mode) {
            self.config.fractional_scale_mode = target.fractional_scale_mode.clone();
            self.refresh_render_scales();
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

    /// Apply the keys whose value differs between `before` and `after`, the values `keyboard.conf` held.
    pub fn apply_keyboard_change(&mut self, before: &KeyboardConf, after: &KeyboardConf) {
        self.apply_keyboard_change_preloaded(before, after, None, None);
    }

    /// Like [`Self::apply_keyboard_change`], taking decoded layouts and accent table already read off the compositor
    /// loop so live reloads perform no filesystem reads on the loop thread.
    fn apply_keyboard_change_preloaded(
        &mut self,
        before: &KeyboardConf,
        after: &KeyboardConf,
        preloaded_decoded: Option<HashMap<String, DecodedSource>>,
        preloaded_accent: Option<AccentTable>,
    ) {
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
        if changed!(
            accent_picker,
            accent_table,
            accent_keyboards,
            accent_disabled_apps
        ) {
            let bridge = self.overlays.accent.bridge;
            self.overlays.accent = match preloaded_accent {
                Some(table) => {
                    crate::kbd_ui::AccentUi::with_table(&self.config.keyboard.accent, table)
                }
                None => crate::kbd_ui::AccentUi::new(&self.config.keyboard.accent),
            };
            self.overlays.accent.bridge = bridge;
        }
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
            self.refresh_global_shortcut_reserved();
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
            self.apply_keyboard_settings_with_decoded(preloaded_decoded);
        } else if changed!(repeat_delay_ms, repeat_interval_ms, slow_keys) {
            if let Some(keyboard) = self.seat.get_keyboard() {
                let info = self.config.keyboard.repeat_info();
                keyboard.change_repeat_info(info.rate, info.delay);
            }
        }
    }
}

/// Read, parse and verify a changed settings or policy file off the compositor loop.
fn read_off_loop(
    kind: ReloadKind,
    path: PathBuf,
    system_before: CompositorSettings,
    user_before: CompositorSettings,
    keyboard_before: KeyboardConf,
    keyboard_system: crate::localed::SystemLayout,
    power_before: PowerSettings,
) -> ReloadPayload {
    match kind {
        ReloadKind::UserCompositor | ReloadKind::SystemCompositor => {
            let system = kind == ReloadKind::SystemCompositor;
            let before = if system { &system_before } else { &user_before };
            let after = match CompositorSettings::load(&path, Some(before)) {
                Ok((after, problems)) => {
                    for p in problems {
                        if system {
                            tracing::error!(path = %path.display(), key = p.key, error = %p.message, "invalid system configuration: the key keeps its value");
                        } else {
                            tracing::warn!(path = %path.display(), key = p.key, error = %p.message, "invalid configuration: the key keeps its value");
                        }
                    }
                    Some(after)
                }
                Err(e) => {
                    if system {
                        tracing::error!(path = %path.display(), error = %e, "cannot read the changed system configuration: the running settings stay");
                    } else {
                        tracing::warn!(path = %path.display(), error = %e, "cannot read the changed configuration: the running settings stay");
                    }
                    None
                }
            };
            let dev_folder = if system {
                Some(
                    crate::config::read_system_file(&path)
                        .map(|text| {
                            text.and_then(|t| CompositorSettings::parse(&t, None).0.dev_folder)
                                .map(|f| DevFolder {
                                    path: f.path.into(),
                                    components: f.components,
                                })
                        })
                        .map_err(|e| e.to_string()),
                )
            } else {
                None
            };
            ReloadPayload::Compositor {
                path,
                system,
                after,
                dev_folder,
            }
        }
        ReloadKind::Keyboard => {
            let after = match KeyboardConf::load(&path, Some(&keyboard_before)) {
                Ok((after, problems)) => {
                    for p in problems {
                        tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "invalid keyboard setting: the key keeps its value");
                    }
                    Some(after)
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot read the changed keyboard settings: the running settings stay");
                    None
                }
            };
            let (decoded, accent_table) = match &after {
                Some(conf) => {
                    let mut target = crate::kbd::KeyboardSettings {
                        system: keyboard_system,
                        ..crate::kbd::KeyboardSettings::default()
                    };
                    target.apply_file(conf);
                    target.use_system_layout();
                    let (kb, level3) = (target.layout_keyboard, target.decoded_level3());
                    let mut map = HashMap::new();
                    for spec in &target.sources {
                        let Some(decoded_path) = spec.decoded.as_ref() else {
                            continue;
                        };
                        match crate::kbd::generate_decoded(decoded_path, kb, level3) {
                            Ok(source) => {
                                tracing::info!(
                                    source = spec.id,
                                    layout = source.label,
                                    "decoded layout loaded"
                                );
                                map.insert(spec.id.clone(), source);
                            }
                            Err(e) => {
                                tracing::error!(source = spec.id, error = %e, "decoded layout not loaded; the source cannot be selected");
                            }
                        }
                    }
                    let table = target
                        .accent
                        .table
                        .as_ref()
                        .and_then(|p| {
                            std::fs::read_to_string(p)
                                .map_err(|e| {
                                    tracing::warn!(path = %p.display(), error = %e, "accent table not read")
                                })
                                .ok()
                        })
                        .and_then(|text| {
                            let kbs: Vec<&str> =
                                target.accent.keyboards.iter().map(String::as_str).collect();
                            AccentTable::from_json(&text, &kbs)
                                .map_err(|e| tracing::warn!(error = %e, "accent table is not valid"))
                                .ok()
                        })
                        .unwrap_or_default();
                    (Some(map), Some(table))
                }
                None => (None, None),
            };
            ReloadPayload::Keyboard {
                path,
                after,
                decoded,
                accent_table,
            }
        }
        ReloadKind::Power => {
            let after = match PowerSettings::load(&path, Some(&power_before)) {
                Ok((after, problems)) => {
                    for p in problems {
                        tracing::warn!(path = %path.display(), line = p.line, error = %p.message, "invalid power setting: the key keeps its value");
                    }
                    Some(after)
                }
                Err(e) => {
                    tracing::warn!(path = %path.display(), error = %e, "cannot read the changed power settings: the running settings stay");
                    None
                }
            };
            ReloadPayload::Power { path, after }
        }
        ReloadKind::SystemAccessibility => ReloadPayload::SystemAccessibility {
            settings: crate::accessibility::read_system_accessibility(&path).unwrap_or_default(),
        },
        ReloadKind::DataControl => {
            let allowed = match DataControlConf::load(&path) {
                Ok((conf, problems)) => {
                    for problem in problems {
                        tracing::warn!(path = %path.display(), line = problem.line, problem = problem.message, "data control: ignoring a line");
                    }
                    conf.allowed
                }
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "data control: the file is refused, no application is allowed");
                    Vec::new()
                }
            };
            ReloadPayload::DataControl { allowed }
        }
        ReloadKind::DesktopControl => {
            let store = match DesktopControlConf::load(&path) {
                Ok((conf, problems)) => {
                    for problem in problems {
                        tracing::warn!(path = %path.display(), line = problem.line, problem = problem.message, "desktop control: ignoring a line");
                    }
                    conf
                }
                Err(e) => {
                    tracing::error!(path = %path.display(), error = %e, "desktop control: the file is refused, no app is allowed");
                    DesktopControlConf::default()
                }
            };
            ReloadPayload::DesktopControl { store }
        }
        ReloadKind::Appearance => ReloadPayload::Appearance {
            settings: AppearanceSettings::current(),
        },
    }
}
