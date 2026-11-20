//! The desktop's arrangement through its public interface: the grid's cells, where items go, what a drag, a clean up
//! and the arrow keys do, and what is kept between sessions.

#![expect(
    clippy::disallowed_methods,
    reason = "the tests read and write their own files, with no UI loop to keep free"
)]

use std::collections::{BTreeMap, HashSet};

use cupertino_kit::files::geom::Direction;
use cupertino_kit::files::sort::SortKey;
use cupertino_kit::geometry::{Point, Rect};
use cupertino_shell_desktop::arrangement::{
    arrange, backdrop_radius, cleaned, dropped, item_frames, neighbour, Arrangement, DesktopPrefs,
    Grid, Slot,
};
use cupertino_shell_desktop::metrics as m;

/// The menu bar's room in these tests, in points.
const TOP: f64 = 24.0;

/// A 640 × 400 desktop: six columns of three rows.
fn grid() -> Grid {
    Grid::new(640.0, 400.0, TOP)
}

fn s(column: u32, row: u32) -> Slot {
    Slot::new(column, row)
}

#[test]
fn the_grid_fits_as_many_cells_as_the_output_holds_below_the_menu_bar_and_one_at_least() {
    for (width, height, columns, rows) in [
        (640.0, 400.0, 6, 3),
        (1920.0, 1080.0, 19, 9),
        (50.0, 50.0, 1, 1),
    ] {
        let grid = Grid::new(width, height, TOP);
        assert_eq!(
            (grid.columns(), grid.rows()),
            (columns, rows),
            "{width} × {height}"
        );
    }
}

#[test]
fn column_zero_is_at_the_right_and_row_zero_under_the_menu_bar() {
    let grid = grid();
    let first = grid.cell(s(0, 0));
    assert_eq!(
        first,
        Rect::new(
            640.0 - m::RIGHT_MARGIN - m::COLUMN_PITCH,
            TOP + m::TOP_MARGIN,
            m::COLUMN_PITCH,
            m::ROW_PITCH
        )
    );
    assert_eq!(grid.cell(s(1, 2)).x(), first.x() - m::COLUMN_PITCH);
    assert_eq!(grid.cell(s(1, 2)).y(), first.y() + 2.0 * m::ROW_PITCH);
    for slot in grid.order() {
        assert_eq!(grid.nearest(grid.cell(slot).center()), slot);
    }
    // a point beyond the grid lands on its edge
    assert_eq!(grid.nearest(Point::new(-500.0, 900.0)), s(5, 2));
    assert!(grid.contains(s(5, 2)) && !grid.contains(s(6, 0)) && !grid.contains(s(0, 3)));
}

#[test]
fn items_fill_each_column_downwards_then_the_next_one_to_its_left() {
    let order: Vec<Slot> = grid().order().take(5).collect();
    assert_eq!(order, [s(0, 0), s(0, 1), s(0, 2), s(1, 0), s(1, 1)]);
}

#[test]
fn the_nearest_free_cell_is_the_cell_itself_when_free_else_its_closest_neighbour() {
    let grid = grid();
    assert_eq!(grid.nearest_free(s(2, 1), &HashSet::new()), Some(s(2, 1)));
    let taken = HashSet::from([s(2, 1)]);
    // a row (106) is farther than a column (96)
    assert_eq!(grid.nearest_free(s(2, 1), &taken), Some(s(1, 1)));
    let all: HashSet<Slot> = grid.order().collect();
    assert_eq!(grid.nearest_free(s(0, 0), &all), None);
}

