//! The browser window on CupertinoKit, run headless: what it lists and reads to assistive technology, where the
//! sidebar and the menus take it, and a folder changing under it.

#![cfg(feature = "app")]

mod common;

use std::collections::HashMap;
use std::sync::{mpsc, Mutex};
use std::time::Duration;

use common::{action, place, Run};
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_files::trash::encode_path;
use cupertino_kit::core::{AccessibilityNode, Key, NodeId, Role};
use cupertino_kit::platform::Mods;
use cupertino_kit::platform::WindowEvent;
use cupertino_test_bus::{missing_tool, PrivateBus};
use cupertino_ui_test::{semantic_gaps, semantic_outline, whole_tree};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedValue;

/// A notification service that hands on the summary and body of each notification.
struct Notifications(Mutex<mpsc::Sender<String>>);

#[zbus::interface(name = "org.freedesktop.Notifications")]
impl Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        _app_name: String,
        _replaces_id: u32,
        _app_icon: String,
        summary: String,
        body: String,
        _actions: Vec<String>,
        _hints: HashMap<String, OwnedValue>,
        _expire_timeout: i32,
    ) -> u32 {
        let _ = self.0.lock().unwrap().send(format!("{summary} / {body}"));
        1
    }
}

fn notifications(bus: &PrivateBus) -> (Connection, mpsc::Receiver<String>) {
    let (sent, received) = mpsc::channel();
    let service = bus
        .blocking_builder()
        .name("org.freedesktop.Notifications")
        .unwrap()
        .serve_at(
            "/org/freedesktop/Notifications",
            Notifications(Mutex::new(sent)),
        )
        .unwrap()
        .build()
        .unwrap();
    (service, received)
}

impl Run {
    /// A window on `home/Documents`, which holds the folder `Notes` and the files `b.txt` and `a.txt`, with
    /// Documents and Downloads in the sidebar and its trash in `home/Trash`.
    fn new() -> Run {
        Run::with(false, Notices::start(None))
    }

