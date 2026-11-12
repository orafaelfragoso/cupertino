//! The browser window as assistive technology and the keyboard meet it, run headless: every control's role, name,
//! state and default action performed as a screen reader asks for it; decorative pictures; the Tab order and where the
//! keyboard goes when a menu or a rename closes; what the live region announces; and the colours of a selected row and
//! of the focus ring in every appearance.

#![cfg(feature = "app")]

use crate::common;

use std::collections::HashMap;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::Duration;

use common::{place, Host, Run};
use cupertino_files::kit::Observer;
use cupertino_files::notice::Notices;
use cupertino_files::places::PlaceKind;
use cupertino_kit::core::{
    AccessibilityAdapter, AccessibilityNode, AccessibleValue, Action, ActionData, NodeId, Role,
};
use cupertino_kit::geometry::Point;
use cupertino_kit::platform::{Mods, PlatformEvent, WindowStates};
use cupertino_kit::render::{Paint, PaintNodeId, PaintOp};
use cupertino_kit::theme::{
    Appearance, Color, ColorRole, StateVariant, ThemePreferences, ThemeTokens,
};
use cupertino_test_bus::{missing_tool, PrivateBus};
use cupertino_theme::Theme;
use cupertino_ui_test::{describe, semantic_outline, whole_tree, AssistiveTechnology};
use zbus::blocking::Connection;
use zbus::zvariant::OwnedObjectPath;

const TAB: u32 = 0xff09;
const LEFT_TAB: u32 = 0xfe20;
const ESCAPE: u32 = 0xff1b;
const RETURN: u32 = 0xff0d;
const SPACE: u32 = 0x20;
const MENU_KEY: u32 = 0xff67;
const LEFT: u32 = 0xff51;

/// A window on `home/Documents`, which holds `a.txt`, `b.txt` and the folder `Notes` with `todo.txt` in it, shown as a
/// list in `appearance`, with Documents and Downloads in the sidebar and an assistive technology listening.
fn start(appearance: Appearance) -> (Run, AssistiveTechnology) {
    start_with(appearance, false, None)
}

/// [`start`] with Reduce Motion on when `still`, and every transfer calling `observer`.
fn start_with(
    appearance: Appearance,
    still: bool,
    observer: Option<Observer>,
) -> (Run, AssistiveTechnology) {
    let at = AssistiveTechnology::new();
    let run = start_hosted(appearance, still, observer, at.adapter());
    (run, at)
}

/// [`start_with`] read by the assistive technology `accessibility`.
fn start_hosted(
    appearance: Appearance,
    still: bool,
    observer: Option<Observer>,
    accessibility: Box<dyn AccessibilityAdapter>,
) -> Run {
    let home = tempfile::tempdir().unwrap();
    let documents = home.path().join("Documents");
    let downloads = home.path().join("Downloads");
    std::fs::create_dir_all(documents.join("Notes")).unwrap();
    std::fs::create_dir_all(&downloads).unwrap();
    std::fs::write(documents.join("Notes/todo.txt"), "t").unwrap();
    std::fs::write(documents.join("b.txt"), "bb").unwrap();
    std::fs::write(documents.join("a.txt"), "a").unwrap();
    let places = vec![
        place("Documents", &documents, PlaceKind::Documents),
        place("Downloads", &downloads, PlaceKind::Downloads),
    ];
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: still,
    };
    let host = Host {
        accessibility: Some(accessibility),
        ..Host::default()
    };
    let configure = move |start: cupertino_files::kit::Start| match observer {
        Some(observer) => start.with_transfer_observer(observer),
        None => start,
    };
    let mut run = Run::start_hosted(
        home,
        documents,
        places,
        Notices::start(None),
        None,
        preferences,
        configure,
        host,
    );
    run.wait_until(|run| run.rows().len() == 3);
    run
}

fn nodes(run: &Run) -> HashMap<NodeId, AccessibilityNode> {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    snapshot.accessibility.into_iter().collect()
}

/// The accessible node with `role` and `name`.
fn find(run: &Run, role: Role, name: &str) -> NodeId {
    let found = nodes(run)
        .into_iter()
        .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name));
    found.unwrap_or_else(|| panic!("no {role:?} {name:?}")).0
}

/// The text of the first static text under `id`, in reading order.
fn first_text(nodes: &HashMap<NodeId, AccessibilityNode>, id: NodeId) -> Option<String> {
    let mut stack = vec![id];
    while let Some(id) = stack.pop() {
        let node = &nodes[&id];
        if node.role == Role::StaticText {
            return node.name.clone();
        }
        stack.extend(node.children.iter().rev());
    }
    None
}

/// The list's row whose first cell reads `name`.
fn row(run: &Run, name: &str) -> NodeId {
    let nodes = nodes(run);
    let found = nodes.iter().find(|(&id, n)| {
        n.role == Role::Row
            && n.children.iter().all(|c| nodes[c].role == Role::Cell)
            && first_text(&nodes, id).as_deref() == Some(name)
    });
    *found.unwrap_or_else(|| panic!("no row {name:?}")).0
}

/// Asks node `target` for `action`, as a screen reader does, and lets the window answer.
fn ask(
    run: &mut Run,
    at: &AssistiveTechnology,
    target: NodeId,
    action: Action,
    data: Option<ActionData>,
) {
    run.driver.ask(at, target, action, data);
    run.settle();
}

/// What the live regions said, oldest first.
fn heard(at: &AssistiveTechnology) -> Vec<String> {
    at.announcements().into_iter().map(|a| a.text).collect()
}

/// The live region node the reader holds that reads `text`.
fn speaker(run: &Run, at: &AssistiveTechnology, text: &str) -> Option<NodeId> {
    let tree = at.tree(run.window)?;
    let found = tree
        .nodes
        .into_iter()
        .find(|(_, n)| n.live.is_some() && n.name.as_deref().map(str::trim) == Some(text));
    found.map(|(id, _)| id)
}

