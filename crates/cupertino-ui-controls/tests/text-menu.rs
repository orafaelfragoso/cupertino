//! The menu of a text field: a secondary click, the Menu key, Shift+F10 or a resting finger opens Cut, Copy, Paste
//! and Select All on every kind of field, without its application declaring a menu. Each row is enabled only when it
//! can act, does what its keys do, and the menu is a menu as any other.

use crate::support;

use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_controls::{
    BareField, BareSecureField, SearchField, SecureField, Stack, TextChange, TextEditor, TextField,
};
use cupertino_ui_core::{
    AccessibleValue, Application, Command, Element, NodeId, Role, Window, WindowKey,
};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{
    ClipboardData, ClipboardKind, Mods, MouseButton, PointerEvent, PopupEdge, TouchEvent,
    WindowEvent, WindowId, WindowStates,
};
use cupertino_ui_text::ContentPurpose;
use support::{Harness, ESCAPE, RETURN, TAB};

const LEFT: (u32, u32) = (105, 0xff51);
const A: u32 = 30;
const C: u32 = 46;
const V: u32 = 47;
const Z: u32 = 44;
const MENU: u32 = 0xff67;
const F10: u32 = 0xffc7;

/// The text fields, by the key of each: a plain one, a search field, a multi-line editor and a field without chrome.
const TEXTS: [&str; 4] = ["name", "search", "notes", "bare"];
const ROWS: [&str; 4] = ["Cut", "Copy", "Paste", "Select All"];

struct Form<const ENABLED: bool>;
type App = Form<true>;

#[derive(Default)]
struct Model {
    /// The text of each of [`TEXTS`], then of the field without chrome that has no menu.
    texts: [String; 5],
    revisions: [u64; 5],
    changes: usize,
    /// The length of the password after each edit.
    typed: Vec<usize>,
    /// The same for the password field without chrome.
    lock: Vec<usize>,
    /// The plain field is gone from the window.
    hidden: bool,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Changed(usize, TextChange),
    Typed(usize),
    Lock(usize),
    Hide,
}

impl<const ENABLED: bool> Form<ENABLED> {
    fn field(model: &Model, index: usize) -> TextField<Message> {
        TextField {
            label: format!("field {index}"),
            placeholder: String::new(),
            value: model.texts[index].clone(),
            revision: model.revisions[index],
            purpose: ContentPurpose::Normal,
            enabled: ENABLED,
            on_change: Some(Box::new(move |change| Message::Changed(index, change))),
            on_submit: Some(Message::Hide),
            selection: None,
            initial: false,
        }
    }
}

impl<const ENABLED: bool> Application for Form<ENABLED> {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let mut model = Model::default();
        if !ENABLED {
            model.texts[0] = "Ada".into();
            model.texts[2] = "Two\nlines".into();
        }
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Changed(index, change) => {
                model.texts[index] = change.text;
                model.revisions[index] = change.revision;
                model.changes += 1;
            }
            Message::Typed(len) => model.typed.push(len),
            Message::Lock(len) => model.lock.push(len),
            Message::Hide => model.hidden = true,
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let mut children = Vec::new();
        if !model.hidden {
            children.push(Element::new(Self::field(model, 0)).key("name"));
        }
        children.extend([
            Element::new(SearchField {
                field: Self::field(model, 1),
                size: None,
            })
            .key("search"),
            Element::new(TextEditor {
                label: "notes".into(),
                placeholder: String::new(),
                value: model.texts[2].clone(),
                revision: model.revisions[2],
                enabled: ENABLED,
                bordered: false,
                on_change: Some(Box::new(|change| Message::Changed(2, change))),
            })
            .key("notes"),
            Element::new(BareField {
                menu: true,
                field: Self::field(model, 3),
                size: 20.0,
                hold_caret: false,
                blink_start: None,
                caret_layer: None,
                on_blink: None,
            })
            .key("bare"),
            Element::new(BareField {
                menu: false,
                field: Self::field(model, 4),
                size: 20.0,
                hold_caret: false,
                blink_start: None,
                caret_layer: None,
                on_blink: None,
            })
            .key("login"),
            Element::new(SecureField {
                label: "password".into(),
                enabled: ENABLED,
                on_change: Some(Box::new(|secret| {
                    Message::Typed(secret.with_secret(|s| s.chars().count()))
                })),
                ..SecureField::default()
            })
            .key("password"),
            Element::new(BareSecureField {
                field: SecureField {
                    label: "lock".into(),
                    enabled: ENABLED,
                    on_change: Some(Box::new(|secret| {
                        Message::Lock(secret.with_secret(|s| s.chars().count()))
                    })),
                    ..SecureField::default()
                },
                size: 20.0,
                weight: None,
                placeholder_color: None,
                centered: false,
                submit_width: 0.0,
            })
            .key("lock"),
        ]);
        support::window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 4.0,
            align: Align::Stretch,
            children,
        }))
    }
}

