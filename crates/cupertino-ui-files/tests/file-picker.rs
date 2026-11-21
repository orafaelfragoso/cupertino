//! The model of a file chooser: what its keys do to the highlight and the folder, which answer of a worker it takes,
//! how it lists a folder in each mode, in the family's order, and what its Choose button picks.

use std::path::{Path, PathBuf};

use cupertino_ui_files::entry::Entry;
use cupertino_ui_files::filepicker::{Key, Listing, Mode, Outcome, Picker, Request};

fn entry(dir: &str, name: &str, is_dir: bool) -> Entry {
    Entry {
        path: PathBuf::from(dir).join(name),
        ..Entry::synthetic(name, is_dir, 0)
    }
}

/// A folder of two folders and two files, whatever the folder, mode or dotfile choice, which the reader ignores.
fn read(dir: &Path, _: Mode, _: bool) -> Option<Vec<Entry>> {
    match dir.to_str()? {
        "/home/u" => Some(vec![
            entry("/home/u", "docs", true),
            entry("/home/u", "music", true),
            entry("/home/u", "a.txt", false),
            entry("/home/u", "b.txt", false),
        ]),
        "/home" => Some(vec![entry("/home", "u", true), entry("/home", "v", true)]),
        "/home/u/docs" => Some(Vec::new()),
        _ => None,
    }
}

/// A chooser in `/home/u` in `mode` with its folder listed.
fn opened(mode: Mode) -> Picker {
    let mut picker = Picker::new(Path::new("/home/u"), mode);
    let listing = picker
        .take_request()
        .expect("a first request")
        .run_with(&read);
    assert_eq!(picker.apply(listing), Outcome::Changed);
    picker
}

/// Answer the request the chooser made, as its worker would.
fn answer(picker: &mut Picker) -> Outcome {
    let listing = picker.take_request().expect("a request").run_with(&read);
    picker.apply(listing)
}

#[test]
fn a_chooser_lists_nothing_until_its_request_is_answered_and_its_keys_move_the_highlight_and_report_what_changed(
) {
    let mut picker = Picker::new(Path::new("/home/u"), Mode::File);
    assert!(picker.loading());
    assert!(picker.entries().is_empty());
    assert_eq!(picker.dir(), Path::new("/home/u"));
    assert!(picker.take_request().is_some());
    assert!(picker.take_request().is_none(), "a request is taken once");

    let mut picker = opened(Mode::File);
    assert!(!picker.loading());
    assert_eq!(picker.entries().len(), 4);
    assert_eq!(picker.highlighted(), None);
    assert_eq!(picker.mode(), Mode::File);
    assert!(picker.take_request().is_none());

    // a fresh chooser's keys
    let mut picker = opened(Mode::File);
    let keys = [
        (Key::Down, Outcome::Changed, Some(0)),
        (Key::Down, Outcome::Changed, Some(1)),
        (Key::End, Outcome::Changed, Some(3)),
        (Key::Down, Outcome::Ignore, Some(3)),
        (Key::Up, Outcome::Changed, Some(2)),
        (Key::Home, Outcome::Changed, Some(0)),
        (Key::Home, Outcome::Ignore, Some(0)),
        (Key::Up, Outcome::Ignore, Some(0)),
        (Key::Escape, Outcome::Cancel, Some(0)),
    ];
    for (key, outcome, highlighted) in keys {
        assert_eq!(picker.key(key, 0.0), outcome, "{key:?}");
        assert_eq!(picker.highlighted(), highlighted, "{key:?}");
    }
    let mut fresh = opened(Mode::File);
    assert_eq!(fresh.key(Key::Up, 0.0), Outcome::Changed);
    assert_eq!(
        fresh.highlighted(),
        Some(3),
        "Up from nothing goes to the last"
    );
    assert_eq!(fresh.highlight(9), Outcome::Ignore);
    assert_eq!(fresh.highlight(1), Outcome::Changed);

    let mut empty = Picker::new(Path::new("/home/u/docs"), Mode::File);
    answer(&mut empty);
    assert_eq!(empty.key(Key::Down, 0.0), Outcome::Ignore);
    assert_eq!(empty.key(Key::Return, 0.0), Outcome::Ignore);
}

