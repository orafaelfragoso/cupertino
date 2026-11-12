//! The browser window on CupertinoKit, run headless: what it lists and reads to assistive technology, where the
//! sidebar and the menus take it, and a folder changing under it.

#![cfg(feature = "app")]

use crate::common;

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
        Run::hosted(trash_listed, notices, false, common::Host::default())
    }

    /// The window of [`Run::with`] on a window system that can do what `host` says, with Reduce Transparency on when
    /// `opaque`.
    fn hosted(trash_listed: bool, notices: Notices, opaque: bool, host: common::Host) -> Run {
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
        let preferences = cupertino_kit::theme::ThemePreferences {
            appearance: cupertino_kit::theme::Appearance::Light,
            reduce_transparency: opaque,
            reduce_motion: false,
        };
        Run::start_hosted(
            home,
            documents,
            places,
            notices,
            None,
            preferences,
            |start| start,
            host,
        )
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
    // the copy runs beside the window
    run.wait_until(|run| run.rows().contains(&"b copy.txt".to_owned()));
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
fn a_window_keeps_its_view_and_bars_and_a_new_window_starts_with_the_last_choice() {
    use cupertino_files::window::ViewMode;
    use cupertino_files::window_prefs::WindowPrefs;
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    let prefs = home.path().join("config/cupertino/files/window.json");
    let kept = WindowPrefs {
        view: ViewMode::Icons,
        path_bar: true,
        status_bar: false,
        column_width: 245.0,
    };
    kept.save(&prefs).unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let file = prefs.clone();
    let preferences = cupertino_kit::theme::ThemePreferences {
        appearance: cupertino_kit::theme::Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = Run::start_configured(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        move |start| start.with_prefs_file(file),
    );
    // the window opens as the file says: the icon view, the path bar shown and the status bar hidden
    assert_eq!(run.menu_label("toggle-path-bar"), "Hide Path Bar");
    assert_eq!(run.menu_label("toggle-status-bar"), "Show Status Bar");
    assert!(
        run.named(Role::Table).is_empty(),
        "the icon view has no table"
    );
    let first = run.window;

    run.menu("toggle-status-bar");
    run.menu("view-list");
    let chosen = WindowPrefs {
        view: ViewMode::List,
        path_bar: true,
        status_bar: true,
        column_width: 245.0,
    };
    assert_eq!(WindowPrefs::load(&prefs), chosen, "the last choice is kept");

    run.menu("new");
    let second = *run.windows().last().unwrap();
    assert_ne!(second, first);
    run.activate(second);
    assert_eq!(run.menu_label("toggle-status-bar"), "Hide Status Bar");
    assert_eq!(
        run.named(Role::Table),
        ["Documents"],
        "the new window lists"
    );

    // each window keeps its own: hiding the new window's path bar leaves the first one's
    run.menu("toggle-path-bar");
    assert_eq!(run.menu_label("toggle-path-bar"), "Show Path Bar");
    assert!(!WindowPrefs::load(&prefs).path_bar);
    run.activate(first);
    assert_eq!(run.menu_label("toggle-path-bar"), "Hide Path Bar");
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
    let notices = Notices::start(Some(bus.connect()));
    let mut run = Run::with(false, notices.clone());
    run.wait_until(|run| run.rows().len() == 3);
    std::fs::write(run.home().join("Trash"), "").unwrap();
    run.menu("select-all");
    run.menu("move-to-trash");
    assert_eq!(
        received.recv_timeout(Duration::from_secs(5)).unwrap(),
        "3 items couldn't be moved to the Trash / File exists."
    );
    notices.flush(Duration::from_secs(5));
    assert!(received.try_recv().is_err(), "one notice");
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
        // rows read as an outline: each top-level row one of the folder's three
        let mut lines = vec![format!("      Row 3 items{state}")];
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
            "      Group \"Tags\"",
            "  Toolbar",
            "    Button \"Back\" disabled",
            "    Button \"Forward\" disabled",
            "    StaticText \"Documents\"",
            "    RadioGroup \"View As\"",
            "      RadioButton \"Icons\" off focusable",
            "      RadioButton \"List\" on focusable selected",
            "      RadioButton \"Columns\" off focusable",
            "      RadioButton \"Gallery\" off focusable",
            "    PopUpButton \"View\" focusable collapsed",
            "    Button \"Share\" disabled",
            "    PopUpButton \"Tags\" focusable collapsed",
            "    PopUpButton \"Action\" focusable collapsed",
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
        lines.extend(row(
            " collapsed",
            ["\"Notes\"", "<date>", "\"--\"", "\"Folder\""],
        ));
        lines.push("  Splitter \"Sidebar\" = 223 of 0..919 focusable".to_owned());
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
fn the_go_menu_shows_recents_and_the_home_folder_and_back_returns() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("go-recents");
    assert_eq!(run.title(), "Recents");
    run.menu("go-home");
    assert_ne!(run.title(), "Recents");
    run.wait_until(|run| run.rows().iter().any(|row| row == "Documents"));
    run.menu("go-back");
    assert_eq!(run.title(), "Recents");
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
fn a_name_too_long_for_its_cell_keeps_to_the_cells_lines() {
    const LONG: &str =
        "Quarterly report with a very long name that goes on and on past two lines.txt";
    let mut run = Run::new();
    std::fs::write(run.home().join("Documents").join(LONG), b"").unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    run.menu("view-icons");
    run.settle();
    let cell = run
        .icon_cells()
        .into_iter()
        .find(|(name, _, _)| name == LONG)
        .map(|(_, rect, _)| rect)
        .expect("the long name's cell");
    // the name is read whole, but laid out on no more lines than its cell gives it: the last ends in an ellipsis
    let text = run.bounds(Role::StaticText, LONG);
    let short = run.bounds(Role::StaticText, "a.txt");
    assert!(
        text.max_y() <= cell.max_y() + 0.5,
        "the name {text:?} stays in its cell {cell:?}"
    );
    assert!(
        text.height() > short.height() && text.height() <= 2.0 * short.height() + 0.5,
        "two lines: {text:?} against one: {short:?}"
    );
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

#[test]
fn the_trash_place_comes_with_the_first_item_trashed_and_leaves_with_the_trash() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let places = |run: &Run| run.named(Role::ListItem);
    assert!(
        !places(&run).contains(&"Trash".to_owned()),
        "nothing trashed yet"
    );
    run.click_row("a.txt");
    run.menu("move-to-trash");
    run.wait_until(|run| places(run).contains(&"Trash".to_owned()));
    run.click("place-Trash");
    assert_eq!(run.title(), "Trash");
    run.wait_until(|run| run.rows() == ["a.txt"]);

    // another program empties the trash and removes its folder: the place goes, and the window with it
    run.click("place-Documents");
    std::fs::remove_dir_all(run.home().join("Trash/files")).unwrap();
    run.wait_until(|run| !places(run).contains(&"Trash".to_owned()));
    assert_eq!(places(&run), ["Documents", "Downloads"]);
}

/// The grid's items in the window, by name.
fn icon_frames(run: &Run) -> HashMap<String, cupertino_kit::geometry::Rect> {
    run.icon_cells()
        .into_iter()
        .map(|(name, rect, _)| (name, rect))
        .collect()
}

#[test]
fn a_band_from_empty_space_selects_the_icons_it_touches_and_modifiers_toggle_them() {
    use cupertino_kit::geometry::Point;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("view-icons");
    let frames = icon_frames(&run);
    let grid = run.bounds(Role::List, &run.title());
    // under the row of items, clear of the window's edges
    let below = frames.values().map(|f| f.max_y()).fold(0.0, f64::max) + 40.0;
    assert!(
        below < grid.max_y() - 40.0,
        "the grid reaches below its items"
    );
    // clear of the divider beside the sidebar
    let empty = Point::new(frames["a.txt"].center().x - 20.0, below);
    run.hold(empty, frames["b.txt"].center(), Mods::NONE);
    run.driver.look(run.window, "files-band-icons").unwrap();
    run.release(frames["b.txt"].center(), Mods::NONE);
    assert_eq!(run.selected_icons(), ["a.txt", "b.txt"]);

    // Shift keeps what was selected and adds what the band touches
    let corner = Point::new(frames["Notes"].max_x() + 40.0, below);
    run.drag(corner, frames["Notes"].center(), Mods::SHIFT);
    assert_eq!(run.selected_icons(), ["a.txt", "b.txt", "Notes"]);
    // Primary takes away what the band touches again
    run.drag(corner, frames["Notes"].center(), Mods::PRIMARY);
    assert_eq!(run.selected_icons(), ["a.txt", "b.txt"]);
    // a plain press on empty space clears the selection
    run.press_at(empty, cupertino_kit::platform::MouseButton::Left);
    assert_eq!(run.selected_icons(), Vec::<String>::new());
}

#[test]
fn a_band_pressed_at_the_bottom_edge_scrolls_the_grid_under_it() {
    use cupertino_kit::geometry::Point;
    use cupertino_kit::platform::{MouseButton, PointerEvent, Timestamp};
    let mut run = Run::new();
    for i in 0..60 {
        std::fs::write(run.home().join(format!("Documents/item {i:02}.txt")), "x").unwrap();
    }
    run.menu("view-icons");
    run.wait_until(|run| icon_frames(run).contains_key("item 00.txt"));
    let grid = run.bounds(Role::List, &run.title());
    let first = icon_frames(&run)["a.txt"];
    let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
    // in the gap after the first item
    let from = Point::new(first.max_x() + 1.0, first.min_y() + 2.0);
    // the band reaches into the second column
    let edge = Point::new(first.max_x() + 40.0, grid.max_y() - 12.0);
    let button = |position, pressed| {
        pointer(PointerEvent::Button {
            position,
            button: MouseButton::Left,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        })
    };
    run.send(pointer(PointerEvent::Motion { position: from }));
    run.send(button(from, true));
    run.send(pointer(PointerEvent::Motion { position: edge }));
    let before = run.selected_icons();
    let top = |run: &Run| icon_frames(run)["a.txt"].min_y();
    let at = top(&run);
    // the pointer stays still: the edge timer goes on scrolling
    run.frames(500, 50);
    assert!(top(&run) < at, "the content moved up under the band");
    let after = run.selected_icons();
    assert!(
        after.iter().any(|name| !before.contains(name)),
        "the band grew over the items scrolled in: {before:?} {after:?}"
    );
    run.send(button(edge, false));
    let selected = run.selected_icons().len();
    run.frames(300, 50);
    assert_eq!(run.selected_icons().len(), selected, "a release ends it");
}

#[test]
fn ctrl_with_a_press_opens_the_context_menu_on_the_icon() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("view-icons");
    let b = icon_frames(&run)["b.txt"];
    run.press_with(
        b.center(),
        cupertino_kit::platform::MouseButton::Left,
        Mods::CTRL,
    );
    assert!(
        run.context_menu().is_some(),
        "Ctrl and a press is a secondary click"
    );
    assert_eq!(run.selected_icons(), ["b.txt"]);
    assert!(run.menu_rows().iter().any(|(label, _)| label == "Rename"));
}

#[test]
fn a_double_click_opens_and_a_click_on_the_selected_name_renames_after_a_pause() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("view-icons");
    let field = |run: &Run| {
        run.driver
            .runtime()
            .find(run.window, &Key::from(format!("name-field/{}", 0)))
            .is_some()
    };
    run.click_icon("b.txt");
    run.frames(600, 100);
    assert!(!field(&run), "the first click only selects");
    // the second click, on the name of the only selected item
    let name = run.bounds(Role::StaticText, "b.txt");
    run.press_at(name.center(), cupertino_kit::platform::MouseButton::Left);
    assert!(!field(&run), "nothing yet: it may be a double click");
    run.frames(600, 100);
    assert!(field(&run), "the name is edited once the interval passed");
    run.key(0xff1b, None);

    run.click_icon("Notes");
    run.click_icon("Notes");
    run.wait_until(|run| run.title() == "Notes");
    run.frames(600, 100);
    assert!(!field(&run), "a double click renames nothing");
}

#[test]
fn tab_and_shift_tab_select_the_next_and_previous_item_by_name() {
    const TAB: u32 = 0xff09;
    const LEFT_TAB: u32 = 0xfe20;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("view-icons");
    run.menu("sort-size");
    let order: Vec<String> = run.icons().into_iter().map(|(n, _)| n).collect();
    assert_eq!(order[..2], ["b.txt", "a.txt"], "shown by size");
    run.click_icon("a.txt");
    run.key(TAB, None);
    assert_eq!(run.selected_icons(), ["b.txt"], "by name, not as shown");
    run.key(TAB, None);
    assert_eq!(run.selected_icons(), ["Notes"]);
    run.key(TAB, None);
    assert_eq!(run.selected_icons(), ["Notes"], "the last stays");
    // past the last the keyboard went on to the next control; Shift+Tab brings it back to the grid, then goes on
    run.key_with(LEFT_TAB, None, Mods::SHIFT);
    assert_eq!(run.selected_icons(), ["Notes"]);
    run.key_with(LEFT_TAB, None, Mods::SHIFT);
    assert_eq!(run.selected_icons(), ["b.txt"]);
}

/// A point on the text of a list row's name, near its start: past the text the name cell is the row's blank part.
fn on_text(name: cupertino_kit::geometry::Rect) -> cupertino_kit::geometry::Point {
    cupertino_kit::geometry::Point::new(name.min_x() + 6.0, name.center().y)
}

/// The list's row showing `name`.
fn row_bounds(run: &Run, name: &str) -> cupertino_kit::geometry::Rect {
    run.listed()
        .into_iter()
        .find(|(shown, _, _)| shown == name)
        .unwrap_or_else(|| panic!("no row {name:?}"))
        .1
}

#[test]
fn a_folders_triangle_and_the_arrows_open_and_close_it_in_the_list() {
    use cupertino_kit::geometry::Point;
    const LEFT: u32 = 0xff51;
    const RIGHT: u32 = 0xff53;
    let mut run = Run::new();
    std::fs::write(run.home().join("Documents/Notes/todo.txt"), "t").unwrap();
    run.wait_until(|run| run.rows().len() == 3);
    run.click_row("a.txt");
    let notes = row_bounds(&run, "Notes");
    // the triangle sits at the row's leading edge, before the name
    let triangle = Point::new(notes.min_x() + 18.0, notes.min_y() + notes.height() / 2.0);
    run.press_at(triangle, cupertino_kit::platform::MouseButton::Left);
    run.wait_until(|run| run.rows().len() == 4);
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes", "todo.txt"]);
    assert_eq!(
        run.selected_rows(),
        ["a.txt"],
        "the triangle selects nothing"
    );
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let levels: Vec<(Option<usize>, Option<bool>)> = snapshot
        .accessibility
        .iter()
        .filter(|(_, n)| n.role == Role::Row && n.level.is_some())
        .map(|(_, n)| (n.level, n.states.expanded))
        .collect();
    assert!(
        levels.contains(&(Some(1), Some(true))),
        "Notes reads as open: {levels:?}"
    );
    assert!(
        levels.contains(&(Some(2), None)),
        "todo.txt one level in: {levels:?}"
    );
    run.driver
        .look(run.window, "files-list-disclosure")
        .unwrap();

    run.press_at(triangle, cupertino_kit::platform::MouseButton::Left);
    run.wait_until(|run| run.rows().len() == 3);
    run.click_row("Notes");
    run.key(RIGHT, None);
    run.wait_until(|run| run.rows().len() == 4);
    run.key(LEFT, None);
    run.wait_until(|run| run.rows().len() == 3);
    run.key(RIGHT, None);
    run.wait_until(|run| run.rows().len() == 4);
    run.menu("view-icons");
    run.menu("view-list");
    assert_eq!(run.rows().len(), 3, "leaving the list closes its folders");
}

#[test]
fn a_band_under_the_rows_selects_the_rows_it_crosses() {
    use cupertino_kit::geometry::Point;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let notes = row_bounds(&run, "Notes");
    let b = row_bounds(&run, "b.txt");
    let under = Point::new(notes.min_x() + notes.width() / 2.0, notes.max_y() + 40.0);
    // up and across, so the band has an area
    let corner = Point::new(b.center().x + 120.0, b.center().y);
    run.hold(under, corner, Mods::NONE);
    assert!(
        run.driver
            .runtime()
            .find(run.window, &Key::from("band-layer"))
            .is_some(),
        "the band shows while it is drawn"
    );
    run.driver.look(run.window, "files-band-list").unwrap();
    run.release(corner, Mods::NONE);
    assert_eq!(run.selected_rows(), ["b.txt", "Notes"]);
    run.drag(under, Point::new(under.x, notes.max_y() + 20.0), Mods::NONE);
    assert_eq!(
        run.selected_rows(),
        Vec::<String>::new(),
        "a band over no row"
    );
    run.click_row("a.txt");
    run.drag(under, notes.center(), Mods::SHIFT);
    assert_eq!(
        run.selected_rows(),
        ["a.txt", "Notes"],
        "Shift adds to the selection"
    );
}

#[test]
fn in_the_list_ctrl_click_opens_the_menu_a_double_click_opens_and_a_pause_renames() {
    use cupertino_kit::platform::MouseButton;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let b = row_bounds(&run, "b.txt");
    run.press_with(b.center(), MouseButton::Left, Mods::CTRL);
    assert!(run.context_menu().is_some());
    assert_eq!(run.selected_rows(), ["b.txt"]);
    run.close_menu();

    let field = |run: &Run| {
        run.driver
            .runtime()
            .find(run.window, &Key::from("name-field/0"))
            .is_some()
    };
    run.frames(600, 100);
    let name = run.bounds(Role::StaticText, "b.txt");
    run.press_at(on_text(name), MouseButton::Left);
    assert!(!field(&run));
    run.frames(600, 100);
    assert!(
        field(&run),
        "a press on the selected name renames it after a pause"
    );
    run.key(0xff1b, None);

    run.click_row("Notes");
    run.click_row("Notes");
    run.wait_until(|run| run.title() == "Notes");
}

/// A left press at `at` in `run`'s window, without its release.
fn press_down(run: &mut Run, at: cupertino_kit::geometry::Point) {
    use cupertino_kit::platform::{MouseButton, PointerEvent, Timestamp};
    let pointer = |event| WindowEvent::Pointer(event, Timestamp(0));
    run.send(pointer(PointerEvent::Motion { position: at }));
    run.send(pointer(PointerEvent::Button {
        position: at,
        button: MouseButton::Left,
        pressed: true,
        serial: 1,
        mods: Mods::NONE,
    }));
}

#[test]
fn in_the_list_a_key_the_triangle_or_a_held_press_keeps_a_pending_rename_from_starting() {
    use cupertino_kit::geometry::Point;
    use cupertino_kit::platform::MouseButton;
    const RIGHT: u32 = 0xff53;
    const LEFT: u32 = 0xff51;
    let mut run = Run::new();
    std::fs::write(run.home().join("Documents/Notes/todo.txt"), "t").unwrap();
    run.wait_until(|run| run.rows().len() == 3);
    let field = |run: &Run| {
        run.driver
            .runtime()
            .find(run.window, &Key::from("name-field/0"))
            .is_some()
    };
    run.click_row("Notes");
    run.frames(600, 100);
    let name = run.bounds(Role::StaticText, "Notes");

    // a key the table takes for itself: Right opens the folder, and no rename follows
    run.press_at(on_text(name), MouseButton::Left);
    run.key(RIGHT, None);
    run.wait_until(|run| run.rows().len() == 4);
    run.frames(600, 100);
    assert!(!field(&run), "a key cancels the rename");
    run.key(LEFT, None);
    run.wait_until(|run| run.rows().len() == 3);
    run.frames(600, 100);

    // the triangle stops its press before the row sees it
    let notes = row_bounds(&run, "Notes");
    let triangle = Point::new(notes.min_x() + 18.0, notes.center().y);
    run.press_at(on_text(name), MouseButton::Left);
    run.press_at(triangle, MouseButton::Left);
    run.wait_until(|run| run.rows().len() == 4);
    run.frames(600, 100);
    assert!(!field(&run), "the triangle cancels the rename");
    run.press_at(triangle, MouseButton::Left);
    run.wait_until(|run| run.rows().len() == 3);
    run.frames(600, 100);

    // a press held past the interval may still be a drag: the rename waits for the release
    press_down(&mut run, on_text(name));
    run.frames(600, 100);
    assert!(!field(&run), "no rename while the button is held");
    run.release(on_text(name), Mods::NONE);
    run.frames(100, 50);
    assert!(field(&run), "the release starts the rename that came due");
}

#[test]
fn in_the_list_ctrl_click_on_a_rows_margin_or_triangle_opens_the_menu() {
    use cupertino_kit::geometry::Point;
    use cupertino_kit::platform::MouseButton;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.click_row("b.txt");
    let a = row_bounds(&run, "a.txt");
    // the leading margin, where a folder's triangle would be: no cell of the row is there
    run.press_with(
        Point::new(a.min_x() + 12.0, a.center().y),
        MouseButton::Left,
        Mods::CTRL,
    );
    assert!(
        run.context_menu().is_some(),
        "Ctrl with a press is the secondary click"
    );
    assert_eq!(
        run.selected_rows(),
        ["a.txt"],
        "the menu's item, not one more"
    );
    run.close_menu();
    let notes = row_bounds(&run, "Notes");
    run.press_with(
        Point::new(notes.min_x() + 18.0, notes.center().y),
        MouseButton::Left,
        Mods::CTRL,
    );
    assert!(run.context_menu().is_some());
    assert_eq!(
        run.rows().len(),
        3,
        "Ctrl on a triangle does not open the folder"
    );
    run.close_menu();
}

#[test]
fn a_band_held_while_the_view_changes_ends_with_the_list() {
    use cupertino_kit::geometry::Point;
    use cupertino_kit::platform::{PointerEvent, Timestamp};
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let notes = row_bounds(&run, "Notes");
    let under = Point::new(notes.center().x, notes.max_y() + 40.0);
    run.hold(under, Point::new(under.x, notes.center().y), Mods::NONE);
    assert_eq!(run.selected_rows(), ["Notes"]);
    run.menu("view-icons");
    run.menu("view-list");
    run.click_row("a.txt");
    let b = row_bounds(&run, "b.txt");
    // a move with no button held draws no band
    let position = Point::new(b.center().x, b.min_y() - 30.0);
    run.send(WindowEvent::Pointer(
        PointerEvent::Motion { position },
        Timestamp(0),
    ));
    assert_eq!(run.selected_rows(), ["a.txt"]);
}

#[test]
fn in_the_list_tab_and_shift_tab_select_by_name() {
    const TAB: u32 = 0xff09;
    const LEFT_TAB: u32 = 0xfe20;
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("sort-size");
    assert_eq!(run.rows()[..2], ["b.txt", "a.txt"]);
    run.click_row("a.txt");
    run.key(TAB, None);
    assert_eq!(run.selected_rows(), ["b.txt"]);
    run.key(TAB, None);
    assert_eq!(run.selected_rows(), ["Notes"]);
    run.key_with(LEFT_TAB, None, Mods::SHIFT);
    assert_eq!(run.selected_rows(), ["b.txt"]);
    run.key(0xff54, None);
    assert_eq!(
        run.selected_rows(),
        ["a.txt"],
        "the arrows go on from the row Tab chose"
    );
}

#[test]
fn the_toolbars_view_tags_and_action_buttons_open_their_menus_below_them() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    let labels = |run: &mut Run| -> Vec<(String, bool)> { run.menu_rows() };

    run.click_named(Role::PopUpButton, "View");
    assert!(run.context_menu().is_some(), "the View menu");
    run.driver.look(run.window, "files-toolbar").unwrap();
    let rows = labels(&mut run);
    assert_eq!(
        rows[..2],
        [("Icons".to_owned(), true), ("List".to_owned(), true)]
    );
    assert!(rows.contains(&("Columns".to_owned(), true)));
    assert!(rows.contains(&("Gallery".to_owned(), true)));
    run.choose("Icons");
    assert_eq!(run.rows(), Vec::<String>::new(), "shown as icons");
    run.click_named(Role::PopUpButton, "View");
    run.choose("List");
    run.wait_until(|run| run.rows().len() == 3);

    run.click_named(Role::PopUpButton, "Action");
    let rows = labels(&mut run);
    assert!(
        rows.contains(&("New Folder".to_owned(), true)),
        "the folder's menu: {rows:?}"
    );
    run.close_menu();
    run.click_row("a.txt");
    run.click_named(Role::PopUpButton, "Action");
    let rows = labels(&mut run);
    assert!(
        rows.contains(&("Rename".to_owned(), true)),
        "the selection's menu: {rows:?}"
    );
    run.close_menu();

    // with a selection Tags opens the tags editor, a popover of its own, and closes it again
    let popovers = |run: &Run| {
        run.driver
            .runtime()
            .windows()
            .into_iter()
            .filter(|(key, _)| format!("{key:?}").contains("tags/"))
            .count()
    };
    run.click_named(Role::PopUpButton, "Tags");
    assert_eq!(popovers(&run), 1, "the tags popover");
    run.click_named(Role::PopUpButton, "Tags");
    assert_eq!(popovers(&run), 0);
    // without one its menu shows the tag colours unavailable
    run.click_row_with("a.txt", Mods::PRIMARY);
    assert!(run.selected_rows().is_empty());
    run.click_named(Role::PopUpButton, "Tags");
    let rows = labels(&mut run);
    assert_eq!(rows.len(), 7, "the tag colours: {rows:?}");
    assert!(rows.iter().all(|(_, enabled)| !*enabled), "{rows:?}");
    run.close_menu();
}

