//! The sidebar: its groups and rows in the design's order and their geometry
//! (§4.2, §4.3).
//!
//! Order: the design's groups that have a row of the minimum set, each in the design's order, except the
//! sortable group, whose rows sort ascending by the localized standard compare of their titles
//! (direction). Row metrics by [`RowSize`]: icons 16 / 20 / 24, rows 24 / 28 / 32.

use std::cmp::Ordering;

use crate::pane::PaneId;

/// The sidebar row size (`SidebarRowSize`: small 0, medium 1, large 2, default medium).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RowSize {
    Small,
    #[default]
    Medium,
    Large,
}

impl RowSize {
    pub const ALL: [RowSize; 3] = [RowSize::Small, RowSize::Medium, RowSize::Large];

    /// Side of a row's icon: the platform sidebar's icon field 16 / 20 / 24. a screenshot
    /// shows 20 at the medium size, so the enclosure fills the field (closes O1a). The settings icon view's own
    /// sizes 16 / 24 / 32 are not what the sidebar draws: its 24 is the large field.
    pub fn icon(self) -> f64 {
        match self {
            RowSize::Small => 16.0,
            RowSize::Medium => 20.0,
            RowSize::Large => 24.0,
        }
    }

    /// Side of a search result's icon: the settings icon view's size, which
    /// with its padding gives the result rows of (22 / 32 / 42).
    pub fn result_icon(self) -> f64 {
        match self {
            RowSize::Small => 16.0,
            RowSize::Medium => 24.0,
            RowSize::Large => 32.0,
        }
    }

    /// Height of a sidebar row: the platform sidebar's; never below the minimum row height
    /// of 24.
    pub fn row_height(self) -> f64 {
        let h: f64 = match self {
            RowSize::Small => 24.0,
            RowSize::Medium => 28.0,
            RowSize::Large => 32.0,
        };
        h.max(MIN_ROW_HEIGHT)
    }

    /// Title size: 11 / 13 / 15 pt (the platform sidebar's font table; for the file manager, its role
    /// here ).
    pub fn font_size(self) -> f64 {
        match self {
            RowSize::Small => 11.0,
            RowSize::Medium => 13.0,
            RowSize::Large => 15.0,
        }
    }
}

/// The smallest row.
pub const MIN_ROW_HEIGHT: f64 = 24.0;

/// Geometry of a row, all approximate unless stated (§4.3).
pub mod row {
    /// The row background's inset from the sidebar's edges and its corner radius (of the platform
    /// source-list style, their use here ).
    pub const BACKGROUND_INSET: f64 = 10.0;
    pub const SELECTION_RADIUS: f64 = 5.0;
    /// The icon's distance from the row background's leading edge. approximate.
    pub const ICON_LEADING: f64 = 6.0;
    /// Between the icon and the title. approximate (5.5 pt measured on a screenshot).
    pub const ICON_TITLE_GAP: f64 = 5.5;
    /// The enclosure's corner radius as a fraction of its side, and the glyph's share of the side. approximate.
    pub const ENCLOSURE_RADIUS: f64 = 0.225;
    pub const ENCLOSURE_GLYPH: f64 = 0.62;
    /// Between two groups. approximate (the platform's 13 pt, the settings sidebar has no sections).
    pub const GROUP_GAP: f64 = 13.0;
}

/// The sidebar groups of the minimum set, top to bottom. The second is the design's sortable group.
pub const GROUPS: [&[PaneId]; 5] = [
    &[
        PaneId::Wifi,
        PaneId::Bluetooth,
        PaneId::Network,
        PaneId::Battery,
    ],
    &[
        PaneId::Accessibility,
        PaneId::Appearance,
        PaneId::ControlPanel,
        PaneId::DesktopDock,
        PaneId::Displays,
        PaneId::Wallpaper,
    ],
    &[PaneId::Sound, PaneId::Focus],
    &[PaneId::LockScreen, PaneId::Privacy],
    &[PaneId::Keyboard, PaneId::Mouse, PaneId::Trackpad],
];

/// Index into [`GROUPS`] of the group whose rows sort by title.
pub const SORTED_GROUP: usize = 1;

/// The localized standard compare for titles: case- and width-insensitive, digit runs by value, then the raw text so
/// that two titles never compare equal (the design's collation flags, this subset without locale tables
/// is approximate).
pub fn standard_compare(a: &str, b: &str) -> Ordering {
    let key = |s: &str| -> Vec<(u8, u64, char)> {
        let mut out = Vec::new();
        let mut chars = s.chars().map(fold_width).peekable();
        while let Some(c) = chars.next() {
            if let Some(mut n) = c.to_digit(10).map(u64::from) {
                while let Some(d) = chars.peek().and_then(|d| d.to_digit(10)) {
                    n = n.saturating_mul(10).saturating_add(u64::from(d));
                    chars.next();
                }
                out.push((1, n, '0'));
            } else if c.is_alphabetic() {
                out.extend(c.to_lowercase().map(|l| (2, 0, l)));
            } else {
                out.push((0, u64::from(u32::from(c)), c));
            }
        }
        out
    };
    key(a).cmp(&key(b)).then_with(|| a.cmp(b))
}

