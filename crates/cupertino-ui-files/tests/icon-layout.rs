//! The icon view's layout: the size of a cell from the icon, the label and the spacing, the cells flowing into rows
//! (packed or justified) under their section headers, what a point and a rubber band hit, the arrow keys' neighbours,
//! a gallery's strip and the desktop's free grid.

use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_files::geom::Direction;
use cupertino_ui_files::layout::icon::{
    ideal_cell_size, layout, max_title_lines, strip, Flow, FontMetrics, FreeGrid, IconLayout,
    IconParams, SectionSpec, MARGIN,
};
use cupertino_ui_files::metrics::icon_view as limits;

const FONT: FontMetrics = FontMetrics {
    char_width: 7.0,
    line_height: 15.0,
    subtitle_line_height: 12.0,
};

/// Icons of `icon_size` points with the label under them, at the default spacing and text size.
fn params(icon_size: u32) -> IconParams {
    IconParams::new(
        icon_size,
        limits::GRID_SPACING_DEFAULT,
        limits::TEXT_SIZE_DEFAULT,
        true,
        false,
        FONT,
    )
}

fn items(count: usize) -> [SectionSpec; 1] {
    [SectionSpec { title: None, count }]
}

/// A width in which exactly `columns` cells of `p` fit.
fn width_for(p: &IconParams, columns: usize) -> f64 {
    let probe = layout(p, 1000.0, &items(1), Flow::Leading);
    2.0 * MARGIN
        + columns as f64 * probe.cell.width
        + (columns as f64 - 1.0) * probe.spacing.0
        + 1.0
}

fn frames_of(layout: &IconLayout) -> Vec<Rect> {
    layout.cells.iter().map(|cell| cell.frame).collect()
}

#[test]
fn a_cell_follows_the_icon_the_label_and_the_spacing_and_the_inputs_stay_in_range() {
    let bottom = params(64);
    let right = IconParams {
        label_on_bottom: false,
        ..bottom
    };
    let described = IconParams {
        subtitle_enabled: true,
        ..bottom
    };
    let wider = IconParams {
        grid_spacing: 100.0,
        ..bottom
    };
    let (cell, big) = (ideal_cell_size(&bottom), ideal_cell_size(&params(128)));
    assert!(
        big.width > cell.width && big.height > cell.height,
        "a larger icon"
    );
    assert!(
        ideal_cell_size(&right).width > cell.width,
        "a label beside the icon is wider"
    );
    assert!(ideal_cell_size(&right).height < cell.height, "and shorter");
    assert!(
        ideal_cell_size(&described).height > cell.height,
        "a second line of info"
    );
    assert!(
        ideal_cell_size(&wider).width > cell.width,
        "a wider spacing gives the label room"
    );

    let least = IconParams {
        grid_spacing: 1.0,
        ..bottom
    };
    let lines = [
        max_title_lines(&least),
        max_title_lines(&IconParams {
            grid_spacing: 13.0,
            ..bottom
        }),
        max_title_lines(&wider),
    ];
    assert!(lines[0] == 1.0 && lines[2] == 2.0 && lines[0] < lines[1] && lines[1] < lines[2]);

    let clamped = IconParams::new(1, 0, 1, true, false, FONT);
    let most = IconParams::new(u32::MAX, u32::MAX, u16::MAX, true, false, FONT);
    assert_eq!(
        (clamped.icon_size, clamped.grid_spacing, clamped.text_size),
        (
            limits::ICON_SIZE_MIN as f64,
            limits::GRID_SPACING_MIN as f64,
            limits::TEXT_SIZE_MIN as f64
        )
    );
    assert_eq!(
        (most.icon_size, most.grid_spacing, most.text_size),
        (
            limits::ICON_SIZE_MAX as f64,
            limits::GRID_SPACING_MAX as f64,
            limits::TEXT_SIZE_MAX as f64
        )
    );
}