#[test]
fn a_click_on_a_folder_of_the_path_bar_goes_to_it() {
    let mut run = Run::new();
    run.wait_until(|run| run.rows().len() == 3);
    run.menu("toggle-path-bar");
    run.click_row("Notes");
    run.click_row("Notes");
    run.wait_until(|run| run.title() == "Notes");
    let segments = run.named(Role::Button);
    assert!(
        segments.ends_with(&["Documents".to_owned(), "Notes".to_owned()]),
        "{segments:?}"
    );
    run.driver.look(run.window, "files-path-bar").unwrap();
    run.click_named(Role::Button, "Documents");
    run.wait_until(|run| run.title() == "Documents");
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"]);
}

#[test]
fn the_sidebar_shows_the_sidebar_material_where_the_window_can_and_its_fill_where_it_cannot() {
    use cupertino_kit::platform::{Capabilities, WindowRequest};
    let materials = |opaque: bool| {
        let host = common::Host {
            capabilities: Capabilities {
                app_effects: true,
                ..Capabilities::default()
            },
            ..common::Host::default()
        };
        let mut run = Run::hosted(false, Notices::start(None), opaque, host);
        run.wait_until(|run| run.rows().len() == 3);
        let system = run.driver.runtime().window_system();
        let requests = system.requests(run.window).unwrap();
        requests
            .iter()
            .rev()
            .find_map(|request| match request {
                WindowRequest::Materials(regions) => Some(regions.clone()),
                _ => None,
            })
            .unwrap_or_default()
    };
    let regions = materials(false);
    let sidebar = regions
        .iter()
        .find(|r| r.material.to_lowercase().contains("sidebar"))
        .unwrap_or_else(|| panic!("the sidebar material: {regions:?}"));
    assert_eq!(sidebar.rect.min_x(), 0.0, "from the window's leading edge");
    assert!(
        sidebar.rect.width() >= 200.0,
        "under the whole sidebar: {sidebar:?}"
    );
    let reduced = materials(true);
    assert!(
        !reduced
            .iter()
            .any(|r| r.material.to_lowercase().contains("sidebar")),
        "with Reduce Transparency the sidebar paints its opaque fill: {reduced:?}"
    );
    // the content pane's background is the compositor's either way, under the toolbar's glass
    for regions in [&regions, &reduced] {
        let background = regions
            .iter()
            .position(|r| r.material == "WindowBackground")
            .unwrap_or_else(|| panic!("the window background: {regions:?}"));
        let glass = regions
            .iter()
            .position(|r| r.glass.is_some())
            .unwrap_or_else(|| panic!("the toolbar's glass: {regions:?}"));
        assert!(
            background < glass,
            "the glass lies over the background: {regions:?}"
        );
    }
}

