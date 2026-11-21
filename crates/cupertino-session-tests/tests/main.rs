//! The integration tests of `cupertino-session-tests`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

#![expect(
    clippy::duplicate_mod,
    reason = "the shared rig and components reach the host of the file using them as `super::host`, so each such file \
              loads them as its own modules"
)]

#[path = "lock-notices.rs"]
mod lock_notices;
#[path = "login-screens.rs"]
mod login_screens;
#[path = "overview-gesture.rs"]
mod overview_gesture;
mod prompts;
mod shell;
#[path = "shell-agent.rs"]
mod shell_agent;
#[path = "shell-controlcenter.rs"]
mod shell_controlcenter;
#[path = "shell-desktop.rs"]
mod shell_desktop;
#[path = "shell-dock.rs"]
mod shell_dock;
#[path = "shell-host.rs"]
mod shell_host;
#[path = "shell-keyboard-panels.rs"]
mod shell_keyboard_panels;
#[path = "shell-launcher.rs"]
mod shell_launcher;
#[path = "shell-menubar.rs"]
mod shell_menubar;
#[path = "shell-notifications.rs"]
mod shell_notifications;
#[path = "shell-overview.rs"]
mod shell_overview;
#[path = "shell-windowmanager.rs"]
mod shell_windowmanager;
