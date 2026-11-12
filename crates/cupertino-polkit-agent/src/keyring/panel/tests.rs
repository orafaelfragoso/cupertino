use super::*;
use crate::keyring::model::tests::asking;
use crate::keyring::model::{Key, Value};

fn content() -> Content {
    Content {
        heading_lines: 1,
        description_lines: 2,
        password: true,
        cancel_title: 40.0,
        continue_title: 40.0,
        ..Content::default()
    }
}

/// The alert's measures: 260 points wide, the 64-point lock 22 points down and 22 in, the controls 16 in from each
/// side, and Cancel and Continue as 28-point capsules 8 apart in equal halves, 16 above the bottom edge.
#[test]
fn a_password_prompt_is_the_alert_with_its_lock_text_field_and_buttons() {
    let f = Frames::new(&content());
    assert_eq!(f.width, 260.0);
    assert_eq!(f.symbol, Rect::new(22.0, 22.0, 64.0, 64.0));
    assert_eq!(f.heading, vec![Rect::new(22.0, 100.0, 216.0, 16.0)]);
    assert_eq!(f.description.len(), 2);
    assert_eq!(f.description[0].y(), 122.0);
    let field = f.password.unwrap();
    assert_eq!(
        (field.x(), field.width(), field.height()),
        (16.0, 228.0, 22.0)
    );
    assert_eq!(field.y(), 122.0 + 32.0 + 16.0);
    assert!(f.confirm.is_none() && f.choice.is_none() && f.status.is_empty());
    assert_eq!(f.cancel, Rect::new(16.0, field.max_y() + 16.0, 110.0, 28.0));
    assert_eq!(
        f.continue_,
        Rect::new(134.0, field.max_y() + 16.0, 110.0, 28.0)
    );
    assert_eq!(f.height, f.cancel.max_y() + 16.0);
}

#[test]
fn a_new_password_adds_its_confirmation_the_choice_and_the_warning_below() {
    let f = Frames::new(&Content {
        confirm: true,
        choice: true,
        status_lines: 2,
        ..content()
    });
    let (password, confirm, choice) = (f.password.unwrap(), f.confirm.unwrap(), f.choice.unwrap());
    assert_eq!(confirm.y(), password.max_y() + 8.0);
    assert_eq!(choice.y(), confirm.max_y() + 10.0);
    assert_eq!(choice.height(), CHOICE_HEIGHT);
    assert_eq!(f.status.len(), 2);
    assert_eq!(f.status[0].y(), choice.max_y() + 6.0);
    assert_eq!(f.cancel.y(), f.status[1].max_y() + 16.0);
    assert_eq!(
        f.control_at(Point::new(30.0, confirm.center().y)),
        Some(Control::Confirm)
    );
    assert_eq!(
        f.control_at(Point::new(30.0, choice.center().y)),
        Some(Control::Choice)
    );
    assert_eq!(f.control_at(f.continue_.center()), Some(Control::Continue));
    assert_eq!(f.control_at(Point::new(30.0, 30.0)), None);
}

#[test]
fn a_long_choice_wraps_to_two_lines_at_most() {
    let one = Frames::new(&Content {
        choice: true,
        choice_lines: 1,
        ..content()
    });
    let two = Frames::new(&Content {
        choice: true,
        choice_lines: 5,
        ..content()
    });
    assert_eq!(one.choice.unwrap().height(), CHOICE_HEIGHT);
    assert_eq!(two.choice.unwrap().height(), 2.0 * crate::panel::BODY_LINE);
    assert_eq!(
        two.height - one.height,
        2.0 * crate::panel::BODY_LINE - CHOICE_HEIGHT
    );
}

#[test]
fn a_confirmation_has_no_field_and_long_titles_stack_the_buttons() {
    let f = Frames::new(&Content {
        password: false,
        continue_title: 150.0,
        ..content()
    });
    assert!(f.password.is_none());
    assert_eq!(f.continue_.width(), 228.0);
    assert_eq!(
        f.cancel.y(),
        f.continue_.max_y() + 8.0,
        "the continue button on top"
    );
    assert_eq!(f.continue_.y(), f.description[1].max_y() + 16.0);
}

#[test]
fn the_text_wraps_to_its_limits() {
    let f = Frames::new(&Content {
        heading_lines: 9,
        description_lines: 20,
        ..content()
    });
    assert_eq!(f.heading.len(), MAX_HEADING_LINES);
    assert_eq!(f.description.len(), MAX_DESCRIPTION_LINES);
    assert_eq!(
        Frames::new(&Content {
            heading_lines: 0,
            ..content()
        })
        .heading
        .len(),
        1
    );
}

#[test]
fn tab_walks_the_controls_return_continues_escape_cancels_and_space_acts_on_the_focus() {
    let (prompter, _) = asking(
        Kind::Password,
        vec![("choice-label", Value::Str("Remember".into()))],
    );
    let mut panel = Panel::new(prompter.prompt().unwrap());
    assert_eq!(panel.focus, Control::Password);
    assert_eq!(
        panel.key(PanelKey::Char('a')),
        Some(Command::Type(Field::Password, Key::Char('a')))
    );
    assert_eq!(
        panel.key(PanelKey::Space),
        Some(Command::Type(Field::Password, Key::Char(' ')))
    );
    assert_eq!(
        panel.key(PanelKey::Backspace),
        Some(Command::Type(Field::Password, Key::Backspace))
    );
    assert_eq!(
        panel.key(PanelKey::Tab { back: false }),
        Some(Command::Focused)
    );
    assert_eq!(panel.focus, Control::Choice);
    assert_eq!(panel.key(PanelKey::Space), Some(Command::ToggleChoice));
    assert_eq!(
        panel.key(PanelKey::Char('a')),
        None,
        "a key on the choice goes nowhere"
    );
    panel.key(PanelKey::Tab { back: false });
    assert_eq!(panel.key(PanelKey::Space), Some(Command::Cancel));
    panel.key(PanelKey::Tab { back: false });
    assert_eq!(panel.key(PanelKey::Space), Some(Command::Continue));
    panel.key(PanelKey::Tab { back: false });
    assert_eq!(panel.focus, Control::Password, "around");
    panel.key(PanelKey::Tab { back: true });
    assert_eq!(panel.focus, Control::Continue);
    assert_eq!(panel.key(PanelKey::Return), Some(Command::Continue));
    assert_eq!(panel.key(PanelKey::Escape), Some(Command::Cancel));
    assert_eq!(panel.click(Control::Choice), Command::ToggleChoice);
    assert_eq!(panel.focus, Control::Choice);
    assert_eq!(panel.click(Control::Password), Command::Focused);
    assert_eq!(panel.click(Control::Confirm), Command::Focused);
    assert_eq!(
        panel.focus,
        Control::Password,
        "a control the panel does not show takes no focus"
    );
}

#[test]
fn a_confirmation_starts_on_its_continue_button() {
    let (prompter, _) = asking(Kind::Confirm, vec![]);
    let panel = Panel::new(prompter.prompt().unwrap());
    assert_eq!(panel.focus, Control::Continue);
    assert_eq!(panel.order(), &[Control::Cancel, Control::Continue]);
    assert_eq!(
        Status::of(prompter.prompt().unwrap(), true),
        vec![],
        "no Caps Lock line without a field"
    );
}
