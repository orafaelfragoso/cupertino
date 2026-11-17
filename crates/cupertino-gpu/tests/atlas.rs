//! Packing and eviction of an atlas, without textures.

use cupertino_gpu::atlas::{
    AtlasConfig, AtlasKind, AtlasModel, Slot, COLOR, COVERAGE, GAP, PATH_MASK,
};
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
fn the_atlases_of_a_window_hold_sixteen_mebibytes_together() {
    assert_eq!(COVERAGE.cap_bytes(), 4 << 20);
    assert_eq!(COLOR.cap_bytes(), 8 << 20);
    assert_eq!(PATH_MASK.cap_bytes(), 4 << 20);
    assert_eq!(
        COVERAGE.cap_bytes() + COLOR.cap_bytes() + PATH_MASK.cap_bytes(),
        16 << 20
    );
}

#[test]
fn a_page_is_a_multiple_of_the_granule_the_driver_rounds_to() {
    for config in [COVERAGE, COLOR, PATH_MASK] {
        assert_eq!(config.page_bytes() % (2 << 20), 0);
    }
    assert_eq!(COVERAGE.page_bytes(), 2 << 20);
    assert_eq!(COLOR.page_bytes(), 4 << 20);
}

#[test]
fn every_kind_has_the_config_of_its_name() {
    assert_eq!(AtlasKind::Coverage.config(), COVERAGE);
    assert_eq!(AtlasKind::Color.config(), COLOR);
    assert_eq!(AtlasKind::PathMask.config(), PATH_MASK);
    assert_eq!(COLOR.format, TextureFormat::Rgba8);
    assert_eq!(COVERAGE.format, TextureFormat::R8);
}

#[test]
fn a_new_model_is_empty() {
    let model = AtlasModel::new(COVERAGE);
    assert!(model.is_empty());
    assert_eq!((model.len(), model.pages()), (0, 0));
    assert_eq!(model.config(), COVERAGE);
}

#[test]
fn the_first_entry_starts_the_first_page_at_its_corner() {
    let mut model = AtlasModel::new(COVERAGE);
    let placement = model.insert(1, (10, 10), 0).unwrap();
    assert_eq!(placement.slot, slot(0, 0, 0, 10, 10));
    assert!(placement.new_page);
    assert!(!placement.evicted);
    assert_eq!((model.len(), model.pages()), (1, 1));
}

#[test]
fn entries_share_a_shelf_with_a_gap_between_them() {
    let mut model = AtlasModel::new(COVERAGE);
    model.insert(1, (10, 10), 0).unwrap();
    let second = model.insert(2, (10, 10), 0).unwrap();
    assert_eq!(second.slot, slot(0, 10 + GAP, 0, 10, 10));
    assert!(!second.new_page);
}

#[test]
fn a_shorter_entry_goes_on_a_shelf_that_is_tall_enough_and_a_taller_one_starts_a_new_shelf() {
    let mut model = AtlasModel::new(COVERAGE);
    model.insert(1, (10, 10), 0).unwrap();
    let short = model.insert(2, (10, 5), 0).unwrap();
    assert_eq!(short.slot.region, Region::new(11, 0, 10, 5));
    let tall = model.insert(3, (10, 20), 0).unwrap();
    assert_eq!(tall.slot.region, Region::new(0, 10 + GAP, 10, 20));
}

#[test]
fn the_lowest_shelf_that_fits_is_used() {
    let mut model = AtlasModel::new(config(64, 1));
    // a shelf of height 11, then one of height 21 below it
    model.insert(1, (10, 10), 0).unwrap();
    let tall = model.insert(2, (10, 20), 0).unwrap();
    assert_eq!(tall.slot.region.y, 11);
    // both shelves have room: the lower one takes it
    let placed = model.insert(3, (10, 5), 0).unwrap();
    assert_eq!(placed.slot.region, Region::new(11, 0, 10, 5));
}

#[test]
fn an_entry_that_does_not_fit_the_row_starts_the_next_shelf() {
    let mut model = AtlasModel::new(config(32, 1));
    model.insert(1, (20, 10), 0).unwrap();
    let next = model.insert(2, (20, 10), 0).unwrap();
    assert_eq!(next.slot.region, Region::new(0, 11, 20, 10));
}