#[test]
fn cells_fill_a_row_across_the_width_and_wrap_to_the_next() {
    let p = params(64);
    // (columns that fit, narrower than one cell)
    for width in [width_for(&p, 1), width_for(&p, 3), width_for(&p, 5), 10.0] {
        let laid = layout(&p, width, &items(7), Flow::Leading);
        let columns = laid.columns;
        assert!(columns >= 1, "at {width}");
        assert_eq!(laid.cells.len(), 7);
        for (k, cell) in laid.cells.iter().enumerate() {
            assert_eq!(cell.index, k);
            let first_of_row = &laid.cells[k - k % columns];
            assert_eq!(
                cell.frame.y(),
                first_of_row.frame.y(),
                "cell {k} at {width}"
            );
            if k % columns > 0 {
                assert!(
                    cell.frame.x() >= laid.cells[k - 1].frame.max_x(),
                    "cell {k}"
                );
            }
            if k >= columns {
                assert!(
                    cell.frame.y() >= laid.cells[k - columns].frame.max_y(),
                    "cell {k}"
                );
            }
        }
        let last = laid.cells.last().unwrap().frame;
        assert!(laid.content.height >= last.max_y(), "at {width}");
        if width > p.icon_box().width {
            assert!(last.max_x() <= width, "at {width}");
        }
    }
    assert_eq!(
        layout(&p, width_for(&p, 3), &items(7), Flow::Leading).columns,
        3
    );
    assert_eq!(layout(&p, 10.0, &items(7), Flow::Leading).columns, 1);
}

#[test]
fn justified_cells_share_the_free_width_evenly_and_packed_cells_start_at_the_margin() {
    let p = params(64);
    let width = width_for(&p, 4) + 90.0;
    let packed = layout(&p, width, &items(4), Flow::Leading);
    let justified = layout(&p, width, &items(4), Flow::Justified);
    assert_eq!(packed.cells[0].frame.x(), MARGIN);
    assert!(justified.cells[0].frame.x() > MARGIN);

    let frames = frames_of(&justified);
    let gap = frames[1].x() - frames[0].max_x();
    for pair in frames.windows(2) {
        assert!((pair[1].x() - pair[0].max_x() - gap).abs() < 1e-9);
    }
    let leading = frames[0].x() - MARGIN;
    let trailing = (width - MARGIN) - frames[3].max_x();
    assert!(
        (leading - trailing).abs() < 1e-9,
        "{leading} against {trailing}"
    );
}

#[test]
fn a_section_with_a_title_has_a_header_above_its_cells_and_the_next_section_starts_below_it() {
    let p = params(64);
    let sections = [
        SectionSpec {
            title: Some("Today".into()),
            count: 5,
        },
        SectionSpec {
            title: None,
            count: 0,
        },
        SectionSpec {
            title: Some("Earlier".into()),
            count: 2,
        },
    ];
    let laid = layout(&p, width_for(&p, 3), &sections, Flow::Leading);
    assert_eq!(laid.cells.len(), 7);
    assert_eq!(
        laid.headers
            .iter()
            .map(|(t, _)| t.as_str())
            .collect::<Vec<_>>(),
        ["Today", "Earlier"]
    );
    let (today, earlier) = (laid.headers[0].1, laid.headers[1].1);
    let first_row_of = |first: usize| laid.cells[first].frame;
    assert!(first_row_of(0).y() >= today.max_y());
    assert!(
        earlier.y()
            >= laid.cells[laid.columns..5]
                .iter()
                .map(|c| c.frame.max_y())
                .fold(0.0, f64::max)
    );
    assert!(first_row_of(5).y() >= earlier.max_y());
    assert!(laid.content.height >= laid.cells[6].frame.max_y());
}

