//! Several sources can hold one key; only the first press and the last release reach the seat.

use cupertino_control_model::{
    Epoch, Input, InputOwners, InputSource, OwnershipError, Transition, MAX_HELD_INPUTS,
};

const PHYSICAL: InputSource = InputSource::Physical(0);

fn controller(n: u64) -> InputSource {
    InputSource::Controller(Epoch::new(n).unwrap())
}

#[test]
fn only_the_first_press_and_the_last_release_are_delivered() {
    let mut owners = InputOwners::default();
    let key = Input::Key(30);
    assert_eq!(
        owners.set(PHYSICAL, key, true),
        Ok(Some(Transition::Press(key)))
    );
    assert_eq!(owners.set(controller(1), key, true), Ok(None));
    assert_eq!(owners.held_count(), 2);
    assert_eq!(owners.set(PHYSICAL, key, false), Ok(None));
    assert!(owners.holds(controller(1), key));
    assert!(!owners.holds(PHYSICAL, key));
    assert_eq!(
        owners.set(controller(1), key, false),
        Ok(Some(Transition::Release(key)))
    );
    assert_eq!(owners.held_count(), 0);
}

#[test]
fn repeated_presses_and_releases_change_nothing() {
    let mut owners = InputOwners::default();
    let key = Input::Key(1);
    owners.set(PHYSICAL, key, true).unwrap();
    assert_eq!(owners.set(PHYSICAL, key, true), Ok(None));
    assert_eq!(owners.held_count(), 1);
    owners.set(PHYSICAL, key, false).unwrap();
    assert_eq!(owners.set(PHYSICAL, key, false), Ok(None));
    assert_eq!(owners.held_count(), 0);
}

#[test]
fn releasing_what_was_never_pressed_is_ignored() {
    let mut owners = InputOwners::default();
    assert_eq!(owners.set(PHYSICAL, Input::Button(1), false), Ok(None));
    owners.set(controller(1), Input::Button(1), true).unwrap();
    assert_eq!(owners.set(PHYSICAL, Input::Button(1), false), Ok(None));
    assert!(owners.holds(controller(1), Input::Button(1)));
}

#[test]
fn a_key_and_a_button_with_one_code_are_different_inputs() {
    let mut owners = InputOwners::default();
    owners.set(PHYSICAL, Input::Key(272), true).unwrap();
    assert_eq!(
        owners.set(PHYSICAL, Input::Button(272), true),
        Ok(Some(Transition::Press(Input::Button(272))))
    );
    assert_eq!(owners.held_count(), 2);
}

#[test]
fn removing_a_source_releases_only_inputs_nobody_else_holds_in_input_order() {
    let mut owners = InputOwners::default();
    let a = controller(1);
    owners.set(a, Input::Key(50), true).unwrap();
    owners.set(a, Input::Key(10), true).unwrap();
    owners.set(a, Input::Button(5), true).unwrap();
    owners.set(PHYSICAL, Input::Key(50), true).unwrap();
    let releases = owners.release_source(a);
    assert_eq!(
        releases,
        vec![
            Transition::Release(Input::Key(10)),
            Transition::Release(Input::Button(5)),
        ]
    );
    assert_eq!(owners.held_count(), 1);
    assert!(owners.holds(PHYSICAL, Input::Key(50)));
    assert!(owners.release_source(a).is_empty());
}

#[test]
fn has_buttons_looks_at_buttons_of_that_source_only() {
    let mut owners = InputOwners::default();
    owners.set(controller(1), Input::Key(1), true).unwrap();
    owners.set(controller(2), Input::Button(1), true).unwrap();
    assert!(!owners.has_buttons(controller(1)));
    assert!(owners.has_buttons(controller(2)));
    owners.set(controller(2), Input::Button(1), false).unwrap();
    assert!(!owners.has_buttons(controller(2)));
}

#[test]
fn the_table_is_bounded_and_frees_room_on_release() {
    let mut owners = InputOwners::default();
    for code in 0..MAX_HELD_INPUTS as u32 {
        owners.set(PHYSICAL, Input::Key(code), true).unwrap();
    }
    assert_eq!(owners.held_count(), MAX_HELD_INPUTS);
    assert_eq!(
        owners.set(PHYSICAL, Input::Key(u32::MAX), true),
        Err(OwnershipError::Full)
    );
    assert_eq!(
        owners.set(PHYSICAL, Input::Key(0), true),
        Ok(None),
        "a press already held needs no room"
    );
    owners.set(PHYSICAL, Input::Key(0), false).unwrap();
    assert!(owners.set(PHYSICAL, Input::Key(u32::MAX), true).is_ok());
}
