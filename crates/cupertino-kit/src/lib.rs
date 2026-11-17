//! CupertinoKit, the Cupertino UI toolkit. Applications depend on this crate alone; the modules re-export the
//! toolkit's crates and [`prelude`] has what a typical application names.

pub mod app;
pub mod bus;
pub mod file_icons;
mod icons;
mod instance;
pub mod task;
pub mod time;

pub use instance::{Entered, Instance};

pub use cupertino_geometry as geometry;
pub use cupertino_theme::ui as theme;
pub use cupertino_ui_controls as controls;
pub use cupertino_ui_core as core;
pub use cupertino_ui_core::layout;
pub use cupertino_ui_core::platform;
pub use cupertino_ui_render as render;
pub use cupertino_ui_text as text;

pub mod prelude {
    pub use cupertino_ui_controls::{
        Button, ButtonRole, ButtonStyle, Icon, SecureField, Separator, Spacer, Stack, Text,
        TextChange, TextEditor, TextField, Toggle,
    };
    pub use cupertino_ui_core::layout::{Align, Axis};
    pub use cupertino_ui_core::platform::WindowSpec;
    pub use cupertino_ui_core::{
        Application, Command, Element, Glass, GlassStyle, Key, RendererClass, Window,
        WindowBackground, WindowKey,
    };
}

/// Type-to-select for the items of a menu or a pop-up list.
pub mod typeahead {
    pub use cupertino_ui_controls::typeahead::TypeAhead;
}

/// The model of a file chooser: a folder's entries, the highlighted one, navigation, filters and type-to-select.
pub mod filepicker {
    pub use cupertino_ui_controls::filepicker::{
        Entry, Key, Listing, Mode, Outcome, Picker, Request,
    };

    /// The folders of a chooser's sidebar: the home folder and the user directories of `user-dirs.dirs`.
    pub mod places {
        pub use cupertino_ui_controls::filepicker::places::{
            parse_user_dirs, session_places, Place,
        };
    }
}
