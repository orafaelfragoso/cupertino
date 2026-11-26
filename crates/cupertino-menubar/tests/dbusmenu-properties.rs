//! The DBusMenu item and menu properties as the client reads them: toggles and their indeterminate default, removed
//! properties back at their defaults, dispositions, descriptions, key sequences, icon data, children-display, and the
//! menu's own `TextDirection`, `Status`, `IconThemePath` and `Version` laid on the tree.

use std::collections::HashMap;
use std::path::PathBuf;

use cupertino_menubar::dbus::dbusmenu::{
    apply_properties_update, make_node, parse_layout, MenuProperties, MenuTree, RawNode,
};
use cupertino_menubar::item::{
    Disposition, ItemId, MenuItem, ShortcutModifier, ShortcutStyle, Toggle,
};
use zbus::zvariant::{OwnedValue, Value};

fn value<'a>(v: impl Into<Value<'a>>) -> OwnedValue {
    OwnedValue::try_from(v.into()).unwrap()
}

fn props(pairs: Vec<(&str, OwnedValue)>) -> HashMap<String, OwnedValue> {
    pairs.into_iter().map(|(k, v)| (k.to_owned(), v)).collect()
}

/// A menu of one item with `item` as its properties, read as `GetLayout` answers it.
fn item(item: HashMap<String, OwnedValue>) -> MenuItem {
    let node: RawNode = make_node(
        0,
        HashMap::new(),
        vec![make_node(7, item, Vec::new()).unwrap()],
    )
    .unwrap();
    let root = parse_layout(&node).unwrap();
    root.children[0].clone()
}

#[test]
fn a_toggle_without_a_state_is_indeterminate_and_zero_is_off() {
    let check = item(props(vec![("toggle-type", value("checkmark"))]));
    assert_eq!(check.toggle, Toggle::Indeterminate);
    assert!(check.toggle.is_mixed() && check.toggle.is_marked());
    let radio = item(props(vec![("toggle-type", value("radio"))]));
    assert_eq!(radio.toggle, Toggle::RadioIndeterminate);
    let off = item(props(vec![
        ("toggle-type", value("checkmark")),
        ("toggle-state", value(0i32)),
    ]));
    assert_eq!(off.toggle, Toggle::Check(false));
    let on = item(props(vec![
        ("toggle-type", value("radio")),
        ("toggle-state", value(1i32)),
    ]));
    assert_eq!(on.toggle, Toggle::Radio(true));
    let other = item(props(vec![
        ("toggle-type", value("checkmark")),
        ("toggle-state", value(2i32)),
    ]));
    assert_eq!(other.toggle, Toggle::Indeterminate);
    let none = item(props(vec![("toggle-state", value(1i32))]));
    assert_eq!(
        none.toggle,
        Toggle::None,
        "a state without a type marks nothing"
    );
}

#[test]
fn a_radio_stays_a_radio_when_only_its_state_changes() {
    let mut root = MenuItem::submenu(
        0,
        "",
        vec![item(props(vec![("toggle-type", value("radio"))]))],
    );
    apply_properties_update(
        &mut root,
        &[(7, props(vec![("toggle-state", value(1i32))]))],
        &[],
    );
    assert_eq!(root.children[0].toggle, Toggle::Radio(true));
    apply_properties_update(&mut root, &[], &[(7, vec!["toggle-state".into()])]);
    assert_eq!(root.children[0].toggle, Toggle::RadioIndeterminate);
}

#[test]
fn removed_properties_take_their_defaults() {
    let shown = item(props(vec![
        ("type", value("separator")),
        ("label", value("_Old")),
        ("visible", value(false)),
        ("icon-name", value("document-open")),
        ("disposition", value("alert")),
        ("accessible-desc", value("Says more")),
        ("toggle-type", value("checkmark")),
        ("toggle-state", value(1i32)),
    ]));
    assert!(shown.separator && !shown.is_selectable());
    let mut root = MenuItem::submenu(0, "", vec![shown]);
    let removed: Vec<String> = [
        "type",
        "label",
        "visible",
        "icon-name",
        "disposition",
        "accessible-desc",
        "toggle-type",
    ]
    .map(String::from)
    .to_vec();
    apply_properties_update(&mut root, &[], &[(7, removed)]);
    let item = &root.children[0];
    assert!(!item.separator, "a standard item");
    assert!(
        item.enabled,
        "a separator turned standard is enabled unless told otherwise"
    );
    assert!(item.visible);
    assert!(item.is_selectable());
    assert!(item.label.is_empty() && item.mnemonic.is_none());
    assert_eq!(item.icon_name, None);
    assert_eq!(item.disposition, Disposition::Normal);
    assert_eq!(item.description, None);
    assert_eq!(item.toggle, Toggle::None);
}

