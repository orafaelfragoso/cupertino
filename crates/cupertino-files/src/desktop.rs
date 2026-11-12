//! The desktop's arrangement without a toolkit: the items of the desktop folder sit in the cells of a grid that starts
//! at the top right corner of the main display and fills each column downwards before the next one to its left
//! ([`Grid`]). Each item keeps the cell it was put in ([`arrange`]) until it is dragged elsewhere ([`dropped`]), the
//! desktop is cleaned up ([`cleaned`]) or kept sorted by a key, which lays every item out in that order.
//!
//! What the desktop keeps between launches ([`DesktopPrefs`]) is `$XDG_CONFIG_HOME/cupertino/files/desktop.json`
//! ([`location`]): `{"sort": null | "name" | "kind" | "date-modified" | "size", "positions": {NAME: [COLUMN, ROW]}}`,
//! column 0 being the rightmost. A missing or malformed file, or a field of another type, gives that field's default; a
//! write replaces the file atomically.

use std::collections::{BTreeMap, HashSet};
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::geom::{Direction, Point, Rect};
use crate::metrics::desktop as m;
use crate::sort::SortKey;

/// A cell of the grid: `column` 0 is the rightmost, `row` 0 the top one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Slot {
    pub column: u32,
    pub row: u32,
}

impl Slot {
    pub const fn new(column: u32, row: u32) -> Slot {
        Slot { column, row }
    }
}

/// The grid of a desktop `width` by `height` points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    width: f64,
    /// The room the menu bar keeps at the top.
    top: f64,
    columns: u32,
    rows: u32,
}

impl Grid {
    /// The cells that fit a desktop of this size below `top` points the menu bar keeps; one at least.
    pub fn new(width: f64, height: f64, top: f64) -> Grid {
        let fit = |room: f64, pitch: f64| ((room / pitch).floor().max(1.0)) as u32;
        Grid {
            width,
            top,
            columns: fit(width - m::RIGHT_MARGIN, m::COLUMN_PITCH),
            rows: fit(height - top - m::TOP_MARGIN, m::ROW_PITCH),
        }
    }

    pub fn columns(&self) -> u32 {
        self.columns
    }

    pub fn rows(&self) -> u32 {
        self.rows
    }

    pub fn contains(&self, slot: Slot) -> bool {
        slot.column < self.columns && slot.row < self.rows
    }

    /// The cell `slot` covers, in the desktop's points.
    pub fn cell(&self, slot: Slot) -> Rect {
        Rect::new(
            self.width - m::RIGHT_MARGIN - f64::from(slot.column + 1) * m::COLUMN_PITCH,
            self.top + m::TOP_MARGIN + f64::from(slot.row) * m::ROW_PITCH,
            m::COLUMN_PITCH,
            m::ROW_PITCH,
        )
    }

    /// The cell whose middle is nearest to `point`.
    pub fn nearest(&self, point: Point) -> Slot {
        let from_right = self.width - m::RIGHT_MARGIN - point.x;
        let column = (from_right / m::COLUMN_PITCH - 0.5).round();
        let row = ((point.y - self.top - m::TOP_MARGIN) / m::ROW_PITCH - 0.5).round();
        Slot {
            column: column.clamp(0.0, f64::from(self.columns - 1)) as u32,
            row: row.clamp(0.0, f64::from(self.rows - 1)) as u32,
        }
    }

    /// Every cell, in the order items fill them: down each column, the columns from right to left.
    pub fn order(&self) -> impl Iterator<Item = Slot> + '_ {
        (0..self.columns)
            .flat_map(move |column| (0..self.rows).map(move |row| Slot { column, row }))
    }

    /// The last cell, where items that find no free cell pile up.
    fn last(&self) -> Slot {
        Slot {
            column: self.columns - 1,
            row: self.rows - 1,
        }
    }

    /// The free cell nearest to `slot`, if any is free; `slot` itself when it is.
    pub fn nearest_free(&self, slot: Slot, taken: &HashSet<Slot>) -> Option<Slot> {
        let distance = |s: Slot| {
            let dc = f64::from(s.column) - f64::from(slot.column);
            let dr = f64::from(s.row) - f64::from(slot.row);
            (dc * m::COLUMN_PITCH).hypot(dr * m::ROW_PITCH)
        };
        self.order()
            .filter(|s| !taken.contains(s))
            .min_by(|a, b| distance(*a).total_cmp(&distance(*b)))
    }
}

