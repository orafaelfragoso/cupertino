use super::*;

#[test]
fn controls_become_spaces_and_bidi_marks_go() {
    assert_eq!(
        shown("Mount\tthe\ndisk\u{1b}[31m \u{202E}gpj.exe\u{2069}"),
        "Mount the disk [31m gpj.exe"
    );
}

#[test]
fn a_long_string_is_cut_with_an_ellipsis() {
    let long = "a".repeat(MAX_CHARS + 10);
    let cut = shown(&long);
    assert_eq!(cut.chars().count(), MAX_CHARS);
    assert!(cut.ends_with('…'));
    assert_eq!(shown(&"b".repeat(MAX_CHARS)), "b".repeat(MAX_CHARS));
}
