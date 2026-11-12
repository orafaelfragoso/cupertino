use crate::support;

use cupertino_geometry::{Insets, Size};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Dialog, DialogAction, DialogButton, DialogKind, Modal, Stack,
    TextChange, TextField,
};
use cupertino_ui_core::{
    Application, Command, CommandRegistry, Element, NodeId, Role, Shortcut, Window,
    WindowBackground, WindowKey,
};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{DecorationMode, Keysym, Mods, WindowEvent, WindowSpec};
use cupertino_ui_text::{ContentPurpose, ImeBatch, ImeEvent, Preedit};
use support::{Harness, ESCAPE, RETURN, SPACE, TAB};

const N: (u32, u32) = (49, 0x6e);

/// A page with an opener and a button behind it, and a "Save changes?" sheet over it while open. `FIELD` puts a text
/// field in the sheet; `CANCEL` gives it a Cancel button.
struct Page<const FIELD: bool, const CANCEL: bool>;
type App = Page<false, true>;

#[derive(Default)]
struct Model {
    open: bool,
    name: String,
    revision: u64,
    said: Vec<Message>,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Open,
    Behind,
    Shortcut,
    DontSave,
    Cancel,
    Save,
    Submit,
    Edit(TextChange),
}

impl<const FIELD: bool, const CANCEL: bool> Application for Page<FIELD, CANCEL> {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Open => model.open = true,
            Message::DontSave | Message::Cancel | Message::Save => model.open = false,
            Message::Edit(change) => {
                model.name = change.text;
                model.revision = change.revision;
                return Vec::new();
            }
            _ => {}
        }
        model.said.push(message);
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let button = |label: &str, on_press| {
            Element::new(Button {
                label: label.into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(on_press),
            })
        };
        let content = Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 4.0,
            align: Align::Start,
            children: vec![
                button("Open", Message::Open).key("open"),
                button("Behind", Message::Behind).key("behind"),
            ],
        });
        let choice = |label: &str, action, on_press| DialogButton {
            label: label.into(),
            action,
            on_press,
        };
        let mut buttons = vec![choice(
            "Don't Save",
            DialogAction::Destructive,
            Message::DontSave,
        )];
        if CANCEL {
            buttons.push(choice("Cancel", DialogAction::Cancel, Message::Cancel));
        }
        buttons.push(choice("Save", DialogAction::Default, Message::Save));
        let dialog = model.open.then(|| Dialog {
            kind: DialogKind::Alert,
            icon: None,
            title: "Save changes?".into(),
            message: "Your changes are lost if you don't save them.".into(),
            body: FIELD.then(|| {
                Element::new(TextField {
                    label: "File name".into(),
                    placeholder: String::new(),
                    value: model.name.clone(),
                    revision: model.revision,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(Message::Edit)),
                    on_submit: Some(Message::Submit),
                    selection: None,
                    initial: false,
                })
                .key("field")
            }),
            buttons,
        });
        vec![Window {
            key: WindowKey("test".into()),
            spec: WindowSpec {
                title: "Test".into(),
                app_id: "test.controls".into(),
                size: Size::new(480.0, 320.0),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::Preferred,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new().with(
                Shortcut {
                    mods: Mods::PRIMARY,
                    keysym: Keysym(N.1),
                },
                Message::Shortcut,
            ),
            on_close: None,
            background: WindowBackground::Theme,
            popup: None,
            content: Element::from(Modal { content, dialog }),
            menus: None,
        }]
    }
}

/// The bounds of the button named `name`.
fn button_rect<A: Application>(h: &Harness<A>, name: &str) -> cupertino_geometry::Rect {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::Button && n.name.as_deref() == Some(name))
        .map(|(_, n)| n.bounds)
        .unwrap_or_else(|| panic!("no button {name}"))
}

/// Three answers are stacked as full-width capsules, the default on top and the given order reversed below it; two
/// short ones go side by side at equal widths in their given order, the default trailing. The alert hangs under the
/// toolbar row, centred across the window.
#[test]
fn an_alert_stacks_three_answers_default_first_and_puts_two_short_ones_side_by_side() {
    let mut h = opened::<false, true>();
    h.resize(Size::new(480.0, 440.0));
    let (save, cancel, dont) = (
        button_rect(&h, "Save"),
        button_rect(&h, "Cancel"),
        button_rect(&h, "Don't Save"),
    );
    assert!(save.max_y() <= cancel.min_y() && cancel.max_y() <= dont.min_y());
    assert_eq!(cancel.min_y() - save.max_y(), 8.0, "8 pt between capsules");
    for b in [save, cancel, dont] {
        assert_eq!(
            (b.min_x(), b.width(), b.height()),
            (save.min_x(), 228.0, 28.0)
        );
    }
    assert_eq!(
        save.min_x(),
        (480.0 - 260.0) / 2.0 + 16.0,
        "16 pt in from the alert's sides"
    );
    let alert = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == Role::AlertDialog)
        .map(|(_, n)| n.bounds)
        .unwrap();
    assert_eq!((alert.min_y(), alert.width()), (52.0, 260.0));
    assert_eq!(alert.max_y() - dont.max_y(), 16.0, "16 pt under the last");

    let mut two = opened::<false, false>();
    two.resize(Size::new(480.0, 440.0));
    let (dont, save) = (button_rect(&two, "Don't Save"), button_rect(&two, "Save"));
    assert_eq!(dont.min_y(), save.min_y(), "side by side");
    assert_eq!(dont.width(), save.width(), "at equal widths");
    assert_eq!(dont.width(), (228.0 - 8.0) / 2.0);
    assert_eq!(save.min_x() - dont.max_x(), 8.0);
}

