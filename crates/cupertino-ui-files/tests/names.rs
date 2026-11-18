//! Names of items: which a file system takes, when changing an extension asks first, what is selected when a name
//! starts to be edited, the free name a copy takes, and the field a name is edited in.

use cupertino_ui_files::name_edit::{NameEdit, NAME_MAX_LEN};
use cupertino_ui_files::rename::{
    initial_selection, needs_extension_warning, unique_name, validate_name, NameError,
    MAX_NAME_BYTES,
};

#[test]
fn a_name_is_taken_unless_it_is_empty_has_a_slash_or_nul_is_a_dot_entry_or_is_too_long() {
    let longest = "a".repeat(MAX_NAME_BYTES);
    let too_long = "a".repeat(MAX_NAME_BYTES + 1);
    // 128 two-byte letters are more bytes than the limit though fewer letters
    let wide = "é".repeat(MAX_NAME_BYTES / 2 + 1);
    let rows = [
        ("", Err(NameError::Empty)),
        ("a/b", Err(NameError::ContainsSlash)),
        ("a\0b", Err(NameError::ContainsNul)),
        (".", Err(NameError::Dot)),
        ("..", Err(NameError::Dot)),
        (too_long.as_str(), Err(NameError::TooLong)),
        (wide.as_str(), Err(NameError::TooLong)),
        (longest.as_str(), Ok(())),
        ("...", Ok(())),
        (".hidden", Ok(())),
        ("a name with spaces.txt", Ok(())),
    ];
    for (name, expected) in rows {
        let shown: String = name.chars().take(20).collect();
        assert_eq!(validate_name(name), expected, "{shown:?}");
    }
}

#[test]
fn changing_the_extension_of_a_file_asks_and_nothing_else_does() {
    let rows = [
        ("a.txt", "b.txt", false, false),
        ("a.txt", "a.md", false, true),
        ("a.TXT", "a.txt", false, false),
        ("a", "a.txt", false, true),
        ("a.txt", "a", false, true),
        ("a.txt", "a.", false, true),
        (".bashrc", ".zshrc", false, false),
        ("a.tar.gz", "a.tar.xz", false, true),
        ("Photos.old", "Photos.new", true, false),
    ];
    for (old, new, is_dir, asks) in rows {
        assert_eq!(
            needs_extension_warning(old, new, is_dir),
            asks,
            "{old} to {new}"
        );
    }
}

#[test]
fn a_name_starts_edited_with_everything_before_its_extension_selected() {
    let rows = [
        ("report.final.pdf", false, 0..12),
        ("résumé.pdf", false, 0..6),
        ("Photos", true, 0..6),
        ("v1.2 files", true, 0..10),
        (".bashrc", false, 0..7),
        ("README", false, 0..6),
        ("archive.", false, 0..8),
    ];
    for (name, is_dir, selected) in rows {
        assert_eq!(initial_selection(name, is_dir), selected, "{name}");
    }
}

#[test]
fn a_clash_gives_the_name_a_copy_suffix_before_the_extension_and_counts_on() {
    let rows: [(&str, bool, &[&str], &str); 8] = [
        ("a.txt", false, &[], "a.txt"),
        ("a.txt", false, &["a.txt"], "a copy.txt"),
        ("a.txt", false, &["a.txt", "a copy.txt"], "a copy 2.txt"),
        (
            "a.txt",
            false,
            &["a.txt", "a copy.txt", "a copy 2.txt"],
            "a copy 3.txt",
        ),
        ("pics.v1", true, &["pics.v1"], "pics.v1 copy"),
        (".hidden", false, &[".hidden"], ".hidden copy"),
        ("README", false, &["README"], "README copy"),
        ("a.tar.gz", false, &["a.tar.gz"], "a.tar copy.gz"),
    ];
    for (base, is_dir, taken, expected) in rows {
        let name = unique_name(base, is_dir, &|candidate| taken.contains(&candidate));
        assert_eq!(name, expected, "{base} among {taken:?}");
    }
}

#[test]
fn a_name_being_edited_selects_its_stem_commits_only_a_changed_valid_name_and_holds_no_more_than_a_file_system(
) {
    let mut edit = NameEdit::new(3, "report.pdf", false);
    assert_eq!(edit.index(), 3);
    assert_eq!(edit.text(), "report.pdf");
    assert_eq!(edit.field().selected_text(), "report");

    edit.field_mut().insert("summary");
    assert_eq!(edit.text(), "summary.pdf");
    assert_eq!(edit.commit(), Ok(Some("summary.pdf".to_string())));

    edit.field_mut().set_text("report.pdf");
    assert_eq!(edit.commit(), Ok(None));
    edit.field_mut().set_text("a/b.pdf");
    assert_eq!(edit.commit(), Err(NameError::ContainsSlash));
    edit.field_mut().set_text("");
    assert_eq!(edit.commit(), Err(NameError::Empty));

    edit.set_index(1);
    assert_eq!(edit.index(), 1);
    assert_eq!(
        NameEdit::new(0, "Photos", true).field().selected_text(),
        "Photos"
    );

    // and holds no more than a file system does
    let mut edit = NameEdit::new(0, "a", true);
    edit.field_mut().select_all();
    edit.field_mut().insert(&"b".repeat(NAME_MAX_LEN + 10));
    assert_eq!(edit.text().len(), NAME_MAX_LEN);
    assert_eq!(
        edit.commit().map(|name| name.map(|n| n.len())),
        Ok(Some(NAME_MAX_LEN))
    );
}
