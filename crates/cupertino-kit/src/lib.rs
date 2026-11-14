//! CupertinoKit, the Cupertino UI toolkit. Applications depend on this crate alone; the modules re-export the
//! toolkit's crates and [`prelude`] has what a typical application names.

pub mod app;
pub mod file_icons;
mod icons;

pub use cupertino_geometry as geometry;
pub use cupertino_ui_controls as controls;
pub use cupertino_ui_core as core;
pub use cupertino_ui_layout as layout;
pub use cupertino_ui_platform as platform;
pub use cupertino_ui_render as render;
pub use cupertino_ui_text as text;
pub use cupertino_ui_theme as theme;

pub mod prelude {
    pub use cupertino_ui_controls::{
        Button, ButtonRole, ButtonStyle, Icon, SecureField, Separator, Spacer, Stack, Text,
        TextChange, TextEditor, TextField, Toggle,
    };
    pub use cupertino_ui_core::{
        Application, Command, Element, Glass, GlassStyle, Key, RendererClass, Window,
        WindowBackground, WindowKey,
    };
    pub use cupertino_ui_layout::{Align, Axis};
    pub use cupertino_ui_platform::WindowSpec;
}
