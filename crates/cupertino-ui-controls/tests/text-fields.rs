//! Text fields as a person edits them: typing, deleting and moving the caret, selecting by keys and pointer,
//! Return, what a model that lags or sets the text does, a disabled field, and a password field that keeps its
//! text from everything but its handlers.

#[allow(dead_code)]
mod support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{
    Button, ButtonRole, ButtonStyle, SecureField, Stack, TextChange, TextField,
};
use cupertino_ui_core::layout::{Align, Axis};
use cupertino_ui_core::platform::{Keysym, Mods};
use cupertino_ui_core::{AccessibleValue, Element, Role};
use cupertino_ui_text::ContentPurpose;
use support::{Scene, View, BACKSPACE, END, HOME, LEFT, RETURN, RIGHT};

const DELETE: Keysym = Keysym(0xffff);

#[derive(Clone, Debug, PartialEq)]
enum Msg {
    Change(TextChange),
    Submit,
    Secret(String),
    SecretSubmit(String),
    Reset,
}

struct State {
    value: String,
    revision: u64,
    changes: Vec<TextChange>,
    submits: usize,
    secrets: Vec<String>,
    submitted_secret: Option<String>,
}

/// A name field, a disabled one, a password field limited to four bytes and a button that sets the name; a model
/// that keeps up with the field, or with `LAGS`, one that never takes its edits.
struct Form<const LAGS: bool>;

/// The name of a file: its stem is selected each time the field takes the text.
struct Stem;

fn state(value: &str) -> State {
    State {
        value: value.into(),
        revision: 0,
        changes: Vec::new(),
        submits: 0,
        secrets: Vec::new(),
        submitted_secret: None,
    }
}

fn update<const LAGS: bool>(state: &mut State, message: Msg) {
    match message {
        Msg::Change(change) => {
            state.changes.push(change.clone());
            if !LAGS {
                state.value = change.text;
                state.revision = change.revision;
            }
        }
        Msg::Submit => state.submits += 1,
        Msg::Secret(text) => state.secrets.push(text),
        Msg::SecretSubmit(text) => state.submitted_secret = Some(text),
        Msg::Reset => {
            state.value = "reset".into();
            state.revision += 100;
        }
    }
}

fn field(
    state: &State,
    label: &str,
    enabled: bool,
    selection: Option<std::ops::Range<usize>>,
) -> Element<Msg> {
    Element::new(TextField {
        label: label.into(),
        placeholder: String::new(),
        value: state.value.clone(),
        revision: state.revision,
        purpose: ContentPurpose::default(),
        enabled,
        on_change: Some(Box::new(Msg::Change)),
        on_submit: Some(Msg::Submit),
        selection,
        initial: false,
    })
}

fn column(children: Vec<Element<Msg>>) -> Element<Msg> {
    Element::new(Stack {
        axis: Axis::Vertical,
        padding: Insets::uniform(20.0),
        spacing: 14.0,
        align: Align::Start,
        children,
    })
}

impl<const LAGS: bool> Scene for Form<LAGS> {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        state("")
    }

    fn update(state: &mut State, message: Msg) {
        update::<LAGS>(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        let secret = |submit: bool| -> cupertino_ui_controls::SecureHandler<Msg> {
            Box::new(move |buffer| {
                let text = buffer.with_secret(str::to_string);
                if submit {
                    Msg::SecretSubmit(text)
                } else {
                    Msg::Secret(text)
                }
            })
        };
        column(vec![
            field(state, "Name", true, None),
            field(state, "Locked", false, None),
            Element::new(SecureField {
                label: "Password".into(),
                on_change: Some(secret(false)),
                on_submit: Some(secret(true)),
                max_len: Some(4),
                ..SecureField::default()
            }),
            Element::new(Button {
                label: "Reset".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(Msg::Reset),
            }),
        ])
    }
}

impl Scene for Stem {
    type State = State;
    type Msg = Msg;

    fn state() -> State {
        state("report.pdf")
    }

    fn update(state: &mut State, message: Msg) {
        update::<false>(state, message);
    }

    fn view(state: &State) -> Element<Msg> {
        column(vec![field(state, "Name", true, Some(0..6))])
    }
}

/// What the field named `name` holds, its caret and its selection, as assistive technology reads them.
fn read<S: Scene>(view: &View<S>, name: &str) -> (String, usize, std::ops::Range<usize>) {
    match view.node(Role::TextInput, name).and_then(|n| n.value) {
        Some(AccessibleValue::Text {
            value,
            editing: Some(editing),
        }) => (value, editing.caret, editing.selection),
        other => panic!("{name} holds {other:?}"),
    }
}

