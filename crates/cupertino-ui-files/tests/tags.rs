//! An item's tags: kept as names in the file's extended attribute, read back, added and removed, those of a link being
//! its target's; the colours of the names; and the tags a volume of the design's file manager holds.

use crate::scratch;

use cupertino_ui_files::tags::{
    add_tag, parse_reference_tags, parse_tag_list, read_tags, remove_tag, write_tags, Tag,
    TagColor, TagRegistry, XATTR_NAME,
};
use scratch::Scratch;

fn names(tags: &[&str]) -> Vec<String> {
    tags.iter().map(|t| t.to_string()).collect()
}

fn stored(path: &std::path::Path) -> Option<String> {
    xattr::get(path, XATTR_NAME)
        .unwrap()
        .map(|v| String::from_utf8(v).unwrap())
}

#[test]
fn tags_are_stored_as_a_comma_separated_attribute_and_read_back_in_order() {
    let scratch = Scratch::new("tags-roundtrip");
    let path = scratch.file("item.txt", b"x");
    assert_eq!(read_tags(&path).unwrap(), names(&[]));

    write_tags(&path, &names(&["Red", "Work", "a, b"])).unwrap();
    assert_eq!(stored(&path).as_deref(), Some("Red,Work,a  b"));
    assert_eq!(read_tags(&path).unwrap(), names(&["Red", "Work", "a  b"]));

    write_tags(&path, &[]).unwrap();
    assert_eq!(stored(&path), None);
    write_tags(&path, &[]).unwrap();
}

#[test]
fn a_tag_is_added_once_and_removed_whatever_its_case() {
    let scratch = Scratch::new("tags-add-remove");
    let path = scratch.file("item.txt", b"x");
    for tag in ["Red", "Work", "Red"] {
        add_tag(&path, tag).unwrap();
    }
    assert_eq!(read_tags(&path).unwrap(), names(&["Red", "Work"]));
    remove_tag(&path, "RED").unwrap();
    assert_eq!(read_tags(&path).unwrap(), names(&["Work"]));
    remove_tag(&path, "work").unwrap();
    assert_eq!(stored(&path), None);
    remove_tag(&path, "work").unwrap();
}

#[test]
fn the_tags_of_a_link_are_its_targets() {
    let scratch = Scratch::new("tags-link");
    let target = scratch.file("target.txt", b"x");
    let link = scratch.path().join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    add_tag(&link, "Blue").unwrap();
    assert_eq!(read_tags(&target).unwrap(), names(&["Blue"]));
    assert_eq!(read_tags(&link).unwrap(), names(&["Blue"]));
    assert!(read_tags(&scratch.path().join("missing")).is_err());
}

#[test]
fn a_list_of_tags_is_trimmed_and_loses_its_empty_names() {
    let rows = [
        ("Red,Work", names(&["Red", "Work"])),
        (" Red , Work ,, ", names(&["Red", "Work"])),
        ("", names(&[])),
        (",", names(&[])),
    ];
    for (text, expected) in rows {
        assert_eq!(parse_tag_list(text.as_bytes()), expected, "{text:?}");
    }
}

#[test]
fn a_tag_has_the_colour_its_name_or_the_registry_gives_it() {
    let mut registry = TagRegistry::default();
    registry.colors.insert("Work".into(), TagColor::Orange);
    registry.colors.insert("Green".into(), TagColor::Purple);
    let resolved = registry.resolve(&names(&["red", "Work", "Green", "Misc"]));
    let colours: Vec<_> = resolved.iter().map(|t| t.color).collect();
    assert_eq!(
        colours,
        [
            Some(TagColor::Red),
            Some(TagColor::Orange),
            Some(TagColor::Purple),
            None
        ]
    );
    assert_eq!(resolved[0].name, "red");

    assert_eq!(TagColor::from_index(4), Some(TagColor::Blue));
    assert_eq!(TagColor::from_index(0), None);
    assert_eq!(TagColor::from_index(8), None);
    assert_eq!(TagColor::from_title("GRAY"), Some(TagColor::Gray));
    assert_eq!(TagColor::from_title("pink"), None);
}

/// A binary property list holding an array of `strings`, as a volume of the design's file manager keeps tags: an
/// ASCII string as such and any other as UTF-16.
fn property_list(strings: &[&str]) -> Vec<u8> {
    let mut out = b"bplist00".to_vec();
    let mut offsets = vec![out.len()];
    out.push(0xA0 | strings.len() as u8);
    out.extend((1..=strings.len()).map(|i| i as u8));
    for s in strings {
        offsets.push(out.len());
        if s.is_ascii() {
            out.push(0x50 | s.len() as u8);
            out.extend(s.bytes());
        } else {
            let units: Vec<u16> = s.encode_utf16().collect();
            out.push(0x60 | units.len() as u8);
            out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
        }
    }
    let table = out.len();
    out.extend(offsets.iter().map(|o| *o as u8));
    out.extend([0; 6]);
    out.extend([1, 1]);
    out.extend((offsets.len() as u64).to_be_bytes());
    out.extend(0u64.to_be_bytes());
    out.extend((table as u64).to_be_bytes());
    out
}

#[test]
fn the_tags_of_a_volume_of_the_design_are_read_with_their_colours() {
    let list = property_list(&["Red\n6", "Büro\n3", "Plain", "Odd\n99"]);
    let tag = |name: &str, color| Tag {
        name: name.into(),
        color,
    };
    assert_eq!(
        parse_reference_tags(&list),
        Some(vec![
            tag("Red", Some(TagColor::Red)),
            tag("Büro", Some(TagColor::Purple)),
            tag("Plain", None),
            tag("Odd", None),
        ])
    );
    assert_eq!(
        parse_reference_tags(b"not a property list at all, but long enough to try"),
        None
    );
    assert_eq!(parse_reference_tags(&list[..list.len() - 9]), None);
}
