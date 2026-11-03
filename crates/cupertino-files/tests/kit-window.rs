//! The browser window on CupertinoKit, run headless: what it lists and reads to assistive technology, where the
//! sidebar and the menus take it, and a folder changing under it.

#![cfg(feature = "app")]

mod common;

use std::collections::HashMap;

use common::{action, place, Run};
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::{AccessibilityNode, NodeId, Role};

impl Run {
    /// A window on `home/Documents`, which holds the folder `Notes` and the files `b.txt` and `a.txt`, with
    /// Documents and Downloads in the sidebar.
    fn new() -> Run {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(documents.join("Notes")).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(documents.join("b.txt"), "bb").unwrap();
        std::fs::write(documents.join("a.txt"), "a").unwrap();
        let places = vec![
            place("Documents", &documents, PlaceKind::Documents),
            place("Downloads", &downloads, PlaceKind::Downloads),
        ];
        Run::start(home, documents, places, None)
    }
}

#[test]
fn the_folder_is_listed_by_name_under_its_columns_and_read_as_a_table() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    assert_eq!(run.title(), "Documents");
    assert_eq!(run.named(Role::Table), ["Documents"]);
    assert_eq!(
        run.named(Role::ColumnHeader),
        ["Name", "Date Modified", "Size", "Kind"]
    );
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
    run.driver.look(run.window, "files-browser").unwrap();
}

#[test]
fn a_place_in_the_sidebar_opens_its_folder_and_back_returns() {
    let mut run = Run::new();
    run.click("place-Downloads");
    assert_eq!(run.title(), "Downloads");
    assert_eq!(run.rows(), Vec::<String>::new());
    assert!(!run.disabled().contains(&action("go-back")));
    run.menu("go-back");
    assert_eq!(run.title(), "Documents");
}

#[test]
fn the_menus_disable_what_the_window_cannot_do() {
    let mut run = Run::new();
    let disabled = run.disabled();
    for name in ["go-back", "go-forward", "open", "rename", "view-icons"] {
        assert!(disabled.contains(&action(name)), "{name}");
    }
    run.menu("select-all");
    let disabled = run.disabled();
    assert!(!disabled.contains(&action("open")));
    assert!(disabled.contains(&action("rename")), "not yet");
}

#[test]
fn sorting_by_size_from_the_menu_puts_the_largest_first() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("sort-size");
    assert_eq!(run.rows()[..2], ["b.txt", "a.txt"]);
}

#[test]
fn the_bars_show_the_path_and_the_item_count() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("toggle-path-bar");
    run.menu("toggle-status-bar");
    let texts = run.named(Role::StaticText);
    assert!(texts.iter().any(|t| t.starts_with("3 items")), "{texts:?}");
    assert!(texts.contains(&"Documents".to_owned()), "{texts:?}");
}

#[test]
fn a_file_made_in_the_folder_is_listed_without_input() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    std::fs::write(run.home().join("Documents/c.txt"), "c").unwrap();
    run.wait_until(|run| run.rows().contains(&"c.txt".to_owned()));
}

#[test]
fn a_name_longer_than_its_column_stays_inside_its_cell() {
    let mut run = Run::new();
    let long = "a name long enough to run past the column and into the next one ".repeat(4);
    std::fs::write(run.home().join("Documents").join(long.trim()), "").unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let nodes: HashMap<NodeId, &AccessibilityNode> = snapshot
        .accessibility
        .iter()
        .map(|(id, node)| (*id, node))
        .collect();
    for cell in nodes.values().filter(|node| node.role == Role::Cell) {
        for text in cell.children.iter().map(|id| nodes[id]) {
            assert!(
                text.bounds.min_x() >= cell.bounds.min_x()
                    && text.bounds.max_x() <= cell.bounds.max_x(),
                "{:?} spills out of its cell {:?}",
                text.name,
                cell.bounds
            );
        }
    }
}

#[test]
fn make_alias_links_each_selected_item_as_name_alias() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let documents = run.home().join("Documents");
    std::os::unix::fs::symlink("elsewhere", documents.join("a.txt alias")).unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    run.click_row("a.txt");
    run.menu("make-alias");
    run.wait_until(|run| run.rows().contains(&"a.txt alias 2".to_owned()));
    assert_eq!(
        std::fs::read_link(documents.join("a.txt alias 2")).unwrap(),
        documents.canonicalize().unwrap().join("a.txt")
    );
}

#[test]
fn show_original_goes_to_the_target_of_the_link_and_selects_it() {
    let mut run = Run::new();
    let downloads = run.home().join("Downloads");
    std::fs::write(downloads.join("report.pdf"), "").unwrap();
    std::fs::write(downloads.join("other.pdf"), "").unwrap();
    std::os::unix::fs::symlink(
        downloads.join("report.pdf"),
        run.home().join("Documents/report link"),
    )
    .unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    assert!(run.disabled().contains(&action("show-original")));
    run.click_row("report link");
    run.menu("show-original");
    assert_eq!(run.title(), "Downloads");
    run.wait_until(|run| run.selected_rows() == ["report.pdf"]);
}
