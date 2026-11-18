//! A modal sheet: while it is shown the content behind takes nothing, Return answers with its default button and
//! Escape with its cancel button, Tab stays inside it, and the content takes input again once it is answered.

#[allow(dead_code)]
mod support;

use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, Dialog, DialogAction, DialogButton, DialogKind, Modal,
};
use cupertino_ui_core::platform::{Keysym, Mods};
use cupertino_ui_core::{Element, Role};
use support::{Scene, View, ESCAPE, RETURN, TAB};

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Behind,
    Delete,
    Cancel,
}

struct State {
    open: bool,
    sent: Vec<Msg>,
}

struct Confirm;

impl Scene for Confirm {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        State {
            open: true,
            sent: Vec::new(),
        }
    }

    fn update(state: &mut State, message: Msg) {
        if message != Msg::Behind {
            state.open = false;
        }
        state.sent.push(message);
    }

    fn view(state: &State) -> Element<Msg> {
        let content = Element::new(Button {
            label: "Behind".into(),
            role: ButtonRole::Normal,
            style: ButtonStyle::Bordered,
            enabled: true,
            busy: false,
            on_press: Some(Msg::Behind),
        });
        let dialog = state.open.then(|| Dialog {
            kind: DialogKind::Alert,
            icon: None,
            title: "Delete it?".into(),
            message: "It cannot be undone.".into(),
            body: None,
            buttons: vec![
                DialogButton {
                    label: "Cancel".into(),
                    action: DialogAction::Cancel,
                    on_press: Msg::Cancel,
                },
                DialogButton {
                    label: "Delete".into(),
                    action: DialogAction::Default,
                    on_press: Msg::Delete,
                },
            ],
        });
        Element::from(Modal { content, dialog })
    }
}

#[test]
fn return_answers_with_the_default_button_and_escape_with_the_cancel_button() {
    let rows = [(RETURN, Msg::Delete), (ESCAPE, Msg::Cancel)];
    for (key, answer) in rows {
        let mut view = View::<Confirm>::open();
        view.key(key, None, Mods::NONE);
        assert_eq!(view.state().sent, [answer]);
        assert!(!view.state().open);
    }
    let mut view = View::<Confirm>::open();
    view.key(Keysym('x' as u32), Some('x'), Mods::NONE);
    assert!(
        view.state().sent.is_empty() && view.state().open,
        "any other key stops at the sheet"
    );
}

#[test]
fn the_content_behind_takes_no_press_or_focus_until_the_sheet_is_answered() {
    let mut view = View::<Confirm>::open();
    let behind = view.node(Role::Button, "Behind");
    assert!(
        behind.is_none(),
        "hidden from assistive technology while the sheet is shown"
    );
    for _ in 0..3 {
        view.key(TAB, None, Mods::NONE);
    }
    view.click(cupertino_geometry::Point::new(10.0, 10.0), Mods::NONE);
    assert!(view.state().sent.is_empty());

    let cancel = view.center_of(Role::Button, "Cancel");
    view.click(cancel, Mods::NONE);
    assert_eq!(view.state().sent, [Msg::Cancel]);

    let bounds = view.bounds_of(Role::Button, "Behind");
    assert!(
        bounds.contains(cupertino_geometry::Point::new(10.0, 10.0)),
        "the press above was on it"
    );
    let behind = bounds.center();
    view.click(behind, Mods::NONE);
    assert_eq!(view.state().sent, [Msg::Cancel, Msg::Behind]);
}
