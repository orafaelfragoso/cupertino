//! A folder's stack: the files of a folder tile shown as a grid in a panel over the bar, the newest first, with a
//! button that opens the folder itself. This module holds what the stack shows and where: its entries, its layout and
//! its entry motion; the window is [`crate::view::stack`], and the model opens and closes it ([`crate::kit`]).

use std::path::{Path, PathBuf};

use cupertino_wm::geometry::{Point, Rect};

use crate::config::Orientation;

/// One file or folder a stack shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StackEntry {
    /// The file's name, as the stack labels it.
    pub name: String,
    pub path: PathBuf,
    /// Name of its icon: the icon of its kind (`document.pdf`, `document.image`, …, `folder`), as the asset packs
    /// name them.
    pub icon: String,
}

/// The most columns and rows the grid has; the stack shows that many of the newest entries, and the button opens the
/// folder for the rest.
pub const COLUMNS: usize = 5;
pub const ROWS: usize = 3;
pub const MAX_ENTRIES: usize = COLUMNS * ROWS;

/// What the stack's button is labelled.
pub const OPEN_FOLDER: &str = "Open in Files";
/// What an empty folder's stack says.
pub const EMPTY: &str = "No Items";

/// How long the stack takes to come in: it fades in, scaling up from [`ENTRY_SCALE`] towards the bar; under Reduce
/// Motion it only fades.
pub const ENTRY_S: f64 = 0.25;
pub const ENTRY_SCALE: f64 = 0.9;

/// The icon of the user's downloads folder, the stack the bar shows by default.
pub const DOWNLOADS_ICON: &str = "folder.downloads";
/// The icon of a folder of no special kind.
pub const FOLDER_ICON: &str = "folder";

/// Where the parts of a stack go in its window, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackLayout {
    pub width: f64,
    pub height: f64,
    pub columns: usize,
    pub rows: usize,
    /// The folder's name along the top.
    pub title: Rect,
    /// The grid of entries.
    pub grid: Rect,
    /// The button that opens the folder, along the bottom.
    pub button: Rect,
}

impl StackLayout {
    /// The space around the parts, the title's row and the button's.
    pub const PADDING: f64 = 16.0;
    pub const TITLE_HEIGHT: f64 = 28.0;
    pub const TITLE_FONT_PT: f64 = 13.0;
    pub const TITLE_WEIGHT: u16 = 600;
    pub const BUTTON_HEIGHT: f64 = 28.0;
    pub const BUTTON_WIDTH: f64 = 120.0;
    pub const BUTTON_FONT_PT: f64 = 13.0;
    /// One entry's cell, its icon's side and where the icon's top is in it, the label's centre line and size.
    pub const CELL: (f64, f64) = (96.0, 96.0);
    pub const ICON: f64 = 56.0;
    pub const ICON_TOP: f64 = 8.0;
    pub const LABEL_CY: f64 = 80.0;
    pub const LABEL_FONT_PT: f64 = 11.0;
    pub const LABEL_INSET: f64 = 4.0;
    /// The panel's corner radius; the highlight under a pressed entry's corner radius.
    pub const RADIUS: f64 = 20.0;
    pub const HIGHLIGHT_RADIUS: f64 = 8.0;
    /// The columns an empty folder's stack is as wide as.
    const EMPTY_COLUMNS: usize = 3;

    /// The stack of `count` entries: as many columns as entries up to [`COLUMNS`], as many rows as they fill up to
    /// [`ROWS`], one row for an empty folder.
    pub fn new(count: usize) -> Self {
        let shown = count.min(MAX_ENTRIES);
        let columns = if shown == 0 {
            Self::EMPTY_COLUMNS
        } else {
            shown.min(COLUMNS)
        };
        let rows = shown.div_ceil(COLUMNS).max(1);
        let (cw, ch) = Self::CELL;
        let p = Self::PADDING;
        let width = 2.0 * p + columns as f64 * cw;
        let grid_top = p + Self::TITLE_HEIGHT;
        let grid = Rect::new(p, grid_top, columns as f64 * cw, rows as f64 * ch);
        let button_top = grid.bottom() + p / 2.0;
        let height = button_top + Self::BUTTON_HEIGHT + p;
        let button_width = Self::BUTTON_WIDTH.min(width - 2.0 * p);
        Self {
            width,
            height,
            columns,
            rows,
            title: Rect::new(p, p, width - 2.0 * p, Self::TITLE_HEIGHT),
            grid,
            button: Rect::new(
                (width - button_width) / 2.0,
                button_top,
                button_width,
                Self::BUTTON_HEIGHT,
            ),
        }
    }

