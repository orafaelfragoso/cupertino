use serde::{Deserialize, Deserializer};
use std::fmt;

/// A semantic icon name such as `folder.documents` or `symbol.chevron.left`: dot-separated segments of
/// lowercase ASCII letters, digits and `-`. The valid set grows with the contract, so it is checked by
/// shape, not by list; whether the directory has the icon is answered by the lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct IconName(String);

impl IconName {
    pub fn new(name: &str) -> Result<Self, String> {
        let segment_ok = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        };
        if name.split('.').all(segment_ok) {
            Ok(Self(name.to_string()))
        } else {
            Err(format!("invalid semantic icon name {name:?}"))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for IconName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for IconName {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Self::new(&s).map_err(serde::de::Error::custom)
    }
}

/// The raster icon names the shell looks up (the logical names of the asset contract).
pub const RASTER_NAMES: [&str; 26] = [
    "alias.badge",
    "application.file-manager",
    "application.generic",
    "application.settings",
    "computer",
    "document.archive",
    "document.audio",
    "document.executable",
    "document.generic",
    "document.image",
    "document.pdf",
    "document.text",
    "document.video",
    "folder",
    "folder.applications",
    "folder.desktop",
    "folder.documents",
    "folder.downloads",
    "folder.generic-open",
    "folder.home",
    "folder.movies",
    "folder.music",
    "folder.pictures",
    "locked.badge",
    "trash.empty",
    "trash.full",
];

/// The template symbol names the shell looks up.
pub const SYMBOL_NAMES: [&str; 100] = [
    "symbol.backward",
    "symbol.bell",
    "symbol.dock.hide",
    "symbol.forward",
    "symbol.pause",
    "symbol.play",
    "symbol.accessibility",
    "symbol.appearance",
    "symbol.applications",
    "symbol.battery",
    "symbol.bluetooth",
    "symbol.checkmark",
    "symbol.chevron.down",
    "symbol.chevron.left",
    "symbol.chevron.right",
    "symbol.chevron.up",
    "symbol.chevron.up.down",
    "symbol.cloud",
    "symbol.columns",
    "symbol.computer",
    "symbol.control-panel",
    "symbol.desktop",
    "symbol.document",
    "symbol.download",
    "symbol.eject",
    "symbol.ellipsis.circle",
    "symbol.externaldrive",
    "symbol.file-sharing",
    "symbol.film",
    "symbol.flash-drive",
    "symbol.folder",
    "symbol.gallery",
    "symbol.grid",
    "symbol.house",
    "symbol.keyboard",
    "symbol.keyboard-brightness",
    "symbol.keyboard-brightness.off",
    "symbol.list",
    "symbol.lock",
    "symbol.lock-screen",
    "symbol.magnifyingglass",
    "symbol.memory-card",
    "symbol.mic",
    "symbol.mic.slash",
    "symbol.moon",
    "symbol.moon.fill",
    "symbol.mouse",
    "symbol.music",
    "symbol.network",
    "symbol.photo",
    "symbol.privacy",
    "symbol.recents",
    "symbol.screen-mirroring",
    "symbol.search",
    "symbol.share",
    "symbol.sidebar",
    "symbol.speaker",
    "symbol.speaker.high",
    "symbol.speaker.low",
    "symbol.speaker.medium",
    "symbol.speaker.slash",
    "symbol.stage",
    "symbol.sun",
    "symbol.tag",
    "symbol.trackpad",
    "symbol.trash",
    "symbol.wifi",
    "symbol.xmark.circle",
    // filled editions, for sidebar rows
    "symbol.accessibility.fill",
    "symbol.appearance.fill",
    "symbol.applications.fill",
    "symbol.battery.fill",
    "symbol.bell.fill",
    "symbol.bluetooth.fill",
    "symbol.computer.fill",
    "symbol.control-panel.fill",
    "symbol.desktop.fill",
    "symbol.document.fill",
    "symbol.download.fill",
    "symbol.externaldrive.fill",
    "symbol.film.fill",
    "symbol.flash-drive.fill",
    "symbol.grid.fill",
    "symbol.house.fill",
    "symbol.keyboard.fill",
    "symbol.lock-screen.fill",
    "symbol.memory-card.fill",
    "symbol.mouse.fill",
    "symbol.music.fill",
    "symbol.network.fill",
    "symbol.photo.fill",
    "symbol.privacy.fill",
    "symbol.recents.fill",
    "symbol.screen-mirroring.fill",
    "symbol.speaker.high.fill",
    "symbol.sun.fill",
    "symbol.tag.fill",
    "symbol.trackpad.fill",
    "symbol.trash.fill",
    "symbol.wifi.fill",
];

/// The cursor names the server asks for (the CSS cursor names of the cursor-shape protocol).
pub const CURSOR_NAMES: [&str; 34] = [
    "default",
    "context-menu",
    "help",
    "pointer",
    "progress",
    "wait",
    "cell",
    "crosshair",
    "text",
    "vertical-text",
    "alias",
    "copy",
    "move",
    "no-drop",
    "not-allowed",
    "grab",
    "grabbing",
    "e-resize",
    "n-resize",
    "ne-resize",
    "nw-resize",
    "s-resize",
    "se-resize",
    "sw-resize",
    "w-resize",
    "ew-resize",
    "ns-resize",
    "nesw-resize",
    "nwse-resize",
    "col-resize",
    "row-resize",
    "all-scroll",
    "zoom-in",
    "zoom-out",
];