/// A point in the toolbar row that none of its items covers, scanning from the row's trailing end.
fn empty_toolbar_point(run: &Run) -> cupertino_kit::geometry::Point {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let nodes: HashMap<NodeId, &AccessibilityNode> = snapshot
        .accessibility
        .iter()
        .map(|(id, n)| (*id, n))
        .collect();
    let toolbar = nodes
        .values()
        .find(|n| n.role == Role::Toolbar)
        .expect("a toolbar");
    let items: Vec<_> = toolbar.children.iter().map(|c| nodes[c].bounds).collect();
    let bar = toolbar.bounds;
    let y = bar.min_y() + bar.height() / 2.0;
    let mut x = bar.max_x() - 12.0;
    while x > bar.min_x() + 12.0 {
        let p = cupertino_kit::geometry::Point::new(x, y);
        if items.iter().all(|r| !r.outset(4.0).contains(p)) {
            return p;
        }
        x -= 4.0;
    }
    panic!("the toolbar row has no room between its items")
}

fn window_requests(run: &Run) -> Vec<cupertino_kit::platform::WindowRequest> {
    let system = run.driver.runtime().window_system();
    system.requests(run.window).unwrap().to_vec()
}

#[test]
fn the_toolbar_moves_the_window_on_a_drag_and_zooms_it_on_a_double_click() {
    use cupertino_kit::geometry::Point;
    use cupertino_kit::platform::{MouseButton, PointerEvent, Timestamp, WindowRequest};
    let mut run = Run::new();
    let at = |time: u64, event| WindowEvent::Pointer(event, Timestamp(time));
    let button = |position, pressed| PointerEvent::Button {
        position,
        button: MouseButton::Left,
        pressed,
        serial: 1,
        mods: Mods::NONE,
    };
    let drag = |run: &mut Run, from: Point, time: u64| {
        let to = Point::new(from.x + 40.0, from.y + 30.0);
        run.send(at(time, PointerEvent::Motion { position: from }));
        run.send(at(time, button(from, true)));
        run.send(at(time + 1, PointerEvent::Motion { position: to }));
        run.send(at(time + 2, button(to, false)));
        run.send(at(time + 3, PointerEvent::Leave));
    };
    let moves = |run: &Run| {
        window_requests(run)
            .iter()
            .filter(|r| **r == WindowRequest::Move)
            .count()
    };
    // the search field keeps its drag
    let search = run.bounds(Role::TextInput, "Search");
    drag(&mut run, search.center(), 0);
    assert_eq!(moves(&run), 0);
    let gap = empty_toolbar_point(&run);
    drag(&mut run, gap, 10_000_000_000);
    assert_eq!(
        moves(&run),
        1,
        "a drag on the toolbar between its items moves the window"
    );
    for time in [20_000_000_000, 20_100_000_000] {
        run.send(at(time, button(gap, true)));
        run.send(at(time + 1, button(gap, false)));
    }
    assert!(window_requests(&run).contains(&WindowRequest::Maximize(true)));
}

