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
