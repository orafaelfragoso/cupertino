//! A decoded layout is parsed from the text it is given.

use cupertino_keyboard::Layout;

const LAYOUT: &str = r#"{
    "name": "Test",
    "id": 7,
    "sets": [{
        "first": 0,
        "last": 0,
        "modmap": [0, 0, 1],
        "default_table": 0,
        "tables": [
            [{"c": "a"}, {"c": "s"}],
            [{"c": "A"}, {"c": "S"}]
        ]
    }]
}"#;

#[test]
fn layout_parses_from_text() {
    let layout = Layout::from_json(LAYOUT).expect("layout parses");
    assert_eq!(layout.name, "Test");
    assert_eq!(layout.id, 7);
    assert_eq!(layout.language, "");
    assert_eq!(layout.sets.len(), 1);
    let set = &layout.sets[0];
    assert_eq!(set.translate(1, 0, 0).text, "s");
    assert_eq!(set.translate(0, 2, 0).text, "A");
}

#[test]
fn malformed_layout_text_is_an_error() {
    assert!(Layout::from_json("{\"name\": \"Test\"}").is_err());
    assert!(Layout::from_json("not json").is_err());
}