#[test]
fn return_enters_a_folder_or_chooses_a_file_and_the_choose_key_picks_by_the_mode() {
    // (mode, row highlighted, what Return does, what Choose does)
    let rows = [
        (Mode::File, 0, "enters /home/u/docs", Outcome::Ignore),
        (
            Mode::File,
            2,
            "chooses /home/u/a.txt",
            Outcome::Choose("/home/u/a.txt".into()),
        ),
        (
            Mode::Photo,
            3,
            "chooses /home/u/b.txt",
            Outcome::Choose("/home/u/b.txt".into()),
        ),
        (
            Mode::Folder,
            1,
            "enters /home/u/music",
            Outcome::Choose("/home/u/music".into()),
        ),
    ];
    for (mode, row, returned, choose) in rows {
        let mut picker = opened(mode);
        picker.highlight(row);
        assert_eq!(picker.key(Key::Choose, 0.0), choose, "{mode:?} {row}");
        let outcome = picker.key(Key::Return, 0.0);
        match returned.strip_prefix("enters ") {
            Some(folder) => {
                assert_eq!(outcome, Outcome::Changed, "{mode:?} {row}");
                assert_eq!(picker.take_request().unwrap().key().0, Path::new(folder));
            }
            None => assert_eq!(
                outcome,
                Outcome::Choose(returned.strip_prefix("chooses ").unwrap().into()),
                "{mode:?} {row}"
            ),
        }
    }

    // nothing highlighted: a folder chooser picks the folder it shows, the others nothing
    assert_eq!(
        opened(Mode::Folder).key(Key::Choose, 0.0),
        Outcome::Choose("/home/u".into())
    );
    assert_eq!(opened(Mode::File).key(Key::Choose, 0.0), Outcome::Ignore);
    assert_eq!(
        opened(Mode::Folder).choice(),
        Some(PathBuf::from("/home/u"))
    );
    assert_eq!(opened(Mode::Program).choice(), None);
}

#[test]
fn the_folder_shown_stays_until_the_one_asked_for_is_listed_which_is_the_latest_asked_and_a_failure_leaves_it(
) {
    let mut picker = opened(Mode::File);
    picker.highlight(0);
    assert_eq!(picker.key(Key::Return, 0.0), Outcome::Changed);
    assert!(picker.loading());
    assert_eq!(picker.dir(), Path::new("/home/u"), "still the folder shown");
    assert_eq!(picker.entries().len(), 4);
    assert_eq!(answer(&mut picker), Outcome::Changed);
    assert_eq!(picker.dir(), Path::new("/home/u/docs"));
    assert!(picker.entries().is_empty());
    assert!(!picker.loading());

    picker.go(Path::new("/nowhere"));
    answer(&mut picker);
    assert!(picker.unreadable());
    assert_eq!(
        picker.dir(),
        Path::new("/home/u/docs"),
        "the folder that was open stays"
    );
    picker.go(Path::new("/home/u"));
    answer(&mut picker);
    assert!(!picker.unreadable());
    let mut picker = opened(Mode::File);
    picker.go(Path::new("/home"));
    let first = picker.take_request().unwrap().run_with(&read);
    picker.go(Path::new("/home/u/docs"));
    let second = picker.take_request().unwrap().run_with(&read);
    assert_eq!(picker.apply(first), Outcome::Ignore);
    assert!(picker.loading());
    assert_eq!(picker.dir(), Path::new("/home/u"));
    assert_eq!(picker.apply(second), Outcome::Changed);
    assert_eq!(picker.dir(), Path::new("/home/u/docs"));
}

#[test]
fn the_parent_is_listed_with_the_folder_just_left_highlighted_and_a_missing_start_shows_the_nearest_folder(
) {
    let mut picker = opened(Mode::File);
    assert_eq!(picker.key(Key::Parent, 0.0), Outcome::Changed);
    answer(&mut picker);
    assert_eq!(picker.dir(), Path::new("/home"));
    assert_eq!(picker.highlighted(), Some(0), "u, the folder left");

    let mut root = Picker::new(Path::new("/"), Mode::File);
    assert_eq!(root.key(Key::Parent, 0.0), Outcome::Ignore);
    assert!(root.take_request().is_some(), "only the first request");
    assert!(root.take_request().is_none());

    let mut missing = Picker::new(Path::new("/home/u/docs/not/yet/made"), Mode::File);
    answer(&mut missing);
    assert_eq!(missing.dir(), Path::new("/home/u/docs"));
    assert!(!missing.unreadable());
}

