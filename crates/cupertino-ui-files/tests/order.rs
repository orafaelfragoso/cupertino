//! The order of names (case, numbers, accents and width) and of items under a sort: each key both ways, folders on
//! top, ties by name, where a new item is inserted, and the text typing to select matches.

use std::cmp::Ordering;
use std::time::{Duration, UNIX_EPOCH};

use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::natural::{compare_names, type_select_key};
use cupertino_ui_files::sort::{insertion_index, sort_entries, SortKey, SortSpec};

fn names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

fn sorted(
    mut entries: Vec<Entry>,
    key: SortKey,
    ascending: bool,
    folders_first: bool,
) -> Vec<Entry> {
    let spec = SortSpec {
        key,
        ascending,
        folders_first,
    };
    sort_entries(&mut entries, &spec);
    entries
}

fn file(name: &str, size: u64) -> Entry {
    Entry::synthetic(name, false, size)
}

#[test]
fn names_order_by_punctuation_then_numbers_by_value_then_letters_whatever_their_case_width_or_accent(
) {
    let rows = [
        ("a symbol before a digit", "_x", "1", Ordering::Less),
        ("a digit before a letter", "9", "a", Ordering::Less),
        ("numbers by value", "file2", "file10", Ordering::Less),
        (
            "numbers past the width of an integer",
            "f99999999999999999999",
            "f100000000000000000000",
            Ordering::Less,
        ),
        (
            "case does not decide before the letters do",
            "apricot",
            "Banana",
            Ordering::Less,
        ),
        (
            "the lower case first among the same letters",
            "a",
            "A",
            Ordering::Less,
        ),
        (
            "an accent after the plain letter",
            "resume",
            "résumé",
            Ordering::Less,
        ),
        (
            "an accent after its letters, before the next letter",
            "résumé",
            "resumf",
            Ordering::Less,
        ),
        (
            "the letters of a full width form",
            "Ａb",
            "ac",
            Ordering::Less,
        ),
        ("sharp s as two", "straße", "strassf", Ordering::Less),
        (
            "leading zeros do not make two names one",
            "file02",
            "file2",
            Ordering::Less,
        ),
        ("the same name", "same", "same", Ordering::Equal),
    ];
    for (what, a, b, expected) in rows {
        assert_eq!(compare_names(a, b), expected, "{what}: {a} against {b}");
        assert_eq!(
            compare_names(b, a),
            expected.reverse(),
            "{what}, the other way round"
        );
    }
}

#[test]
fn typing_to_select_ignores_case_and_keeps_accents_and_reads_digits_as_text() {
    let key = |s| type_select_key(s);
    assert_eq!(key("Éclair"), key("éclair"));
    assert_ne!(key("eclair"), key("éclair"));
    assert!(key("File10") < key("File2"));
    assert!(key("Report final").starts_with(&key("rep")));
    assert_eq!(key("Ａ"), key("a"));
}

#[test]
fn a_sort_orders_by_its_key_both_ways_and_ties_go_by_name() {
    let by = |name: &str, size: u64, modified: u64, mime: &str, tags: &[&str]| {
        let mut e = file(name, size);
        e.modified = Some(UNIX_EPOCH + Duration::from_secs(modified));
        e.created = Some(UNIX_EPOCH + Duration::from_secs(100 - modified));
        e.accessed = e.modified;
        e.mime = Some(mime.into());
        e.tags = tags.iter().map(|t| t.to_string()).collect();
        e
    };
    let items = || {
        vec![
            by("b.png", 30, 5, "image/png", &["Red"]),
            by("a.txt", 10, 9, "text/plain", &[]),
            by("c.png", 30, 1, "image/png", &["Blue", "Red"]),
            by("d.txt", 20, 9, "text/plain", &["Blue"]),
        ]
    };
    let rows: [(SortKey, [&str; 4], [&str; 4]); 7] = [
        (
            SortKey::Name,
            ["a.txt", "b.png", "c.png", "d.txt"],
            ["d.txt", "c.png", "b.png", "a.txt"],
        ),
        (
            SortKey::Size,
            ["a.txt", "d.txt", "b.png", "c.png"],
            ["b.png", "c.png", "d.txt", "a.txt"],
        ),
        (
            SortKey::DateModified,
            ["c.png", "b.png", "a.txt", "d.txt"],
            ["a.txt", "d.txt", "b.png", "c.png"],
        ),
        (
            SortKey::DateCreated,
            ["a.txt", "d.txt", "b.png", "c.png"],
            ["c.png", "b.png", "a.txt", "d.txt"],
        ),
        (
            SortKey::DateAccessed,
            ["c.png", "b.png", "a.txt", "d.txt"],
            ["a.txt", "d.txt", "b.png", "c.png"],
        ),
        (
            SortKey::Kind,
            ["b.png", "c.png", "a.txt", "d.txt"],
            ["a.txt", "d.txt", "b.png", "c.png"],
        ),
        (
            SortKey::Tags,
            ["a.txt", "c.png", "d.txt", "b.png"],
            ["b.png", "c.png", "d.txt", "a.txt"],
        ),
    ];
    for (key, ascending, descending) in rows {
        let up = sorted(items(), key, true, false);
        assert_eq!(names(&up), ascending, "{key:?} ascending");
        let down = sorted(items(), key, false, false);
        assert_eq!(names(&down), descending, "{key:?} descending");
    }
    let given = vec![file("c", 1), file("a", 1), file("b", 1)];
    let unsorted = sorted(given, SortKey::None, true, false);
    assert_eq!(names(&unsorted), ["c", "a", "b"], "no key");
}

#[test]
fn folders_stay_on_top_in_either_direction_when_asked() {
    let items = || {
        vec![
            file("b", 5),
            Entry::synthetic("y dir", true, 0),
            file("a", 9),
            Entry::synthetic("x dir", true, 0),
        ]
    };
    let rows = [
        (true, ["x dir", "y dir", "a", "b"]),
        (false, ["y dir", "x dir", "b", "a"]),
    ];
    for (ascending, expected) in rows {
        let result = sorted(items(), SortKey::Name, ascending, true);
        assert_eq!(names(&result), expected, "ascending {ascending}");
    }
    let mixed = sorted(items(), SortKey::Name, true, false);
    assert_eq!(names(&mixed), ["a", "b", "x dir", "y dir"]);
}

#[test]
fn a_new_item_is_inserted_where_the_sort_keeps_the_list_in_order() {
    let list = sorted(
        vec![file("alpha", 1), file("charlie", 1), file("delta", 1)],
        SortKey::Name,
        true,
        false,
    );
    let spec = SortSpec::default();
    let rows = [
        ("first", "Aardvark", 0),
        ("between", "bravo", 1),
        ("last", "zulu", 3),
    ];
    for (what, name, at) in rows {
        assert_eq!(insertion_index(&list, &file(name, 1), &spec), at, "{what}");
    }
    let down = sorted(list.clone(), SortKey::Name, false, false);
    let spec = SortSpec {
        ascending: false,
        ..spec
    };
    assert_eq!(insertion_index(&down, &file("bravo", 1), &spec), 2);
}