fn start<const E: bool>() -> Harness<Form<E>> {
    let mut h = Harness::<Form<E>>::new();
    h.resize(Size::new(320.0, 400.0));
    h
}

/// A harness with the keyboard in the field `key`.
fn focused(key: &'static str) -> Harness<App> {
    let mut h = start::<true>();
    let at = Point::new(h.rect(key).max_x() - 4.0, h.center(key).y);
    h.click(at);
    assert!(h.focused(key));
    h
}

/// The text of the field `key`, and what of it is selected, as assistive technology reads them.
fn editing<const E: bool>(
    h: &Harness<Form<E>>,
    key: &'static str,
) -> (String, std::ops::Range<usize>) {
    match h.accessible(key).value {
        Some(AccessibleValue::Text {
            value,
            editing: Some(editing),
        }) => (value, editing.selection),
        other => panic!("not editable text: {other:?}"),
    }
}

fn menu<const E: bool>(h: &Harness<Form<E>>) -> Option<WindowId> {
    h.rt.windows()
        .into_iter()
        .find(|(k, _)| *k == WindowKey("cupertino.text-menu".into()))
        .map(|(_, id)| id)
}

fn secondary_click<const E: bool>(h: &mut Harness<Form<E>>, position: Point) {
    h.pointer(PointerEvent::Motion { position });
    for pressed in [true, false] {
        h.pointer(PointerEvent::Button {
            position,
            button: MouseButton::Right,
            pressed,
            serial: 1,
            mods: Mods::NONE,
        });
    }
}

/// The rows of the open menu from the top: each one's node, name, whether it is enabled, and its rectangle.
fn rows<const E: bool>(h: &Harness<Form<E>>) -> Vec<(NodeId, String, bool, Rect)> {
    let id = menu(h).expect("the menu is open");
    let snapshot = h.rt.snapshot(id).expect("the menu has drawn");
    let mut rows: Vec<_> = snapshot
        .accessibility
        .iter()
        .filter(|(_, node)| node.role == Role::MenuItem)
        .map(|(id, node)| {
            (
                *id,
                node.name.clone().unwrap_or_default(),
                !node.states.disabled,
                node.bounds,
            )
        })
        .collect();
    rows.sort_by(|a, b| a.3.min_y().total_cmp(&b.3.min_y()));
    rows
}

/// The names of the rows that can be chosen.
fn enabled<const E: bool>(h: &Harness<Form<E>>) -> Vec<String> {
    rows(h)
        .into_iter()
        .filter(|row| row.2)
        .map(|row| row.1)
        .collect()
}

/// Lets the open menu draw, so its rows can be read. Input still goes to the window it was opened in.
fn show<const E: bool>(h: &mut Harness<Form<E>>) {
    let main = h.window;
    let id = menu(h).expect("the menu is open");
    h.rt.window_system_mut()
        .configure(id, None, WindowStates::default())
        .unwrap();
    h.window = id;
    h.settle();
    h.window = main;
}

/// Opens the menu of the field `key` with a secondary click at `at`.
fn open_at<const E: bool>(h: &mut Harness<Form<E>>, at: Point) {
    secondary_click(h, at);
    show(h);
}

/// Opens the menu of the field `key` with a secondary click past the end of its text.
fn open<const E: bool>(h: &mut Harness<Form<E>>, key: &'static str) {
    let at = Point::new(h.rect(key).max_x() - 4.0, h.center(key).y);
    open_at(h, at);
}

/// Chooses the row named `name` of the open menu with a click, and waits for the menu to close.
fn choose<const E: bool>(h: &mut Harness<Form<E>>, name: &str) {
    let main = h.window;
    let row = rows(h)
        .into_iter()
        .find(|row| row.1 == name)
        .expect("the row");
    h.window = menu(h).unwrap();
    h.click(Point::new(row.3.min_x() + 20.0, row.3.min_y() + 10.0));
    h.finish_flash();
    h.window = main;
    h.settle();
    assert_eq!(menu(h), None, "choosing closes the menu");
}