    /// The cell of entry `index`, in reading order.
    pub fn cell(&self, index: usize) -> Rect {
        let (cw, ch) = Self::CELL;
        let (row, col) = (index / self.columns, index % self.columns);
        Rect::new(
            self.grid.x + col as f64 * cw,
            self.grid.y + row as f64 * ch,
            cw,
            ch,
        )
    }

    /// The icon's square in `cell`.
    pub fn icon(cell: Rect) -> Rect {
        Rect::new(
            cell.x + (cell.w - Self::ICON) / 2.0,
            cell.y + Self::ICON_TOP,
            Self::ICON,
            Self::ICON,
        )
    }

    /// What a press at `p` is on: an entry among `count`, the button, or neither.
    pub fn hit(&self, p: Point, count: usize) -> Option<StackHit> {
        if self.button.contains(p) {
            return Some(StackHit::OpenFolder);
        }
        (0..count.min(MAX_ENTRIES))
            .find(|&i| self.cell(i).contains(p))
            .map(StackHit::Entry)
    }
}

/// The pointer under a stack that stands over the bar: its width where it leaves the panel, and its depth.
pub const POINTER_WIDTH: f64 = 30.0;
pub const POINTER_DEPTH: f64 = 11.0;
/// How far the pointer's tip stands above the bar's top edge, or above the stack's tile where that stands higher.
pub const TIP_GAP: f64 = 3.0;
/// How far a stack beside a bar at the side of the output stands off the bar.
pub const SIDE_GAP: f64 = 8.0;
/// The least room the stack keeps to the edges of the output.
pub const SCREEN_MARGIN: f64 = 8.0;

/// Where a stack's popup stands by its tile, in the coordinates of the bar's window. Over a bar at the bottom of the
/// output the popup is the panel with its pointer under it, the pointer's tip [`TIP_GAP`] above the bar; beside a bar
/// at a side it is the panel alone, [`SIDE_GAP`] off the bar. The panel is centred on its tile and moves along the bar
/// only to stay [`SCREEN_MARGIN`] inside the output; the pointer stays over the tile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StackPlacement {
    /// The popup's window: the panel at its top-leading corner, and the pointer's depth under it where it has one.
    pub window: Rect,
    /// What the popup anchors to, so the window system puts it at `window` without having to move it: over a bar at
    /// the bottom the window's width from the bar's top edge down, the popup centred above it; at a side a line from
    /// the output's edge to the side of the panel nearest the bar, at the panel's top, the popup beside it.
    pub anchor: Rect,
    /// How far the middle of the pointer is from the window's leading edge; `None` beside a bar at a side, where the
    /// stack has no pointer.
    pub pointer: Option<f64>,
}

impl StackPlacement {
    /// The popup of a panel `width` by `height` for the tile at `tile` of the bar at `bar`, both in the bar's window,
    /// which covers `screen` (the output's whole extent along the bar, at the output's edge). Everything is on whole
    /// points, which is all the window system places a popup at.
    pub fn new(
        orientation: Orientation,
        (width, height): (f64, f64),
        bar: Rect,
        tile: Rect,
        screen: Rect,
    ) -> Self {
        // along the bar: centred on the tile, kept inside the output
        let along = |centre: f64, extent: f64, from: f64, to: f64| {
            let (lo, hi) = (
                (from + SCREEN_MARGIN).ceil(),
                (to - SCREEN_MARGIN - extent).floor(),
            );
            (centre - extent / 2.0).round().min(hi).max(lo)
        };
        match orientation {
            Orientation::Bottom => {
                let x = along(tile.cx(), width, screen.x, screen.right());
                let top = bar.y.min(tile.y).floor();
                let depth = height + POINTER_DEPTH;
                // the pointer keeps off the panel's round corners
                let (lo, hi) = (
                    StackLayout::RADIUS + POINTER_WIDTH / 2.0,
                    width - StackLayout::RADIUS - POINTER_WIDTH / 2.0,
                );
                Self {
                    window: Rect::new(x, top - TIP_GAP - depth, width, depth),
                    anchor: Rect::new(x, top, width, (screen.bottom() - top).max(1.0)),
                    pointer: Some((tile.cx() - x).min(hi).max(lo)),
                }
            }
            Orientation::Left => {
                let y = along(tile.cy(), height, screen.y, screen.bottom());
                let edge = (bar.right().max(tile.right()) + SIDE_GAP).ceil();
                Self {
                    window: Rect::new(edge, y, width, height),
                    anchor: Rect::new(screen.x, y, (edge - screen.x).max(1.0), 1.0),
                    pointer: None,
                }
            }
            Orientation::Right => {
                let y = along(tile.cy(), height, screen.y, screen.bottom());
                let edge = (bar.x.min(tile.x) - SIDE_GAP).floor();
                Self {
                    window: Rect::new(edge - width, y, width, height),
                    anchor: Rect::new(edge, y, (screen.right() - edge).max(1.0), 1.0),
                    pointer: None,
                }
            }
        }
    }
}