enum Step {
    Type(&'static str),
    Key(Keysym, Mods),
    Chord(char, Mods),
}

fn run<S: Scene>(view: &mut View<S>, step: &Step) {
    match step {
        Step::Type(text) => view.type_text(text),
        Step::Key(key, mods) => view.key(*key, None, *mods),
        Step::Chord(c, mods) => view.key(Keysym(*c as u32), Some(*c), *mods),
    }
}

#[test]
fn typing_deleting_and_moving_the_caret_edit_the_text_where_the_caret_is() {
    use Step::{Chord, Key, Type};
    let none = Mods::NONE;
    // each step with the text, the caret and the selection it leaves
    let steps = [
        (Type("hello"), "hello", 5, 5..5),
        (Key(BACKSPACE, none), "hell", 4, 4..4),
        (Key(LEFT, none), "hell", 3, 3..3),
        (Type("X"), "helXl", 4, 4..4),
        (Key(HOME, none), "helXl", 0, 0..0),
        (Key(DELETE, none), "elXl", 0, 0..0),
        (Key(END, none), "elXl", 4, 4..4),
        (Key(LEFT, Mods::SHIFT), "elXl", 3, 3..4),
        (Key(LEFT, Mods::SHIFT), "elXl", 2, 2..4),
        (Type("-"), "el-", 3, 3..3),
        (Key(HOME, none), "el-", 0, 0..0),
        (Key(RIGHT, Mods::SHIFT), "el-", 1, 0..1),
        (Key(BACKSPACE, none), "l-", 0, 0..0),
        (Chord('a', Mods::PRIMARY), "l-", 2, 0..2),
        (Type("all"), "all", 3, 3..3),
        (Chord('z', Mods::PRIMARY), "l-", 2, 0..2),
    ];
    let mut view = View::<Form<false>>::open();
    view.focus(Role::TextInput, "Name");
    for (i, (step, text, caret, selection)) in steps.iter().enumerate() {
        run(&mut view, step);
        let held = read(&view, "Name");
        assert_eq!(
            held,
            (text.to_string(), *caret, selection.clone()),
            "step {i}"
        );
    }

    let revisions: Vec<u64> = view.state().changes.iter().map(|c| c.revision).collect();
    assert!(
        revisions.windows(2).all(|pair| pair[0] < pair[1]),
        "{revisions:?}"
    );
    assert_eq!(
        view.state().changes.last().unwrap().text,
        "l-",
        "an undo is an edit like another"
    );
}

#[test]
fn a_press_puts_the_caret_where_it_lands_and_a_double_press_selects_the_word() {
    let mut view = View::<Form<false>>::open();
    view.focus(Role::TextInput, "Name");
    view.type_text("one two");
    let bounds = view.bounds_of(Role::TextInput, "Name");
    let left = Point::new(bounds.min_x() + 2.0, bounds.center().y);
    view.click(left, Mods::NONE);
    assert_eq!(read(&view, "Name").1, 0);

    let right = Point::new(bounds.max_x() - 2.0, bounds.center().y);
    view.click(right, Mods::NONE);
    assert_eq!(read(&view, "Name").1, 7);

    view.wait(1.0);
    let inside_first = Point::new(bounds.min_x() + 14.0, bounds.center().y);
    view.click(inside_first, Mods::NONE);
    view.click(inside_first, Mods::NONE);
    assert_eq!(read(&view, "Name").2, 0..3, "the word under the pointer");
}

#[test]
fn the_selection_a_field_is_given_is_what_typing_replaces() {
    let mut view = View::<Stem>::open();
    view.focus(Role::TextInput, "Name");
    assert_eq!(read(&view, "Name").2, 0..6);
    view.type_text("summary");
    assert_eq!(read(&view, "Name").0, "summary.pdf");
}

#[test]
fn return_submits_the_field() {
    let mut view = View::<Form<false>>::open();
    view.focus(Role::TextInput, "Name");
    view.type_text("a");
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.state().submits, 1);
}

#[test]
fn a_disabled_field_takes_no_keys_and_no_press_and_reads_as_disabled() {
    let mut view = View::<Form<false>>::open();
    let locked = view.node(Role::TextInput, "Locked").unwrap();
    assert!(locked.states.disabled);
    view.click(locked.bounds.center(), Mods::NONE);
    view.type_text("x");
    assert!(view.state().changes.is_empty());
    assert_eq!(read(&view, "Locked").0, "");
    assert!(!view.node(Role::TextInput, "Locked").unwrap().states.focused);
}

#[test]
fn a_field_keeps_what_it_typed_while_the_model_lags_and_takes_the_text_the_model_sets_after_it() {
    let mut view = View::<Form<true>>::open();
    view.focus(Role::TextInput, "Name");
    view.type_text("ab");
    let texts: Vec<&str> = view
        .state()
        .changes
        .iter()
        .map(|c| c.text.as_str())
        .collect();
    assert_eq!(texts, ["a", "ab"], "the model never took the first edit");
    assert_eq!(read(&view, "Name").0, "ab");

    let reset = view.center_of(Role::Button, "Reset");
    view.click(reset, Mods::NONE);
    assert_eq!(read(&view, "Name").0, "reset");

    view.focus(Role::TextInput, "Name");
    view.type_text("!");
    assert_eq!(view.state().changes.last().unwrap().text, "reset!");
}

#[test]
fn a_password_reaches_the_handlers_only_is_read_as_protected_and_stops_at_its_limit() {
    let mut view = View::<Form<false>>::open();
    view.focus(Role::PasswordInput, "Password");
    view.type_text("hunter2");
    assert_eq!(
        view.state().secrets,
        ["h", "hu", "hun", "hunt"],
        "no edit past the limit"
    );
    let node = view.node(Role::PasswordInput, "Password").unwrap();
    assert_eq!(node.value, Some(AccessibleValue::Protected));
    view.key(RETURN, None, Mods::NONE);
    assert_eq!(view.state().submitted_secret.as_deref(), Some("hunt"));
}
