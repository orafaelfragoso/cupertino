//! The count an application shows on its tile: a red badge over the top trailing corner of its icon. An application
//! sets it with the freedesktop launcher entry signal (`com.canonical.Unity.LauncherEntry.Update`, the application's
//! desktop file named as `application://ID.desktop`, with the properties `count` and `count-visible`); the dock keeps
//! the last count each application asked to show.

use std::collections::HashMap;

/// The bus interface and member of the launcher entry signal.
pub const LAUNCHER_ENTRY_INTERFACE: &str = "com.canonical.Unity.LauncherEntry";
pub const LAUNCHER_ENTRY_UPDATE: &str = "Update";

/// The badge's side (its height, and its width with one digit) at a 64 point tile; it scales with the tile.
pub const DIAMETER: f64 = 23.0;
/// How far the badge's middle stands inside the icon's top and trailing edges at a 64 point tile: a quarter of the badge
/// overhangs each.
pub const INSET: f64 = 5.75;
/// The count's size and weight at a 64 point tile, and the space either side of a count of more than one digit.
pub const FONT_PT: f64 = 13.0;
pub const FONT_WEIGHT: u16 = 500;
pub const PADDING: f64 = 6.5;
/// The badge's red, a little lighter at the bottom than at the top, and the count's white.
pub const RED_TOP: [f32; 4] = [0.922, 0.188, 0.153, 1.0];
pub const RED_BOTTOM: [f32; 4] = [0.98, 0.239, 0.208, 1.0];
pub const TEXT: [f32; 4] = [1.0, 1.0, 1.0, 1.0];
/// The longest count drawn in full; a larger one reads as this followed by a plus.
pub const MOST: u64 = 9999;

/// What one launcher entry signal asks of an application's badge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BadgeUpdate {
    /// The application's desktop file ID, without `.desktop`.
    pub app: String,
    /// The new count, when the signal names one.
    pub count: Option<u64>,
    /// Whether to show it, when the signal says.
    pub visible: Option<bool>,
}

/// The desktop file ID an `application://ID.desktop` URI names; `None` for any other URI.
pub fn app_of_uri(uri: &str) -> Option<String> {
    let id = uri.strip_prefix("application://")?;
    let id = id.strip_suffix(".desktop").unwrap_or(id);
    (!id.is_empty() && !id.contains('/')).then(|| id.to_owned())
}

/// The badge update of a launcher entry signal: its URI and the `count` (a 64-bit integer; a negative one is no count)
/// and `count-visible` properties, as integers and booleans already read from the bus. `None` for a URI that names no
/// application.
pub fn update(uri: &str, count: Option<i64>, visible: Option<bool>) -> Option<BadgeUpdate> {
    Some(BadgeUpdate {
        app: app_of_uri(uri)?,
        count: count.map(|c| u64::try_from(c).unwrap_or(0)),
        visible,
    })
}

/// The counts of the applications, by desktop file ID.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Badges {
    entries: HashMap<String, Entry>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Entry {
    count: u64,
    visible: bool,
}

impl Badges {
    /// Apply an update: the count and the visibility each change only when the signal names them. Returns whether what
    /// shows changed.
    pub fn apply(&mut self, u: &BadgeUpdate) -> bool {
        let before = self.shown(&u.app);
        let entry = self.entries.entry(u.app.to_lowercase()).or_default();
        if let Some(count) = u.count {
            entry.count = count;
        }
        if let Some(visible) = u.visible {
            entry.visible = visible;
        }
        if *entry == Entry::default() {
            self.entries.remove(&u.app.to_lowercase());
        }
        self.shown(&u.app) != before
    }

    /// The count application `app` shows, if any: a visible count above zero.
    pub fn shown(&self, app: &str) -> Option<u64> {
        self.entries
            .get(&app.to_lowercase())
            .filter(|e| e.visible && e.count > 0)
            .map(|e| e.count)
    }

    /// The application ran its course: its badge goes.
    pub fn forget(&mut self, app: &str) -> bool {
        self.entries.remove(&app.to_lowercase()).is_some()
    }
}

/// The text of a count.
pub fn text(count: u64) -> String {
    if count > MOST {
        format!("{MOST}+")
    } else {
        count.to_string()
    }
}

/// The badge's rectangle `[x, y, width, height]` over an icon whose artwork occupies `artwork` (`[x, y, side, side]`),
/// for a tile of `tile` points and a count text `text_width` points wide at the badge's font: a circle with one digit,
/// a capsule as wide as the text and its padding with more, its middle [`INSET`] inside the icon's top trailing corner.
pub fn rect(artwork: [f64; 4], tile: f64, text_width: f64) -> [f64; 4] {
    let k = tile / 64.0;
    let height = DIAMETER * k;
    let width = height.max(text_width + 2.0 * PADDING * k);
    let (cx, cy) = (artwork[0] + artwork[2] - INSET * k, artwork[1] + INSET * k);
    [cx - width / 2.0, cy - height / 2.0, width, height]
}
