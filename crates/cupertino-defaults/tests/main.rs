//! The integration tests of `cupertino-defaults`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#[path = "accessibility-conf.rs"]
mod accessibility_conf;
#[path = "appearance-conf.rs"]
mod appearance_conf;
#[path = "atomic-write.rs"]
mod atomic_write;
#[path = "compositor-json.rs"]
mod compositor_json;
#[path = "conf-text.rs"]
mod conf_text;
#[path = "config-paths.rs"]
mod config_paths;
#[path = "config-watcher.rs"]
mod config_watcher;
#[path = "data-control-conf.rs"]
mod data_control_conf;
#[path = "desktop-control-conf.rs"]
mod desktop_control_conf;
#[path = "displays-conf.rs"]
mod displays_conf;
#[path = "dock-conf.rs"]
mod dock_conf;
#[path = "file-lock.rs"]
mod file_lock;
#[path = "global-shortcuts-conf.rs"]
mod global_shortcuts_conf;
#[path = "keyboard-conf.rs"]
mod keyboard_conf;
#[path = "launcher-control-panel-power-conf.rs"]
mod launcher_control_panel_power_conf;
#[path = "login-conf.rs"]
mod login_conf;
#[path = "menubar-conf.rs"]
mod menubar_conf;
#[path = "notifications-conf.rs"]
mod notifications_conf;
#[path = "screen-sharing-conf.rs"]
mod screen_sharing_conf;
#[path = "system-files-and-terminal.rs"]
mod system_files_and_terminal;
#[path = "user-dirs.rs"]
mod user_dirs;