/// The node with the keyboard focus as a screen reader reads it without its states, a row by its first cell:
/// `Row [a.txt]`.
fn focused(run: &Run) -> String {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let Some(focus) = snapshot.focus else {
        return "nothing".into();
    };
    let nodes: HashMap<NodeId, AccessibilityNode> = snapshot.accessibility.into_iter().collect();
    let Some(node) = nodes.get(&focus) else {
        return format!("{focus:?}, which is not in the tree");
    };
    match node.role {
        Role::Row => format!("Row [{}]", first_text(&nodes, focus).unwrap_or_default()),
        _ => describe(node)
            .split(" focusable")
            .next()
            .unwrap()
            .to_owned(),
    }
}

/// The children of the group named "Path": the path bar's folders, each a button and a Tab stop.
fn path_segments(run: &Run) -> Vec<String> {
    let nodes = nodes(run);
    let bar = nodes
        .values()
        .find(|n| n.role == Role::Group && n.name.as_deref() == Some("Path"))
        .expect("the path bar reads as a group named Path");
    bar.children
        .iter()
        .map(|c| &nodes[c])
        .map(|n| {
            assert_eq!(n.role, Role::Button, "a folder of the path bar");
            assert!(n.states.focusable, "{n:?} is a Tab stop");
            n.name.clone().unwrap()
        })
        .collect()
}

#[test]
fn every_control_reads_its_role_name_and_state_and_does_its_default_action_when_asked() {
    let (mut run, at) = start(Appearance::Light);
    run.menu("toggle-path-bar");

    // a place of the sidebar goes to its folder; Back and Forward come back and go again
    let downloads = find(&run, Role::ListItem, "Downloads");
    assert_eq!(nodes(&run)[&downloads].actions, [Action::Click]);
    ask(&mut run, &at, downloads, Action::Click, None);
    run.wait_until(|run| run.title() == "Downloads");
    assert!(nodes(&run)[&downloads].states.selected, "the folder shown");
    let back = find(&run, Role::Button, "Back");
    assert!(!nodes(&run)[&back].states.disabled);
    ask(&mut run, &at, back, Action::Click, None);
    run.wait_until(|run| run.title() == "Documents");
    let forward = find(&run, Role::Button, "Forward");
    ask(&mut run, &at, forward, Action::Click, None);
    run.wait_until(|run| run.title() == "Downloads");
    let back = find(&run, Role::Button, "Back");
    ask(&mut run, &at, back, Action::Click, None);
    run.wait_until(|run| run.rows().len() == 3);

    // each pop-up button opens its menu
    for name in ["View", "Tags", "Action"] {
        let button = find(&run, Role::PopUpButton, name);
        assert_eq!(nodes(&run)[&button].actions, [Action::Click]);
        ask(&mut run, &at, button, Action::Click, None);
        assert!(run.context_menu().is_some(), "{name} opens its menu");
        run.close_menu();
    }

    // the search takes its text from assistive technology
    let search = find(&run, Role::TextInput, "Search");
    let text = |t: &str| Some(ActionData::Text(t.into()));
    ask(&mut run, &at, search, Action::SetValue, text("b"));
    run.wait_until(|run| run.rows() == ["b.txt"]);
    ask(&mut run, &at, search, Action::SetValue, text(""));
    run.wait_until(|run| run.rows().len() == 3);

    // a column header reads its sort and sorts by its column
    use cupertino_kit::core::SortOrder::{self, Ascending, Descending};
    let sorts = |run: &Run| -> Vec<(String, Option<SortOrder>)> {
        let nodes = nodes(run);
        let mut headers: Vec<_> = nodes
            .values()
            .filter(|n| n.role == Role::ColumnHeader)
            .map(|n| (n.column_index, n.name.clone().unwrap(), n.sort))
            .collect();
        headers.sort_by_key(|h| h.0);
        headers
            .into_iter()
            .map(|(_, name, sort)| (name, sort))
            .collect()
    };
    let unsorted = |name: &str| (name.to_owned(), None);
    assert_eq!(
        sorts(&run),
        [
            ("Name".to_owned(), Some(Ascending)),
            unsorted("Date Modified"),
            unsorted("Size"),
            unsorted("Kind"),
        ]
    );
    let size = find(&run, Role::ColumnHeader, "Size");
    ask(&mut run, &at, size, Action::Click, None);
    assert_eq!(sorts(&run)[0].1, None);
    assert_eq!(sorts(&run)[2], ("Size".to_owned(), Some(Ascending)));
    ask(&mut run, &at, size, Action::Click, None);
    assert_eq!(sorts(&run)[2], ("Size".to_owned(), Some(Descending)));
    assert_eq!(run.rows()[..2], ["b.txt", "a.txt"], "the largest first");
    let name = find(&run, Role::ColumnHeader, "Name");
    ask(&mut run, &at, name, Action::Click, None);
    assert_eq!(sorts(&run)[0].1, Some(Ascending));

    // a row selects itself alone; a folder's row opens and closes and reads its level and place in its set
    let b = row(&run, "b.txt");
    assert_eq!(nodes(&run)[&b].actions, [Action::Click]);
    ask(&mut run, &at, b, Action::Click, None);
    assert_eq!(run.selected_rows(), ["b.txt"]);
    let notes = row(&run, "Notes");
    let read = |run: &Run, id: NodeId| {
        let n = &nodes(run)[&id];
        (n.level, n.item_index, n.item_count, n.states.expanded)
    };
    assert_eq!(read(&run, notes), (Some(1), Some(2), Some(3), Some(false)));
    assert_eq!(nodes(&run)[&notes].actions, [Action::Expand, Action::Click]);
    ask(&mut run, &at, notes, Action::Expand, None);
    run.wait_until(|run| run.rows().len() == 4);
    assert_eq!(read(&run, notes), (Some(1), Some(2), Some(3), Some(true)));
    let todo = row(&run, "todo.txt");
    assert_eq!(read(&run, todo), (Some(2), Some(0), Some(1), None));
    ask(&mut run, &at, notes, Action::Collapse, None);
    run.wait_until(|run| run.rows().len() == 3);
    assert_eq!(read(&run, notes).3, Some(false));

    // the path bar's folders go where they say
    let segments = path_segments(&run);
    assert_eq!(segments.last().map(String::as_str), Some("Documents"));
    let parent = segments[segments.len() - 2].clone();
    let up = find(&run, Role::Button, &parent);
    ask(&mut run, &at, up, Action::Click, None);
    run.wait_until(|run| run.title() == parent);

    // an icon is an item of a list, which selects itself
    let documents = find(&run, Role::ListItem, "Documents");
    ask(&mut run, &at, documents, Action::Click, None);
    run.wait_until(|run| run.title() == "Documents");
    run.menu("view-icons");
    run.wait_until(|run| run.icons().len() == 3);
    let a = find(&run, Role::ListItem, "a.txt");
    assert_eq!(nodes(&run)[&a].actions, [Action::Click]);
    ask(&mut run, &at, a, Action::Click, None);
    assert_eq!(run.selected_icons(), ["a.txt"]);
}