#[test]
fn a_separator_turned_standard_keeps_a_disabled_state_it_was_given() {
    let shown = item(props(vec![
        ("type", value("separator")),
        ("enabled", value(false)),
    ]));
    let mut root = MenuItem::submenu(0, "", vec![shown]);
    apply_properties_update(
        &mut root,
        &[(7, props(vec![("type", value("standard"))]))],
        &[],
    );
    assert!(!root.children[0].separator);
    assert!(!root.children[0].enabled);
}

#[test]
fn dispositions_and_descriptions_are_read() {
    for (name, disposition) in [
        ("normal", Disposition::Normal),
        ("informative", Disposition::Informative),
        ("warning", Disposition::Warning),
        ("alert", Disposition::Alert),
        ("unknown", Disposition::Normal),
    ] {
        let item = item(props(vec![("disposition", value(name))]));
        assert_eq!(item.disposition, disposition, "{name}");
    }
    let described = item(props(vec![(
        "accessible-desc",
        value("Opens\u{202e} the file"),
    )]));
    assert_eq!(described.description.as_deref(), Some("Opens the file"));
}

/// The `aas` of a key sequence.
fn sequence(chords: &[&[&str]]) -> OwnedValue {
    let chords: Vec<Vec<String>> = chords
        .iter()
        .map(|c| c.iter().map(|t| (*t).to_owned()).collect())
        .collect();
    value(chords)
}

#[test]
fn a_shortcut_is_every_chord_of_its_sequence() {
    let single = item(props(vec![(
        "shortcut",
        sequence(&[&["Control", "Shift", "Super", "Alt", "q"]]),
    )]));
    let sc = single.shortcut.unwrap();
    assert_eq!(
        sc.modifiers,
        [
            ShortcutModifier::Ctrl,
            ShortcutModifier::Alt,
            ShortcutModifier::Shift,
            ShortcutModifier::Primary
        ]
    );
    assert_eq!(sc.key, "q");
    assert!(sc.before.is_empty());
    let chain = item(props(vec![(
        "shortcut",
        sequence(&[&["Control", "K"], &["Control", "C"]]),
    )]));
    let sc = chain.shortcut.unwrap();
    assert_eq!(sc.key, "C");
    assert_eq!(sc.before.len(), 1);
    assert_eq!(sc.before[0].key, "K");
    let shown = sc.shown(ShortcutStyle::Names);
    assert_eq!(shown.modifiers, "Ctrl+K Ctrl+");
    assert_eq!(shown.key, "C");
    let bad = item(props(vec![("shortcut", sequence(&[&["Hyper", "K"]]))]));
    assert_eq!(bad.shortcut, None, "an unknown modifier is no shortcut");
}

#[test]
fn icon_data_is_kept_as_its_bytes_and_children_display_marks_a_submenu() {
    let png = vec![0x89u8, b'P', b'N', b'G', 1, 2, 3];
    let item = item(props(vec![
        ("icon-data", value(png.clone())),
        ("children-display", value("submenu")),
    ]));
    assert_eq!(item.icon_data, Some(png));
    assert!(item.has_submenu && item.needs_load());
}

#[test]
fn the_menus_properties_are_laid_on_the_tree() {
    let mut menu = MenuProperties::default();
    assert!(
        menu.has_groups(),
        "an exporter that says no version is tried"
    );
    let changed = props(vec![
        ("Version", value(2u32)),
        ("TextDirection", value("rtl")),
        ("Status", value("notice")),
        (
            "IconThemePath",
            value(vec!["/opt/app/icons".to_owned(), "relative".to_owned()]),
        ),
    ]);
    assert!(menu.apply(&changed, &[]));
    assert!(!menu.has_groups(), "version 2 has no group calls");
    assert!(menu.rtl && menu.notice);
    assert_eq!(&*menu.icon_dirs, [PathBuf::from("/opt/app/icons")]);
    let mut root = MenuItem::submenu(
        0,
        "",
        vec![MenuItem::submenu(1, "File", vec![MenuItem::new(2, "Open")])],
    );
    menu.decorate(&mut root);
    assert!(root.notice);
    let open = root.find(ItemId(2)).unwrap();
    assert!(open.rtl);
    assert_eq!(&*open.icon_dirs, [PathBuf::from("/opt/app/icons")]);
    assert!(menu.apply(&HashMap::new(), &["Status".into()]));
    assert!(!menu.notice, "an invalidated status is normal");
    assert!(!menu.apply(&HashMap::new(), &[]));
}

#[test]
fn a_tree_shows_with_its_menus_properties() {
    let mut tree = MenuTree::default();
    tree.apply_reply(
        ItemId(0),
        1,
        MenuItem::submenu(0, "", vec![MenuItem::new(2, "Open")]),
    );
    tree.apply_menu_properties(&props(vec![("TextDirection", value("rtl"))]), &[]);
    assert!(tree.shown().unwrap().find(ItemId(2)).unwrap().rtl);
    assert!(!tree.root().unwrap().find(ItemId(2)).unwrap().rtl);
    assert!(tree.holds(ItemId(2)) && !tree.holds(ItemId(3)));
}