#[test]
fn an_item_takes_its_kept_cell_and_the_rest_take_the_first_free_ones() {
    let grid = grid();
    let kept = |pairs: &[(&str, Slot)]| DesktopPrefs {
        arrangement: Arrangement::Free,
        positions: pairs
            .iter()
            .map(|(n, slot)| (n.to_string(), *slot))
            .collect(),
    };
    let cases: [(&str, DesktopPrefs, Vec<Slot>); 5] = [
        (
            "nothing kept",
            DesktopPrefs::default(),
            vec![s(0, 0), s(0, 1), s(0, 2)],
        ),
        (
            "b kept at the second column",
            kept(&[("b", s(1, 0))]),
            vec![s(0, 0), s(1, 0), s(0, 1)],
        ),
        (
            "a and c kept in one cell: a has it",
            kept(&[("a", s(0, 1)), ("c", s(0, 1))]),
            vec![s(0, 1), s(0, 0), s(0, 2)],
        ),
        (
            "a kept outside the grid",
            kept(&[("a", s(9, 9))]),
            vec![s(0, 0), s(0, 1), s(0, 2)],
        ),
        (
            "kept sorted: the kept cells are not used",
            DesktopPrefs {
                arrangement: Arrangement::Sorted(SortKey::Name),
                positions: kept(&[("a", s(3, 2))]).positions,
            },
            vec![s(0, 0), s(0, 1), s(0, 2)],
        ),
    ];
    for (what, prefs, slots) in cases {
        assert_eq!(arrange(&["a", "b", "c"], &prefs, &grid), slots, "{what}");
    }
}

#[test]
fn items_beyond_the_last_free_cell_pile_up_on_the_last_cell() {
    let grid = Grid::new(200.0, 260.0, TOP);
    assert_eq!((grid.columns(), grid.rows()), (2, 2));
    let slots = arrange(
        &["a", "b", "c", "d", "e", "f"],
        &DesktopPrefs::default(),
        &grid,
    );
    assert_eq!(
        slots,
        [s(0, 0), s(0, 1), s(1, 0), s(1, 1), s(1, 1), s(1, 1)]
    );
}

#[test]
fn dragged_items_move_as_many_cells_as_the_held_one_and_avoid_the_others() {
    let grid = grid();
    let slots = [s(0, 0), s(0, 1), s(0, 2), s(1, 0)];
    let left = Point::new(-m::COLUMN_PITCH, 0.0);
    let down = Point::new(0.0, m::ROW_PITCH);
    let cases: [(&str, Vec<usize>, Point, Vec<Slot>); 5] = [
        ("one a column left onto a free cell", vec![1], left, vec![s(0, 0), s(1, 1), s(0, 2), s(1, 0)]),
        // (0, 0) and (2, 0) are as near: the first in the order cells fill wins
        ("one a column left onto a taken cell", vec![0], left, vec![s(0, 0), s(0, 1), s(0, 2), s(1, 0)]),
        // a column away is nearer than a row away
        ("one a cell up and left onto a taken cell", vec![1], Point::new(-m::COLUMN_PITCH, -m::ROW_PITCH), vec![s(0, 0), s(2, 0), s(0, 2), s(1, 0)]),
        (
            "two together a row down: the held one moves, the other one's cell is taken so it keeps the nearest free",
            vec![3, 0],
            down,
            vec![s(0, 0), s(0, 1), s(0, 2), s(1, 1)],
        ),
        ("nothing held", vec![], left, slots.to_vec()),
    ];
    for (what, moving, delta, after) in cases {
        assert_eq!(dropped(&slots, &moving, delta, &grid), after, "{what}");
    }
    // a drag past the edge stops at it
    assert_eq!(
        dropped(&slots, &[3], Point::new(-5000.0, 0.0), &grid)[3],
        s(5, 0)
    );
}

#[test]
fn a_clean_up_closes_the_gaps_keeping_the_order_down_each_column() {
    let grid = grid();
    let slots = [s(3, 2), s(0, 2), s(1, 0), s(0, 0)];
    assert_eq!(cleaned(&slots, &grid), [s(1, 0), s(0, 1), s(0, 2), s(0, 0)]);
}

#[test]
fn the_arrow_keys_move_to_the_nearest_item_that_way() {
    //   column: 2      1      0
    //   row 0:  d      c      a
    //   row 1:                b
    let slots = [s(0, 0), s(0, 1), s(1, 0), s(2, 0)];
    for (from, direction, to) in [
        (0, Direction::Down, Some(1)),
        (1, Direction::Up, Some(0)),
        (0, Direction::Left, Some(2)),
        (2, Direction::Left, Some(3)),
        (3, Direction::Right, Some(2)),
        (1, Direction::Left, Some(2)),
        (0, Direction::Right, None),
        (0, Direction::Up, None),
        (9, Direction::Up, None),
    ] {
        assert_eq!(
            neighbour(&slots, from, direction),
            to,
            "{from} {direction:?}"
        );
    }
}

