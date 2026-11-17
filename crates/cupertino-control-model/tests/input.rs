//! Several sources can hold one key; only the first press and the last release reach the seat.

use cupertino_control_model::{
    Epoch, Input, InputOwners, InputSource, OwnershipError, Transition, MAX_HELD_INPUTS,
};

const PHYSICAL: InputSource = InputSource::Physical(0);

fn controller(n: u64) -> InputSource {
    InputSource::Controller(Epoch::new(n).unwrap())
}

#[test]
fn only_the_first_press_and_the_last_release_reach_the_seat() {
    let key = Input::Key(272);
    let button = Input::Button(272);
    let mut owners = InputOwners::default();
    for (step, source, input, pressed, expected, held) in [
        ("release never pressed", PHYSICAL, key, false, None, 0),
        (
            "first press",
            PHYSICAL,
            key,
            true,
            Some(Transition::Press(key)),
            1,
        ),
        ("repeated press", PHYSICAL, key, true, None, 1),
        ("second holder", controller(1), key, true, None, 2),
        (
            "a button with the key's code is another input",
            PHYSICAL,
            button,
            true,
            Some(Transition::Press(button)),
            3,
        ),
        ("first holder lets go", PHYSICAL, key, false, None, 2),
        ("repeated release", PHYSICAL, key, false, None, 2),
        (
            "last holder lets go",
            controller(1),
            key,
            false,
            Some(Transition::Release(key)),
            1,
        ),
    ] {
        assert_eq!(owners.set(source, input, pressed), Ok(expected), "{step}");
        assert_eq!(owners.held_count(), held, "{step}");
        assert_eq!(owners.holds(source, input), pressed, "{step}");
    }
}

#[test]
fn removing_a_source_releases_only_inputs_nobody_else_holds_in_input_order() {
    let mut owners = InputOwners::default();
    let a = controller(1);
    owners.set(a, Input::Key(50), true).unwrap();
    owners.set(a, Input::Key(10), true).unwrap();
    owners.set(a, Input::Button(5), true).unwrap();
    owners.set(PHYSICAL, Input::Key(50), true).unwrap();
    assert_eq!(
        owners.release_source(a),
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