    /// The window of [`Run::new`] posting to `notices`, with the trash's folder as a place after Downloads when
    /// `trash_listed`.
    fn with(trash_listed: bool, notices: Notices) -> Run {
        let home = tempfile::tempdir().unwrap();
        let documents = home.path().join("Documents");
        let downloads = home.path().join("Downloads");
        std::fs::create_dir_all(documents.join("Notes")).unwrap();
        std::fs::create_dir_all(&downloads).unwrap();
        std::fs::write(documents.join("b.txt"), "bb").unwrap();
        std::fs::write(documents.join("a.txt"), "a").unwrap();
        let mut places = vec![
            place("Documents", &documents, PlaceKind::Documents),
            place("Downloads", &downloads, PlaceKind::Downloads),
        ];
        if trash_listed {
            let trash = home.path().join("Trash/files");
            places.push(place("Trash", &trash, PlaceKind::Trash));
        }
        Run::start(home, documents, places, notices, None)
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
    for name in ["go-back", "go-forward", "open", "rename"] {
        assert!(disabled.contains(&action(name)), "{name}");
    }
    assert!(!disabled.contains(&action("view-icons")));
    run.menu("select-all");
    let disabled = run.disabled();
    assert!(!disabled.contains(&action("open")));
    assert!(!disabled.contains(&action("duplicate")));
    assert!(disabled.contains(&action("rename")), "one item at a time");
}

#[test]
fn a_new_folder_is_named_in_its_row_and_a_duplicate_sits_next_to_its_file() {
    const RETURN: u32 = 0xff0d;
    let mut run = Run::new();
    let documents = run.home().join("Documents");
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("new-folder");
    run.wait_until(|run| {
        let runtime = run.driver.runtime();
        runtime
            .find(run.window, &Key::from("name-field/0"))
            .is_some()
    });
    run.type_text("Projects");
    run.key(RETURN, None);
    assert!(documents.join("Projects").is_dir());
    assert!(!documents.join("untitled folder").exists());
    run.wait_until(|run| run.rows().contains(&"Projects".to_owned()));

    run.type_text("b");
    run.menu("duplicate");
    assert_eq!(std::fs::read(documents.join("b copy.txt")).unwrap(), b"bb");
    let mut names: Vec<String> = std::fs::read_dir(&documents)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["Notes", "Projects", "a.txt", "b copy.txt", "b.txt"]);
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
fn items_moved_to_the_trash_are_put_back_where_they_were() {
    let mut run = Run::with(true, Notices::start(None));
    run.wait_until(|run| run.rows().len() == 3);
    let a = run.home().join("Documents/a.txt");
    let trash = run.home().join("Trash");
    run.menu("select-all");
    run.menu("move-to-trash");
    run.wait_until(|run| run.rows().is_empty());
    let info = std::fs::read_to_string(trash.join("info/a.txt.trashinfo")).unwrap();
    let path = format!("[Trash Info]\nPath={}\n", encode_path(&a.to_string_lossy()));
    assert!(info.starts_with(&path), "{info}");
    assert_eq!(
        std::fs::read_to_string(trash.join("files/a.txt")).unwrap(),
        "a"
    );
    assert!(!a.exists());

    run.click("place-Trash");
    assert_eq!(run.title(), "Trash");
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("select-all");
    assert!(!run.disabled().contains(&action("put-back")));
    run.menu("put-back");
    run.wait_until(|run| run.rows().is_empty());
    assert_eq!(std::fs::read_to_string(&a).unwrap(), "a");
    assert!(run.home().join("Documents/Notes").is_dir());
    assert!(!trash.join("info/a.txt.trashinfo").exists());
}

#[test]
fn items_that_cannot_go_to_the_trash_stay_and_are_reported_in_one_notice() {
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (_service, received) = notifications(&bus);
    let mut run = Run::with(false, Notices::start(Some(bus.connect())));
    run.wait_until(|run| run.rows().len() == 3);
    std::fs::write(run.home().join("Trash"), "").unwrap();
    run.menu("select-all");
    run.menu("move-to-trash");
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap(),
        "3 items couldn't be moved to the Trash / File exists."
    );
    assert!(received.recv_timeout(Duration::from_millis(200)).is_err());
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
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

/// The window's tree as a screen reader reads it, a line a node ([`cupertino_ui_test::semantic_outline`]), with each
/// date made today or yesterday read as `<date>`: the files were made at the start of the test.
fn outline(run: &Run) -> Vec<String> {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let tree = whole_tree(run.window, snapshot.accessibility, snapshot.focus);
    semantic_outline(&tree)
        .into_iter()
        .map(|line| {
            let text = line.trim_start();
            if text.starts_with("StaticText \"Today at ")
                || text.starts_with("StaticText \"Yesterday at ")
            {
                format!("{}StaticText <date>", &line[..line.len() - text.len()])
            } else {
                line
            }
        })
        .collect()
}

#[test]
fn the_window_reads_its_sidebar_toolbar_and_table_by_role_name_value_and_state() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let mut gaps = semantic_gaps(&whole_tree(
        run.window,
        snapshot.accessibility,
        snapshot.focus,
    ));
    // Kit names no window root and no toolbar, and a cell is read by its text
    gaps.retain(|gap| {
        !matches!(
            gap.as_str(),
            "Window #0: no name" | "Window #0 > Toolbar #1: no name"
        ) && !(gap.contains(" > Cell #") && gap.ends_with(": no name"))
    });
    assert_eq!(gaps, Vec::<String>::new());
    let row = |state: &str, cells: [&str; 4]| {
        let mut lines = vec![format!("      Row{state}")];
        for cell in cells {
            lines.push("        Cell".to_owned());
            lines.push(format!("          StaticText {cell}"));
        }
        lines
    };
    let window = |first: &str| {
        let mut lines: Vec<String> = [
            "Window",
            "  ScrollView",
            "    List \"Sidebar\" 2 items",
            "      Group \"Favorites\"",
            "        ListItem \"Documents\" focusable selected",
            "        ListItem \"Downloads\"",
            "  Toolbar",
            "    Button \"Back\" disabled",
            "    Button \"Forward\" disabled",
            "    StaticText \"Documents\"",
            "    TextInput \"Search\" = \"\" focusable",
            "  Table \"Documents\" 3 items multiselectable",
            "    Row",
            "      ColumnHeader \"Name\" focusable",
            "        StaticText \"Name\"",
            "      ColumnHeader \"Date Modified\" focusable",
            "        StaticText \"Date Modified\"",
            "      ColumnHeader \"Size\" focusable",
            "        StaticText \"Size\"",
            "      ColumnHeader \"Kind\" focusable",
            "        StaticText \"Kind\"",
            "    ScrollView",
        ]
        .map(str::to_owned)
        .to_vec();
        lines.extend(row(
            first,
            ["\"a.txt\"", "<date>", "\"1 byte\"", "\"Text\""],
        ));
        lines.extend(row("", ["\"b.txt\"", "<date>", "\"2 bytes\"", "\"Text\""]));
        lines.extend(row("", ["\"Notes\"", "<date>", "\"--\"", "\"Folder\""]));
        lines.push("  Splitter \"Sidebar\" = 200 of 0..919 focusable".to_owned());
        lines
    };
    assert_eq!(outline(&run), window(" focusable"), "nothing selected");
    // a click on the first item's name selects it and takes the focus to it
    run.click_row("a.txt");
    assert_eq!(
        outline(&run),
        window(" focusable focused selected"),
        "the clicked item reads selected and focused"
    );
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

#[test]
fn a_folder_opens_in_a_second_window_whose_menus_act_on_it_alone() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let first = run.window;
    assert!(run.disabled().contains(&action("open-in-new-window")));
    run.click_row("Notes");
    assert!(!run.disabled().contains(&action("open-in-new-window")));
    run.menu("open-in-new-window");
    let windows = run.windows();
    assert_eq!(windows.len(), 2);
    run.activate(windows[1]);
    assert_eq!(run.title(), "Notes");