#[test]
fn icons_are_decorative_and_nothing_reads_a_path() {
    let (mut run, _) = start(Appearance::Light);
    run.menu("toggle-path-bar");
    run.menu("toggle-status-bar");
    let home = run.home().display().to_string();
    let check = |run: &Run, view: &str| {
        let nodes = nodes(run);
        let images: Vec<_> = nodes.values().filter(|n| n.role == Role::Image).collect();
        assert!(images.is_empty(), "{view}: icons are not read: {images:?}");
        for node in nodes.values() {
            for text in [&node.name, &node.description].into_iter().flatten() {
                // the path bar's first folder is the file system's root, named "/"
                assert!(
                    !text.contains(&home) && (text == "/" || !text.contains('/')),
                    "{view}: {:?} reads a path: {text:?}",
                    node.role
                );
            }
        }
    };
    check(&run, "the list");
    run.menu("view-icons");
    run.wait_until(|run| run.icons().len() == 3);
    check(&run, "the icons");
    // an item reads its name, and its text under it once more; its icon not at all
    let nodes = nodes(&run);
    let a = &nodes[&find(&run, Role::ListItem, "a.txt")];
    let under: Vec<_> = a.children.iter().map(|c| nodes[c].role).collect();
    assert_eq!(under, [Role::StaticText]);
}

#[test]
fn the_rubber_band_is_never_read() {
    let (mut run, _) = start(Appearance::Light);
    let count = |run: &Run| nodes(run).len();
    let before = count(&run);
    let notes = run.row_bounds("Notes");
    let under = Point::new(notes.center().x, notes.max_y() + 40.0);
    let corner = Point::new(notes.center().x + 100.0, notes.center().y);
    run.hold(under, corner, Mods::NONE);
    let band = cupertino_kit::core::Key::from("band-layer");
    assert!(run.driver.runtime().find(run.window, &band).is_some());
    assert_eq!(count(&run), before, "the band adds nothing to the tree");
    run.release(corner, Mods::NONE);
    assert_eq!(run.selected_rows(), ["Notes"]);
}

#[test]
fn tab_goes_from_the_sidebar_to_the_toolbar_the_items_and_the_path_bar_and_around() {
    let (mut run, _) = start(Appearance::Light);
    run.menu("toggle-path-bar");
    let segments = path_segments(&run);
    let mut expected: Vec<String> = [
        "ListItem \"Documents\"",
        // the view modes, then the menus
        "RadioButton \"Icons\" off",
        "RadioButton \"List\" on",
        "RadioButton \"Columns\" off",
        "RadioButton \"Gallery\" off",
        "PopUpButton \"View\"",
        "PopUpButton \"Tags\"",
        "PopUpButton \"Action\"",
        "TextInput \"Search\" = \"\"",
        // the headers in the order they are shown, then the rows
        "ColumnHeader \"Name\"",
        "ColumnHeader \"Date Modified\"",
        "ColumnHeader \"Size\"",
        "ColumnHeader \"Kind\"",
        // Tab in the list selects the next item by name, and past the last goes on
        "Row [a.txt]",
        "Row [b.txt]",
        "Row [Notes]",
    ]
    .map(str::to_owned)
    .to_vec();
    expected.extend(segments.iter().map(|s| format!("Button {s:?}")));
    expected.push("Splitter \"Sidebar\" = 223 of 0..919".to_owned());
    expected.push("ListItem \"Documents\"".to_owned());
    let mut stops = Vec::new();
    for _ in 0..expected.len() {
        run.key(TAB, None);
        stops.push(focused(&run));
    }
    assert_eq!(stops, expected);
    assert_eq!(run.selected_rows(), ["Notes"], "Tab selected as it went");

    // Shift+Tab before the first item by name leaves the list too, back through the headers
    run.click_row("a.txt");
    run.key_with(LEFT_TAB, None, Mods::SHIFT);
    assert_eq!(focused(&run), "ColumnHeader \"Kind\"");
    for _ in 0..4 {
        run.key_with(LEFT_TAB, None, Mods::SHIFT);
    }
    assert_eq!(focused(&run), "TextInput \"Search\" = \"\"");

    // the icon grid lets Tab go on past its last item the same way
    run.menu("view-icons");
    run.wait_until(|run| run.icons().len() == 3);
    run.click_icon("Notes");
    run.key(TAB, None);
    assert_eq!(focused(&run), format!("Button {:?}", segments[0]));
    assert_eq!(run.selected_icons(), ["Notes"], "the selection stays");
}

