//! Packing and eviction of an atlas, without textures.

use cupertino_gpu::atlas::{AtlasConfig, AtlasModel, Slot, COLOR, COVERAGE, GAP, PATH_MASK};
use cupertino_gpu::{Error, Region, TextureFormat};

fn config(page: u32, max_pages: usize) -> AtlasConfig {
    AtlasConfig {
        format: TextureFormat::R8,
        page_size: (page, page),
        max_pages,
    }
}

fn slot(page: usize, x: u32, y: u32, width: u32, height: u32) -> Slot {
    Slot {
        page,
        region: Region::new(x, y, width, height),
    }
}

#[test]
fn the_atlases_of_a_window_hold_sixteen_mebibytes_in_pages_of_the_drivers_granule() {
    for config in [COVERAGE, COLOR, PATH_MASK] {
        assert_eq!(config.page_bytes() % (2 << 20), 0, "{config:?}");
    }
    assert_eq!(
        COVERAGE.cap_bytes() + COLOR.cap_bytes() + PATH_MASK.cap_bytes(),
        16 << 20
    );
}

#[test]
fn entries_are_packed_on_the_lowest_shelf_that_fits_with_a_gap_and_pages_added_up_to_the_cap() {
    for (name, config, earlier, size, expected, new_page) in [
        (
            "the first at the corner",
            COVERAGE,
            &[][..],
            (10, 10),
            slot(0, 0, 0, 10, 10),
            true,
        ),
        (
            "beside the last with a gap",
            COVERAGE,
            &[(10, 10)][..],
            (10, 10),
            slot(0, 10 + GAP, 0, 10, 10),
            false,
        ),
        (
            "a shorter one on a shelf tall enough",
            COVERAGE,
            &[(10, 10)][..],
            (10, 5),
            slot(0, 11, 0, 10, 5),
            false,
        ),
        (
            "a taller one on a new shelf",
            COVERAGE,
            &[(10, 10), (10, 5)][..],
            (10, 20),
            slot(0, 0, 10 + GAP, 10, 20),
            false,
        ),
        (
            "the lower of two shelves with room",
            config(64, 1),
            &[(10, 10), (10, 20)][..],
            (10, 5),
            slot(0, 11, 0, 10, 5),
            false,
        ),
        (
            "past the row's end on the next shelf",
            config(32, 1),
            &[(20, 10)][..],
            (20, 10),
            slot(0, 0, 11, 20, 10),
            false,
        ),
        (
            "the largest of a page leaves the gap",
            config(16, 1),
            &[][..],
            (15, 15),
            slot(0, 0, 0, 15, 15),
            true,
        ),
        (
            "a full page adds a page",
            config(16, 2),
            &[(15, 15)][..],
            (15, 15),
            slot(1, 0, 0, 15, 15),
            true,
        ),
    ] {
        let mut model = AtlasModel::new(config);
        assert!(model.is_empty(), "{name}");
        for (key, size) in (1..).zip(earlier) {
            model.insert(key, *size, 0).unwrap();
        }
        let placement = model.insert(100, size, 0).unwrap();
        assert_eq!(
            (placement.slot, placement.new_page, placement.evicted),
            (expected, new_page, false),
            "{name}"
        );
        assert_eq!(model.len(), earlier.len() + 1, "{name}");
        assert_eq!(model.pages(), expected.page + 1, "{name}");
        assert_eq!(model.get(100, 0), Some(expected), "{name}");
    }
}

#[test]
fn an_entry_that_is_empty_or_larger_than_a_page_less_the_gap_is_refused_without_overflow() {
    for (config, size) in [
        (COVERAGE, (0, 5)),
        (COVERAGE, (5, 0)),
        (config(16, 1), (16, 1)),
        (config(16, 1), (1, 16)),
        // an entry of u32::MAX pixels once overflowed the page-size check
        (COVERAGE, (u32::MAX, 1)),
        (COVERAGE, (1, u32::MAX)),
        (COVERAGE, (u32::MAX, u32::MAX)),
        (COVERAGE, (u32::MAX - 1, 5)),
    ] {
        let mut model = AtlasModel::new(config);
        assert!(
            matches!(model.insert(1, size, 0), Err(Error::Invalid(_))),
            "{size:?}"
        );
        assert!(model.is_empty(), "{size:?}");
    }
}

