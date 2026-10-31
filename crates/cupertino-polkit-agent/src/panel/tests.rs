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
fn the_parts_stack_top_to_bottom_and_the_buttons_end_at_the_trailing_padding() {
    let f = Frames::new(&content());
    assert_eq!(f.symbol, Rect::new(160.0, 24.0, 40.0, 40.0));
    assert_eq!(f.title, Rect::new(24.0, 72.0, 312.0, 16.0));
    assert_eq!(
        f.message,
        [
            Rect::new(24.0, 92.0, 312.0, 16.0),
            Rect::new(24.0, 108.0, 312.0, 16.0)
        ]
    );
    assert_eq!(f.user, None);
    assert_eq!(f.field, Rect::new(24.0, 140.0, 312.0, 22.0));
    assert_eq!(f.details, Rect::new(24.0, 170.0, 312.0, 14.0));
    assert_eq!(f.authenticate, Rect::new(232.0, 204.0, 104.0, 20.0));
    assert_eq!(f.cancel, Rect::new(144.0, 204.0, 80.0, 20.0));
    assert_eq!(f.height, 248.0);
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
    assert_eq!(
        p.key(PanelKey::Space),
        Some(Command::Field(Key::Char(' ')))
    );
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
