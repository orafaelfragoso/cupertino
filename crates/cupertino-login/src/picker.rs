//! The greeter's user picker: the tiles (the login users sorted by name, then "Other…" when it shows), the selection, the grid
//! that holds them and the entry of a name for the "Other…" tile. Pure: no drawing, no clock.

use std::ops::Range;

use crate::user::User;

/// Tiles per page on an output larger than [`SMALL_SCREEN`], and on one that is not (25 or 12 by
/// the screen size).
const LARGE_PAGE: usize = 25;
const SMALL_PAGE: usize = 12;
/// The screen size at which the page size changes, in points (threshold 1024 x 768; whether
/// "larger" means both sides or one is not read, approximate here as both).
const SMALL_SCREEN: (f64, f64) = (1024.0, 768.0);

/// Rows and columns of the grid for n = 1 … 24 tiles (tables at ``).
const ROWS: [usize; 24] = [
    1, 2, 3, 2, 3, 3, 4, 4, 3, 4, 4, 4, 5, 5, 5, 4, 5, 5, 5, 5, 6, 6, 6, 6,
];
const COLUMNS: [usize; 24] = [
    1, 1, 1, 2, 2, 2, 2, 2, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4,
];
/// The grid for the larger page and for any n outside 1 … 24.
const FULL_GRID: Grid = Grid {
    rows: 5,
    columns: 5,
};

/// The longest name the "Other…" tile takes, in bytes (approximate: `useradd(8) limits a login name to 32).
const MAX_NAME_BYTES: usize = 32;

/// The switch between the picker and the password screen: how long it takes, eased in and out.
pub const SWITCH_DURATION: f64 = 0.5;

/// How far the switch is `elapsed` seconds after it began, 0 to 1: the opacity of the screen it shows.
pub fn switched(elapsed: f64) -> f64 {
    let t = (elapsed / SWITCH_DURATION).clamp(0.0, 1.0);
    cupertino_animation::CubicBezier::EASE_IN_EASE_OUT.eval(t)
}

/// Tiles per page for an output of `width` x `height` points.
pub fn max_per_page(width: f64, height: f64) -> usize {
    if width > SMALL_SCREEN.0 && height > SMALL_SCREEN.1 {
        LARGE_PAGE
    } else {
        SMALL_PAGE
    }
}

/// Rows and columns of the grid of one page.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Grid {
    pub rows: usize,
    pub columns: usize,
}

impl Grid {
    /// The grid for `n` = the smaller of the page size and the tile count.
    pub fn for_count(n: usize) -> Grid {
        match n {
            1..=24 => Grid {
                rows: ROWS[n - 1],
                columns: COLUMNS[n - 1],
            },
            _ => FULL_GRID,
        }
    }
}

/// Where tile `index` sits: its page, and its row and column within the page. The design lays the pages side by side
/// (its column is `column + page * columns`); the picker shows one page at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    pub page: usize,
    pub row: usize,
    pub column: usize,
}

/// The cell of tile `index` with `max_per_page` tiles a page, in the grid of `count` tiles
/// (`page = i / M`, `k = i % M`, `column = k % columns`, `row = k / columns`).
pub fn cell_of(index: usize, count: usize, max_per_page: usize) -> Cell {
    let grid = Grid::for_count(max_per_page.min(count));
    let k = index % max_per_page;
    Cell {
        page: index / max_per_page,
        row: k / grid.columns,
        column: k % grid.columns,
    }
}

/// One tile of the picker.
#[derive(Debug)]
pub enum Tile {
    Account(User),
    /// "Other…": type the name of an account that is not listed.
    Other,
}

/// An arrow key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// The tiles and the selected one. Nothing shows as selected until the first arrow key or the pointer over a tile.
#[derive(Debug)]
pub struct Picker {
    tiles: Vec<Tile>,
    selected: usize,
    /// The selection shows.
    shown: bool,
}

/// A key that moves the selection a long way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    /// Home: the first tile.
    First,
    /// End: the last tile.
    Last,
    /// Page Up: the first tile of the page before.
    PageBack,
    /// Page Down: the first tile of the page after.
    PageForward,
}

/// There is no account to offer and the "Other…" tile is hidden, so nobody could log in.
#[derive(Debug, PartialEq, Eq)]
pub struct NoTiles;

impl std::fmt::Display for NoTiles {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the picker has no account to offer and no \"Other…\" tile; the account filters leave nothing to log in with")
    }
}

impl std::error::Error for NoTiles {}

