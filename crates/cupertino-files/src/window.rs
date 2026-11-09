//! What the browser window shows besides the folder's view, without a toolkit: its size limits and bar metrics, how it
//! shows the folder, the folders of its path bar and the text of its status bar. The window itself is [`crate::kit`].

use crate::format::available_size;
use crate::places::{recents_place, Place, PlaceKind};
use std::path::{Path, PathBuf};

/// Window chrome metrics.
pub mod metrics {
    use crate::metrics::{sidebar, window};

    /// Height of the toolbar strip, which is also the title bar.
    pub const TOOLBAR_HEIGHT: f64 = window::UNIFIED_TOOLBAR_HEIGHT;
    /// Separator lines between sidebar, toolbar and content.
    pub const SEPARATOR: f64 = 1.0;
    /// Smallest sidebar: its minimum thickness; overlay scrollers add nothing.
    pub const SIDEBAR_MIN: f64 = sidebar::MIN_WIDTH_BASE;
    /// Smallest browser area.
    pub const BROWSER_MIN: (f64, f64) = (316.0, 100.0);
    /// Smallest window content: the minimum sidebar, the divider, the minimum browser and the toolbar.
    pub const WINDOW_MIN: (f64, f64) = (
        SIDEBAR_MIN + SEPARATOR + BROWSER_MIN.0,
        TOOLBAR_HEIGHT + SEPARATOR + BROWSER_MIN.1,
    );
    /// Leading and trailing inset of the path bar and the status bar.
    pub const PATH_BAR_INSET: f64 = 8.0;
    /// Space between two items of the path bar.
    pub const PATH_ICON_GAP: f64 = 4.0;
}

/// How the folder is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ViewMode {
    Icons,
    List,
    /// The chain of folders side by side ([`crate::columns`]).
    Columns,
    /// A large preview of one item over a row of every item's picture.
    Gallery,
}

/// The folders the path bar shows for `dir`, outermost first, with their names. The chain starts at the file system
/// root under the computer place's name; a folder that is the Trash or Recents place starts it under the place's name
/// instead.
pub fn path_chain(dir: &Path, places: &[Place]) -> Vec<(PathBuf, String)> {
    let mut chain = Vec::new();
    if dir == crate::recents::location() {
        return vec![(dir.to_path_buf(), recents_place().name)];
    }
    for folder in dir.ancestors() {
        if let Some(place) = places
            .iter()
            .find(|p| p.path == folder && matches!(p.kind, PlaceKind::Trash | PlaceKind::Recents))
        {
            chain.push((folder.to_path_buf(), place.name.clone()));
            break;
        }
        let name = match folder.file_name() {
            Some(name) => name.to_string_lossy().into_owned(),
            None => places
                .iter()
                .find(|p| p.kind == PlaceKind::Computer && p.path == folder)
                .map_or_else(|| folder.display().to_string(), |p| p.name.clone()),
        };
        chain.push((folder.to_path_buf(), name));
    }
    chain.reverse();
    chain
}

/// The status bar's text: "1 item" or "N items", then the space left on the folder's file system when it has one,
/// "207 items, 227.27 GB available".
pub fn status_text(items: usize, available: Option<u64>) -> String {
    let count = if items == 1 {
        "1 item".to_string()
    } else {
        format!("{items} items")
    };
    match available {
        Some(bytes) => format!("{count}, {} available", available_size(bytes)),
        None => count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::places::PlaceGroup;

    fn places() -> Vec<Place> {
        let p = |name: &str, path: &str, group, kind| Place {
            name: name.into(),
            path: PathBuf::from(path),
            group,
            kind,
        };
        vec![
            p("Home", "/home/ada", PlaceGroup::Favorites, PlaceKind::Home),
            p(
                "Desktop",
                "/home/ada/Desktop",
                PlaceGroup::Favorites,
                PlaceKind::Desktop,
            ),
            p("Computer", "/", PlaceGroup::Locations, PlaceKind::Computer),
        ]
    }

    #[test]
    fn the_path_chain_starts_at_the_root_under_the_computer_name_or_at_the_trash_or_recents() {
        let mut ps = places();
        ps.push(Place {
            name: "Trash".into(),
            path: PathBuf::from("/home/ada/.local/share/Trash/files"),
            group: PlaceGroup::Locations,
            kind: PlaceKind::Trash,
        });
        let chain = |dir: &str| {
            path_chain(Path::new(dir), &ps)
                .into_iter()
                .map(|(p, n)| (p.display().to_string(), n))
                .collect::<Vec<_>>()
        };
        let s = |p: &str, n: &str| (p.to_string(), n.to_string());
        assert_eq!(
            chain("/home/ada/Desktop"),
            [
                s("/", "Computer"),
                s("/home", "home"),
                s("/home/ada", "ada"),
                s("/home/ada/Desktop", "Desktop")
            ]
        );
        assert_eq!(chain("/"), [s("/", "Computer")]);
        assert_eq!(
            chain("/home/ada/.local/share/Trash/files/old"),
            [
                s("/home/ada/.local/share/Trash/files", "Trash"),
                s("/home/ada/.local/share/Trash/files/old", "old")
            ]
        );
        assert_eq!(chain("recent:///"), [s("recent:///", "Recents")]);
    }

    #[test]
    fn the_status_text_counts_the_items_and_the_space_left() {
        assert_eq!(
            status_text(207, Some(227_270_000_000)),
            "207 items, 227.27 GB available"
        );
        assert_eq!(
            status_text(1, Some(250_000_000)),
            "1 item, 250 MB available"
        );
        assert_eq!(status_text(0, None), "0 items");
    }
}