/// The accessibility node with `role` named `name`.
fn find<A: Application>(h: &Harness<A>, role: Role, name: &str) -> Option<NodeId> {
    h.snapshot()
        .accessibility
        .into_iter()
        .find(|(_, n)| n.role == role && n.name.as_deref() == Some(name))
        .map(|(id, _)| id)
}

/// The name of the focused node.
fn focus<A: Application>(h: &Harness<A>) -> Option<String> {
    let snapshot = h.snapshot();
    let focus = snapshot.focus?;
    snapshot
        .accessibility
        .into_iter()
        .find(|(id, _)| *id == focus)
        .and_then(|(_, n)| n.name)
}

/// Opens the sheet from the keyboard, as a user does.
fn opened<const FIELD: bool, const CANCEL: bool>() -> Harness<Page<FIELD, CANCEL>> {
    let mut h = Harness::<Page<FIELD, CANCEL>>::new();
    h.tap(TAB);
    assert!(h.focused("open"));
    h.tap(SPACE);
    assert!(h.model().open);
    h
}

fn said<const FIELD: bool, const CANCEL: bool>(
    h: &Harness<Page<FIELD, CANCEL>>,
) -> Option<Message> {
    h.model().said.last().cloned()
}

#[test]
fn the_sheet_takes_the_focus_keeps_tab_inside_and_gives_it_back_when_it_closes() {
    let mut h = opened::<false, true>();
    let mut seen = vec![focus(&h).unwrap()];
    for _ in 0..3 {
        h.tap(TAB);
        seen.push(focus(&h).unwrap());
    }
    assert_eq!(
        seen,
        ["Cancel", "Don't Save", "Save", "Cancel"],
        "Tab goes down the alert's column (Save, Cancel, Don't Save) and wraps inside it"
    );

    h.tap(ESCAPE);
    assert_eq!(said(&h), Some(Message::Cancel));
    assert!(!h.model().open);
    assert!(h.focused("open"), "the focus goes back to the opener");
}

#[test]
fn return_activates_the_default_button_wherever_the_focus_is_in_the_sheet() {
    for tabs in 0..3 {
        let mut h = opened::<false, true>();
        for _ in 0..tabs {
            h.tap(TAB);
        }
        h.tap(RETURN);
        assert_eq!(said(&h), Some(Message::Save), "after {tabs} tabs");
    }
}

#[test]
fn space_activates_the_focused_button() {
    let mut h = opened::<false, true>();
    h.tap(TAB);
    assert_eq!(focus(&h).as_deref(), Some("Don't Save"));
    h.tap(SPACE);
    assert_eq!(said(&h), Some(Message::DontSave));
}

#[test]
fn without_a_cancel_button_escape_leaves_the_sheet_open() {
    let mut h = opened::<false, false>();
    h.tap(ESCAPE);
    assert!(h.model().open);
    assert_eq!(said(&h), Some(Message::Open));
}

#[test]
fn the_content_behind_takes_no_press_or_shortcut_while_the_sheet_is_up() {
    let mut h = Harness::<App>::new();
    let behind = h.center("behind");
    let shortcut = |h: &mut Harness<App>| {
        h.press_key(N.0, N.1, true, false, Mods::PRIMARY);
        h.press_key(N.0, N.1, false, false, Mods::PRIMARY);
    };
    h.click(behind);
    shortcut(&mut h);
    assert_eq!(h.model().said, [Message::Behind, Message::Shortcut]);

    h.tap(TAB);
    h.tap(SPACE);
    h.click(behind);
    shortcut(&mut h);
    assert_eq!(
        said(&h),
        Some(Message::Open),
        "the scrim and the sheet take them"
    );
    assert!(h.model().open);
}

#[test]
fn a_body_field_takes_the_focus_when_the_sheet_opens() {
    let mut h = opened::<true, true>();
    assert!(h.focused("field"));
    h.type_text("a");
    assert_eq!(h.model().name, "a");
}

#[test]
fn a_focused_field_keeps_return_and_escape_while_composing() {
    let mut h = opened::<true, true>();
    h.send(WindowEvent::Ime(ImeEvent::Done(ImeBatch {
        preedit: Some(Preedit {
            text: "ka".into(),
            cursor: None,
        }),
        commit: None,
        delete: (0, 0),
        current: true,
    })));
    h.tap(ESCAPE);
    assert!(
        h.model().open,
        "Escape cancels the composition, not the sheet"
    );
    h.tap(RETURN);
    assert_eq!(
        said(&h),
        Some(Message::Submit),
        "the field's submit wins over the default button"
    );
    assert!(h.model().open);
}

#[test]
fn the_sheet_is_a_modal_alert_named_by_its_title_and_the_content_behind_is_hidden() {
    let mut h = Harness::<App>::new();
    assert!(find(&h, Role::Button, "Behind").is_some());
    h.tap(TAB);
    h.tap(SPACE);
    let sheet = find(&h, Role::AlertDialog, "Save changes?").expect("the sheet is an alert");
    let node = h
        .snapshot()
        .accessibility
        .into_iter()
        .find(|(id, _)| *id == sheet)
        .unwrap()
        .1;
    assert!(node.states.modal);
    assert_eq!(
        node.description.as_deref(),
        Some("Your changes are lost if you don't save them.")
    );
    assert!(
        find(&h, Role::Button, "Behind").is_none(),
        "hidden from assistive technology"
    );
}
