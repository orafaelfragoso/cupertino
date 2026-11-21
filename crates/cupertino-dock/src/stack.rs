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
    pub const TITLE_FONT_PT: f64 = 14.0;
    pub const TITLE_WEIGHT: u16 = 600;
    pub const BUTTON_HEIGHT: f64 = 28.0;
    pub const BUTTON_WIDTH: f64 = 120.0;
    pub const BUTTON_FONT_PT: f64 = 13.0;
    /// One entry's cell, its icon's side and where the icon's top is in it, the label's centre line and size. The
    /// cell is as wide as eight times the label's size and a label's inset either side, so a name of about fifteen
    /// letters shows whole; the label's centre line is 19 points under the icon and as far from the cell's
    /// bottom, so its line stands clear of both rows' icons.
    pub const CELL: (f64, f64) = (112.0, 102.0);
    pub const ICON: f64 = 56.0;
    pub const ICON_TOP: f64 = 8.0;
    pub const LABEL_CY: f64 = 83.0;
    pub const LABEL_FONT_PT: f64 = 13.0;
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
#[expect(
    clippy::disallowed_methods,
    reason = "runs on the I/O pool (kit::task::io)"
)]
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