/// The time the next test's window words its dates against: noon (UTC) of a day long before the test runs.
fn fixed_now() -> std::time::SystemTime {
    std::time::UNIX_EPOCH + Duration::from_secs(1_792_670_400)
}

#[test]
fn the_list_words_its_dates_against_the_clock_it_was_started_with() {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    std::fs::create_dir_all(&documents).unwrap();
    let file = documents.join("a.txt");
    std::fs::write(&file, "a").unwrap();
    let modified = fixed_now() - Duration::from_secs(2 * 3_600);
    std::fs::File::options()
        .write(true)
        .open(&file)
        .and_then(|f| f.set_modified(modified))
        .unwrap();
    let places = vec![place("Documents", &documents, PlaceKind::Documents)];
    let preferences = cupertino_kit::theme::ThemePreferences {
        appearance: cupertino_kit::theme::Appearance::Light,
        reduce_transparency: false,
        reduce_motion: false,
    };
    let mut run = Run::start_configured(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        |start| start.with_clock(fixed_now),
    );
    run.wait_until(|run| run.rows() == ["a.txt"]);
    let date = cupertino_files::format::relative_date(modified, fixed_now());
    assert!(
        run.read_aloud().contains(&date),
        "{date:?} in {:?}",
        run.read_aloud()
    );
    // the system's clock would word it as a date weeks ago, never as today or yesterday
    assert_ne!(
        date,
        cupertino_files::format::relative_date(modified, std::time::SystemTime::now())
    );
}