/// The stops Tab (or Shift+Tab, `back`) visits from where the keyboard is, until it comes back there.
fn walk(run: &mut Run, back: bool) -> Vec<String> {
    let start = focused(run);
    let mut stops = Vec::new();
    for _ in 0..64 {
        if back {
            run.key_with(LEFT_TAB, None, Mods::SHIFT);
        } else {
            run.key(TAB, None);
        }
        let stop = focused(run);
        if stop == start {
            return stops;
        }
        stops.push(stop);
    }
    panic!("Tab never came back to {start}: {stops:?}");
}

#[test]
fn shift_tab_visits_the_stops_tab_does_in_the_opposite_order() {
    let (mut run, _) = start(Appearance::Light);
    run.menu("toggle-path-bar");
    run.click_named(Role::ListItem, "Documents");
    assert_eq!(focused(&run), "ListItem \"Documents\"");
    let forward = walk(&mut run, false);
    for header in ["Name", "Date Modified", "Size", "Kind"] {
        let header = format!("ColumnHeader {header:?}");
        assert!(forward.contains(&header), "{header} is a stop: {forward:?}");
    }
    let back = walk(&mut run, true);
    let mut mirrored = back.clone();
    mirrored.reverse();
    // Share is a stop only while something is selected, and the walk through the rows selects them: Tab first passes
    // it with nothing selected, Shift+Tab after the rows
    let share = "PopUpButton \"Share\"".to_owned();
    assert!(!forward.contains(&share) && mirrored.contains(&share));
    mirrored.retain(|stop| *stop != share);
    assert_eq!(mirrored, forward, "Shift+Tab is Tab backwards");
    assert_eq!(focused(&run), "ListItem \"Documents\"");
}

#[test]
fn tab_in_an_empty_folder_walks_the_headers_and_goes_on_past_the_list() {
    let (mut run, _) = start(Appearance::Light);
    run.click_named(Role::ListItem, "Downloads");
    run.wait_until(|run| run.title() == "Downloads");
    assert_eq!(focused(&run), "ListItem \"Downloads\"");
    let forward = walk(&mut run, false);
    let headers: Vec<&str> = forward
        .iter()
        .filter_map(|s| s.strip_prefix("ColumnHeader "))
        .collect();
    assert_eq!(
        headers,
        ["\"Name\"", "\"Date Modified\"", "\"Size\"", "\"Kind\""],
        "{forward:?}"
    );
    assert!(
        !forward.iter().any(|s| s.starts_with("Row")),
        "no row to stop on: {forward:?}"
    );
    let mut back = walk(&mut run, true);
    back.reverse();
    assert_eq!(back, forward, "Shift+Tab is Tab backwards");
}

#[test]
fn the_keyboard_comes_back_to_what_opened_a_menu_or_a_rename() {
    let (mut run, _) = start(Appearance::Light);

    // a pop-up button opened with Space
    run.click_row("a.txt");
    while focused(&run) != "PopUpButton \"View\"" {
        run.key_with(LEFT_TAB, None, Mods::SHIFT);
    }
    let view = find(&run, Role::PopUpButton, "View");
    assert_eq!(nodes(&run)[&view].states.expanded, Some(false));
    run.key(SPACE, Some(' '));
    assert!(run.context_menu().is_some(), "the View menu");
    assert_eq!(
        nodes(&run)[&view].states.expanded,
        Some(true),
        "the button says its menu is open"
    );
    run.close_menu();
    assert!(run.context_menu().is_none());
    assert_eq!(nodes(&run)[&view].states.expanded, Some(false));
    assert_eq!(focused(&run), "PopUpButton \"View\"");

    // the context menu, opened with the Menu key on a row
    run.click_row("b.txt");
    assert_eq!(focused(&run), "Row [b.txt]");
    run.key(MENU_KEY, None);
    assert!(run.context_menu().is_some(), "the item's menu");
    run.close_menu();
    assert_eq!(focused(&run), "Row [b.txt]");

    // Escape ends a rename without renaming, and the row has the keyboard again
    run.menu("rename");
    assert_eq!(focused(&run), "TextInput \"Name\" = \"b.txt\"");
    run.type_text("zzz");
    run.key(ESCAPE, None);
    assert_eq!(focused(&run), "Row [b.txt]");
    assert_eq!(run.rows(), ["a.txt", "b.txt", "Notes"], "nothing renamed");
    assert!(run.home().join("Documents/b.txt").exists());

    // Return renames, and the keyboard follows the item to its new name
    run.menu("rename");
    run.key_with(u32::from('a'), Some('a'), Mods::PRIMARY);
    run.type_text("c.txt");
    run.key(RETURN, None);
    run.wait_until(|run| run.rows().contains(&"c.txt".to_owned()));
    run.wait_until(|run| focused(run) == "Row [c.txt]");

    // the same in the icon grid
    run.menu("view-icons");
    run.wait_until(|run| run.icons().len() == 3);
    run.click_icon("a.txt");
    assert_eq!(focused(&run), "ListItem \"a.txt\"");
    run.menu("rename");
    run.key(ESCAPE, None);
    assert_eq!(focused(&run), "ListItem \"a.txt\"");
    run.menu("rename");
    run.key_with(u32::from('a'), Some('a'), Mods::PRIMARY);
    run.type_text("d.txt");
    run.key(RETURN, None);
    run.wait_until(|run| run.icons().iter().any(|(name, _)| name == "d.txt"));
    run.wait_until(|run| focused(run) == "ListItem \"d.txt\"");
}