#[test]
fn typing_highlights_the_next_entry_that_starts_with_it() {
    let mut picker = opened(Mode::File);
    let typed = [
        ('m', 0.0, Outcome::Changed, Some(1)),
        ('x', 5.0, Outcome::Ignore, Some(1)),
        ('b', 7.0, Outcome::Changed, Some(3)),
    ];
    for (c, at, outcome, highlighted) in typed {
        assert_eq!(picker.key(Key::Type(c), at), outcome, "{c}");
        assert_eq!(picker.highlighted(), highlighted, "{c}");
    }
}

#[test]
fn showing_hidden_names_lists_the_folder_again_with_the_highlight_kept() {
    let mut picker = opened(Mode::File);
    picker.highlight(2);
    picker.set_hidden(true);
    let request = picker.take_request().expect("the folder is read again");
    assert_eq!(request.key(), (PathBuf::from("/home/u"), true, Mode::File));
    let listing = request.run_with(&|dir, _, hidden| {
        let mut entries = read(dir, Mode::File, hidden)?;
        entries.insert(0, entry("/home/u", ".config", true));
        Some(entries)
    });
    picker.apply(listing);
    assert_eq!(picker.highlighted(), Some(3), "a.txt, one row further down");

    picker.set_hidden(true);
    assert!(
        picker.take_request().is_none(),
        "nothing changed, nothing is read"
    );
}

/// A folder of the test's own under the target directory, removed when the test is done.
struct Scratch(PathBuf);

impl Scratch {
    fn new(test: &str) -> Scratch {
        let root =
            PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("ui-files-picker-{test}"));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        Scratch(root)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(listing: &Listing, picker: &mut Picker) -> Vec<String> {
    assert_eq!(picker.apply(listing.clone()), Outcome::Changed);
    picker.entries().iter().map(|e| e.name.clone()).collect()
}

#[test]
fn a_folder_is_listed_in_the_familys_order_with_the_files_its_mode_chooses() {
    use std::os::unix::fs::PermissionsExt;
    let scratch = Scratch::new("listing");
    for dir in ["zeta", "Alpha", ".hidden-dir"] {
        std::fs::create_dir(scratch.0.join(dir)).unwrap();
    }
    for (file, mode) in [
        ("b.txt", 0o644),
        ("A.PNG", 0o644),
        ("c.jpeg", 0o644),
        (".dot.png", 0o644),
        ("run.sh", 0o755),
        ("file 10.txt", 0o644),
        ("file 9.txt", 0o644),
    ] {
        let path = scratch.0.join(file);
        std::fs::write(&path, b"x").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
    }
    // by name as Files sorts it, without regard to case and with numbers in order; folders among the files
    let rows: [(Mode, bool, &[&str]); 6] = [
        (Mode::Folder, false, &["Alpha", "zeta"]),
        (Mode::Photo, false, &["A.PNG", "Alpha", "c.jpeg", "zeta"]),
        (Mode::Program, false, &["Alpha", "run.sh", "zeta"]),
        (
            Mode::File,
            false,
            &[
                "A.PNG",
                "Alpha",
                "b.txt",
                "c.jpeg",
                "file 9.txt",
                "file 10.txt",
                "run.sh",
                "zeta",
            ],
        ),
        (Mode::Folder, true, &[".hidden-dir", "Alpha", "zeta"]),
        (
            Mode::Photo,
            true,
            &[
                ".dot.png",
                ".hidden-dir",
                "A.PNG",
                "Alpha",
                "c.jpeg",
                "zeta",
            ],
        ),
    ];
    for (mode, hidden, expected) in rows {
        let mut picker = Picker::new(&scratch.0, mode);
        picker.set_hidden(hidden);
        let request: Request = picker.take_request().expect("a request");
        let listing = request.run();
        assert_eq!(
            names(&listing, &mut picker),
            expected,
            "{mode:?}, hidden {hidden}"
        );
    }

    let mut open = Picker::open(&scratch.0, Mode::File);
    assert_eq!(open.entries().len(), 8);
    assert!(!open.entries()[0].is_dir && open.entries()[1].is_dir);
    open.key(Key::Down, 0.0);
    open.key(Key::Down, 0.0);
    assert_eq!(
        open.choice(),
        None,
        "a folder is not chosen in a file chooser"
    );
}
