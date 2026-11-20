//! The controls shown and their order: what a stored layout gives, and how editing changes it within the grid.

use cupertino_controlcenter::arrangement::Arrangement;
use cupertino_controlcenter::Module;
use cupertino_geometry::Point;

#[test]
fn nothing_stored_shows_every_module_in_reading_order() {
    let all = Arrangement::default();
    assert_eq!(all.shown(), Module::ALL);
    assert!(all.available().is_empty());
    assert!(all.height() > 0.0);
    let ids: Vec<&str> = Module::ALL.iter().map(|m| m.id()).collect();
    assert_eq!(all.ids(), ids);
    assert_eq!(Arrangement::from_ids(&ids), all);
}

#[test]
fn a_stored_layout_skips_unknown_and_repeated_names() {
    let shown = Arrangement::from_ids(&["sound", "wifi", "sound", "teleport"]);
    assert_eq!(shown.shown(), [Module::Sound, Module::WiFi]);
    assert_eq!(shown.available().len(), Module::ALL.len() - 2);
    let empty = Arrangement::from_ids::<&str>(&[]);
    assert_eq!((empty.rows(), empty.height()), (0, 0.0));
    assert!(empty.places().is_empty());
}

#[test]
fn a_module_is_removed_added_after_the_others_and_moved_a_place_at_a_time() {
    let mut shown = Arrangement::from_ids(&["wifi", "bluetooth", "mute"]);
    assert!(shown.remove(Module::Bluetooth));
    assert!(!shown.remove(Module::Bluetooth));
    assert_eq!(shown.shown(), [Module::WiFi, Module::Mute]);
    assert!(shown.add(Module::LockScreen));
    assert!(!shown.add(Module::LockScreen));
    assert_eq!(shown.position(Module::LockScreen), Some(2));
    assert!(shown.step(Module::LockScreen, false));
    assert_eq!(
        shown.shown(),
        [Module::WiFi, Module::LockScreen, Module::Mute]
    );
    assert!(
        !shown.step(Module::WiFi, false),
        "the first place has nothing before it"
    );
    assert!(!shown.move_to(Module::WiFi, 0), "it is there already");
    assert!(shown.move_to(Module::WiFi, 9));
    assert_eq!(shown.position(Module::WiFi), Some(2));
}

#[test]
fn every_shown_module_has_a_tile_and_none_overlap() {
    let all = Arrangement::default();
    let places = all.places();
    assert_eq!(places.len(), Module::ALL.len());
    for (i, (a, ra)) in places.iter().enumerate() {
        assert_eq!(all.frame(*a), Some(*ra));
        for (b, rb) in &places[i + 1..] {
            let apart = ra.x() + ra.width() <= rb.x()
                || rb.x() + rb.width() <= ra.x()
                || ra.y() + ra.height() <= rb.y()
                || rb.y() + rb.height() <= ra.y();
            assert!(apart, "{a:?} {ra:?} overlaps {b:?} {rb:?}");
        }
    }
    assert_eq!(Arrangement::from_ids(&["wifi"]).frame(Module::Sound), None);
}

#[test]
fn a_tile_moved_near_another_place_takes_it() {
    let mut shown = Arrangement::from_ids(&["mute", "lock-screen", "microphone"]);
    // the corner the tile would have at the last place
    let mut moved = shown.clone();
    assert!(moved.move_to(Module::Mute, 2));
    let last = moved.frame(Module::Mute).unwrap();
    let last = Point::new(last.x(), last.y());
    assert!(shown.move_near(Module::Mute, last));
    assert_eq!(shown.position(Module::Mute), Some(2));
    assert!(!shown.move_near(Module::Mute, last));
    assert!(!shown.move_near(Module::Sound, last), "not shown");
}
