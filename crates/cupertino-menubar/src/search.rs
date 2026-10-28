//! The Help menu's search: which items of the bar match the text of its field, and the Help menu that shows them.
//!
//! The search runs over the enabled leaf items of the system menu, the App menu and the application's menus as the bar
//! shows them (merged), the Help menu's own items included; only the bar's search row and result rows are left out. An
//! item matches when its label holds the text, case ignored, or the title of a submenu on its way does ("Export" finds
//! "File ▸ Export ▸ PDF"). Labels that start with the text come first, then labels that hold it, then the items found
//! by their submenu alone, each group in menu order, and at most [`MAX_SEARCH_RESULTS`] show. Each result keeps the
//! slot of the menu it is in, its id and its check or radio state, so activating it is activating the item itself,
//! routed by its origin.

use crate::fallback::{self, HELP_SEARCH, MAX_SEARCH_RESULTS, NO_SEARCH_RESULTS};
use crate::item::{ItemId, MenuItem, Origin, Shortcut, Toggle};
use crate::layout::{BarContent, SlotKey};

/// What a result's path calls the system menu, whose title is a mark.
const SYSTEM_MENU_TITLE: &str = "System";
/// Separates the menu titles of a result's path.
const PATH_SEPARATOR: &str = " ▸ ";

/// An item that matches the search.
#[derive(Clone, Debug, PartialEq)]
pub struct Match {
    /// The menu of the bar the item is in.
    pub slot: SlotKey,
    pub item: ItemId,
    pub label: String,
    /// The titles of the menus that lead to the item ("File ▸ Export").
    pub path: String,
    pub shortcut: Option<Shortcut>,
    /// The item's check or radio state.
    pub toggle: Toggle,
}

/// How well an item matches, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum Rank {
    LabelStarts,
    LabelHolds,
    SubmenuHolds,
}

/// The items of `content` whose label, or the title of a submenu on whose way they are, holds `text`, case ignored;
/// none for a blank text.
pub fn search(content: &BarContent, text: &str) -> Vec<Match> {
    let needle = text.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits = Vec::new();
    let mut collect = |slot: SlotKey, title: &str, items: &[MenuItem]| {
        let walk = Walk {
            slot: &slot,
            needle: &needle,
        };
        walk.items(items, title, false, &mut hits)
    };
    collect(SlotKey::System, SYSTEM_MENU_TITLE, &content.system_menu);
    collect(SlotKey::AppMenu, &content.app_name, &content.app_menu);
    for menu in &content.menus {
        collect(SlotKey::Menu(menu.id), &menu.label, &menu.children);
    }
    // a stable sort keeps the menu order within each group
    hits.sort_by_key(|(rank, _)| *rank);
    hits.into_iter()
        .map(|(_, hit)| hit)
        .take(MAX_SEARCH_RESULTS)
        .collect()
}

/// The search through the menu of one slot.
struct Walk<'a> {
    slot: &'a SlotKey,
    needle: &'a str,
}

impl Walk<'_> {
    /// Collect the matches among `items`, whose menu titles lead along `path`; `in_found_submenu`: a submenu on the
    /// way matches the text.
    fn items(
        &self,
        items: &[MenuItem],
        path: &str,
        in_found_submenu: bool,
        hits: &mut Vec<(Rank, Match)>,
    ) {
        for item in items
            .iter()
            .filter(|i| i.visible && i.enabled && !i.separator && !is_search_row(i))
        {
            let label = item.label.to_lowercase();
            if item.has_submenu {
                let path = format!("{path}{PATH_SEPARATOR}{}", item.label);
                let found = in_found_submenu || label.contains(self.needle);
                self.items(&item.children, &path, found, hits);
                continue;
            }
            let rank = if label.starts_with(self.needle) {
                Rank::LabelStarts
            } else if label.contains(self.needle) {
                Rank::LabelHolds
            } else if in_found_submenu {
                Rank::SubmenuHolds
            } else {
                continue;
            };
            hits.push((
                rank,
                Match {
                    slot: self.slot.clone(),
                    item: item.id,
                    label: item.label.clone(),
                    path: path.to_owned(),
                    shortcut: item.shortcut.clone(),
                    toggle: item.toggle,
                },
            ));
        }
    }
}

/// The bar's own rows of the Help search: the field, the "No Results" row and the result rows.
fn is_search_row(item: &MenuItem) -> bool {
    item.origin == Origin::Bar
        && (item.id == HELP_SEARCH
            || item.id == NO_SEARCH_RESULTS
            || fallback::search_result_row(item.id).is_some())
}