/// What a press on the stack is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StackHit {
    Entry(usize),
    OpenFolder,
}

/// How far the stack has come in at `t` seconds since it opened: its opacity and its scale.
pub fn entry(t: f64, reduce_motion: bool) -> (f64, f64) {
    let u = (t / ENTRY_S).clamp(0.0, 1.0);
    // ease out: quick at first, settling at the end
    let eased = 1.0 - (1.0 - u).powi(3);
    let scale = if reduce_motion {
        1.0
    } else {
        ENTRY_SCALE + (1.0 - ENTRY_SCALE) * eased
    };
    (eased, scale)
}

/// The entries of a folder as a stack shows them: hidden files left out, the most recently changed first (by name
/// when equal), at most [`MAX_ENTRIES`]. Each item is the entry and when it last changed.
pub fn newest_first(mut items: Vec<(StackEntry, std::time::SystemTime)>) -> Vec<StackEntry> {
    items.retain(|(e, _)| !e.name.starts_with('.'));
    items.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.name.cmp(&b.0.name)));
    items.truncate(MAX_ENTRIES);
    items.into_iter().map(|(e, _)| e).collect()
}

/// The entries of `folder` a stack shows ([`newest_first`]), each with the icon `icon_of` names for its path and
/// whether it is a folder. A hidden file and a name that is not UTF-8 (the session's opener takes the path as text) are
/// left out before anything is read of them; a link shows what it points at, and one that points nowhere is left out.
/// Only the entries shown have their icon looked up. It reads the whole folder: run it away from the model's thread.
pub fn list_folder(folder: &Path, icon_of: impl Fn(&Path, bool) -> String) -> Vec<StackEntry> {
    let Ok(dir) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let items = dir
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            if name.starts_with('.') {
                return None;
            }
            let path = e.path();
            let changed = std::fs::metadata(&path)
                .ok()?
                .modified()
                .unwrap_or(std::time::UNIX_EPOCH);
            let entry = StackEntry {
                name,
                path,
                icon: String::new(),
            };
            Some((entry, changed))
        })
        .collect();
    let mut entries = newest_first(items);
    for entry in &mut entries {
        let is_dir = std::fs::metadata(&entry.path).is_ok_and(|m| m.is_dir());
        entry.icon = icon_of(&entry.path, is_dir);
    }
    entries
}

/// The user's downloads folder: `XDG_DOWNLOAD_DIR` of a `user-dirs.dirs` file (`user_dirs`, its text), `$HOME`
/// expanded, else `Downloads` in `home`.
pub fn downloads_dir(user_dirs: Option<&str>, home: &Path) -> PathBuf {
    let assigned = user_dirs.and_then(|text| {
        text.lines().find_map(|line| {
            let value = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=")?;
            let value = value.trim().trim_matches('"');
            let path = match value.strip_prefix("$HOME") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => PathBuf::from(value),
            };
            // a value naming home itself means the folder is not set
            (path.is_absolute() && path != home).then_some(path)
        })
    });
    assigned.unwrap_or_else(|| home.join("Downloads"))
}

