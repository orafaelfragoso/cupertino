//! Shell design values from the active theme data (`docs/architecture/shell-host.md#the-session-outputs-and-design-values`).

use cupertino_kit::text::{FontWeight, TextStyle};

/// Default menu bar height in logical points.
pub const MENU_BAR_HEIGHT_PT: f64 = 24.0;
/// Default menu bar selection pill corner radius in logical points.
pub const MENU_BAR_SELECTION_RADIUS_PT: f64 = 4.0;
/// Default menu bar item font size in logical points.
pub const MENU_BAR_FONT_SIZE_PT: f64 = 13.0;
/// Cap height over the em used to centre menu bar titles vertically.
pub const MENU_BAR_CAP_HEIGHT_RATIO: f64 = 0.71;

/// The menu bar's values from the theme data.
#[derive(Clone, Debug, PartialEq)]
pub struct MenuBar {
    pub height: f64,
    pub selection_radius: f64,
    /// Style of a menu title.
    pub item: TextStyle,
    /// Style of the active application's name.
    pub app_name: TextStyle,
    /// Cap height over the em: the bar centres its text on it.
    pub cap_height: f64,
}

/// Current theme values for the menu bar.
pub fn menu_bar() -> MenuBar {
    let mut item = TextStyle::new(MENU_BAR_FONT_SIZE_PT as f32);
    item.weight = FontWeight::SEMIBOLD;
    let mut app_name = TextStyle::new(MENU_BAR_FONT_SIZE_PT as f32);
    app_name.weight = FontWeight::BOLD;
    MenuBar {
        height: MENU_BAR_HEIGHT_PT,
        selection_radius: MENU_BAR_SELECTION_RADIUS_PT,
        item,
        app_name,
        cap_height: MENU_BAR_CAP_HEIGHT_RATIO,
    }
}
