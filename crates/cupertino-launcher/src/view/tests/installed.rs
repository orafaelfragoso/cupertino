//! Applications installed, changed and removed while the launcher runs ([`Message::Installed`]): what is on screen
//! follows in place, and the query, the selection and the browser's filter stay.

use super::*;
use crate::engine::tests_support::app;
use crate::DesktopEntry;

/// The test launcher's applications, as the session would send them again.
fn installed(run: &Run) -> Vec<DesktopEntry> {
    run.model().launcher.entries().to_vec()
}

fn titles(run: &Run) -> Vec<String> {
    let results = run.model().panel.results();
    results.iter().map(|item| item.title.clone()).collect()
}

fn selected(run: &Run) -> Option<String> {
    let item = run.model().panel.selected_item();
    item.map(|item| item.title.clone())
}

fn browsed(run: &Run) -> Vec<String> {
    let browser = run.model().browser.as_ref().expect("the browser is open");
    browser.items().map(|app| app.name.clone()).collect()
}

#[test]
fn an_application_installed_while_a_query_shows_joins_its_results_and_the_selection_stays() {
    let mut run = Run::new();
    run.type_text("fire");
    run.key("Down", DOWN_CODE, DOWN);
    assert_eq!(selected(&run).as_deref(), Some("Firewall Configuration"));
    assert!(!titles(&run).contains(&"Firefly".to_owned()));

    let mut entries = installed(&run);
    entries.push(app("firefly", "Firefly", "Media Player", "firefly"));
    run.deliver(Message::Installed(entries));
    let model = run.model();
    assert_eq!(model.panel.query(), "fire", "the query is kept");
    assert!(
        titles(&run).contains(&"Firefly".to_owned()),
        "{:?}",
        titles(&run)
    );
    assert_eq!(
        selected(&run).as_deref(),
        Some("Firewall Configuration"),
        "the selected result stays selected wherever it moved"
    );
    // it is drawn: a screen reader finds its row
    let snapshot = run.snapshot();
    assert!(snapshot
        .accessibility
        .iter()
        .any(|(_, n)| n.role == Role::ListItem && n.name.as_deref() == Some("Firefly")));
    // and typing goes on from the same query
    run.type_text("f");
    assert_eq!(run.model().panel.query(), "firef");
    assert_eq!(titles(&run), ["Firefly", "Firefox"]);
}

#[test]
fn an_entry_changed_or_replaced_while_a_query_shows_is_listed_as_it_is_now() {
    let mut run = Run::new();
    run.type_text("fire");
    assert_eq!(selected(&run).as_deref(), Some("Firefox"));
    let mut entries = installed(&run);
    let browser = entries.iter_mut().find(|e| e.id == "browser.desktop");
    let browser = browser.unwrap();
    browser.name = "Firefox Nightly".into();
    browser.icon = Some("nightly".into());
    run.deliver(Message::Installed(entries.clone()));
    assert_eq!(run.model().panel.query(), "fire");
    assert_eq!(selected(&run).as_deref(), Some("Firefox Nightly"));
    assert!(!titles(&run).contains(&"Firefox".to_owned()));
    // the person's entry of the same ID takes the system's place
    let browser = entries.iter_mut().find(|e| e.id == "browser.desktop");
    let browser = browser.unwrap();
    browser.name = "Firefox (mine)".into();
    browser.source = Some("/home/person/.local/share/applications/browser.desktop".into());
    run.deliver(Message::Installed(entries));
    assert_eq!(selected(&run).as_deref(), Some("Firefox (mine)"));
    let launcher = &run.model().launcher;
    assert_eq!(
        launcher.entry("browser.desktop").unwrap().source,
        Some("/home/person/.local/share/applications/browser.desktop".into())
    );
}