    run.menu("go-enclosing-folder");
    assert_eq!(run.title(), "Documents");
    run.menu("fullscreen");
    assert_eq!(run.full_screen_requests(), [true]);
    assert_eq!(run.menu_label("fullscreen"), "Exit Full Screen");
    run.activate(first);
    assert_eq!(run.title(), "Documents");
    assert!(run.disabled().contains(&action("go-back")));
    assert_eq!(run.full_screen_requests(), Vec::<bool>::new());
    assert_eq!(run.menu_label("fullscreen"), "Enter Full Screen");

    run.menu("new");
    assert_eq!(run.windows().len(), 3);
    run.send(WindowEvent::CloseRequested);
    assert_eq!(run.windows().len(), 2);
    assert!(!run.windows().contains(&first));
}

#[test]
fn a_name_edited_in_one_window_does_not_take_the_keyboard_from_another() {
    const RETURN: u32 = 0xff0d;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let first = run.window;
    run.click_row("Notes");
    run.menu("open-in-new-window");
    run.activate(first);
    run.click_row("a.txt");
    run.menu("rename");
    run.wait_until(|run| {
        let runtime = run.driver.runtime();
        runtime.find(first, &Key::from("name-field/0")).is_some()
    });

    let second = run.windows()[1];
    run.activate(second);
    run.menu("new-folder");
    run.wait_until(|run| {
        let runtime = run.driver.runtime();
        runtime.find(second, &Key::from("name-field/1")).is_some()
    });
    run.type_text("Projects");
    run.key(RETURN, None);
    assert!(run.home().join("Documents/Notes/Projects").is_dir());
    assert!(run.home().join("Documents/a.txt").exists());
}

#[test]
fn full_screen_asks_the_platform_and_the_menu_follows() {
    let mut run = Run::new();
    assert_eq!(run.menu_label("fullscreen"), "Enter Full Screen");
    run.menu("fullscreen");
    assert_eq!(run.full_screen_requests(), [true]);
    assert_eq!(run.menu_label("fullscreen"), "Exit Full Screen");
    run.menu("fullscreen");
    assert_eq!(run.full_screen_requests(), [true, false]);
    assert_eq!(run.menu_label("fullscreen"), "Enter Full Screen");
}

#[test]
fn as_icons_shows_the_folder_as_a_grid_of_named_icons_read_as_a_list() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("view-icons");
    assert_eq!(run.rows(), Vec::<String>::new(), "the table is gone");
    let shown = |names: &[&str]| {
        names
            .iter()
            .map(|n| (n.to_string(), false))
            .collect::<Vec<_>>()
    };
    assert_eq!(run.icons(), shown(&["a.txt", "b.txt", "Notes"]));
    run.click_icon("b.txt");
    assert_eq!(run.selected_icons(), ["b.txt"]);
    run.check_golden("icon-view");
    run.menu("view-list");
    assert_eq!(run.selected_rows(), ["b.txt"], "the selection stays");
}

#[test]
fn the_keyboard_moves_extends_and_types_to_select_in_the_icon_grid() {
    const LEFT: u32 = 0xff51;
    const RIGHT: u32 = 0xff53;
    const HOME: u32 = 0xff50;
    const END: u32 = 0xff57;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("view-icons");
    run.click_icon("a.txt");
    run.key(RIGHT, None);
    assert_eq!(run.selected_icons(), ["b.txt"]);
    run.key(END, None);
    assert_eq!(run.selected_icons(), ["Notes"]);
    run.key(HOME, None);
    assert_eq!(run.selected_icons(), ["a.txt"]);
    run.key(LEFT, None);
    assert_eq!(run.selected_icons(), ["a.txt"], "the first item stays");
    run.key_with(RIGHT, None, Mods::SHIFT);
    run.key_with(RIGHT, None, Mods::SHIFT);
    assert_eq!(run.selected_icons(), ["a.txt", "b.txt", "Notes"]);
    run.type_text("b");
    assert_eq!(run.selected_icons(), ["b.txt"]);
}

#[test]
fn each_window_keeps_its_own_view() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let first = run.window;
    run.menu("new");
    let second = run.windows()[1];
    run.activate(second);
    run.menu("view-icons");
    assert_eq!(run.rows(), Vec::<String>::new());
    run.activate(first);
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
}