#[test]
fn a_rename_that_keeps_the_name_gives_the_keyboard_back_at_once_and_never_later() {
    let (mut run, _) = start(Appearance::Light);
    run.click_row("b.txt");
    run.menu("rename");
    assert_eq!(focused(&run), "TextInput \"Name\" = \"b.txt\"");
    run.key(RETURN, None);
    assert_eq!(focused(&run), "Row [b.txt]");

    // once the keyboard is elsewhere, a change in the folder leaves it there
    run.click_named(Role::TextInput, "Search");
    std::fs::write(run.home().join("Documents/c.txt"), "c").unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    assert_eq!(focused(&run), "TextInput \"Search\" = \"\"");

    // a name committed by a press on another row leaves the keyboard on that row
    run.click_row("a.txt");
    run.menu("rename");
    run.click_row("c.txt");
    assert_eq!(focused(&run), "Row [c.txt]");
    std::fs::write(run.home().join("Documents/d.txt"), "d").unwrap();
    run.wait_until(|run| run.rows().len() == 5);
    assert_eq!(focused(&run), "Row [c.txt]");
}

#[test]
fn a_folder_closed_over_the_focused_row_leaves_the_keyboard_on_the_folder() {
    let (mut run, at) = start(Appearance::Light);
    let notes = row(&run, "Notes");
    ask(&mut run, &at, notes, Action::Expand, None);
    run.wait_until(|run| run.rows().len() == 4);
    run.click_row("todo.txt");
    assert_eq!(focused(&run), "Row [todo.txt]");
    let notes = row(&run, "Notes");
    ask(&mut run, &at, notes, Action::Collapse, None);
    run.wait_until(|run| run.rows().len() == 3);
    assert_eq!(focused(&run), "Row [Notes]");

    // Left on an item inside a folder goes to the folder, and again closes it
    let notes = row(&run, "Notes");
    ask(&mut run, &at, notes, Action::Expand, None);
    run.wait_until(|run| run.rows().len() == 4);
    run.click_row("todo.txt");
    run.key(LEFT, None);
    assert_eq!(focused(&run), "Row [Notes]");
    run.key(LEFT, None);
    run.wait_until(|run| run.rows().len() == 3);
    assert_eq!(focused(&run), "Row [Notes]");
}

/// A gate a transfer waits at once it copied part of a file, until the test opens it; `reached` hears when it waits.
struct Gate {
    open: Mutex<bool>,
    opened: Condvar,
}

impl Gate {
    fn observer(gate: &Arc<Gate>, reached: mpsc::Sender<()>) -> Observer {
        let gate = Arc::clone(gate);
        let reached = Mutex::new(Some(reached));
        Arc::new(move |progress| {
            if progress.bytes == 0 || progress.bytes == progress.total_bytes {
                return;
            }
            if let Some(reached) = reached.lock().unwrap().take() {
                let _ = reached.send(());
            }
            let open = gate.open.lock().unwrap();
            let _open = gate
                .opened
                .wait_timeout_while(open, Duration::from_secs(10), |open| !*open)
                .unwrap();
        })
    }

    fn release(&self) {
        *self.open.lock().unwrap() = true;
        self.opened.notify_all();
    }
}

#[test]
fn transfers_searches_and_selections_are_announced_without_moving_the_keyboard() {
    let gate = Arc::new(Gate {
        open: Mutex::new(false),
        opened: Condvar::new(),
    });
    let (reached, waits) = mpsc::channel();
    let observer = Gate::observer(&gate, reached);
    let (mut run, at) = start_with(Appearance::Light, false, Some(observer));

    // more than one item selected is counted; one is read by the focus on it
    run.click_row("a.txt");
    assert!(heard(&at).is_empty(), "{:?}", heard(&at));
    run.click_row_with("b.txt", Mods::SHIFT);
    assert_eq!(heard(&at), ["2 items selected"]);
    assert_eq!(focused(&run), "Row [b.txt]", "the keyboard stays");

    // a search's result count, once the folder is read
    run.click_named(Role::TextInput, "Search");
    run.type_text("txt");
    run.wait_until(|run| run.rows().len() == 2);
    run.type_text("zz");
    run.wait_until(|run| run.rows().is_empty());
    let said = heard(&at);
    assert!(said.contains(&"2 results".to_owned()), "{said:?}");
    assert_eq!(said.last().map(String::as_str), Some("No Results"));
    assert_eq!(focused(&run), "TextInput \"Search\" = \"txtzz\"");
    run.key(ESCAPE, None);
    run.wait_until(|run| run.rows().len() == 3);

    // a copy as it starts and as it ends: the progress bar reads its share, the Stop button stops it
    std::fs::write(run.home().join("Documents/big.bin"), vec![5u8; 4 << 20]).unwrap();
    run.wait_until(|run| run.rows().len() == 4);
    run.click_row("big.bin");
    run.menu("copy");
    run.click_named(Role::ListItem, "Downloads");
    run.wait_until(|run| run.title() == "Downloads");
    let before = focused(&run);
    let moves = at.focus_moves(run.window);
    run.menu("paste");
    waits
        .recv_timeout(Duration::from_secs(10))
        .expect("the copy started");
    run.wait_until(|run| !run.named(Role::ProgressIndicator).is_empty());
    let copying = "Copying “big.bin” to “Downloads”";
    assert_eq!(heard(&at).last().map(String::as_str), Some(copying));
    let bar = &nodes(&run)[&find(&run, Role::ProgressIndicator, copying)];
    assert!(
        matches!(
            bar.value,
            Some(AccessibleValue::Number {
                min: 0.0,
                max: 1.0,
                ..
            })
        ),
        "{:?}",
        bar.value
    );
    let stop = find(&run, Role::Button, "Stop");
    assert_eq!(nodes(&run)[&stop].actions, [Action::Click]);
    assert_eq!(focused(&run), before, "the keyboard stays");
    ask(&mut run, &at, stop, Action::Click, None);
    gate.release();
    run.wait_until(|run| run.named(Role::ProgressIndicator).is_empty());
    assert_eq!(
        heard(&at).last().map(String::as_str),
        Some("Stopped copying “big.bin” to “Downloads”")
    );
    assert!(!run.home().join("Downloads/big.bin").exists());

    // a copy that runs to its end
    let copied = "Copied “big.bin” to “Downloads”";
    run.menu("paste");
    run.wait_until(|run| {
        run.named(Role::ProgressIndicator).is_empty() && run.rows() == ["big.bin"]
    });
    assert_eq!(heard(&at).last().map(String::as_str), Some(copied));
    assert_eq!(focused(&run), before, "the keyboard stays");
    let first = speaker(&run, &at, copied).expect("the region reads the copy");

    // the same copy again says the same words again, from a node of its own: a reader hears it even when the copy
    // starts and ends before the window next sends its tree
    run.menu("paste");
    run.wait_until(|run| run.named(Role::ProgressIndicator).is_empty() && run.rows().len() == 2);
    let said = heard(&at);
    assert_eq!(said.iter().filter(|s| *s == copied).count(), 2, "{said:?}");
    let second = speaker(&run, &at, copied).expect("the region reads the second copy");
    assert_ne!(first, second, "each message is a node of its own");
    assert_eq!(
        at.focus_moves(run.window),
        moves,
        "no announcement moved the keyboard"
    );
}