/// The Help menu `help` with the results of a search for `text` below its field: a row per hit, or a disabled "No
/// Results" row when nothing matched. A blank text leaves the menu as it is.
pub fn help_menu(help: &MenuItem, text: &str, hits: &[Match]) -> MenuItem {
    let mut help = help.clone();
    let Some(field) = help.children.iter().position(|i| i.id == HELP_SEARCH) else {
        return help;
    };
    if text.trim().is_empty() {
        return help;
    }
    let rows: Vec<MenuItem> = if hits.is_empty() {
        vec![MenuItem::new(NO_SEARCH_RESULTS.0, "No Results").disabled()]
    } else {
        hits.iter()
            .enumerate()
            .map(|(n, hit)| MenuItem {
                shortcut: hit.shortcut.clone(),
                toggle: hit.toggle,
                detail: Some(hit.path.clone()),
                ..MenuItem::new(fallback::search_result(n).0, &hit.label)
            })
            .collect()
    };
    help.children.splice(field + 1..field + 1, rows);
    help
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fallback::HELP_MENU;

    fn content() -> BarContent {
        BarContent {
            app_name: "Editor".into(),
            system_menu: vec![
                MenuItem::new(-4000, "About This Computer"),
                MenuItem::separator(-4050),
                MenuItem::new(-4001, "Settings…"),
            ],
            app_menu: vec![
                MenuItem::new(-1000, "About Editor"),
                MenuItem::new(-1005, "Settings…").disabled(),
                MenuItem::new(-1004, "Quit Editor"),
            ],
            menus: vec![
                MenuItem::submenu(
                    1,
                    "File",
                    vec![
                        MenuItem::new(2, "Save"),
                        MenuItem::new(3, "Save As…").disabled(),
                        MenuItem::separator(4),
                        MenuItem::submenu(
                            5,
                            "Export",
                            vec![MenuItem::new(6, "PDF"), MenuItem::new(7, "Image")],
                        ),
                        MenuItem::submenu(8, "Recent", vec![MenuItem::new(9, "notes.txt")])
                            .disabled(),
                    ],
                )
                .exported(),
                MenuItem::submenu(
                    10,
                    "View",
                    vec![
                        MenuItem::new(11, "Show Settings Panel"),
                        MenuItem {
                            visible: false,
                            ..MenuItem::new(12, "Hidden Settings")
                        },
                    ],
                ),
                MenuItem::submenu(
                    HELP_MENU.0,
                    "Help",
                    vec![
                        MenuItem::new(HELP_SEARCH.0, "Search").disabled(),
                        MenuItem::new(61, "Editor Settings Guide"),
                    ],
                ),
            ],
            ..BarContent::default()
        }
    }

    fn found(text: &str) -> Vec<(SlotKey, i32)> {
        search(&content(), text)
            .into_iter()
            .map(|h| (h.slot, h.item.0))
            .collect()
    }

    #[test]
    fn matches_come_from_the_system_menu_the_app_menu_and_every_menu_of_the_app_help_included() {
        assert_eq!(
            found("settings"),
            [
                (SlotKey::System, -4001),
                (SlotKey::Menu(ItemId(10)), 11),
                (SlotKey::Menu(HELP_MENU), 61)
            ]
        );
        assert_eq!(
            found("about"),
            [(SlotKey::System, -4000), (SlotKey::AppMenu, -1000)]
        );
    }

    #[test]
    fn matching_ignores_case_and_finds_text_inside_a_label() {
        assert_eq!(found("PdF"), [(SlotKey::Menu(ItemId(1)), 6)]);
        assert_eq!(found("mag"), [(SlotKey::Menu(ItemId(1)), 7)]);
        assert_eq!(found("  quit "), [(SlotKey::AppMenu, -1004)]);
        assert!(found("").is_empty() && found("   ").is_empty());
        assert!(found("nothing like it").is_empty());
    }

    #[test]
    fn labels_that_start_with_the_text_come_first_then_menu_order() {
        let mut c = content();
        c.menus[0]
            .children
            .push(MenuItem::new(20, "Autosave").exported());
        c.menus[0]
            .children
            .push(MenuItem::new(21, "Save All").exported());
        let ids: Vec<i32> = search(&c, "save").iter().map(|h| h.item.0).collect();
        assert_eq!(ids, [2, 21, 20]);
    }

    #[test]
    fn separators_submenus_hidden_and_disabled_items_are_never_results() {
        // "Save As…" is disabled, "Export" and "Recent" are submenus (the second disabled, so its rows are out of reach),
        // "Hidden Settings" is hidden
        assert_eq!(found("save"), [(SlotKey::Menu(ItemId(1)), 2)]);
        assert!(!found("export").contains(&(SlotKey::Menu(ItemId(1)), 5)));
        assert!(found("notes").is_empty() && found("recent").is_empty());
        assert!(found("hidden").is_empty());
        assert!(!found("settings").contains(&(SlotKey::AppMenu, -1005)));
    }

    #[test]
    fn a_submenu_title_finds_its_items_after_the_labels_that_hold_the_text() {
        let file = SlotKey::Menu(ItemId(1));
        assert_eq!(found("export"), [(file.clone(), 6), (file.clone(), 7)]);
        let mut c = content();
        c.menus[1]
            .children
            .push(MenuItem::new(13, "Export Selection"));
        c.menus[1].children.push(MenuItem::new(14, "Quick Export"));
        let hits: Vec<(SlotKey, i32)> = search(&c, "export")
            .into_iter()
            .map(|h| (h.slot, h.item.0))
            .collect();
        let view = SlotKey::Menu(ItemId(10));
        assert_eq!(
            hits,
            [(view.clone(), 13), (view, 14), (file.clone(), 6), (file, 7)],
            "label starts, label holds, then the submenu's items"
        );
        assert!(
            found("file").is_empty(),
            "a menu title of the bar is no submenu on the way"
        );
    }

    #[test]
    fn a_result_keeps_the_check_or_radio_state_of_its_item() {
        let mut c = content();
        c.menus[1].children[0].toggle = Toggle::Check(true);
        c.menus[0].children[3].children[1].toggle = Toggle::Radio(true);
        let hits = search(&c, "settings panel");
        let help = help_menu(&c.menus[2], "settings panel", &hits);
        assert_eq!(help.children[1].toggle, Toggle::Check(true));
        let hits = search(&c, "image");
        let help = help_menu(&c.menus[2], "image", &hits);
        assert_eq!(help.children[1].toggle, Toggle::Radio(true));
        let hits = search(&c, "pdf");
        assert_eq!(
            help_menu(&c.menus[2], "pdf", &hits).children[1].toggle,
            Toggle::None
        );
    }

    #[test]
    fn the_bars_search_rows_are_never_results_but_the_help_menus_items_are() {
        let mut c = content();
        // the field, the "No Results" row and the result rows, even enabled and labelled like an item
        c.menus[2].children.extend([
            MenuItem::new(NO_SEARCH_RESULTS.0, "Guide"),
            MenuItem::new(fallback::search_result(0).0, "Guide"),
        ]);
        c.menus[2].children[0].enabled = true;
        c.menus[2].children[0].label = "Guide".into();
        // an application's own Help item, and an item of the bar's Help menu
        c.menus[2]
            .children
            .push(MenuItem::new(62, "User Guide").exported());
        c.menus[2].children.push(MenuItem::new(-1200, "Guide Tour"));
        let help = SlotKey::Menu(HELP_MENU);
        assert_eq!(
            search(&c, "guide")
                .into_iter()
                .map(|h| (h.slot, h.item.0))
                .collect::<Vec<_>>(),
            [(help.clone(), -1200), (help.clone(), 61), (help, 62)]
        );
    }

    #[test]
    fn at_most_twelve_results_show() {
        let mut c = content();
        c.menus[1].children = (100..120)
            .map(|id| MenuItem::new(id, &format!("Item {id}")))
            .collect();
        let hits = search(&c, "item");
        assert_eq!(hits.len(), MAX_SEARCH_RESULTS);
        assert_eq!(hits.last().map(|h| h.item.0), Some(111));
    }

    #[test]
    fn a_result_carries_its_path_and_chord_and_keeps_its_items_origin() {
        let mut c = content();
        c.menus[0].children[3].children[0].shortcut = Shortcut::from_tokens(&["Control", "P"]);
        let hit = &search(&c, "pdf")[0];
        assert_eq!(hit.path, "File ▸ Export");
        assert_eq!(hit.label, "PDF");
        assert_eq!(hit.shortcut, Shortcut::from_tokens(&["Control", "P"]));
        let menu = c.menus.iter().find(|m| SlotKey::Menu(m.id) == hit.slot);
        assert_eq!(
            menu.and_then(|m| m.find(hit.item)).map(|i| i.origin),
            Some(Origin::Exporter)
        );
        assert_eq!(search(&c, "quit")[0].path, "Editor");
        assert_eq!(search(&c, "this computer")[0].path, "System");
    }

    #[test]
    fn the_help_menu_shows_the_results_below_its_field_or_no_results() {
        let c = content();
        let help = &c.menus[2];
        let ids = |m: &MenuItem| m.children.iter().map(|i| i.id.0).collect::<Vec<_>>();
        assert_eq!(ids(&help_menu(help, "", &[])), [-1030, 61]);

        let hits = search(&c, "e");
        let shown = help_menu(help, "e", &hits);
        assert_eq!(shown.children.len(), 2 + hits.len());
        assert_eq!(&ids(&shown)[..3], [-1030, -1050, -1051]);
        assert_eq!(ids(&shown).last(), Some(&61));
        let first = &shown.children[1];
        assert_eq!(
            (first.label.as_str(), first.detail.as_deref()),
            (hits[0].label.as_str(), Some(hits[0].path.as_str()))
        );
        assert!(first.is_selectable());

        let none = help_menu(help, "zzz", &[]);
        assert_eq!(ids(&none), [-1030, -1031, 61]);
        assert_eq!(none.children[1].label, "No Results");
        assert!(!none.children[1].is_selectable());
    }
}
