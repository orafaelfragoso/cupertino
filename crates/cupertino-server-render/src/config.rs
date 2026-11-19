//! Rendering configuration types shared between the compositor and `cupertino-server-render`.

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

/// The wallpaper images of the user (`wallpaper`, `wallpaper_dark` in the JSON file); a pack's wallpaper and then the
/// procedural one stand in for what is not set or cannot be read.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WallpaperSettings {
    pub light: Option<std::path::PathBuf>,
    /// The dark appearance's; `light` serves both when this is not set.
    pub dark: Option<std::path::PathBuf>,
}

impl WallpaperSettings {
    /// The user's image for an appearance, if one is set.
    pub fn for_appearance(&self, dark: bool) -> Option<&std::path::Path> {
        let chosen = if dark {
            self.dark.as_ref().or(self.light.as_ref())
        } else {
            self.light.as_ref()
        };
        chosen.map(std::path::PathBuf::as_path)
    }
}