/// Fullwidth forms as their ASCII letters (the width-insensitive part of the collation).
pub fn fold_width(c: char) -> char {
    match c {
        '\u{3000}' => ' ',
        '\u{FF01}'..='\u{FF5E}' => char::from_u32(u32::from(c) - 0xFEE0)
            .expect("the fullwidth ASCII block maps into ASCII"),
        _ => c,
    }
}

/// The rows of `groups` in display order: each group in its order, the sortable one (`sorted`) by title.
pub fn ordered<T: Copy>(
    groups: &[&[T]],
    sorted: usize,
    title: impl Fn(T) -> String,
) -> Vec<Vec<T>> {
    groups
        .iter()
        .enumerate()
        .map(|(i, g)| {
            let mut rows = g.to_vec();
            if i == sorted {
                rows.sort_by(|a, b| standard_compare(&title(*a), &title(*b)));
            }
            rows
        })
        .collect()
}

/// The sidebar groups of the minimum set in display order.
pub fn groups() -> Vec<Vec<PaneId>> {
    ordered(&GROUPS, SORTED_GROUP, |p| p.pane().title.to_string())
}

/// Every row of the sidebar, top to bottom.
pub fn rows() -> Vec<PaneId> {
    groups().into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimum_set_shows_in_the_reference_order() {
        let titles: Vec<Vec<&str>> = groups()
            .iter()
            .map(|g| g.iter().map(|p| p.pane().title).collect())
            .collect();
        assert_eq!(
            titles,
            [
                vec!["Wi-Fi", "Bluetooth", "Network", "Battery"],
                vec![
                    "Accessibility",
                    "Appearance",
                    "Control Panel",
                    "Desktop & Dock",
                    "Displays",
                    "Wallpaper"
                ],
                vec!["Sound", "Focus"],
                vec!["Lock Screen", "Privacy & Security"],
                vec!["Keyboard", "Mouse", "Trackpad"],
            ]
        );
    }

    #[test]
    fn only_the_sortable_group_sorts_and_control_panel_falls_between_appearance_and_desktop() {
        // a mock full set: the design's sortable group in the design's order, and an unsorted group that must keep its order
        let full: [&[&str]; 2] = [
            &[
                "Wallpaper",
                "Displays",
                "Accessibility",
                "Desktop & Dock",
                "Control Panel",
                "Appearance",
                "General",
                "Screen Saver",
            ],
            &["Zeta", "Alpha"],
        ];
        let order = ordered(&full, 0, |t: &str| t.to_string());
        assert_eq!(
            order[0],
            [
                "Accessibility",
                "Appearance",
                "Control Panel",
                "Desktop & Dock",
                "Displays",
                "General",
                "Screen Saver",
                "Wallpaper"
            ]
        );
        assert_eq!(
            order[1],
            ["Zeta", "Alpha"],
            "groups other than the sortable one keep their order"
        );
    }

    #[test]
    fn the_standard_compare_ignores_case_and_width_and_reads_numbers_by_value() {
        assert_eq!(
            standard_compare("display 10", "Display 9"),
            Ordering::Greater
        );
        assert_eq!(
            standard_compare("ａｐｐｅａｒａｎｃｅ", "Appearance"),
            Ordering::Greater,
            "equal but for width and case, then the raw text decides"
        );
        assert_eq!(
            standard_compare("ａｐｐｅａｒａｎｃｅ", "Arrange"),
            Ordering::Less
        );
        assert!(
            !standard_compare("Éclair", "Eclair").is_eq(),
            "diacritics count"
        );
    }

    #[test]
    fn rows_and_icons_follow_the_row_size() {
        let sizes: Vec<(f64, f64, f64)> = RowSize::ALL
            .iter()
            .map(|r| (r.icon(), r.row_height(), r.font_size()))
            .collect();
        assert_eq!(
            sizes,
            [(16.0, 24.0, 11.0), (20.0, 28.0, 13.0), (24.0, 32.0, 15.0)]
        );
        assert_eq!(RowSize::default(), RowSize::Medium);
        assert!(RowSize::ALL
            .iter()
            .all(|r| r.row_height() >= MIN_ROW_HEIGHT));
    }
}
