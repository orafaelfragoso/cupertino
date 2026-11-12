use super::*;

pub(crate) fn caller(n: u32) -> Caller {
    Caller {
        name: format!(":1.{n}"),
        path: "/org/test/prompt".into(),
    }
}

fn s(v: &str) -> Value {
    Value::Str(v.into())
}

/// A prompter whose caller 1 asks `kind` with `properties`, and that caller's end of the exchange.
pub(crate) fn asking(kind: Kind, properties: Vec<(&str, Value)>) -> (Prompter, Exchange) {
    let mut prompter = Prompter::new();
    let mut theirs = Exchange::new().unwrap();
    let effects = prompter.begin(caller(1)).unwrap();
    let [Effect::Ready {
        exchange,
        reply: Reply::Ready,
        ..
    }] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    theirs.receive(exchange).unwrap();
    let properties = properties.into_iter().map(|(k, v)| (k.to_owned(), v));
    assert_eq!(
        prompter.perform(&caller(1), kind, properties, &theirs.begin()),
        Ok(Vec::new())
    );
    (prompter, theirs)
}

#[test]
fn mnemonic_marks_are_dropped_from_labels_and_an_empty_label_takes_the_default() {
    assert_eq!(label_or("_Unlock", CONTINUE), "Unlock");
    assert_eq!(label_or("Save __As", CONTINUE), "Save _As");
    assert_eq!(label_or("", CONTINUE), "Continue");
    assert_eq!(label_or("  ", CANCEL), "Cancel");
}

#[test]
fn properties_take_known_names_of_the_right_type_and_keep_the_rest() {
    let mut p = Properties::default();
    p.update([
        ("title".into(), s("Unlock Keyring")),
        ("description".into(), s("An application wants access")),
        ("password-new".into(), Value::Bool(true)),
        ("password-strength".into(), Value::Int(3)),
        ("choice-label".into(), s("Remember")),
        ("caller-window".into(), s("x11:12345")),
        ("continue-label".into(), s("_Unlock")),
        ("cancel-label".into(), s("_Deny")),
        // wrong type and unknown names change nothing
        ("message".into(), Value::Bool(true)),
        ("colour".into(), s("red")),
    ]);
    assert_eq!(
        p.heading(),
        "Unlock Keyring",
        "without a message the title heads the panel"
    );
    assert!(p.password_new);
    assert_eq!(p.password_strength, 3);
    assert_eq!(p.caller_window, "x11:12345");
    assert_eq!(
        (p.continue_title(), p.cancel_title()),
        ("Unlock".into(), "Deny".into())
    );
    p.update([("message".into(), s("Authentication required"))]);
    assert_eq!(p.heading(), "Authentication required");
    assert_eq!(
        p.title, "Unlock Keyring",
        "a later question keeps what is not changed"
    );
}

#[test]
fn a_caller_is_ready_at_once_and_its_first_question_shows_the_panel() {
    let mut prompter = Prompter::new();
    let effects = prompter.begin(caller(1)).unwrap();
    assert!(
        matches!(&effects[..], [Effect::Ready { reply: Reply::Ready, properties, .. }] if properties.is_empty())
    );
    assert!(prompter.prompt().is_some() && prompter.shown().is_none());
    let (prompter, _) = asking(Kind::Password, vec![("message", s("Unlock"))]);
    let prompt = prompter.shown().unwrap();
    assert_eq!(prompt.phase, Phase::Asking);
    assert_eq!(prompt.properties.message, "Unlock");
}

#[test]
fn the_fields_never_grow_and_take_no_control_characters() {
    let (mut prompter, _) = asking(Kind::Password, vec![("password-new", Value::Bool(true))]);
    let before = prompter.prompt().unwrap().password.as_ptr();
    for _ in 0..MAX_PASSWORD + 10 {
        prompter.key(Field::Password, Key::Char('a'));
    }
    prompter.key(Field::Password, Key::Char('\u{7}'));
    let prompt = prompter.prompt().unwrap();
    assert_eq!(prompt.field_len(Field::Password), MAX_PASSWORD);
    assert_eq!(prompt.password.as_ptr(), before, "the field never moved");
    prompter.key(Field::Password, Key::Backspace);
    prompter.key(Field::Confirm, Key::Char('b'));
    let prompt = prompter.prompt().unwrap();
    assert_eq!(
        (
            prompt.field_len(Field::Password),
            prompt.field_len(Field::Confirm)
        ),
        (MAX_PASSWORD - 1, 1)
    );
}

