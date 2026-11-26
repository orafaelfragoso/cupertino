//! The cache of an exporter's GMenuModel menus, which any application fills: a record makes a menu only by inserting
//! into it and a menu it empties goes, the menus kept are bounded however the reply and the records come, and a record
//! of a signal sent before the `Start` reply that holds it is not applied again.

use std::collections::HashMap;

use cupertino_menubar::dbus::gtk::{GtkMenus, RawChange, MAX_MENUS};
use zbus::zvariant::{OwnedValue, Value};

fn row(label: &str) -> HashMap<String, OwnedValue> {
    let label = OwnedValue::try_from(Value::from(label)).unwrap();
    HashMap::from([("label".to_owned(), label)])
}

/// The labels of the menu `(group, menu)`; `None` when the cache has no such menu.
fn labels(menus: &GtkMenus, group: u32, menu: u32) -> Option<Vec<String>> {
    let items = menus.get(group, menu)?;
    let label = |item: &HashMap<String, OwnedValue>| match item.get("label").map(|v| &**v) {
        Some(Value::Str(s)) => s.as_str().to_owned(),
        _ => String::new(),
    };
    Some(items.iter().map(label).collect())
}

/// Group 0 subscribed with its root menu of one row, from a reply of serial `serial`.
fn subscribed(serial: Option<u32>) -> GtkMenus {
    let mut menus = GtkMenus::default();
    menus.begin(&[0]);
    menus.started(&[0], serial, vec![(0, 0, vec![row("New")])]);
    menus
}

#[test]
fn a_record_makes_a_menu_only_by_inserting_into_it_and_an_emptied_menu_goes() {
    let mut menus = subscribed(None);
    let records: [RawChange; 3] = [
        // a removal from a menu the cache has not
        (0, 5, 0, 1, Vec::new()),
        // an insertion past its start
        (0, 6, 1, 0, vec![row("Cut")]),
        // an insertion of nothing
        (0, 7, 0, 0, Vec::new()),
    ];
    for record in records {
        menus.apply_change(1, record);
    }
    assert_eq!(menus.menu_count(), 1, "no record made a menu");

    menus.apply_change(1, (0, 8, 0, 0, vec![row("Paste")]));
    assert_eq!(labels(&menus, 0, 8), Some(vec!["Paste".to_owned()]));
    menus.apply_change(1, (0, 8, 0, 1, Vec::new()));
    assert_eq!(labels(&menus, 0, 8), None, "a menu left empty goes");
    assert_eq!(menus.menu_count(), 1);
}

#[test]
fn an_exporter_cannot_grow_the_cache_past_its_bound() {
    let over = MAX_MENUS as u32 + 10;
    let reply = (0..over).map(|m| (0, m, vec![row("Row")])).collect();
    let mut menus = GtkMenus::default();
    menus.begin(&[0]);
    menus.started(&[0], Some(1), reply);
    assert_eq!(
        menus.menu_count(),
        MAX_MENUS,
        "the reply's menus past the bound"
    );

    for m in over..over + 100 {
        menus.apply_change(2, (0, m, 0, 0, vec![row("More")]));
    }
    assert_eq!(
        menus.menu_count(),
        MAX_MENUS,
        "the records' menus past the bound"
    );
    // the menus kept still change
    menus.apply_change(2, (0, 0, 1, 0, vec![row("Second")]));
    assert_eq!(
        labels(&menus, 0, 0),
        Some(vec!["Row".to_owned(), "Second".to_owned()])
    );
}

#[test]
fn a_record_sent_before_the_reply_that_holds_it_is_not_applied_again() {
    let mut menus = subscribed(Some(10));
    // sent before the reply, delivered after it: the reply's rows hold it already
    menus.apply_change(9, (0, 0, 1, 0, vec![row("Open")]));
    menus.apply_change(10, (0, 0, 1, 0, vec![row("Open")]));
    assert_eq!(labels(&menus, 0, 0), Some(vec!["New".to_owned()]));
    menus.apply_change(11, (0, 0, 1, 0, vec![row("Save")]));
    assert_eq!(
        labels(&menus, 0, 0),
        Some(vec!["New".to_owned(), "Save".to_owned()])
    );

    // ended and started again from a reply without a serial, the earlier reply no longer counts
    menus.ended(&[0]);
    menus.begin(&[0]);
    menus.started(&[0], None, vec![(0, 0, vec![row("New")])]);
    menus.apply_change(5, (0, 0, 1, 0, vec![row("Close")]));
    assert_eq!(
        labels(&menus, 0, 0),
        Some(vec!["New".to_owned(), "Close".to_owned()])
    );
}
