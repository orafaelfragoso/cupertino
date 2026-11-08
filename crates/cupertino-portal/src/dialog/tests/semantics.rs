//! What a screen reader announces of each portal dialog: the role, name, value and states of its nodes, as the
//! window opens and after a row is picked.

use cupertino_kit::core::AccessibilityUpdate;
use cupertino_ui_test::{semantic_gaps, semantic_outline, whole_tree};

use super::*;

fn tree(driver: &TestDriver<Dialogs>, window: WindowId) -> AccessibilityUpdate {
    let snapshot = driver.runtime().snapshot(window).unwrap();
    whole_tree(window, snapshot.accessibility, snapshot.focus)
}

/// The window's outline without the rows of its Places list, which are the session's folders and differ by machine.
fn outline(driver: &TestDriver<Dialogs>, window: WindowId) -> Vec<String> {
    let mut lines = Vec::new();
    let mut places: Option<usize> = None;
    for line in semantic_outline(&tree(driver, window)) {
        let depth = line.len() - line.trim_start().len();
        match places {
            Some(d) if depth > d => continue,
            _ => places = None,
        }
        if line.trim_start().starts_with("List \"Places\"") {
            places = Some(depth);
        }
        lines.push(line);
    }
    lines
}

/// What the window reads that a screen reader cannot announce, besides what Kit leaves unnamed in every window: the
/// root, and a list row, which has no name of its own and is read by its text (the outlines below show that text).
fn gaps(driver: &TestDriver<Dialogs>, window: WindowId) -> Vec<String> {
    let mut gaps = semantic_gaps(&tree(driver, window));
    gaps.retain(|gap| {
        gap != "Window #0: no name"
            && !(gap.contains(" > ListItem #") && gap.ends_with(": no name"))
    });
    gaps
}

fn lines(lines: &[&str]) -> Vec<String> {
    lines.iter().map(|l| (*l).to_owned()).collect()
}

/// The node assistive technology is told holds the focus is the Cancel button, so a screen reader announces it first.
fn announces_cancel(driver: &TestDriver<Dialogs>, window: WindowId) {
    let runtime = driver.runtime();
    let snapshot = runtime.snapshot(window).unwrap();
    let cancel = runtime.find(window, &Key::from("cancel"));
    assert!(cancel.is_some());
    assert_eq!(snapshot.accessibility_focus, cancel);
    let name = snapshot
        .accessibility
        .iter()
        .find(|(id, _)| Some(*id) == cancel)
        .and_then(|(_, node)| node.name.clone());
    assert_eq!(name.as_deref(), Some("Cancel"));
}

fn reads(driver: &TestDriver<Dialogs>, window: WindowId, expected: &[&str]) {
    assert_eq!(outline(driver, window), lines(expected));
    assert_eq!(gaps(driver, window), Vec::<String>::new());
}