/// Dismisses the open menu as the compositor does on a press outside it.
fn dismiss<const E: bool>(h: &mut Harness<Form<E>>) {
    let main = h.window;
    h.window = menu(h).expect("the menu is open");
    h.send(WindowEvent::PopupDone);
    h.window = main;
    h.settle();
    assert_eq!(menu(h), None);
}

fn clipboard<const E: bool>(h: &Harness<Form<E>>) -> Option<String> {
    h.rt.window_system()
        .clipboard(ClipboardKind::Clipboard)
        .map(|d| String::from_utf8(d.bytes.clone()).unwrap())
}

/// Another application copies `text`.
fn copied<const E: bool>(h: &mut Harness<Form<E>>, mime: &str, bytes: &[u8]) {
    h.rt.window_system_mut().offer_clipboard(
        ClipboardKind::Clipboard,
        Some(ClipboardData {
            mime: mime.into(),
            bytes: bytes.to_vec(),
        }),
    );
    h.settle();
}

fn select_all(h: &mut Harness<App>) {
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
}

#[test]
fn a_secondary_click_opens_the_menu_at_the_pointer_on_every_kind_of_field() {
    for key in TEXTS.into_iter().chain(["password"]) {
        let mut h = start::<true>();
        let at = Point::new(h.rect(key).max_x() - 4.0, h.center(key).y);
        open_at(&mut h, at);
        let id = menu(&h).unwrap_or_else(|| panic!("{key} has a menu"));
        let system = h.rt.window_system();
        assert_eq!(system.parent(id).unwrap(), Some(h.window), "{key}");
        let placed = system.placement(id).unwrap().unwrap();
        assert_eq!(placed.anchor_rect, Rect::new(at.x, at.y, 1.0, 1.0), "{key}");
        assert_eq!(
            (placed.anchor, placed.gravity),
            (PopupEdge::BottomRight, PopupEdge::BottomRight),
            "{key}"
        );
        assert!(placed.grab, "{key}: it takes the keyboard");
        let names: Vec<String> = rows(&h).into_iter().map(|row| row.1).collect();
        assert_eq!(names, ROWS, "{key}");
        assert!(h.focused(key), "{key} takes the keyboard as on any press");
        // an empty field with nothing on the clipboard: no row can act
        assert!(enabled(&h).is_empty(), "{key}: {:?}", enabled(&h));
    }
}

#[test]
fn it_is_a_menu_as_any_other() {
    let mut h = start::<true>();
    open(&mut h, "name");
    let id = menu(&h).unwrap();
    let snapshot = h.rt.snapshot(id).unwrap();
    assert!(
        snapshot
            .accessibility
            .iter()
            .any(|(_, node)| node.role == Role::Menu && node.name.as_deref() == Some("Edit")),
        "a menu by name"
    );
    let separators = snapshot
        .accessibility
        .iter()
        .filter(|(_, node)| node.role == Role::Separator)
        .count();
    assert_eq!(separators, 1);
    let rows: Vec<Rect> = rows(&h).into_iter().map(|row| row.3).collect();
    // rows 24 points tall, the first 5 points under the panel's top, 11 points for the separator
    assert!(rows.iter().all(|r| r.height() == 24.0), "{rows:?}");
    assert_eq!(rows[0].min_y(), 5.0);
    let gaps: Vec<f64> = rows
        .windows(2)
        .map(|w| w[1].min_y() - w[0].max_y())
        .collect();
    assert_eq!(
        gaps,
        [0.0, 0.0, 11.0],
        "the separator stands before Select All"
    );
    assert!(rows[0].width() >= 120.0 - 10.0, "{rows:?}");
}

#[test]
fn each_row_is_enabled_only_when_it_can_act() {
    for key in TEXTS {
        let mut h = focused(key);
        h.type_text("Ada");
        open(&mut h, key);
        assert_eq!(enabled(&h), ["Select All"], "{key}: text, no selection");
        dismiss(&mut h);

        select_all(&mut h);
        // inside the selection, which stays
        let inside = Point::new(h.rect(key).min_x() + 12.0, h.rect(key).min_y() + 10.0);
        open_at(&mut h, inside);
        assert_eq!(enabled(&h), ["Cut", "Copy", "Select All"], "{key}");
        dismiss(&mut h);

        copied(&mut h, "image/png", &[1, 2, 3]);
        open_at(&mut h, inside);
        assert_eq!(
            enabled(&h),
            ["Cut", "Copy", "Select All"],
            "{key}: a picture is nothing to paste"
        );
        dismiss(&mut h);

        copied(&mut h, "text/plain;charset=utf-8", b"Grace");
        open_at(&mut h, inside);
        assert_eq!(enabled(&h), ROWS, "{key}");
        dismiss(&mut h);

        h.rt.window_system_mut()
            .offer_clipboard(ClipboardKind::Clipboard, None);
        h.settle();
        open_at(&mut h, inside);
        assert_eq!(enabled(&h), ["Cut", "Copy", "Select All"], "{key}: cleared");
    }
}