#[test]
fn the_largest_entry_of_a_page_leaves_exactly_the_gap() {
    let mut model = AtlasModel::new(config(16, 1));
    let placed = model.insert(1, (15, 15), 0).unwrap();
    assert_eq!(placed.slot.region, Region::new(0, 0, 15, 15));
    assert!(matches!(
        model.insert(2, (16, 1), 0),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        model.insert(3, (1, 16), 0),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn an_empty_entry_is_invalid() {
    let mut model = AtlasModel::new(COVERAGE);
    assert!(matches!(model.insert(1, (0, 5), 0), Err(Error::Invalid(_))));
    assert!(matches!(model.insert(1, (5, 0), 0), Err(Error::Invalid(_))));
    assert!(model.is_empty());
}

#[test]
fn an_entry_of_any_size_is_checked_without_overflow() {
    let mut model = AtlasModel::new(COVERAGE);
    for size in [
        (u32::MAX, 1),
        (1, u32::MAX),
        (u32::MAX, u32::MAX),
        (u32::MAX - 1, 5),
    ] {
        assert!(
            matches!(model.insert(1, size, 0), Err(Error::Invalid(_))),
            "{size:?}"
        );
    }
    assert!(model.is_empty());
}

#[test]
fn a_full_page_adds_a_page_until_the_cap() {
    let mut model = AtlasModel::new(config(16, 2));
    assert_eq!(model.insert(1, (15, 15), 0).unwrap().slot.page, 0);
    let second = model.insert(2, (15, 15), 0).unwrap();
    assert_eq!(second.slot, slot(1, 0, 0, 15, 15));
    assert!(second.new_page);
    assert_eq!(model.pages(), 2);
}

#[test]
fn at_the_cap_the_page_used_least_recently_is_emptied_and_reused() {
    let mut model = AtlasModel::new(config(16, 2));
    model.insert(1, (15, 15), 1).unwrap();
    model.insert(2, (15, 15), 2).unwrap();
    let third = model.insert(3, (15, 15), 5).unwrap();
    assert_eq!(third.slot, slot(0, 0, 0, 15, 15));
    assert!(third.evicted);
    assert!(!third.new_page);
    assert_eq!(model.get(1, 5), None, "the evicted entry is gone");
    assert_eq!(model.get(2, 5), Some(slot(1, 0, 0, 15, 15)));
    assert_eq!(model.get(3, 5), Some(slot(0, 0, 0, 15, 15)));
    assert_eq!(model.pages(), 2);
}

#[test]
fn a_get_marks_the_page_used_so_the_other_is_evicted() {
    let mut model = AtlasModel::new(config(16, 2));
    model.insert(1, (15, 15), 1).unwrap();
    model.insert(2, (15, 15), 2).unwrap();
    assert!(model.get(1, 4).is_some());
    let third = model.insert(3, (15, 15), 5).unwrap();
    assert_eq!(third.slot.page, 1);
    assert!(third.evicted);
    assert!(model.get(1, 5).is_some());
    assert_eq!(model.get(2, 5), None);
}

#[test]
fn when_every_page_was_used_in_the_current_frame_the_insert_is_over_budget() {
    let mut model = AtlasModel::new(config(16, 2));
    model.insert(1, (15, 15), 3).unwrap();
    model.insert(2, (15, 15), 3).unwrap();
    match model.insert(3, (15, 15), 3) {
        Err(Error::OverBudget { what, need, cap }) => {
            assert_eq!(what, "atlas");
            assert_eq!(need, 225);
            assert_eq!(cap, 512);
        }
        other => panic!("{other:?}"),
    }
    // nothing was lost
    assert_eq!(model.len(), 2);
    assert!(model.get(1, 3).is_some() && model.get(2, 3).is_some());
    // the next frame may evict
    assert!(model.insert(3, (15, 15), 4).unwrap().evicted);
}

#[test]
fn an_older_frame_does_not_lower_the_use_of_a_page() {
    let mut model = AtlasModel::new(config(16, 1));
    model.insert(1, (15, 15), 9).unwrap();
    model.get(1, 2);
    assert!(matches!(
        model.insert(2, (15, 15), 9),
        Err(Error::OverBudget { .. })
    ));
}

#[test]
fn inserting_a_key_again_replaces_its_entry() {
    let mut model = AtlasModel::new(COVERAGE);
    let first = model.insert(1, (10, 10), 0).unwrap().slot;
    let again = model.insert(1, (20, 20), 0).unwrap().slot;
    assert_ne!(first, again);
    assert_eq!(model.len(), 1);
    assert_eq!(model.get(1, 0), Some(again));
}

#[test]
fn a_replaced_key_is_not_evicted_with_its_old_page() {
    let mut model = AtlasModel::new(config(16, 2));
    model.insert(1, (15, 15), 1).unwrap();
    model.insert(2, (15, 15), 2).unwrap();
    // key 2 moves to the other page by eviction of page 0; its first slot is gone from the books
    let moved = model.insert(2, (15, 15), 5).unwrap();
    assert_eq!(model.get(2, 5), Some(moved.slot));
    model.insert(9, (15, 15), 6).unwrap();
    assert_eq!(model.len(), 2);
}

#[test]
fn clearing_forgets_every_entry_and_page() {
    let mut model = AtlasModel::new(config(16, 2));
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

#[test]
fn an_unknown_key_is_not_found() {
    let mut model = AtlasModel::new(COVERAGE);
    assert_eq!(model.get(42, 0), None);
}