#[test]
fn a_point_hits_the_icon_or_the_name_of_an_item_and_not_the_room_around_them() {
    let p = params(64);
    let laid = layout(&p, width_for(&p, 3), &items(6), Flow::Leading);
    let cell = &laid.cells[4];
    let corner = Point::new(cell.frame.x() + 0.5, cell.frame.y() + 0.5);
    let cases = [
        ("its icon", cell.icon_frame.center(), Some(4)),
        ("its name", cell.label_frame.center(), Some(4)),
        ("the corner of its cell", corner, None),
        ("outside the layout", Point::new(-20.0, -20.0), None),
    ];
    for (what, at, expected) in cases {
        assert_eq!(laid.hit(at), expected, "{what}");
    }

    let (first, second) = (&laid.cells[0], &laid.cells[1]);
    let across = Rect::from_ltrb(
        first.icon_frame.center().x,
        first.icon_frame.center().y,
        second.icon_frame.center().x,
        first.icon_frame.center().y + 1.0,
    );
    assert_eq!(laid.hits_in_rect(&across), [0, 1]);
    let gap = Rect::new(
        first.frame.max_x() + 0.1,
        first.frame.y(),
        0.1,
        first.frame.height(),
    );
    assert!(laid.hits_in_rect(&gap).is_empty());
    assert_eq!(
        laid.hits_in_rect(&Rect::new(0.0, 0.0, 10_000.0, 10_000.0)),
        [0, 1, 2, 3, 4, 5]
    );
}

#[test]
fn an_arrow_key_goes_to_the_nearest_cell_in_its_direction_preferring_its_row_and_column() {
    let p = params(64);
    // three columns: 0 1 2 / 3 4 5 / 6
    let laid = layout(&p, width_for(&p, 3), &items(7), Flow::Leading);
    assert_eq!(laid.columns, 3);
    let cases = [
        (0, Direction::Right, Some(1)),
        (1, Direction::Left, Some(0)),
        (2, Direction::Right, None),
        (0, Direction::Left, None),
        (0, Direction::Down, Some(3)),
        (4, Direction::Up, Some(1)),
        (1, Direction::Up, None),
        (4, Direction::Down, Some(6)),
        (6, Direction::Down, None),
        (99, Direction::Right, None),
    ];
    for (from, direction, expected) in cases {
        assert_eq!(
            laid.neighbour(from, direction),
            expected,
            "{from} {direction:?}"
        );
    }
}

#[test]
fn a_strip_is_one_row_of_pictures_without_names() {
    let p = params(48);
    let laid = strip(&p, 4);
    assert_eq!(laid.cells.len(), 4);
    assert!(laid.cells.windows(2).all(|pair| {
        pair[0].frame.y() == pair[1].frame.y() && pair[1].frame.x() >= pair[0].frame.max_x()
    }));
    assert!(laid.cells.iter().all(|cell| cell.label_frame.is_empty()));
    assert_eq!(laid.hit(laid.cells[2].icon_frame.center()), Some(2));
    assert_eq!(laid.hit(laid.cells[2].label_frame.center()), None);
    assert!(strip(&p, 5).content.width > laid.content.width);
    assert!(strip(&p, 0).cells.is_empty());
}

#[test]
fn the_free_grid_snaps_a_point_to_its_nearest_cell_and_never_outside_it() {
    let grid = FreeGrid {
        bounds: Rect::new(0.0, 0.0, 400.0, 300.0),
        cell: Size::new(100.0, 100.0),
        margin: 0.0,
    };
    assert_eq!(grid.dimensions(), (4, 3));
    let cases = [
        (Point::new(149.0, 10.0), (1, 0)),
        (Point::new(151.0, 10.0), (2, 0)),
        (Point::new(-50.0, -50.0), (0, 0)),
        (Point::new(1.0e6, 1.0e6), (3, 2)),
    ];
    for (at, expected) in cases {
        assert_eq!(grid.nearest(at), expected, "{at:?}");
    }
    for col in 0..4 {
        for row in 0..3 {
            let frame = grid.cell_frame(col, row);
            assert_eq!(grid.nearest(Point::new(frame.x(), frame.y())), (col, row));
        }
    }
    let tiny = FreeGrid {
        bounds: Rect::new(0.0, 0.0, 10.0, 10.0),
        ..grid
    };
    assert_eq!(tiny.dimensions(), (1, 1));
}