#[test]
fn without_a_new_password_the_confirmation_takes_no_keys() {
    let (mut prompter, _) = asking(Kind::Password, vec![]);
    prompter.key(Field::Confirm, Key::Char('b'));
    assert_eq!(prompter.prompt().unwrap().field_len(Field::Confirm), 0);
}

#[test]
fn an_answer_carries_the_secret_and_the_changed_properties_once() {
    let (mut prompter, mut theirs) = asking(
        Kind::Password,
        vec![
            ("choice-label", s("Remember")),
            ("choice-chosen", Value::Bool(true)),
        ],
    );
    for c in "hunter2".chars() {
        prompter.key(Field::Password, Key::Char(c));
    }
    prompter.toggle_choice();
    let effects = prompter.answer();
    let [Effect::Ready {
        caller: to,
        reply: Reply::Yes,
        properties,
        exchange,
    }] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    assert_eq!(*to, caller(1));
    assert_eq!(
        properties,
        &vec![
            ("choice-chosen", Value::Bool(false)),
            ("password-strength", Value::Int(1))
        ]
    );
    assert_eq!(
        theirs
            .receive(exchange)
            .unwrap()
            .as_deref()
            .map(Vec::as_slice),
        Some(&b"hunter2"[..])
    );
    assert!(
        !format!("{effects:?}").contains(exchange.as_str()),
        "Debug prints no exchange"
    );
    // answered: busy, and neither keys nor a second answer do anything
    assert_eq!(prompter.prompt().unwrap().phase, Phase::Checking);
    prompter.key(Field::Password, Key::Char('x'));
    assert_eq!(prompter.prompt().unwrap().field_len(Field::Password), 0);
    assert!(prompter.answer().is_empty() && prompter.cancel().is_empty());
}

#[test]
fn a_mismatched_new_password_is_kept_back_until_both_agree() {
    let (mut prompter, _) = asking(Kind::Password, vec![("password-new", Value::Bool(true))]);
    prompter.key(Field::Password, Key::Char('a'));
    prompter.key(Field::Confirm, Key::Char('b'));
    assert!(prompter.answer().is_empty());
    assert!(prompter.prompt().unwrap().mismatch);
    prompter.key(Field::Confirm, Key::Char('a'));
    assert!(
        !prompter.prompt().unwrap().mismatch,
        "typing clears the mismatch"
    );
    assert!(matches!(
        &prompter.answer()[..],
        [Effect::Ready {
            reply: Reply::Yes,
            ..
        }]
    ));
}

#[test]
fn an_empty_password_is_an_answer_of_strength_zero() {
    let (mut prompter, mut theirs) = asking(Kind::Password, vec![]);
    let effects = prompter.answer();
    let [Effect::Ready {
        reply: Reply::Yes,
        properties,
        exchange,
        ..
    }] = &effects[..]
    else {
        panic!("{effects:?}");
    };
    assert!(properties.contains(&("password-strength", Value::Int(0))));
    assert_eq!(
        theirs
            .receive(exchange)
            .unwrap()
            .as_deref()
            .map(Vec::as_slice),
        Some(&b""[..])
    );
}

#[test]
fn stopping_or_leaving_makes_the_next_caller_active_and_a_waiting_one_just_goes() {
    let mut prompter = Prompter::new();
    prompter.begin(caller(1)).unwrap();
    prompter.begin(caller(2)).unwrap();
    prompter.begin(caller(3)).unwrap();
    assert_eq!(prompter.waiting(), 2);
    assert!(prompter.stop(&caller(2)).is_empty());
    assert_eq!(prompter.waiting(), 1);
    let effects = prompter.vanished(":1.1");
    assert!(
        matches!(&effects[..], [Effect::Ready { caller: c, reply: Reply::Ready, .. }] if *c == caller(3))
    );
    assert!(prompter.stop(&caller(3)).is_empty());
    assert!(prompter.prompt().is_none());
}

#[test]
fn too_many_waiting_callers_are_refused() {
    let mut prompter = Prompter::new();
    for n in 0..=MAX_WAITING as u32 {
        prompter.begin(caller(n)).unwrap();
    }
    assert_eq!(prompter.begin(caller(99)), Err(PromptError::Busy));
}
