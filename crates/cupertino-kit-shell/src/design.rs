//! Shell design values from the active theme data (`docs/architecture/shell-host.md#the-session-outputs-and-design-values`).

use std::sync::OnceLock;

use cupertino_kit::text::{FontWeight, TextStyle};
use cupertino_theme::{ShapeTokens, Typography};

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

/// Current theme values for the menu bar: its height and the corner radius of a pressed title's highlight from the
/// theme's shapes, the titles' and the application name's type from its typography, and the cap height of its font.
/// The theme data is read once per process; without it the theme's built-in values stand.
pub fn menu_bar() -> MenuBar {
    static MENU_BAR: OnceLock<MenuBar> = OnceLock::new();
    MENU_BAR
        .get_or_init(|| match cupertino_assets::theme::load() {
            Ok(theme) => from_theme(&theme.shapes, theme.tokens.typography()),
            Err(e) => {
                tracing::warn!(error = %e, "no theme data; the menu bar takes the built-in values");
                from_theme(&ShapeTokens::default(), &Typography::default())
            }
        })
        .clone()
}

fn from_theme(shapes: &ShapeTokens, typography: &Typography) -> MenuBar {
    let style = |theme: cupertino_theme::typography::TextStyle| {
        let mut style = TextStyle::new(theme.size as f32);
        style.weight = FontWeight(theme.weight);
        style
    };
    MenuBar {
        height: shapes.menu_bar_height,
        selection_radius: shapes.menu_bar_selection_corner_radius,
        item: style(typography.menu_bar_item()),
        app_name: style(typography.menu_bar_app_name()),
        cap_height: typography.metrics.cap_height / typography.metrics.units_per_em,
    }
}