#[test]
fn what_the_field_itself_copied_can_be_pasted() {
    let mut h = focused("name");
    h.type_text("Ada");
    select_all(&mut h);
    h.press_key(C, 'c' as u32, true, false, Mods::PRIMARY);
    open(&mut h, "search");
    assert_eq!(enabled(&h), ["Paste"]);
    choose(&mut h, "Paste");
    assert_eq!(h.model().texts[1], "Ada");
}

#[test]
fn the_rows_do_what_their_keys_do() {
    for (index, key) in TEXTS.into_iter().enumerate() {
        let mut h = focused(key);
        h.type_text("Ada");
        let changes = h.model().changes;

        open(&mut h, key);
        choose(&mut h, "Select All");
        assert_eq!(editing(&h, key), ("Ada".into(), 0..3), "{key}");
        assert!(h.focused(key), "{key} has the keyboard again");
        assert_eq!(h.model().changes, changes, "{key}: selecting edits nothing");

        let inside = Point::new(h.rect(key).min_x() + 12.0, h.rect(key).min_y() + 10.0);
        open_at(&mut h, inside);
        choose(&mut h, "Copy");
        assert_eq!(clipboard(&h).as_deref(), Some("Ada"), "{key}");
        assert_eq!(editing(&h, key), ("Ada".into(), 0..3), "{key}");
        assert_eq!(h.model().changes, changes, "{key}: copying edits nothing");

        open_at(&mut h, inside);
        choose(&mut h, "Cut");
        assert_eq!(clipboard(&h).as_deref(), Some("Ada"), "{key}");
        assert_eq!(h.model().texts[index], "", "{key}");
        assert_eq!(h.model().changes, changes + 1, "{key}: one edit");

        copied(&mut h, "text/plain;charset=utf-8", "Grace".as_bytes());
        open(&mut h, key);
        choose(&mut h, "Paste");
        assert_eq!(h.model().texts[index], "Grace", "{key}");
        assert_eq!(editing(&h, key), ("Grace".into(), 5..5), "{key}");
        assert_eq!(h.model().changes, changes + 2, "{key}");

        // each is one step to undo, as from the keys: the paste, then the cut
        h.press_key(Z, 'z' as u32, true, false, Mods::PRIMARY);
        assert_eq!(h.model().texts[index], "", "{key}");
        h.press_key(Z, 'z' as u32, true, false, Mods::PRIMARY);
        assert_eq!(h.model().texts[index], "Ada", "{key}");
    }
}

#[test]
fn the_paste_keys_insert_the_clipboards_text_in_place_of_the_selection() {
    for (index, key) in TEXTS.into_iter().enumerate() {
        let mut h = focused(key);
        h.type_text("Ada");
        copied(&mut h, "text/plain;charset=utf-8", "Gr\u{e4}ce".as_bytes());
        h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
        assert_eq!(h.model().texts[index], "AdaGr\u{e4}ce", "{key}");
        select_all(&mut h);
        h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
        assert_eq!(h.model().texts[index], "Gr\u{e4}ce", "{key}");
        // without text on the clipboard the keys do nothing
        copied(&mut h, "image/png", &[1, 2, 3]);
        h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
        assert_eq!(h.model().texts[index], "Gr\u{e4}ce", "{key}");
    }
}

#[test]
fn a_line_break_pasted_into_a_field_is_a_space_and_stays_in_an_editor() {
    let mut h = focused("name");
    copied(&mut h, "text/plain", b"Two\nlines");
    h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
    assert_eq!(h.model().texts[0], "Two lines");
    let mut h = focused("notes");
    copied(&mut h, "text/plain", b"Two\nlines");
    h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
    assert_eq!(h.model().texts[2], "Two\nlines");
}