#[test]
fn a_changed_application_s_icon_is_loaded_again_and_the_others_are_kept() {
    LOADS.set(true);
    let mut run = Run::new();
    run.type_text("fire");
    let asked = |run: &Run, id: &str| run.model().icons.contains_key(id);
    assert!(asked(&run, "app:browser.desktop") && asked(&run, "app:firewall.desktop"));
    let rendition = || {
        vec![ImageSource {
            id: cupertino_kit::core::next_resource_id(),
            image: cupertino_kit::render::ImageResource {
                revision: 0,
                width: 1,
                height: 1,
                stride: 4,
                format: cupertino_kit::render::PixelFormat::Rgba8Premultiplied,
                pixels: Arc::from(&[0u8, 0, 0, 255][..]),
            },
        }]
    };
    for id in ["app:browser.desktop", "app:firewall.desktop"] {
        run.deliver(Message::ResultIcon(id.into(), rendition()));
    }
    let firewall = run.model().icons["app:firewall.desktop"].clone();
    assert!(!firewall.is_empty());
    let mut entries = installed(&run);
    entries[0].icon = Some("nightly".into());
    assert_eq!(entries[0].id, "browser.desktop");
    run.deliver(Message::Installed(entries));
    LOADS.set(false);
    let icons = &run.model().icons;
    assert_eq!(
        icons.get("app:browser.desktop"),
        Some(&Vec::new()),
        "asked for again, blank until it arrives"
    );
    assert_eq!(icons.get("app:firewall.desktop"), Some(&firewall));
}

#[test]
fn an_application_removed_while_a_query_shows_leaves_its_results() {
    let mut run = Run::new();
    run.type_text("fire");
    assert_eq!(selected(&run).as_deref(), Some("Firefox"));
    let mut entries = installed(&run);
    entries.retain(|e| e.id != "browser.desktop");
    run.deliver(Message::Installed(entries.clone()));
    assert_eq!(run.model().panel.query(), "fire");
    assert_eq!(titles(&run), ["Firewall Configuration"]);
    assert_eq!(
        selected(&run).as_deref(),
        Some("Firewall Configuration"),
        "the selection stays at its place in the list"
    );
    // the last match goes: the list empties and the query stays
    entries.retain(|e| e.id != "firewall.desktop");
    run.deliver(Message::Installed(entries));
    assert_eq!(run.model().panel.query(), "fire");
    assert!(titles(&run).is_empty(), "{:?}", titles(&run));
    // Return on nothing starts nothing
    run.key("Return", RETURN_CODE, RETURN);
    assert!(launches().is_empty(), "{:?}", launches());
}

#[test]
fn the_browser_follows_installed_and_removed_applications_and_keeps_its_filter() {
    let mut run = Run::new();
    run.deliver(Message::Browse);
    run.type_text("fi");
    assert_eq!(
        browsed(&run),
        ["Files", "Firefox", "Firewall Configuration"]
    );
    run.key("Down", DOWN_CODE, DOWN);
    let chosen = |run: &Run| {
        let browser = run.model().browser.as_ref().unwrap();
        browser.selected_app().map(|app| app.name.clone())
    };
    let was = chosen(&run);
    assert!(was.is_some());

    let mut entries = installed(&run);
    entries.push(app("firefly", "Firefly", "Media Player", "firefly"));
    entries.push(app("notes", "Notes", "", "notes"));
    run.deliver(Message::Installed(entries.clone()));
    assert_eq!(run.model().panel.query(), "fi", "the filter is kept");
    assert_eq!(
        browsed(&run),
        ["Files", "Firefly", "Firefox", "Firewall Configuration"],
        "the new application is listed in its place by name"
    );
    assert_eq!(chosen(&run), was, "the selection stays on its application");

    entries.retain(|e| e.id != "browser.desktop");
    run.deliver(Message::Installed(entries));
    assert_eq!(
        browsed(&run),
        ["Files", "Firefly", "Firewall Configuration"]
    );
    // without the filter every application is there, the one installed meanwhile too
    for _ in 0..2 {
        run.key("Backspace", BACKSPACE_CODE, BACKSPACE);
    }
    assert!(browsed(&run).contains(&"Notes".to_owned()));
    assert!(!browsed(&run).contains(&"Firefox".to_owned()));
}

#[test]
fn an_application_installed_while_the_launcher_is_hidden_is_found_at_the_next_opening() {
    let mut run = Run::new();
    run.deliver(Message::Toggle);
    assert!(!run.model().presenting());
    let mut entries = installed(&run);
    entries.push(app("firefly", "Firefly", "Media Player", "firefly"));
    entries.retain(|e| e.id != "firewall.desktop");
    run.deliver(Message::Installed(entries));
    assert!(!run.model().presenting(), "a change opens nothing");
    run.deliver(Message::Toggle);
    run.type_text("fire");
    assert_eq!(titles(&run), ["Firefly", "Firefox"]);
    run.deliver(Message::Browse);
    assert!(browsed(&run).contains(&"Firefly".to_owned()));
}