#[test]
fn a_rubber_band_says_how_many_items_it_selected_once_it_is_let_go() {
    let (mut run, at) = start(Appearance::Light);
    let notes = run.row_bounds("Notes");
    let first = run.row_bounds("a.txt");
    let under = Point::new(notes.center().x, notes.max_y() + 40.0);
    let over = Point::new(first.center().x + 60.0, first.center().y);
    run.hold(under, over, Mods::NONE);
    assert_eq!(run.selected_rows().len(), 3);
    assert!(
        heard(&at).is_empty(),
        "nothing while it is drawn: {:?}",
        heard(&at)
    );
    run.release(over, Mods::NONE);
    assert_eq!(heard(&at), ["3 items selected"]);
}

/// The theme the window draws with in `appearance`.
fn theme(appearance: Appearance) -> ThemeTokens {
    let data = concat!(env!("CARGO_MANIFEST_DIR"), "/../../data/theme");
    let preferences = ThemePreferences {
        appearance,
        reduce_transparency: false,
        reduce_motion: false,
    };
    ThemeTokens::cupertino(&Theme::from_dir(data).unwrap(), preferences).unwrap()
}

/// The colour of the glyphs node `id` paints.
fn glyph_color(run: &Run, id: NodeId) -> Color {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    let paint = snapshot
        .display_list
        .nodes
        .iter()
        .find(|n| n.id == PaintNodeId::from(id))
        .expect("the text paints");
    let glyphs = paint.ops.iter().find_map(|op| match op {
        PaintOp::Glyphs { color, .. } => Some(*color),
        _ => None,
    });
    glyphs.expect("glyphs")
}

/// The static texts of the row whose first cell reads `name`, in reading order.
fn row_texts(run: &Run, name: &str) -> Vec<NodeId> {
    let nodes = nodes(run);
    let mut out = Vec::new();
    let mut stack = vec![row(run, name)];
    while let Some(id) = stack.pop() {
        if nodes[&id].role == Role::StaticText {
            out.push(id);
        }
        stack.extend(nodes[&id].children.iter().rev());
    }
    out
}

const APPEARANCES: [Appearance; 4] = [
    Appearance::Light,
    Appearance::Dark,
    Appearance::LightHighContrast,
    Appearance::DarkHighContrast,
];

#[test]
fn a_selected_rows_text_is_drawn_in_the_selected_control_text_colour_in_every_appearance() {
    for appearance in APPEARANCES {
        let theme = theme(appearance);
        let get = |role| theme.colors.get(role, StateVariant::NORMAL).unwrap();
        let (mut run, _) = start(appearance);
        run.click_row("a.txt");
        run.click_row_with("Notes", Mods::PRIMARY);
        for name in ["a.txt", "Notes"] {
            for text in row_texts(&run, name) {
                assert_eq!(
                    glyph_color(&run, text),
                    get(ColorRole::SelectedControlText),
                    "{appearance:?}: {:?} of the selected row {name}",
                    nodes(&run)[&text].name
                );
            }
        }
        let b = row_texts(&run, "b.txt");
        assert_eq!(
            glyph_color(&run, b[0]),
            get(ColorRole::Text),
            "{appearance:?}: the name of a row not selected"
        );
        for text in &b[1..] {
            assert_eq!(
                glyph_color(&run, *text),
                get(ColorRole::SecondaryText),
                "{appearance:?}: a detail of a row not selected"
            );
        }
        // light and dark at the scale CUPERTINO_LOOK_SCALE asks for, when CUPERTINO_LOOK_DIR names a folder
        if appearance == Appearance::Light {
            run.driver.look(run.window, "files-selected-rows").unwrap();
        }
        // a window that is not the key window draws its selection's text in the inactive selection's colour
        run.driver
            .window_system_mut()
            .configure(run.window, None, WindowStates::default())
            .unwrap();
        run.settle();
        for text in row_texts(&run, "a.txt") {
            assert_eq!(
                glyph_color(&run, text),
                get(ColorRole::SecondarySelectedControlText),
                "{appearance:?}: {:?} of a selected row in an inactive window",
                nodes(&run)[&text].name
            );
        }
    }
}