#[test]
fn a_secondary_click_inside_the_selection_keeps_it_and_one_outside_moves_the_caret_there() {
    for key in TEXTS {
        let mut h = focused(key);
        h.type_text("Ada Lovelace");
        for _ in 0..4 {
            h.press_key(LEFT.0, LEFT.1, true, false, Mods::SHIFT);
        }
        assert_eq!(editing(&h, key).1, 8..12, "{key}");
        // on the selected end of the text
        let end = Point::new(h.rect(key).max_x() - 4.0, h.center(key).y);
        let start = Point::new(h.rect(key).min_x() + 1.0, h.rect(key).min_y() + 10.0);
        select_all(&mut h);
        open_at(&mut h, start);
        assert_eq!(editing(&h, key).1, 0..12, "{key}: inside");
        dismiss(&mut h);
        assert_eq!(editing(&h, key).1, 0..12, "{key}: it outlives the menu");

        h.click(end);
        for _ in 0..4 {
            h.press_key(LEFT.0, LEFT.1, true, false, Mods::SHIFT);
        }
        assert_eq!(editing(&h, key).1, 8..12, "{key}");
        open_at(&mut h, start);
        let selection = editing(&h, key).1;
        assert!(
            selection.is_empty() && selection.start <= 1,
            "{key}: the caret went to the click: {selection:?}"
        );
        assert_eq!(enabled(&h), ["Select All"], "{key}");
    }
}

#[test]
fn the_menu_key_and_shift_f10_open_it_under_the_caret() {
    for key in TEXTS {
        let mut h = focused(key);
        let field = h.rect(key);
        h.tap(MENU);
        show(&mut h);
        let empty =
            h.rt.window_system()
                .placement(menu(&h).unwrap())
                .unwrap()
                .unwrap()
                .anchor_rect;
        assert!(
            field.min_x() <= empty.min_x() && empty.min_x() < field.min_x() + 40.0,
            "{key}: {empty:?} in {field:?}"
        );
        assert!(
            field.min_y() < empty.min_y() && empty.min_y() <= field.max_y(),
            "{key}: under the caret, {empty:?} in {field:?}"
        );
        dismiss(&mut h);

        h.type_text("Ada Lovelace");
        h.key_event(F10, true, false, Mods::SHIFT);
        show(&mut h);
        let typed =
            h.rt.window_system()
                .placement(menu(&h).unwrap())
                .unwrap()
                .unwrap()
                .anchor_rect;
        assert!(
            typed.min_x() > empty.min_x() + 30.0,
            "{key}: the caret moved with the text: {typed:?} after {empty:?}"
        );
        assert_eq!(enabled(&h), ["Select All"], "{key}");
        // from the keyboard the menu's rows are reached with the arrows: Select All is the one enabled row
        let main = h.window;
        h.window = menu(&h).unwrap();
        h.key(RETURN, true);
        h.finish_flash();
        h.window = main;
        h.settle();
        assert_eq!(menu(&h), None, "{key}");
        assert_eq!(editing(&h, key).1, 0..12, "{key}");
    }
}

#[test]
fn escape_closes_it_and_the_field_keeps_the_keyboard_and_its_selection() {
    let mut h = focused("name");
    h.type_text("Ada");
    select_all(&mut h);
    h.tap(MENU);
    show(&mut h);
    let main = h.window;
    h.window = menu(&h).unwrap();
    h.key(ESCAPE, true);
    h.window = main;
    h.settle();
    assert_eq!(menu(&h), None);
    assert!(h.focused("name"));
    assert_eq!(editing(&h, "name"), ("Ada".into(), 0..3));
    assert_eq!(h.model().changes, 3, "nothing was edited");
}

#[test]
fn a_finger_resting_on_a_field_opens_its_menu_there() {
    let mut h = start::<true>();
    let at = h.center("name");
    h.touch(TouchEvent::Down {
        id: 1,
        position: at,
    });
    assert_eq!(menu(&h), None, "not before it rested");
    h.later(600);
    let id = menu(&h).expect("the rest opened it");
    let placed = h.rt.window_system().placement(id).unwrap().unwrap();
    assert_eq!(placed.anchor_rect, Rect::new(at.x, at.y, 1.0, 1.0));

    // a finger that moves on is no rest
    let mut h = start::<true>();
    h.touch(TouchEvent::Down {
        id: 1,
        position: at,
    });
    h.touch(TouchEvent::Motion {
        id: 1,
        position: Point::new(at.x + 30.0, at.y),
    });
    h.later(600);
    assert_eq!(menu(&h), None);
}