#[test]
fn an_item_is_its_icon_on_a_padded_backdrop_with_its_name_centred_below() {
    let cell = grid().cell(s(0, 0));
    let frames = item_frames(cell);
    assert_eq!(frames.icon.width(), m::ICON_SIZE);
    assert_eq!(frames.icon.height(), m::ICON_SIZE);
    assert_eq!(
        frames.backdrop.width(),
        m::ICON_SIZE + 2.0 * m::BACKDROP_PAD
    );
    assert_eq!(frames.backdrop.center().x, cell.center().x);
    assert_eq!(frames.backdrop.y(), cell.y() + m::ICON_TOP);
    assert_eq!(frames.label_top, frames.backdrop.max_y() + m::LABEL_GAP);
    assert_eq!(frames.label_center, cell.center().x);
    assert_eq!(
        backdrop_radius(),
        m::BACKDROP_RADIUS_MAX.min(m::ICON_SIZE * m::BACKDROP_RADIUS_RATIO)
    );
}

#[test]
fn the_kept_arrangement_reads_back_what_was_written_and_defaults_what_is_malformed() {
    let prefs = DesktopPrefs {
        arrangement: Arrangement::Sorted(SortKey::DateModified),
        positions: BTreeMap::from([
            ("notes.txt".to_string(), s(2, 1)),
            ("Résumé.pdf".into(), s(0, 0)),
        ]),
    };
    assert_eq!(DesktopPrefs::parse(&prefs.text()), prefs);
    for (text, arrangement, positions) in [
        ("", Arrangement::Free, 0),
        ("not json", Arrangement::Free, 0),
        (r#"{"sort": "size"}"#, Arrangement::Sorted(SortKey::Size), 0),
        (
            r#"{"sort": "kind", "positions": []}"#,
            Arrangement::Sorted(SortKey::Kind),
            0,
        ),
        (
            r#"{"sort": 4, "positions": {"a": [1, 2], "b": [1], "c": "x", "d": [-1, 0]}}"#,
            Arrangement::Free,
            1,
        ),
        (
            r#"{"sort": null, "positions": {"a": [0, 0]}}"#,
            Arrangement::Free,
            1,
        ),
    ] {
        let read = DesktopPrefs::parse(text);
        assert_eq!(read.arrangement, arrangement, "{text}");
        assert_eq!(read.positions.len(), positions, "{text}");
    }
    assert_eq!(
        DesktopPrefs::parse(r#"{"positions": {"a": [1, 2]}}"#).positions["a"],
        s(1, 2)
    );
}

#[test]
fn sizes_and_dates_sort_from_the_largest_and_newest_names_and_kinds_from_a() {
    for (key, ascending) in [
        (SortKey::Name, true),
        (SortKey::Kind, true),
        (SortKey::DateModified, false),
        (SortKey::Size, false),
    ] {
        assert_eq!(Arrangement::ascending(key), ascending, "{key:?}");
    }
}

#[test]
fn the_kept_arrangement_is_written_in_place_and_read_back_from_its_file() {
    let dir = std::env::temp_dir().join(format!(
        "cupertino-shell-desktop-prefs-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("cupertino/files/desktop.json");
    assert_eq!(DesktopPrefs::load(&file), DesktopPrefs::default());
    let prefs = DesktopPrefs {
        arrangement: Arrangement::Free,
        positions: BTreeMap::from([("Drafts".to_string(), s(1, 0))]),
    };
    prefs.save(&file).expect("the arrangement is written");
    assert_eq!(DesktopPrefs::load(&file), prefs);
    let replaced = DesktopPrefs::default();
    replaced
        .save(&file)
        .expect("the arrangement is written again");
    assert_eq!(DesktopPrefs::load(&file), replaced);
    std::fs::remove_dir_all(&dir).expect("the test's folder goes");
}
