use super::*;

fn content() -> Content {
    Content {
        message_lines: 2,
        users: false,
        status_lines: 0,
        details: None,
        cancel_title: 40.0,
        authenticate_title: 80.0,
    }
}

#[test]
fn the_parts_stack_as_an_alert_with_the_buttons_side_by_side_in_equal_halves() {
    let f = Frames::new(&content());
    assert_eq!(f.width, 260.0);
    assert_eq!(RADIUS, 26.0);
    assert_eq!(
        f.symbol,
        Rect::new(22.0, 22.0, 64.0, 64.0),
        "at the text's edge"
    );
    assert_eq!(f.title, Rect::new(22.0, 100.0, 216.0, 16.0));
    assert_eq!(
        f.message,
        [
            Rect::new(22.0, 126.0, 216.0, 16.0),
            Rect::new(22.0, 142.0, 216.0, 16.0)
        ]
    );
    assert_eq!(f.user, None);
    assert_eq!(f.field, Rect::new(16.0, 174.0, 228.0, 22.0));
    assert_eq!(f.details, Rect::new(16.0, 204.0, 228.0, 14.0));
    assert_eq!(f.cancel, Rect::new(16.0, 234.0, 110.0, 28.0));
    assert_eq!(f.authenticate, Rect::new(134.0, 234.0, 110.0, 28.0));
    assert_eq!(
        f.authenticate.x() - f.cancel.max_x(),
        8.0,
        "8 pt between the buttons"
    );
    assert_eq!(
        f.width - f.authenticate.max_x(),
        16.0,
        "16 pt in from the edge"
    );
    assert_eq!(f.height - f.authenticate.max_y(), 16.0);
    assert_eq!(f.height, 278.0);
}

#[test]
fn buttons_whose_titles_do_not_fit_a_half_stack_full_width_with_authenticate_on_top() {
    assert!(side_by_side(&[86.0, 86.0]));
    assert!(!side_by_side(&[40.0, 87.0]));
    let side = Frames::new(&content());
    let f = Frames::new(&Content {
        authenticate_title: 120.0,
        ..content()
    });
    assert_eq!(f.authenticate, Rect::new(16.0, 234.0, 228.0, 28.0));
    assert_eq!(f.cancel, Rect::new(16.0, 270.0, 228.0, 28.0));
    assert_eq!(f.height - side.height, 28.0 + 8.0);
}

#[test]
fn the_message_stops_at_six_lines_and_what_is_shown_grows_the_panel() {
    let short = Frames::new(&content());
    let long = Frames::new(&Content {
        message_lines: 40,
        ..content()
    });
    assert_eq!(long.message.len(), MAX_MESSAGE_LINES);
    assert_eq!(long.height - short.height, 4.0 * BODY_LINE);

    let all = Frames::new(&Content {
        users: true,
        status_lines: 2,
        details: Some(2),
        ..content()
    });
    let user = all.user.expect("more than one user");
    assert_eq!(user.y(), short.field.y());
    assert_eq!(all.field.y(), user.max_y() + 8.0);
    assert_eq!(all.status[0].y(), all.field.max_y() + 6.0);
    assert_eq!(all.details.y(), all.status[1].max_y() + 8.0);
    assert_eq!(all.detail_rows[0].y(), all.details.max_y());
    assert_eq!(
        all.height - short.height,
        CONTROL_HEIGHT + 8.0 + 6.0 + 2.0 * CAPTION_LINE + 2.0 * CAPTION_LINE
    );
}

#[test]
fn a_click_finds_the_control_under_it() {
    let f = Frames::new(&Content {
        users: true,
        ..content()
    });
    let at = |r: Rect| f.control_at(r.center());
    assert_eq!(at(f.user.unwrap()), Some(Control::User));
    assert_eq!(at(f.field), Some(Control::Field));
    assert_eq!(at(f.details), Some(Control::Details));
    assert_eq!(at(f.cancel), Some(Control::Cancel));
    assert_eq!(at(f.authenticate), Some(Control::Authenticate));
    assert_eq!(at(f.title), None);
}

