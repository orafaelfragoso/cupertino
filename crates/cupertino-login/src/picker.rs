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

/// The tiles and the selected one.
#[derive(Debug)]
pub struct Picker {
    tiles: Vec<Tile>,
    selected: usize,
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
        Ok(Picker { tiles, selected: 0 })
    }

    pub fn tiles(&self) -> &[Tile] {
        &self.tiles
    }

    pub fn selected(&self) -> &Tile {
        &self.tiles[self.selected]
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    /// Select tile `index`, for a pointer over it.
    pub fn select(&mut self, index: usize) {
        assert!(index < self.tiles.len(), "the tile exists");
        self.selected = index;
    }

    /// The tiles of the page the selection is on: the range of their indices.
    pub fn page(&self, max_per_page: usize) -> Range<usize> {
        let start = self.selected / max_per_page * max_per_page;
        start..(start + max_per_page).min(self.tiles.len())
    }

    /// Move the selection one tile left or right, or one row up or down within its page; at an edge it stays.
    pub fn step(&mut self, direction: Direction, max_per_page: usize) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn account(login: &str, display: &str) -> User {
        User {
            login: login.into(),
            display: display.into(),
            picture: None,
            uid: None,
            logged_in: false,
        }
    }

    fn picker(count: usize) -> Picker {
        Picker::new(
            (0..count)
                .map(|i| account(&format!("u{i:02}"), &format!("User {i:02}")))
                .collect(),
            true,
        )
        .unwrap()
    }

    fn labels(picker: &Picker) -> Vec<String> {
        picker
            .tiles()
            .iter()
            .map(|tile| match tile {
                Tile::Account(user) => user.display.clone(),
                Tile::Other => "Other".into(),
            })
            .collect()
    }

    #[test]
    fn the_grid_follows_the_register_tables() {
        let grids: Vec<(usize, usize)> = [1, 2, 3, 4, 5, 9, 12, 13, 16, 24, 25]
            .into_iter()
            .map(Grid::for_count)
            .map(|g| (g.rows, g.columns))
            .collect();
        assert_eq!(
            grids,
            [
                (1, 1),
                (2, 1),
                (3, 1),
                (2, 2),
                (3, 2),
                (3, 3),
                (4, 3),
                (5, 3),
                (4, 4),
                (6, 4),
                (5, 5)
            ]
        );
        // outside the tables the full grid
        assert_eq!(
            (Grid::for_count(0), Grid::for_count(40)),
            (
                Grid {
                    rows: 5,
                    columns: 5
                },
                Grid {
                    rows: 5,
                    columns: 5
                }
            )
        );
    }

    #[test]
    fn tiles_are_assigned_to_pages_rows_and_columns() {
        // 14 tiles on pages of 12: the grid is that of 12 tiles, 4 rows of 3
        let cells: Vec<(usize, usize, usize)> = [0, 2, 3, 11, 12, 13]
            .into_iter()
            .map(|i| cell_of(i, 14, 12))
            .map(|c| (c.page, c.row, c.column))
            .collect();
        assert_eq!(
            cells,
            [
                (0, 0, 0),
                (0, 0, 2),
                (0, 1, 0),
                (0, 3, 2),
                (1, 0, 0),
                (1, 0, 1)
            ]
        );
        // 4 tiles: 2 rows of 2
        assert_eq!(
            cell_of(3, 4, 25),
            Cell {
                page: 0,
                row: 1,
                column: 1
            }
        );
        // 30 tiles on pages of 25: 5 x 5
        assert_eq!(
            cell_of(24, 30, 25),
            Cell {
                page: 0,
                row: 4,
                column: 4
            }
        );
        assert_eq!(
            cell_of(29, 30, 25),
            Cell {
                page: 1,
                row: 0,
                column: 4
            }
        );
    }

    #[test]
    fn the_page_size_depends_on_the_screen() {
        assert_eq!(
            (
                max_per_page(1920.0, 1080.0),
                max_per_page(1024.0, 768.0),
                max_per_page(800.0, 600.0)
            ),
            (25, 12, 12)
        );
    }

    #[test]
    fn accounts_sort_without_regard_to_case_and_other_comes_last() {
        let picker = Picker::new(
            vec![
                account("zed", "zed"),
                account("bob", "Bob"),
                account("alice", "alice"),
                account("carol", "Carol"),
                account("al2", "alice"),
            ],
            true,
        )
        .unwrap();
        assert_eq!(
            labels(&picker),
            ["alice", "alice", "Bob", "Carol", "zed", "Other"]
        );
        // equal names by account name
        assert!(matches!(&picker.tiles()[0], Tile::Account(user) if user.login == "al2"));
    }

    #[test]
    fn with_no_accounts_there_is_only_other() {
        let picker = Picker::new(Vec::new(), true).unwrap();
        assert_eq!(labels(&picker), ["Other"]);
        assert!(matches!(picker.selected(), Tile::Other));
    }

    #[test]
    fn without_the_other_tile_the_accounts_are_all_there_is() {
        let picker = Picker::new(
            vec![account("bob", "Bob"), account("alice", "Alice")],
            false,
        )
        .unwrap();
        assert_eq!(labels(&picker), ["Alice", "Bob"]);
    }

    #[test]
    fn no_account_and_no_other_tile_is_an_error() {
        assert_eq!(Picker::new(Vec::new(), false).err(), Some(NoTiles));
    }

    #[test]
    fn arrows_move_through_the_grid_and_stop_at_its_edges() {
        // 5 tiles (4 users and Other): a 3 x 2 grid, rows [0 1] [2 3] [4]
        let mut p = picker(4);
        p.step(Direction::Down, 12);
        assert_eq!(p.selected_index(), 2);
        p.step(Direction::Right, 12);
        assert_eq!(p.selected_index(), 3);
        // nothing below 3: row 3 would hold index 5
        p.step(Direction::Down, 12);
        assert_eq!(p.selected_index(), 3);
        p.step(Direction::Right, 12);
        p.step(Direction::Right, 12);
        assert_eq!(p.selected_index(), 4);
        p.step(Direction::Up, 12);
        p.step(Direction::Up, 12);
        p.step(Direction::Up, 12);
        assert_eq!(p.selected_index(), 0);
        p.step(Direction::Left, 12);
        assert_eq!(p.selected_index(), 0);
    }

    #[test]
    fn the_selection_crosses_pages_with_left_and_right_only() {
        // 14 tiles on pages of 12, 3 columns: index 11 is the last of page 0
        let mut p = picker(13);
        p.select(11);
        p.step(Direction::Down, 12);
        assert_eq!(p.selected_index(), 11);
        assert_eq!(p.page(12), 0..12);
        p.step(Direction::Right, 12);
        assert_eq!(p.selected_index(), 12);
        assert_eq!(p.page(12), 12..14);
        p.step(Direction::Up, 12);
        assert_eq!(p.selected_index(), 12);
    }

    #[test]
    fn the_name_for_other_takes_visible_characters_and_submits_on_return() {
        let mut entry = NameEntry::default();
        assert_eq!(entry.submit(), None);
        entry.set("ca rol\t");
        assert_eq!(entry.text(), "carol");
        assert_eq!(entry.revision(), 1, "the field shows what was kept");
        entry.set("caro");
        assert_eq!(
            entry.revision(),
            1,
            "a name kept as typed keeps its revision"
        );
        assert_eq!(entry.submit(), Some("caro".into()));
        assert_eq!((entry.text(), entry.revision()), ("", 2));
    }

    #[test]
    fn the_name_stops_at_the_limit() {
        let mut entry = NameEntry::default();
        entry.set(&"a".repeat(MAX_NAME_BYTES + 5));
        assert_eq!(entry.text().len(), MAX_NAME_BYTES);
        // a two-byte character past the limit is left out whole
        entry.set(&format!("{}é", "a".repeat(MAX_NAME_BYTES - 1)));
        assert_eq!(entry.text().len(), MAX_NAME_BYTES - 1);
    }
}