/// Whether node `id` strokes a ring in `color`.
fn strokes(run: &Run, id: NodeId, color: Color) -> bool {
    let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
    snapshot
        .display_list
        .nodes
        .iter()
        .filter(|n| n.id == PaintNodeId::from(id))
        .flat_map(|n| &n.ops)
        .any(|op| matches!(op, PaintOp::Stroke { paint: Paint::Solid(c), .. } if *c == color))
}

#[test]
fn the_focus_ring_shows_on_the_toolbar_the_path_bar_and_a_selected_row_with_increase_contrast() {
    for appearance in [Appearance::LightHighContrast, Appearance::DarkHighContrast] {
        let colors = theme(appearance).colors;
        let ring = colors
            .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            .unwrap();
        let (mut run, _) = start(appearance);
        run.menu("toggle-path-bar");
        let segments = path_segments(&run);
        // the sidebar, the four view modes, then the View menu
        for _ in 0..6 {
            run.key(TAB, None);
        }
        assert_eq!(focused(&run), "PopUpButton \"View\"");
        let view = find(&run, Role::PopUpButton, "View");
        assert!(strokes(&run, view, ring), "{appearance:?}: View's ring");
        while focused(&run) != format!("Button {:?}", segments[0]) {
            run.key(TAB, None);
        }
        let first = find(&run, Role::Button, &segments[0]);
        assert!(
            strokes(&run, first, ring),
            "{appearance:?}: the path bar folder's ring"
        );
        if appearance == Appearance::LightHighContrast {
            run.driver.look(run.window, "files-path-bar-focus").unwrap();
        }
        // over the selection the ring takes the selected text colour
        run.key_with(LEFT_TAB, None, Mods::SHIFT);
        assert_eq!(focused(&run), "Row [Notes]");
        let notes = row(&run, "Notes");
        let on_selection = colors
            .get(ColorRole::SelectedControlText, StateVariant::NORMAL)
            .unwrap();
        assert!(
            strokes(&run, notes, on_selection),
            "{appearance:?}: the focused row's ring"
        );
    }
}

#[test]
fn reduce_motion_opens_the_search_field_at_once() {
    let width = |run: &Run| run.bounds(Role::TextInput, "Search").width();
    let (mut run, _) = start_with(Appearance::Light, true, None);
    let rest = width(&run);
    run.click_named(Role::TextInput, "Search");
    let open = width(&run);
    assert!(open > rest, "the field opens: {rest} to {open}");
    run.frames(500, 16);
    assert_eq!(width(&run), open, "and is already as wide as it gets");
}

#[test]
fn an_empty_folder_reads_its_name_and_no_items() {
    let (mut run, at) = start(Appearance::Light);
    let downloads = find(&run, Role::ListItem, "Downloads");
    ask(&mut run, &at, downloads, Action::Click, None);
    run.wait_until(|run| run.title() == "Downloads");
    let outline = |run: &Run| -> Vec<String> {
        let snapshot = run.driver.runtime().snapshot(run.window).unwrap();
        let tree = whole_tree(run.window, snapshot.accessibility, snapshot.focus);
        semantic_outline(&tree)
            .into_iter()
            .map(|l| l.trim().to_owned())
            .collect()
    };
    let lines = outline(&run);
    let table = "Table \"Downloads\" 0 items multiselectable".to_owned();
    assert!(lines.contains(&table), "{lines:#?}");
    run.menu("view-icons");
    let lines = outline(&run);
    let list = "List \"Downloads\" 0 items multiselectable".to_owned();
    assert!(lines.contains(&list), "{lines:#?}");
}

/// Set in the copy of the test binary that plays the window read over AT-SPI.
const ATSPI_WINDOW: &str = "CUPERTINO_TEST_FILES_ATSPI_LIST";
const ATSPI_ROOT: &str = "/org/a11y/atspi/accessible/root";
/// AT-SPI's role numbers (`AtspiRole`) and state bits (`AtspiStateType`).
const ATSPI_LABEL: u32 = 29;
const ATSPI_TABLE: u32 = 55;
const ATSPI_TABLE_ROW: u32 = 90;
const ATSPI_MULTISELECTABLE: u32 = 18;
const ATSPI_SELECTED: u32 = 23;

struct Unpark(std::thread::Thread);

impl std::task::Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// The window read over AT-SPI: the list with `a.txt` and `b.txt` selected. Once the selection is the row a click
/// over AT-SPI asked for, it waits for the test to close its input and exits successfully; it fails when no click
/// arrives.
fn atspi_window() {
    use std::io::Read;
    let waker = std::task::Waker::from(Arc::new(Unpark(std::thread::current())));
    let adapter = cupertino_ui_atspi::AtspiAdapter::new(waker);
    let mut run = start_hosted(Appearance::Light, false, None, Box::new(adapter));
    run.send(cupertino_kit::platform::WindowEvent::Focus(true));
    run.click_row("a.txt");
    run.click_row_with("b.txt", Mods::SHIFT);
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while std::time::Instant::now() < deadline {
        run.driver
            .window_system_mut()
            .enqueue(PlatformEvent::Wake)
            .unwrap();
        run.settle();
        if run.selected_rows() == ["Notes"] {
            std::io::stdin().read_to_end(&mut Vec::new()).unwrap();
            return;
        }
        std::thread::park_timeout(Duration::from_millis(50));
    }
    panic!(
        "no click over AT-SPI selected Notes: {:?}",
        run.selected_rows()
    );
}

struct A11yStatus;

#[zbus::interface(name = "org.a11y.Status")]
impl A11yStatus {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
    }
}

/// The registry: it records which bus name embedded its application root.
struct Socket(mpsc::Sender<String>);