#[test]
fn tab_cycles_inside_the_panel_and_the_user_row_only_when_there_is_one() {
    let mut p = Panel::new(false);
    let mut seen = vec![p.focus];
    for _ in 0..4 {
        assert_eq!(p.key(PanelKey::Tab { back: false }), Some(Command::Focused));
        seen.push(p.focus);
    }
    use Control::*;
    assert_eq!(seen, [Field, Details, Cancel, Authenticate, Field]);
    p.key(PanelKey::Tab { back: true });
    assert_eq!(p.focus, Authenticate);

    let mut p = Panel::new(true);
    p.key(PanelKey::Tab { back: true });
    assert_eq!(p.focus, User);
}

#[test]
fn return_and_escape_answer_or_cancel_wherever_the_focus_is() {
    for focus in [Control::User, Control::Field, Control::Cancel] {
        let mut p = Panel::new(true);
        p.focus = focus;
        assert_eq!(
            p.key(PanelKey::Typed(Key::Return)),
            Some(Command::Authenticate)
        );
        assert_eq!(p.key(PanelKey::Typed(Key::Escape)), Some(Command::Cancel));
    }
}

#[test]
fn keys_go_to_the_field_only_while_it_has_the_focus() {
    let mut p = Panel::new(true);
    assert_eq!(
        p.key(PanelKey::Typed(Key::Char('a'))),
        Some(Command::Field(Key::Char('a')))
    );
    assert_eq!(p.key(PanelKey::Space), Some(Command::Field(Key::Char(' '))));
    p.focus = Control::Cancel;
    assert_eq!(p.key(PanelKey::Typed(Key::Char('a'))), None);
    assert_eq!(p.key(PanelKey::Typed(Key::Backspace)), None);
    assert_eq!(p.key(PanelKey::Space), Some(Command::Cancel));
    p.focus = Control::Authenticate;
    assert_eq!(p.key(PanelKey::Space), Some(Command::Authenticate));
    p.focus = Control::User;
    assert_eq!(p.key(PanelKey::Up), Some(Command::User { back: true }));
    assert_eq!(p.key(PanelKey::Down), Some(Command::User { back: false }));
}

#[test]
fn the_details_open_and_close_by_key_and_by_click() {
    let mut p = Panel::new(false);
    p.focus = Control::Details;
    assert_eq!(p.key(PanelKey::Space), Some(Command::ToggleDetails));
    assert!(p.details);
    assert_eq!(p.click(Control::Details), Command::ToggleDetails);
    assert!(!p.details);
    assert_eq!(p.click(Control::Field), Command::Focused);
    assert_eq!(p.focus, Control::Field);
}

fn asking(echo: bool) -> crate::model::Agent {
    use crate::agent::{Request, User};
    use crate::helper::Line;
    let mut agent = crate::model::Agent::new(User(1000));
    agent.begin(Request {
        cookie: "c".into(),
        action: "org.example.mount".into(),
        message: "Authentication is required to mount the disk".into(),
        details: Vec::new(),
        users: vec![User(1000)],
    });
    agent.helper(Line::Info("Touch the reader".into()));
    agent.helper(Line::Prompt {
        text: "Password: ".into(),
        echo,
    });
    agent
}

#[test]
fn the_status_lines_are_the_error_then_the_information_then_caps_lock_for_a_secret() {
    let mut agent = asking(false);
    for c in "wrong".chars() {
        agent.key(Key::Char(c));
    }
    agent.key(Key::Return);
    agent.helper_lost();
    agent.helper(crate::helper::Line::Info("Touch the reader".into()));
    let prompt = agent.prompt().unwrap();
    assert_eq!(
        Status::of(prompt, true),
        [
            Status::Error(crate::model::TRY_AGAIN.into()),
            Status::Info("Touch the reader".into()),
            Status::CapsLock
        ]
    );
    assert_eq!(Status::of(prompt, true)[2].text(), "Caps Lock is on");
    let clear = asking(true);
    assert_eq!(
        Status::of(clear.prompt().unwrap(), true),
        [Status::Info("Touch the reader".into())],
        "Caps Lock is told only for a secret"
    );
}

#[test]
fn the_field_is_labelled_password_for_the_password_question_and_by_any_other_question() {
    assert_eq!(field_label("Password: "), "Password");
    assert_eq!(field_label("password:"), "Password");
    assert_eq!(field_label(""), "Password");
    assert_eq!(field_label("Verification code: "), "Verification code");
}