impl Picker {
    /// The login `accounts` sorted by name without regard to case
    /// (approximated by comparing lower-case text; equal names by account name), then "Other…" when `other` (see
    /// the session manager's filter). There is no Guest tile: it exists only with the design's remote-locate
    /// feature, which this system does not have. The first tile is selected; with no tile at all there is no picker.
    pub fn new(mut accounts: Vec<User>, other: bool) -> Result<Picker, NoTiles> {
        accounts.sort_by_cached_key(|user| (user.display.to_lowercase(), user.login.clone()));
        let tiles: Vec<Tile> = accounts
            .into_iter()
            .map(Tile::Account)
            .chain(other.then_some(Tile::Other))
            .collect();
        if tiles.is_empty() {
            return Err(NoTiles);
        }
        Ok(Picker {
            tiles,
            selected: 0,
            shown: false,
        })
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    pub fn selected(&self) -> &Tile {
        &self.tiles[self.selected]
    }

    /// The tile a choice without a pointer takes (Return): the selected one, or the first while none shows.
    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// The selected tile while the selection shows.
    pub fn selection(&self) -> Option<usize> {
        self.shown.then_some(self.selected)
    }

    /// Select tile `index`, for a pointer over it.
    pub fn select(&mut self, index: usize) {
        assert!(index < self.tiles.len(), "the tile exists");
        self.selected = index;
        self.shown = true;
    }

    /// The focus came to tile `index`: it is the one Return and the arrows start from; the selection shows only once
    /// an arrow or the pointer showed it.
    pub fn focus(&mut self, index: usize) {
        assert!(index < self.tiles.len(), "the tile exists");
        self.selected = index;
    }

    /// Move the selection to the first or last tile, or a page back or forward, and show it.
    pub fn jump(&mut self, jump: Jump, max_per_page: usize) {
        let page = max_per_page.max(1);
        let last = self.tiles.len() - 1;
        self.shown = true;
        self.selected = match jump {
            Jump::First => 0,
            Jump::Last => last,
            Jump::PageBack => (self.selected / page).saturating_sub(1) * page,
            Jump::PageForward => ((self.selected / page + 1) * page).min(last),
        };
    }

    /// The tiles of the page the selection is on: the range of their indices.
    pub fn page(&self, max_per_page: usize) -> Range<usize> {
        let start = self.selected / max_per_page * max_per_page;
        start..(start + max_per_page).min(self.tiles.len())
    }

    /// Move the selection one tile left or right, or one row up or down within its page; at an edge it stays. While no
    /// selection shows, the first arrow only shows it on the first tile.
    pub fn step(&mut self, direction: Direction, max_per_page: usize) {
        if !self.shown {
            self.shown = true;
            return;
        }
        let columns = Grid::for_count(max_per_page.min(self.tiles.len())).columns;
        let target = match direction {
            Direction::Left => self.selected.checked_sub(1),
            Direction::Right => Some(self.selected + 1),
            Direction::Up => (cell_of(self.selected, self.tiles.len(), max_per_page).row > 0)
                .then(|| self.selected - columns),
            Direction::Down => Some(self.selected + columns)
                .filter(|&below| below / max_per_page == self.selected / max_per_page),
        };
        if let Some(target) = target.filter(|&target| target < self.tiles.len()) {
            self.selected = target;
        }
    }
}

/// The name typed for the "Other…" tile, and for an administrator: visible characters without whitespace, at most
/// [`MAX_NAME_BYTES`]. The text field shows it and hands every edit to [`NameEntry::set`].
#[derive(Debug, Default)]
pub struct NameEntry {
    text: String,
    /// Counts the times [`NameEntry::set`] changed what was typed: the field then shows the name kept.
    revision: u64,
}

impl NameEntry {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// The revision of [`NameEntry::text`] the field shows.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// The field holds `typed`: keep its visible characters without whitespace, up to the limit. A name that differs
    /// from what was typed gets a new revision, so the field shows what was kept.
    pub fn set(&mut self, typed: &str) {
        let mut kept = String::new();
        for c in typed
            .chars()
            .filter(|c| !c.is_control() && !c.is_whitespace())
        {
            if kept.len() + c.len_utf8() > MAX_NAME_BYTES {
                break;
            }
            kept.push(c);
        }
        if kept != typed {
            self.revision += 1;
        }
        self.text = kept;
    }

    /// Return: the name to log in as, and an empty field; `None` while the field is empty.
    pub fn submit(&mut self) -> Option<String> {
        if self.text.is_empty() {
            return None;
        }
        self.revision += 1;
        Some(std::mem::take(&mut self.text))
    }
}