/// Where the parts of an item are in its `cell`: the backdrop a selected icon sits on, the icon in it, and the top of
/// the name, which is centred under the icon and at most [`m::LABEL_WIDTH`] wide.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ItemFrames {
    pub backdrop: Rect,
    pub icon: Rect,
    pub label_top: f64,
    pub label_center: f64,
}

pub fn item_frames(cell: Rect) -> ItemFrames {
    let side = m::ICON_SIZE + 2.0 * m::BACKDROP_PAD;
    let backdrop = Rect::new(
        cell.x + (cell.w - side) / 2.0,
        cell.y + m::ICON_TOP,
        side,
        side,
    );
    ItemFrames {
        backdrop,
        icon: backdrop.inset(m::BACKDROP_PAD, m::BACKDROP_PAD),
        label_top: backdrop.bottom() + m::LABEL_GAP,
        label_center: cell.x + cell.w / 2.0,
    }
}

/// The corner radius of the backdrop behind a selected icon.
pub fn backdrop_radius() -> f64 {
    (m::ICON_SIZE * m::BACKDROP_RADIUS_RATIO).min(m::BACKDROP_RADIUS_MAX)
}

/// How the desktop orders its items.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Arrangement {
    /// Each item stays where it was put.
    #[default]
    Free,
    /// Every item in this order, filling the grid from its first cell.
    Sorted(SortKey),
}

impl Arrangement {
    fn name(self) -> Option<&'static str> {
        match self {
            Arrangement::Free => None,
            Arrangement::Sorted(SortKey::Kind) => Some("kind"),
            Arrangement::Sorted(SortKey::DateModified) => Some("date-modified"),
            Arrangement::Sorted(SortKey::Size) => Some("size"),
            Arrangement::Sorted(_) => Some("name"),
        }
    }

    fn of_name(name: &str) -> Arrangement {
        match name {
            "name" => Arrangement::Sorted(SortKey::Name),
            "kind" => Arrangement::Sorted(SortKey::Kind),
            "date-modified" => Arrangement::Sorted(SortKey::DateModified),
            "size" => Arrangement::Sorted(SortKey::Size),
            _ => Arrangement::Free,
        }
    }

    /// Whether the key orders from the largest or newest down; a name or a kind reads from A to Z.
    pub fn ascending(key: SortKey) -> bool {
        !matches!(key, SortKey::DateModified | SortKey::Size)
    }
}

/// What the desktop keeps between launches: how it orders its items, and the cell of each item by name.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DesktopPrefs {
    pub arrangement: Arrangement,
    pub positions: BTreeMap<String, Slot>,
}

/// Where the session keeps the desktop's arrangement: `files/desktop.json` in Cupertino's configuration directory,
/// `None` when neither `$XDG_CONFIG_HOME` nor `$HOME` is set.
pub fn location() -> Option<PathBuf> {
    cupertino_defaults::config_home().map(|config| config.join("cupertino/files/desktop.json"))
}

impl DesktopPrefs {
    /// The arrangement in the text of the file; what is missing or malformed at its default.
    pub fn parse(text: &str) -> DesktopPrefs {
        let Ok(value) = serde_json::from_str::<Value>(text) else {
            return DesktopPrefs::default();
        };
        let arrangement = value
            .get("sort")
            .and_then(Value::as_str)
            .map_or(Arrangement::Free, Arrangement::of_name);
        let slot = |v: &Value| {
            let pair = v.as_array()?;
            let at = |i: usize| pair.get(i)?.as_u64().and_then(|n| u32::try_from(n).ok());
            Some(Slot::new(at(0)?, at(1)?))
        };
        let positions = value
            .get("positions")
            .and_then(Value::as_object)
            .map(|map| {
                map.iter()
                    .filter_map(|(name, v)| Some((name.clone(), slot(v)?)))
                    .collect()
            })
            .unwrap_or_default();
        DesktopPrefs {
            arrangement,
            positions,
        }
    }

    /// The text of the file.
    pub fn text(&self) -> String {
        let positions: Map<String, Value> = self
            .positions
            .iter()
            .map(|(name, slot)| (name.clone(), json!([slot.column, slot.row])))
            .collect();
        let value = json!({
            "sort": self.arrangement.name(),
            "positions": positions,
        });
        format!("{value:#}\n")
    }