#[test]
fn at_the_cap_the_page_used_least_recently_is_emptied_and_reused() {
    // (frame of a get of key 1 before the third insert, the page reused)
    for (touch, reused) in [(None, 0), (Some(4), 1)] {
        let mut model = AtlasModel::new(config(16, 2));
        model.insert(1, (15, 15), 1).unwrap();
        model.insert(2, (15, 15), 2).unwrap();
        if let Some(frame) = touch {
            assert!(model.get(1, frame).is_some());
        }
        let third = model.insert(3, (15, 15), 5).unwrap();
        assert_eq!(third.slot, slot(reused, 0, 0, 15, 15), "{touch:?}");
        assert!(third.evicted && !third.new_page, "{touch:?}");
        assert_eq!(model.pages(), 2);
        let (evicted, kept) = if reused == 0 { (1, 2) } else { (2, 1) };
        assert_eq!(model.get(evicted, 5), None, "{touch:?}");
        assert_eq!(
            model.get(kept, 5),
            Some(slot(1 - reused, 0, 0, 15, 15)),
            "{touch:?}"
        );
        assert_eq!(model.get(3, 5), Some(third.slot), "{touch:?}");
    }
}

#[test]
fn when_every_page_was_used_in_the_current_frame_the_insert_is_over_budget_and_nothing_is_lost() {
    let mut model = AtlasModel::new(config(16, 2));
    model.insert(1, (15, 15), 3).unwrap();
    model.insert(2, (15, 15), 3).unwrap();
    match model.insert(3, (15, 15), 3) {
        Err(Error::OverBudget { what, need, cap }) => {
            assert_eq!((what, need, cap), ("atlas", 225, 512));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(model.len(), 2);
    assert!(model.get(1, 3).is_some() && model.get(2, 3).is_some());
    // the next frame may evict
    assert!(model.insert(3, (15, 15), 4).unwrap().evicted);

    // a get from an older frame does not lower the use of a page
    let mut model = AtlasModel::new(config(16, 1));
    model.insert(1, (15, 15), 9).unwrap();
    model.get(1, 2);
    assert!(matches!(
        model.insert(2, (15, 15), 9),
        Err(Error::OverBudget { .. })
    ));
}

#[test]
fn inserting_a_key_again_replaces_its_entry_and_leaves_its_old_slot_out_of_the_books() {
    let mut model = AtlasModel::new(COVERAGE);
    let first = model.insert(1, (10, 10), 0).unwrap().slot;
    let again = model.insert(1, (20, 20), 0).unwrap().slot;
    assert_ne!(first, again);
    assert_eq!(model.len(), 1);
    assert_eq!(model.get(1, 0), Some(again));

    let mut model = AtlasModel::new(config(16, 2));
    model.insert(1, (15, 15), 1).unwrap();
    model.insert(2, (15, 15), 2).unwrap();
    // key 2 moves to the other page by eviction of page 0
    let moved = model.insert(2, (15, 15), 5).unwrap();
    assert_eq!(model.get(2, 5), Some(moved.slot));
    model.insert(9, (15, 15), 6).unwrap();
    assert_eq!(model.len(), 2);
}

#[test]
fn clearing_forgets_every_entry_and_page() {
    let mut model = AtlasModel::new(config(16, 2));
    assert_eq!(model.get(42, 0), None);
    model.insert(1, (15, 15), 0).unwrap();
    model.insert(2, (15, 15), 0).unwrap();
    model.clear();
    assert!(model.is_empty());
    assert_eq!(model.pages(), 0);
    assert_eq!(model.get(1, 0), None);
    let again = model.insert(1, (15, 15), 0).unwrap();
    assert!(again.new_page);
    assert_eq!(again.slot.page, 0);
}