#[test]
fn the_file_chooser_reads_its_places_folder_rows_and_buttons_and_enables_open_on_a_pick() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir(folder.path().join("Work")).unwrap();
    std::fs::write(folder.path().join("a.txt"), "x").unwrap();
    let name = folder.path().file_name().unwrap().to_str().unwrap();
    let path = format!("  StaticText {:?}", folder.path().display().to_string());
    let list = format!("    List {name:?} 2 items");

    let answer = post(21, ask(Kind::Open, folder.path()));
    let (mut driver, window) = start();
    let open = |opened: &str, row: &str| {
        vec![
            "Window".to_owned(),
            "  ScrollView".to_owned(),
            "    List \"Places\" 6 items".to_owned(),
            "  Button \"Enclosing Folder\" focusable".to_owned(),
            path.clone(),
            "  ScrollView".to_owned(),
            list.clone(),
            format!("      ListItem focusable{row}"),
            "        StaticText \"Work\"".to_owned(),
            "      ListItem".to_owned(),
            "        StaticText \"a.txt\"".to_owned(),
            "  Splitter \"Sidebar\" = 180 of 0..719 focusable".to_owned(),
            "  Button \"Cancel\" focusable".to_owned(),
            opened.to_owned(),
        ]
    };
    let places = outline(&driver, window)[2].clone();
    assert!(
        places.starts_with("    List \"Places\" "),
        "the session's places: {places}"
    );
    let opened = |driver: &TestDriver<Dialogs>| {
        let mut o = outline(driver, window);
        assert!(o[2].starts_with("    List \"Places\" "), "{}", o[2]);
        o[2] = "    List \"Places\" 6 items".to_owned();
        o
    };
    assert_eq!(
        opened(&driver),
        open("  Button \"Open\" disabled", ""),
        "nothing picked: Open is disabled"
    );
    assert_eq!(gaps(&driver, window), Vec::<String>::new());

    click(&mut driver, window, "a.txt");
    let picked = opened(&driver);
    assert!(
        picked.contains(&"  Button \"Open\" focusable".to_owned()),
        "a picked file enables Open:\n{}",
        picked.join("\n")
    );
    let row = picked
        .iter()
        .position(|l| l == "        StaticText \"a.txt\"")
        .unwrap();
    assert!(
        picked[row - 1].starts_with("      ListItem") && picked[row - 1].ends_with(" selected"),
        "the picked row reads as selected:\n{}",
        picked.join("\n")
    );
    send(&mut driver, window, WindowEvent::CloseRequested);
    assert_eq!(answer.try_recv().unwrap(), Answer::Cancelled);

    let answer = post(22, ask(Kind::Save, folder.path()));
    let window = show(&mut driver);
    let mut saved = outline(&driver, window);
    assert!(saved[4].starts_with("    List \"Places\" "), "{}", saved[4]);
    saved[4] = "    List \"Places\" 6 items".to_owned();
    assert_eq!(
        saved,
        lines(&[
            "Window",
            "  StaticText \"Save As:\"",
            "  TextInput \"Save As\" = \"new.txt\" focusable focused",
            "  ScrollView",
            "    List \"Places\" 6 items",
            "  Button \"Enclosing Folder\" focusable",
            &path,
            "  ScrollView",
            &list,
            "      ListItem focusable",
            "        StaticText \"Work\"",
            "      ListItem",
            "        StaticText \"a.txt\"",
            "  Splitter \"Sidebar\" = 180 of 0..719 focusable",
            "  Button \"Cancel\" focusable",
            "  Button \"Save\" focusable",
        ]),
        "Save As opens on its name, and Save needs no pick"
    );
    assert_eq!(gaps(&driver, window), Vec::<String>::new());
    send(&mut driver, window, WindowEvent::CloseRequested);
    assert_eq!(answer.try_recv().unwrap(), Answer::Cancelled);
}

#[test]
fn the_screen_cast_chooser_reads_its_sources_and_enables_share_on_a_pick() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let (sender, answer) = async_channel::bounded(1);
    inbox().push(screen_cast::Event::Ask {
        handle: handle(23),
        ask: Box::new(screen_cast::Ask {
            app_id: "org.example.Meeting".to_owned(),
            app_name: Some("Meeting".to_owned()),
            app_icon: None,
            offer_remember: false,
            parent_window: String::new(),
            sources: vec![
                Source::Display {
                    output: 4,
                    name: "DP-1".to_owned(),
                    description: "Studio Display".to_owned(),
                },
                Source::Window {
                    identifier: "a1".to_owned(),
                    title: "Notes".to_owned(),
                    app_id: "org.example.Notes".to_owned(),
                },
            ],
        }),
        reply: screen_cast::Reply(sender),
    });
    let (mut driver, window) = start();
    let opened = [
        "Window",
        "  StaticText \"“Meeting” wants to share your screen. Choose a display or a window.\"",
        "  ScrollView",
        "    List \"Displays and Windows\" 2 items",
        "      ListItem focusable",
        "        StaticText \"Studio Display\"",
        "      ListItem",
        "        StaticText \"Notes (org.example.Notes)\"",
        "  Button \"Cancel\" focusable focused",
        "  Button \"Share\" disabled",
    ];
    reads(&driver, window, &opened);
    announces_cancel(&driver, window);
    armed(&mut driver, window);
    reads(&driver, window, &opened);
    click_at(&mut driver, window, Key::Int(1));
    reads(
        &driver,
        window,
        &[
            "Window",
            "  StaticText \"“Meeting” wants to share your screen. Choose a display or a window.\"",
            "  ScrollView",
            "    List \"Displays and Windows\" 2 items",
            "      ListItem",
            "        StaticText \"Studio Display\"",
            "      ListItem focusable focused selected",
            "        StaticText \"Notes (org.example.Notes)\"",
            "  Button \"Cancel\" focusable",
            "  Button \"Share\" focusable",
        ],
    );
    send(&mut driver, window, WindowEvent::CloseRequested);
    assert_eq!(answer.try_recv().unwrap(), screen_cast::Answer::Cancelled);
}

