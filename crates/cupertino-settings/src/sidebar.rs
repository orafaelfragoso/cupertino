//! The sidebar: its groups and rows in the design's order (§4.2).
//!
//! Order: the design's groups that have a row of the minimum set, each in the design's order, except the
//! sortable group, whose rows sort ascending by the localized standard compare of their titles
//! (direction).

use std::cmp::Ordering;

use crate::pane::PaneId;

/// The sidebar groups of the minimum set, top to bottom. The second is the design's sortable group.
pub const GROUPS: [&[PaneId]; 6] = [
    &[PaneId::Account],
    &[
        PaneId::Wifi,
        PaneId::Bluetooth,
        PaneId::Network,
        PaneId::Vpn,
        PaneId::Battery,
    ],
    &[
        PaneId::Accessibility,
        PaneId::Appearance,
        PaneId::ControlPanel,
        PaneId::DesktopDock,
        PaneId::Displays,
        PaneId::General,
        PaneId::MenuBar,
        PaneId::Wallpaper,
    ],
    &[PaneId::Notifications, PaneId::Sound, PaneId::Focus],
    &[
        PaneId::LockScreen,
        PaneId::Privacy,
        PaneId::Search,
        PaneId::Users,
    ],
    &[PaneId::Keyboard, PaneId::Mouse, PaneId::Trackpad],
];

/// Index into [`GROUPS`] of the group whose rows sort by title.
pub const SORTED_GROUP: usize = 2;

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

/// The sidebar groups of the minimum set in display order, without the `hidden` panes (those no service backs); a group
/// left empty goes.
pub fn groups(hidden: &[PaneId]) -> Vec<Vec<PaneId>> {
    ordered(&GROUPS, SORTED_GROUP, |p| p.pane().title.to_string())
        .into_iter()
        .map(|g| {
            g.into_iter()
                .filter(|p| !hidden.contains(p))
                .collect::<Vec<_>>()
        })
        .filter(|g| !g.is_empty())
        .collect()
}

/// Every row of the sidebar, top to bottom.
pub fn rows(hidden: &[PaneId]) -> Vec<PaneId> {
    groups(hidden).into_iter().flatten().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_minimum_set_shows_in_the_design_order() {
        let titles: Vec<Vec<&str>> = groups(&[])
            .iter()
            .map(|g| g.iter().map(|p| p.pane().title).collect())
            .collect();
        assert_eq!(
            titles,
            [
                vec!["Account"],
                vec!["Wi-Fi", "Bluetooth", "Network", "VPN", "Battery"],
                vec![
                    "Accessibility",
                    "Appearance",
                    "Control Panel",
                    "Desktop & Dock",
                    "Displays",
                    "General",
                    "Menu Bar",
                    "Wallpaper"
                ],
                vec!["Notifications", "Sound", "Focus"],
                vec![
                    "Lock Screen",
                    "Privacy & Security",
                    "Search",
                    "Users & Groups"
                ],
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
}