#[test]
fn a_disabled_field_is_read_only_and_its_menu_only_copies_its_text() {
    for (key, text) in [("name", "Ada"), ("notes", "Two\nlines")] {
        let mut h = start::<false>();
        copied(&mut h, "text/plain", b"Grace");
        open(&mut h, key);
        assert_eq!(enabled(&h), ["Copy"], "{key}: neither Cut nor Paste");
        assert!(!h.focused(key), "{key} takes no keyboard");
        choose(&mut h, "Copy");
        assert_eq!(clipboard(&h).as_deref(), Some(text), "{key}");
        assert_eq!(h.model().changes, 0, "{key}");
    }
    // an empty one has nothing to copy, and a disabled password field nothing at all
    for key in ["search", "password"] {
        let mut h = start::<false>();
        copied(&mut h, "text/plain", b"Grace");
        open(&mut h, key);
        assert!(enabled(&h).is_empty(), "{key}");
    }
}

#[test]
fn a_password_field_never_cuts_or_copies_and_takes_a_paste() {
    let mut h = focused("password");
    h.type_text("secret");
    select_all(&mut h);
    copied(&mut h, "text/plain;charset=utf-8", b"hunter2\n");
    let before = clipboard(&h);
    let inside = Point::new(h.rect("password").min_x() + 12.0, h.center("password").y);
    open_at(&mut h, inside);
    assert_eq!(
        enabled(&h),
        ["Paste", "Select All"],
        "a selected password is neither cut nor copied"
    );
    choose(&mut h, "Paste");
    assert_eq!(
        h.model().typed.last(),
        Some(&7),
        "the paste replaced the selection, without its line break"
    );
    assert_eq!(clipboard(&h), before, "nothing left the field");
    assert_eq!(
        h.accessible("password").value,
        Some(AccessibleValue::Protected)
    );

    // Select All, then typing replaces everything
    open(&mut h, "password");
    choose(&mut h, "Select All");
    h.type_text("x");
    assert_eq!(h.model().typed.last(), Some(&1));

    // the keys paste too
    h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
    assert_eq!(h.model().typed.last(), Some(&8));
}

#[test]
fn a_password_field_without_chrome_has_no_menu_and_takes_no_paste() {
    let mut h = focused("lock");
    h.type_text("secret");
    copied(&mut h, "text/plain;charset=utf-8", b"hunter2");
    let at = h.center("lock");
    secondary_click(&mut h, at);
    assert_eq!(menu(&h), None);
    h.tap(MENU);
    h.key_event(F10, true, false, Mods::SHIFT);
    assert_eq!(menu(&h), None);
    h.touch(TouchEvent::Down {
        id: 1,
        position: at,
    });
    h.later(600);
    assert_eq!(menu(&h), None);
    h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
    assert_eq!(h.model().lock, [1, 2, 3, 4, 5, 6], "only what was typed");
}

#[test]
fn a_field_without_chrome_can_have_no_menu_and_take_no_paste() {
    let mut h = focused("login");
    h.type_text("ada");
    copied(&mut h, "text/plain;charset=utf-8", b"Grace");
    let at = h.center("login");
    secondary_click(&mut h, at);
    assert_eq!(menu(&h), None);
    h.tap(MENU);
    h.key_event(F10, true, false, Mods::SHIFT);
    assert_eq!(menu(&h), None);
    h.press_key(V, 'v' as u32, true, false, Mods::PRIMARY);
    assert_eq!(h.model().texts[4], "ada");
    assert!(h.focused("login"));
}

#[test]
fn the_menu_goes_with_its_field() {
    let mut h = focused("name");
    h.type_text("Ada");
    open(&mut h, "name");
    assert!(menu(&h).is_some());
    // the application takes the field out of its window
    h.key(RETURN, true);
    assert!(h.model().hidden);
    assert_eq!(menu(&h), None);

    // another field's menu opens in place of one that is open, and only one is ever open
    let mut h = focused("search");
    h.type_text("Ada");
    open(&mut h, "search");
    open(&mut h, "notes");
    assert_eq!(h.rt.windows().len(), 2);
    assert!(
        enabled(&h).is_empty(),
        "the menu of the empty editor, not the field's"
    );
    choose_nothing(&mut h);
}

/// Tab in the window under an open menu changes nothing the menu depends on.
fn choose_nothing(h: &mut Harness<App>) {
    h.tap(TAB);
    dismiss(h);
}