#[test]
fn the_app_chooser_reads_its_prompt_and_apps_with_the_last_choice_selected_and_focused() {
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let choice = |id: &str, name: &str| app_chooser::Choice {
        id: id.to_owned(),
        name: name.to_owned(),
        icon: None,
    };
    let (sender, answer) = async_channel::bounded(1);
    inbox().push(app_chooser::Event::Ask {
        handle: handle(24),
        ask: Box::new(app_chooser::Ask {
            app_id: "org.example.Mail".to_owned(),
            parent_window: String::new(),
            target: Some("report.pdf".to_owned()),
            choices: vec![
                choice("org.example.Viewer", "Document Viewer"),
                choice("org.example.Draw", "Drawing"),
            ],
            last_choice: Some("org.example.Draw".to_owned()),
            wants_token: false,
        }),
        reply: app_chooser::Reply(sender),
    });
    let (mut driver, window) = start();
    reads(
        &driver,
        window,
        &[
            "Window",
            "  StaticText \"Open “report.pdf” with:\"",
            "  ScrollView",
            "    List \"Apps\" 2 items",
            "      ListItem",
            "        StaticText \"Document Viewer\"",
            "      ListItem focusable focused selected",
            "        StaticText \"Drawing\"",
            "  Button \"Cancel\" focusable",
            "  Button \"Open\" focusable",
        ],
    );
    send(&mut driver, window, WindowEvent::CloseRequested);
    assert_eq!(answer.try_recv().unwrap(), app_chooser::Answer::Cancelled);
}

#[test]
fn the_screenshot_consent_reads_its_prompt_and_two_buttons_with_cancel_focused_and_take_armed_late()
{
    let _turn = INBOX.lock().unwrap_or_else(|e| e.into_inner());
    let (sender, answer) = async_channel::bounded(1);
    inbox().push(screenshot::Event::Ask {
        handle: handle(25),
        ask: Box::new(screenshot::Ask {
            app_id: "org.example.Draw".to_owned(),
            app_name: Some("Drawing".to_owned()),
            app_icon: None,
            parent_window: String::new(),
        }),
        reply: screenshot::Reply(sender),
    });
    let (mut driver, window) = start();
    reads(
        &driver,
        window,
        &[
            "Window",
            "  StaticText \"“Drawing” wants to take a screenshot of your screen.\"",
            "  Button \"Cancel\" focusable focused",
            "  Button \"Take Screenshot\" disabled",
        ],
    );
    announces_cancel(&driver, window);
    armed(&mut driver, window);
    reads(
        &driver,
        window,
        &[
            "Window",
            "  StaticText \"“Drawing” wants to take a screenshot of your screen.\"",
            "  Button \"Cancel\" focusable focused",
            "  Button \"Take Screenshot\" focusable",
        ],
    );
    send(&mut driver, window, WindowEvent::CloseRequested);
    assert_eq!(answer.try_recv().unwrap(), screenshot::Answer::Cancelled);
}