#[zbus::interface(name = "org.a11y.atspi.Socket")]
impl Socket {
    fn embed(
        &self,
        plug: (String, OwnedObjectPath),
        #[zbus(connection)] conn: &zbus::Connection,
    ) -> (String, OwnedObjectPath) {
        let _ = self.0.send(plug.0);
        let registry = conn
            .unique_name()
            .map(|n| n.to_string())
            .unwrap_or_default();
        (registry, OwnedObjectPath::try_from(ATSPI_ROOT).unwrap())
    }
}

fn accessible<'a>(conn: &Connection, peer: &str, path: &str) -> zbus::blocking::Proxy<'a> {
    zbus::blocking::Proxy::new(
        conn,
        peer.to_owned(),
        path.to_owned(),
        "org.a11y.atspi.Accessible",
    )
    .unwrap()
}

/// The nodes under `path`, depth first, with their role and name.
fn descendants(conn: &Connection, peer: &str, path: &str) -> Vec<(OwnedObjectPath, u32, String)> {
    let found: zbus::Result<Vec<(String, OwnedObjectPath)>> =
        accessible(conn, peer, path).call("GetChildren", &());
    let mut out = Vec::new();
    for (_, child) in found.unwrap_or_default() {
        let node = accessible(conn, peer, child.as_str());
        let role: u32 = node.call("GetRole", &()).unwrap();
        let name: String = node.get_property("Name").unwrap_or_default();
        out.push((child.clone(), role, name));
        out.extend(descendants(conn, peer, child.as_str()));
    }
    out
}

/// The state bits of the node at `path`.
fn states(conn: &Connection, peer: &str, path: &OwnedObjectPath) -> u32 {
    let state: Vec<u32> = accessible(conn, peer, path.as_str())
        .call("GetState", &())
        .unwrap();
    state[0]
}

/// An assistive technology reads the list over AT-SPI as a table that takes more than one selected row, the two rows
/// selected as such and the third not; a click on the third over AT-SPI selects it alone.
#[test]
fn assistive_technology_reads_the_lists_selection_over_atspi_and_a_rows_click_selects_it() {
    const NAME: &str =
        "assistive_technology_reads_the_lists_selection_over_atspi_and_a_rows_click_selects_it";
    if std::env::var_os(ATSPI_WINDOW).is_some() {
        return atspi_window();
    }
    let Some(bus) = PrivateBus::start() else {
        missing_tool("dbus-daemon");
        return;
    };
    let (embedded, embeds) = mpsc::channel();
    let _status = bus
        .blocking_builder()
        .name("org.a11y.Bus")
        .unwrap()
        .serve_at("/org/a11y/bus", A11yStatus)
        .unwrap()
        .build()
        .unwrap();
    let _registry = bus
        .blocking_builder()
        .name("org.a11y.atspi.Registry")
        .unwrap()
        .serve_at(ATSPI_ROOT, Socket(embedded))
        .unwrap()
        .build()
        .unwrap();
    let mut window = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            cupertino_test_bus::test_path(NAME).as_str(),
            "--exact",
            "--nocapture",
        ])
        .env("CUPERTINO_ASSET_SEARCH_PATH", "")
        .env(ATSPI_WINDOW, "1")
        .env("DBUS_SESSION_BUS_ADDRESS", &bus.address)
        .env("AT_SPI_BUS_ADDRESS", &bus.address)
        .stdin(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    let peer = embeds
        .recv_timeout(Duration::from_secs(20))
        .expect("the window never embedded its root");
    let conn = bus.connect();
    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    // each row of the table by the text of its first cell
    let (table, rows) = loop {
        let nodes = descendants(&conn, &peer, ATSPI_ROOT);
        let table = nodes
            .iter()
            .find(|(_, role, name)| *role == ATSPI_TABLE && name == "Documents")
            .map(|(path, _, _)| path.clone());
        if let Some(table) = table {
            let rows: Vec<(OwnedObjectPath, String)> = nodes
                .iter()
                .filter(|(_, role, _)| *role == ATSPI_TABLE_ROW)
                .filter_map(|(path, _, _)| {
                    let under = descendants(&conn, &peer, path.as_str());
                    let (_, _, name) = under.into_iter().find(|(_, r, _)| *r == ATSPI_LABEL)?;
                    Some((path.clone(), name))
                })
                .collect();
            let selected = rows
                .iter()
                .filter(|(path, _)| states(&conn, &peer, path) & (1 << ATSPI_SELECTED) != 0)
                .count();
            if selected == 2 {
                break (table, rows);
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no table with two rows selected appeared"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_ne!(
        states(&conn, &peer, &table) & (1 << ATSPI_MULTISELECTABLE),
        0,
        "the table takes more than one row"
    );
    let read: Vec<(&str, bool)> = rows
        .iter()
        // the header row reads its first title
        .filter(|(_, name)| name != "Name")
        .map(|(path, name)| {
            let selected = states(&conn, &peer, path) & (1 << ATSPI_SELECTED) != 0;
            (name.as_str(), selected)
        })
        .collect();
    assert_eq!(
        read,
        [("a.txt", true), ("b.txt", true), ("Notes", false)],
        "the rows by name and selection"
    );
    let notes = &rows.iter().find(|(_, name)| name == "Notes").unwrap().0;
    let action = zbus::blocking::Proxy::new(
        &conn,
        peer.clone(),
        notes.as_str().to_owned(),
        "org.a11y.atspi.Action",
    )
    .unwrap();
    let names: Vec<String> = (0..2_i32)
        .filter_map(|i| action.call("GetName", &(i,)).ok())
        .collect();
    let click = names
        .iter()
        .position(|n| n == "click")
        .unwrap_or_else(|| panic!("the row's actions: {names:?}"));
    let _: bool = action.call("DoAction", &(click as i32,)).unwrap();
    drop(window.stdin.take());
    assert!(
        window.wait().unwrap().success(),
        "the row's click did not select it"
    );
}