/// The name a stack and its tile are called by: the folder's own name.
pub fn folder_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map_or_else(|| path.to_owned(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, SystemTime};

    fn entry_named(name: &str) -> StackEntry {
        StackEntry {
            name: name.to_owned(),
            path: PathBuf::from(format!("/d/{name}")),
            icon: "text-x-generic".to_owned(),
        }
    }

    #[test]
    fn the_grid_grows_to_five_columns_then_rows_up_to_three() {
        let one = StackLayout::new(1);
        assert_eq!((one.columns, one.rows), (1, 1));
        assert_eq!(one.width, 2.0 * 16.0 + 96.0);
        let five = StackLayout::new(5);
        assert_eq!((five.columns, five.rows), (5, 1));
        let seven = StackLayout::new(7);
        assert_eq!((seven.columns, seven.rows), (5, 2));
        let many = StackLayout::new(40);
        assert_eq!((many.columns, many.rows), (5, 3), "at most fifteen show");
        assert_eq!(many.width, 2.0 * 16.0 + 5.0 * 96.0);
        assert_eq!(
            many.height,
            16.0 + 28.0 + 3.0 * 96.0 + 8.0 + 28.0 + 16.0,
            "padding, title, rows, half a padding, button, padding"
        );
        let empty = StackLayout::new(0);
        assert_eq!(
            (empty.columns, empty.rows),
            (3, 1),
            "an empty folder keeps a row"
        );
    }

    #[test]
    fn cells_go_in_reading_order_and_presses_find_them_and_the_button() {
        let l = StackLayout::new(7);
        assert_eq!(l.cell(0), Rect::new(16.0, 44.0, 96.0, 96.0));
        assert_eq!(l.cell(4).x, 16.0 + 4.0 * 96.0);
        assert_eq!(l.cell(5), Rect::new(16.0, 140.0, 96.0, 96.0));
        assert_eq!(l.hit(l.cell(6).center(), 7), Some(StackHit::Entry(6)));
        assert_eq!(l.hit(l.cell(7).center(), 7), None, "past the last entry");
        assert_eq!(l.hit(l.button.center(), 7), Some(StackHit::OpenFolder));
        assert_eq!(l.hit(l.title.center(), 7), None);
        let icon = StackLayout::icon(l.cell(0));
        assert_eq!(icon.w, 56.0);
        assert_eq!(icon.cx(), l.cell(0).cx(), "the icon is centred in its cell");
    }

    #[test]
    fn the_stack_comes_in_over_a_quarter_second_scaling_up_unless_motion_is_reduced() {
        assert_eq!(entry(0.0, false), (0.0, ENTRY_SCALE));
        let (a, s) = entry(ENTRY_S / 2.0, false);
        assert!(
            a > 0.5 && a < 1.0,
            "eased out: past half way at half time ({a})"
        );
        assert!(s > ENTRY_SCALE && s < 1.0);
        assert_eq!(entry(ENTRY_S, false), (1.0, 1.0));
        assert_eq!(entry(1.0, false), (1.0, 1.0));
        let (a, s) = entry(ENTRY_S / 2.0, true);
        assert!(a > 0.5 && a < 1.0);
        assert_eq!(s, 1.0, "Reduce Motion only fades");
    }

    /// A 1512 by 982 output seen from a bar window 110 points deep at its bottom edge, the bar's top 21 points down
    /// the window, and a 64 point tile in it whose middle is at `cx`.
    fn over_a_bar(cx: f64, panel: (f64, f64)) -> (StackPlacement, Rect, Rect) {
        let screen = Rect::new(0.0, 0.0, 1512.0, 110.0);
        let bar = Rect::new(400.0, 21.0, 712.0, 84.0);
        let tile = Rect::new(cx - 32.0, 31.0, 64.0, 64.0);
        let placement = StackPlacement::new(Orientation::Bottom, panel, bar, tile, screen);
        (placement, bar, tile)
    }

    #[test]
    fn over_a_bar_the_pointers_tip_is_3_points_above_it_and_the_panel_is_centred_on_the_tile() {
        let layout = StackLayout::new(7);
        let size = (layout.width, layout.height);
        let (p, bar, tile) = over_a_bar(906.0, size);
        assert_eq!(p.window.bottom(), bar.y - TIP_GAP, "the tip");
        assert_eq!(
            p.window.bottom() - POINTER_DEPTH,
            bar.y - 14.0,
            "the panel's bottom edge"
        );
        assert_eq!(p.window.cx(), tile.cx());
        assert_eq!((p.window.w, p.window.h), (size.0, size.1 + POINTER_DEPTH));
        assert_eq!(p.pointer, Some(size.0 / 2.0), "under the panel's middle");
        // the popup centred above the anchor, the gap apart, is the window
        assert_eq!((p.anchor.x, p.anchor.w), (p.window.x, p.window.w));
        assert_eq!(p.anchor.y - TIP_GAP, p.window.bottom());
        assert_eq!(p.anchor.bottom(), 110.0, "down to the window's bottom edge");
    }

    #[test]
    fn a_panel_moved_to_stay_inside_the_output_keeps_its_pointer_over_the_tile() {
        let layout = StackLayout::new(15);
        let size = (layout.width, layout.height);
        // a tile near the output's trailing edge: the panel stops 8 points inside it
        let (p, _, tile) = over_a_bar(1400.0, size);
        assert_eq!(p.window.right(), 1512.0 - SCREEN_MARGIN);
        assert_eq!(p.window.x + p.pointer.unwrap(), tile.cx(), "on the tile");
        assert!(p.pointer.unwrap() > size.0 / 2.0);
        // and near the leading edge
        let (p, _, tile) = over_a_bar(100.0, size);
        assert_eq!(p.window.x, SCREEN_MARGIN);
        assert_eq!(p.window.x + p.pointer.unwrap(), tile.cx(), "on the tile");
        // a tile nearer the edge than the panel's corner: the pointer stops at the corner's end
        let (p, ..) = over_a_bar(1500.0, size);
        assert_eq!(
            p.pointer,
            Some(size.0 - StackLayout::RADIUS - POINTER_WIDTH / 2.0)
        );
        let (p, ..) = over_a_bar(12.0, size);
        assert_eq!(p.pointer, Some(StackLayout::RADIUS + POINTER_WIDTH / 2.0));
    }

    #[test]
    fn over_a_tile_that_stands_above_the_bar_the_tip_is_3_points_above_the_tile() {
        let layout = StackLayout::new(3);
        let screen = Rect::new(0.0, 0.0, 1512.0, 174.0);
        let bar = Rect::new(400.0, 85.0, 712.0, 84.0);
        // a magnified tile, 128 points, its bottom edge where a resting tile's is
        let tile = Rect::new(842.0, 31.0, 128.0, 128.0);
        let p = StackPlacement::new(
            Orientation::Bottom,
            (layout.width, layout.height),
            bar,
            tile,
            screen,
        );
        assert_eq!(p.window.bottom(), tile.y - TIP_GAP);
        assert_eq!(p.window.cx(), tile.cx());
    }

    #[test]
    fn a_place_is_on_whole_points_whatever_the_bar_s_are() {
        let layout = StackLayout::new(2);
        let screen = Rect::new(0.0, 0.0, 1280.0, 96.75);
        let bar = Rect::new(500.25, 7.5, 300.5, 84.25);
        let tile = Rect::new(700.3, 17.4, 64.6, 64.6);
        let p = StackPlacement::new(
            Orientation::Bottom,
            (layout.width, layout.height),
            bar,
            tile,
            screen,
        );
        for v in [
            p.window.x, p.window.y, p.window.w, p.window.h, p.anchor.x, p.anchor.y, p.anchor.w,
        ] {
            assert_eq!(v.fract(), 0.0, "{p:?}");
        }
        let gap = bar.y - p.window.bottom();
        assert!((TIP_GAP..TIP_GAP + 1.0).contains(&gap), "{gap}");
        assert!((p.window.x + p.pointer.unwrap() - tile.cx()).abs() < 1e-9);
    }

    #[test]
    fn beside_a_bar_at_a_side_the_panel_stands_8_points_off_it_without_a_pointer() {
        let layout = StackLayout::new(7);
        let size = (layout.width, layout.height);
        // a bar window 110 points wide at the output's left edge, 982 points tall
        let screen = Rect::new(0.0, 0.0, 110.0, 982.0);
        let bar = Rect::new(5.0, 300.0, 84.0, 400.0);
        let tile = Rect::new(15.0, 600.0, 64.0, 64.0);
        let p = StackPlacement::new(Orientation::Left, size, bar, tile, screen);
        assert_eq!(p.pointer, None);
        assert_eq!(p.window.x, bar.right() + SIDE_GAP);
        assert_eq!(p.window.cy(), tile.cy());
        assert_eq!((p.window.w, p.window.h), size);
        // the popup beside the anchor, their tops aligned, is the window
        assert_eq!((p.anchor.right(), p.anchor.y), (p.window.x, p.window.y));
        // at the right edge, mirrored; a tile near the output's bottom keeps the panel inside it
        let bar = Rect::new(21.0, 300.0, 84.0, 660.0);
        let tile = Rect::new(31.0, 890.0, 64.0, 64.0);
        let p = StackPlacement::new(Orientation::Right, size, bar, tile, screen);
        assert_eq!(p.window.right(), bar.x - SIDE_GAP);
        assert_eq!((p.anchor.x, p.anchor.y), (p.window.right(), p.window.y));
        assert_eq!(p.window.bottom(), 982.0 - SCREEN_MARGIN);
    }

    #[test]
    fn entries_are_the_newest_first_without_hidden_files_at_most_fifteen() {
        let t = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        let mut items = vec![
            (entry_named("old.txt"), t(1)),
            (entry_named(".hidden"), t(9)),
            (entry_named("new.png"), t(5)),
            (entry_named("b"), t(3)),
            (entry_named("a"), t(3)),
        ];
        let names: Vec<String> = newest_first(items.clone())
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(names, ["new.png", "a", "b", "old.txt"]);
        items = (0..20)
            .map(|i| (entry_named(&format!("f{i:02}")), t(i)))
            .collect();
        let kept = newest_first(items);
        assert_eq!(kept.len(), MAX_ENTRIES);
        assert_eq!(kept[0].name, "f19");
    }

    /// A real folder: hidden files, a link that points nowhere and a name that is not UTF-8 are left out, a link to a
    /// folder shows as a folder, a file that cannot be read still shows, the newest first, and only those shown have
    /// their icon looked up.
    #[test]
    fn a_folder_lists_its_shown_files_newest_first_and_looks_up_only_their_icons() {
        use std::os::unix::ffi::OsStrExt;
        use std::os::unix::fs::{symlink, PermissionsExt};
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let at = |name: &str, secs: u64| {
            let path = root.join(name);
            std::fs::write(&path, b"x").unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(secs))
                .unwrap();
        };
        at("old.txt", 1_000);
        at("new.txt", 3_000);
        at(".hidden", 9_000);
        at("locked.txt", 2_000);
        std::fs::set_permissions(
            root.join("locked.txt"),
            std::fs::Permissions::from_mode(0o000),
        )
        .unwrap();
        std::fs::create_dir(root.join("inner")).unwrap();
        let inner = std::fs::File::open(root.join("inner")).unwrap();
        inner
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(500))
            .unwrap();
        symlink(root.join("inner"), root.join("to-inner")).unwrap();
        symlink(root.join("gone"), root.join("dangling")).unwrap();
        std::fs::write(root.join(std::ffi::OsStr::from_bytes(b"bad-\xff")), b"x").unwrap();

        let looked_up = std::cell::RefCell::new(Vec::new());
        let entries = list_folder(root, |path, is_dir| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            looked_up.borrow_mut().push(name);
            if is_dir { "folder" } else { "text" }.to_owned()
        });
        let shown: Vec<(&str, &str)> = entries
            .iter()
            .map(|e| (e.name.as_str(), e.icon.as_str()))
            .collect();
        assert_eq!(
            shown,
            [
                ("new.txt", "text"),
                ("locked.txt", "text"),
                ("old.txt", "text"),
                ("inner", "folder"),
                ("to-inner", "folder"),
            ]
        );
        assert_eq!(entries[0].path, root.join("new.txt"));
        assert_eq!(looked_up.borrow().len(), 5, "only the shown");
        std::fs::set_permissions(
            root.join("locked.txt"),
            std::fs::Permissions::from_mode(0o600),
        )
        .unwrap();

        let many = tempfile::tempdir().unwrap();
        for i in 0..40 {
            std::fs::write(many.path().join(format!("f{i:02}")), b"x").unwrap();
        }
        let looked_up = std::cell::Cell::new(0);
        let entries = list_folder(many.path(), |_, _| {
            looked_up.set(looked_up.get() + 1);
            String::new()
        });
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(looked_up.get(), MAX_ENTRIES, "the rest are not looked up");
        assert!(list_folder(&root.join("missing"), |_, _| String::new()).is_empty());
    }

    #[test]
    fn the_downloads_folder_comes_from_user_dirs_else_home() {
        let home = Path::new("/home/u");
        let text = "# comment\nXDG_DESKTOP_DIR=\"$HOME/Desktop\"\nXDG_DOWNLOAD_DIR=\"$HOME/Telechargements\"\n";
        assert_eq!(
            downloads_dir(Some(text), home),
            home.join("Telechargements")
        );
        assert_eq!(
            downloads_dir(Some("XDG_DOWNLOAD_DIR=\"/srv/dl\"\n"), home),
            PathBuf::from("/srv/dl")
        );
        assert_eq!(
            downloads_dir(Some("XDG_DOWNLOAD_DIR=\"$HOME/\"\n"), home),
            home.join("Downloads"),
            "home itself means unset"
        );
        assert_eq!(downloads_dir(None, home), home.join("Downloads"));
        assert_eq!(folder_name("/home/u/Downloads"), "Downloads");
    }
}