    /// The arrangement kept at `path`; the defaults when it cannot be read.
    pub fn load(path: &Path) -> DesktopPrefs {
        match std::fs::read_to_string(path) {
            Ok(text) => DesktopPrefs::parse(&text),
            Err(e) => {
                if e.kind() != io::ErrorKind::NotFound {
                    eprintln!("cupertino-files: cannot read {}: {e}", path.display());
                }
                DesktopPrefs::default()
            }
        }
    }

    /// Keep this arrangement at `path`, replacing what is there.
    pub fn save(&self, path: &Path) -> io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        cupertino_defaults::atomic::write_atomic(path, self.text().as_bytes())
    }
}

/// The cells of the items named `names`, given in the order they fill free cells. Kept free, an item takes the cell
/// `prefs` keeps for it while the grid has it and no item before took it; kept sorted, or without a kept cell, an item
/// takes the first free cell. Once every cell is taken the rest pile up on the last one.
pub fn arrange(names: &[&str], prefs: &DesktopPrefs, grid: &Grid) -> Vec<Slot> {
    let mut slots: Vec<Option<Slot>> = vec![None; names.len()];
    let mut taken = HashSet::new();
    if prefs.arrangement == Arrangement::Free {
        for (i, name) in names.iter().enumerate() {
            if let Some(&slot) = prefs.positions.get(*name) {
                if grid.contains(slot) && taken.insert(slot) {
                    slots[i] = Some(slot);
                }
            }
        }
    }
    let mut free = grid
        .order()
        .filter(|s| !taken.contains(s))
        .collect::<Vec<_>>()
        .into_iter();
    slots
        .into_iter()
        .map(|slot| slot.unwrap_or_else(|| free.next().unwrap_or_else(|| grid.last())))
        .collect()
}

/// The cells after the items `moving` (indices of `slots`, the one the pointer holds first) were dragged by `delta`
/// points: each moves as many cells as the held one's middle moved, to the nearest free cell where another item sits.
pub fn dropped(slots: &[Slot], moving: &[usize], delta: Point, grid: &Grid) -> Vec<Slot> {
    let Some(&held) = moving.first() else {
        return slots.to_vec();
    };
    let from = grid.cell(slots[held]).center();
    let to = grid.nearest(Point {
        x: from.x + delta.x,
        y: from.y + delta.y,
    });
    let dc = i64::from(to.column) - i64::from(slots[held].column);
    let dr = i64::from(to.row) - i64::from(slots[held].row);
    let moving_set: HashSet<usize> = moving.iter().copied().collect();
    let mut taken: HashSet<Slot> = slots
        .iter()
        .enumerate()
        .filter(|(i, _)| !moving_set.contains(i))
        .map(|(_, s)| *s)
        .collect();
    let mut out = slots.to_vec();
    for &i in moving {
        let clamp = |v: i64, n: u32| v.clamp(0, i64::from(n) - 1) as u32;
        let target = Slot::new(
            clamp(i64::from(slots[i].column) + dc, grid.columns),
            clamp(i64::from(slots[i].row) + dr, grid.rows),
        );
        let slot = grid.nearest_free(target, &taken).unwrap_or(target);
        taken.insert(slot);
        out[i] = slot;
    }
    out
}

/// The cells after a clean up: the items keep their order (down each column, the columns from right to left) and close
/// up from the first cell.
pub fn cleaned(slots: &[Slot], grid: &Grid) -> Vec<Slot> {
    let mut order: Vec<usize> = (0..slots.len()).collect();
    order.sort_by_key(|&i| (slots[i].column, slots[i].row));
    let mut out = slots.to_vec();
    let mut cells = grid.order();
    for i in order {
        out[i] = cells.next().unwrap_or_else(|| grid.last());
    }
    out
}

/// The item an arrow key moves to from item `from`: the nearest one that way, along the row or column first.
pub fn neighbour(slots: &[Slot], from: usize, direction: Direction) -> Option<usize> {
    let at = slots.get(from)?;
    let (c, r) = (i64::from(at.column), i64::from(at.row));
    slots
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != from)
        .filter_map(|(i, s)| {
            let (dc, dr) = (i64::from(s.column) - c, i64::from(s.row) - r);
            // columns count from the right: the left is a larger column
            let (along, across) = match direction {
                Direction::Up => (-dr, dc),
                Direction::Down => (dr, dc),
                Direction::Left => (dc, dr),
                Direction::Right => (-dc, dr),
            };
            (along > 0).then_some((i, (across.abs(), along)))
        })
        .min_by_key(|&(_, key)| key)
        .map(|(i, _)| i)
}
