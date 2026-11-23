//! The integration tests of `cupertino-theme`, one binary: each file beside this one is a module of it, so a
//! test's name starts with its file's.

mod activation;
#[path = "animation-data.rs"]
mod animation_data;
mod color;
#[path = "control-panel-motion.rs"]
mod control_panel_motion;
#[path = "glass-params.rs"]
mod glass_params;
#[path = "glass-variants.rs"]
mod glass_variants;
#[path = "launcher-motion.rs"]
mod launcher_motion;
#[path = "notification-motion.rs"]
mod notification_motion;
#[path = "shadow-and-glass.rs"]
mod shadow_and_glass;
#[path = "theme-data.rs"]
mod theme_data;
mod timing;
mod typography;
#[path = "ui-tokens.rs"]
mod ui_tokens;
