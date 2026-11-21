//! Type-to-select: the next item a typed text finds and when a pause starts the text over.

use cupertino_ui_controls::typeahead::{TypeAhead, RESET_AFTER};

#[test]
fn typing_finds_the_next_item_that_starts_with_it_and_a_pause_starts_the_text_over() {
    let titles = [
        Some("Apricot"),
        Some("Avocado"),
        None,
        Some("Banana"),
        Some("Blueberry"),
        Some("cherry"),
    ];
    // each character with the time it was typed and the item highlighted when it was, and what it finds
    type Script<'a> = (&'a str, &'a [(char, f64, usize, Option<usize>)]);
    let scripts: [Script; 7] = [
        (
            "a first letter looks after the highlighted item",
            &[('b', 0.0, 0, Some(3))],
        ),
        (
            "more letters narrow the search from the highlighted item",
            &[('b', 0.0, 0, Some(3)), ('l', 0.2, 3, Some(4))],
        ),
        (
            "the same letter again goes on to the next item it starts",
            &[
                ('a', 0.0, 3, Some(0)),
                ('a', 0.1, 0, Some(1)),
                ('a', 0.2, 1, Some(0)),
            ],
        ),
        (
            "capitals do not matter and an item with no title is passed over",
            &[('C', 0.0, 0, Some(5))],
        ),
        ("nothing starts with it", &[('z', 0.0, 0, None)]),
        (
            "a pause ends the text",
            &[('b', 0.0, 0, Some(3)), ('l', 2.0, 3, None)],
        ),
        (
            "a character within the pause does not",
            &[('b', 0.0, 0, Some(3)), ('l', RESET_AFTER, 3, Some(4))],
        ),
    ];
    for (what, script) in scripts {
        let mut typed = TypeAhead::default();
        for (c, at, from, expected) in script {
            assert_eq!(
                typed.find(*c, *at, *from, &titles),
                *expected,
                "{what}: {c} at {at}"
            );
        }
    }

    let mut typed = TypeAhead::default();
    assert!(!typed.typing(0.0));
    typed.find('b', 5.0, 0, &titles);
    assert!(typed.typing(5.0 + RESET_AFTER));
    assert!(!typed.typing(5.0 + RESET_AFTER + 0.1));
}
