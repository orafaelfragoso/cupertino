mod support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{SecureField, Stack, TextChange, TextField};
use cupertino_ui_core::{
    AccessibleValue, Application, Command, Element, Role, TextSemantics, Window,
};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_platform::{ClipboardKind, Mods, PointerEvent, Timestamp, WindowEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{ContentPurpose, ImeBatch, ImeEvent, Preedit, CARET_WIDTH};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, label_width, point, window, Harness, ESCAPE, RETURN, TAB};

const LEFT: (u32, u32) = (105, 0xff51);
const BACKSPACE: (u32, u32) = (14, 0xff08);
const A: u32 = 30;
const C: u32 = 46;
const X: u32 = 45;

struct Form<const ENABLED: bool>;
type App = Form<true>;

#[derive(Default)]
struct Model {
    text: String,
    revision: u64,
    changes: usize,
    submitted: Vec<String>,
    password: Option<usize>,
    typed: Vec<usize>,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Changed(TextChange),
    Submit,
    Login(usize),
    Typed(usize),
}

impl<const ENABLED: bool> Application for Form<ENABLED> {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (Model::default(), Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Changed(change) => {
                model.text = change.text;
                model.revision = change.revision;
                model.changes += 1;
            }
            Message::Submit => {
                model.submitted.push(std::mem::take(&mut model.text));
                model.revision += 1;
            }
            Message::Login(len) => model.password = Some(len),
            Message::Typed(len) => model.typed.push(len),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 4.0,
            align: Align::Start,
            children: vec![
                Element::new(TextField {
                    label: "Name".into(),
                    placeholder: "Your name".into(),
                    value: model.text.clone(),
                    revision: model.revision,
                    purpose: ContentPurpose::Name,
                    enabled: ENABLED,
                    on_change: Some(Box::new(Message::Changed)),
                    on_submit: Some(Message::Submit),
                    selection: None,
                    initial: false,
                })
                .key("name"),
                Element::new(SecureField {
                    label: "Password".into(),
                    placeholder: "Required".into(),
                    enabled: ENABLED,
                    on_change: Some(Box::new(|secret| {
                        Message::Typed(secret.with_secret(|s| s.chars().count()))
                    })),
                    on_submit: Some(Box::new(|secret| {
                        Message::Login(secret.with_secret(|s| s.chars().count()))
                    })),
                })
                .key("password"),
            ],
        }))
    }
}

fn editing(h: &Harness<App>) -> (String, TextSemantics) {
    match h.accessible("name").value {
        Some(AccessibleValue::Text {
            value,
            editing: Some(editing),
        }) => (value, editing),
        other => panic!("not editable text: {other:?}"),
    }
}

fn batch(preedit: Option<&str>, commit: Option<&str>, current: bool) -> WindowEvent {
    WindowEvent::Ime(ImeEvent::Done(ImeBatch {
        preedit: preedit.map(|text| Preedit {
            text: text.into(),
            cursor: None,
        }),
        commit: commit.map(Into::into),
        delete: (0, 0),
        current,
    }))
}

/// The plain rectangles a field fills inside its chrome: caret, selection and composition underline.
fn rects(h: &Harness<App>, key: &'static str) -> Vec<(Rect, Color)> {
    h.ops(key)
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::Rect(rect),
                paint: Paint::Solid(c),
            } => Some((rect, c)),
            _ => None,
        })
        .collect()
}

fn caret(h: &Harness<App>) -> Option<Rect> {
    rects(h, "name")
        .into_iter()
        .find(|(r, c)| r.width() == CARET_WIDTH && *c == normal(ColorRole::Text))
        .map(|(r, _)| r)
}

fn glyph_colors(h: &Harness<App>, key: &'static str) -> Vec<Color> {
    h.ops(key)
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { color, .. } => Some(color),
            _ => None,
        })
        .collect()
}

fn normal(role: ColorRole) -> Color {
    color(role, InteractionVariant::Normal, false)
}

fn focused_name() -> Harness<App> {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    assert!(h.focused("name"));
    h
}

#[test]
fn typing_reports_each_edit_with_a_newer_revision_and_the_model_keeps_it() {
    let mut h = focused_name();
    h.type_text("Ada");
    assert_eq!(h.model().text, "Ada");
    assert_eq!((h.model().changes, h.model().revision), (3, 3));

    let (value, editing) = editing(&h);
    assert_eq!(value, "Ada");
    assert_eq!((editing.caret, editing.selection), (3, 3..3));
    let node = h.accessible("name");
    assert_eq!(node.role, Role::TextInput);
    assert_eq!(
        node.name.as_deref(),
        Some("Name"),
        "named by the label, not the placeholder"
    );
}

#[test]
fn deactivating_the_window_while_a_chord_is_held_keeps_the_selection() {
    let mut h = focused_name();
    h.type_text("Ada");
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    h.send(WindowEvent::Focus(false));
    assert_eq!(editing(&h).1.selection, 0..3);
}

#[test]
fn the_editing_chords_move_select_and_delete() {
    let mut h = focused_name();
    h.type_text("Ada");
    h.press_key(LEFT.0, LEFT.1, true, false, Mods::NONE);
    assert_eq!(editing(&h).1.caret, 2);
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    assert_eq!(editing(&h).1.selection, 0..3);
    h.press_key(BACKSPACE.0, BACKSPACE.1, true, false, Mods::NONE);
    assert_eq!(h.model().text, "");
}

#[test]
fn copy_and_cut_put_the_selection_on_the_clipboard() {
    let mut h = focused_name();
    h.type_text("Ada");
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    h.press_key(C, 'c' as u32, true, false, Mods::PRIMARY);
    let clipboard = |h: &Harness<App>| {
        h.rt.window_system()
            .clipboard(ClipboardKind::Clipboard)
            .map(|d| String::from_utf8(d.bytes.clone()).unwrap())
    };
    assert_eq!(clipboard(&h).as_deref(), Some("Ada"));
    assert_eq!(h.model().text, "Ada");
    h.type_text("B");
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    h.press_key(X, 'x' as u32, true, false, Mods::PRIMARY);
    assert_eq!(clipboard(&h).as_deref(), Some("B"));
    assert_eq!(h.model().text, "");
}

#[test]
fn return_submits_and_text_the_application_sets_itself_replaces_the_field() {
    let mut h = focused_name();
    h.type_text("Ada");
    h.tap(RETURN);
    assert_eq!(h.model().submitted, ["Ada"]);
    assert_eq!(
        editing(&h).0,
        "",
        "the model cleared it with a newer revision"
    );
    h.type_text("B");
    assert_eq!(h.model().text, "B");
}

#[test]
fn the_input_method_composes_without_reporting_and_commits_once() {
    let mut h = focused_name();
    let state =
        h.rt.window_system()
            .text_input(h.window)
            .unwrap()
            .cloned()
            .unwrap();
    assert_eq!(state.purpose, ContentPurpose::Name);
    assert!(state.surrounding.is_some());

    h.send(batch(Some("ni"), None, true));
    assert_eq!(h.model().changes, 0, "a composition is not a change");
    assert_eq!(editing(&h).0, "", "nor part of the accessible value");
    assert!(
        !format!("{:?}", h.snapshot()).contains("ni\""),
        "the snapshot never prints composed text"
    );

    h.tap(RETURN);
    assert!(
        h.model().submitted.is_empty(),
        "Return belongs to the composition"
    );

    h.send(batch(None, Some("你"), true));
    assert_eq!(h.model().text, "你");
    assert_eq!(h.model().changes, 1);
}

#[test]
fn escape_and_a_stale_batch_cancel_the_composition() {
    let mut h = focused_name();
    h.type_text("a");
    h.send(batch(Some("ni"), None, true));
    h.tap(ESCAPE);
    h.send(batch(None, Some("x"), false));
    h.send(batch(Some("ha"), None, true));
    h.send(batch(None, None, false));
    assert_eq!(h.model().text, "a");
    assert_eq!(editing(&h).0, "a");
}

#[test]
fn a_password_reaches_only_its_handlers() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    h.tap(TAB);
    assert!(h.focused("password"));
    let state =
        h.rt.window_system()
            .text_input(h.window)
            .unwrap()
            .cloned()
            .unwrap();
    assert_eq!(state.surrounding, None);
    assert_eq!(state.purpose, ContentPurpose::Password);

    h.type_text("hunter2");
    assert_eq!(h.model().typed, [1, 2, 3, 4, 5, 6, 7]);
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    h.press_key(C, 'c' as u32, true, false, Mods::PRIMARY);
    h.press_key(X, 'x' as u32, true, false, Mods::PRIMARY);
    assert_eq!(
        h.rt.window_system().clipboard(ClipboardKind::Clipboard),
        None
    );

    let node = h.accessible("password");
    assert_eq!(node.role, Role::PasswordInput);
    assert_eq!(node.value, Some(AccessibleValue::Protected));
    assert!(!format!("{:?}", h.snapshot()).contains("hunter2"));

    h.send(batch(Some("pre"), None, true));
    h.send(batch(None, Some("!"), true));
    h.tap(RETURN);
    assert_eq!(
        h.model().password,
        Some(1),
        "the committed \"!\" replaced the selected \"hunter2\"; the preedit was never held"
    );
    assert_eq!(
        h.model().typed,
        [1, 2, 3, 4, 5, 6, 7, 1],
        "selecting, copying, cutting and composing change nothing"
    );
}

#[test]
fn disabled_fields_take_no_focus_or_text() {
    let mut h = Harness::<Form<false>>::new();
    h.tap(TAB);
    assert!(!h.focused("name") && !h.focused("password"));
    h.click(h.center("name"));
    h.type_text("x");
    assert_eq!(h.model().text, "");
    assert!(h.accessible("name").states.disabled);
    assert!(h.accessible("password").states.disabled);
}

#[test]
fn the_placeholder_shows_in_its_own_colour_until_there_is_text() {
    let mut h = focused_name();
    let placeholder = glyph_colors(&h, "name");
    assert!(!placeholder.is_empty());
    assert!(placeholder
        .iter()
        .all(|&c| c == normal(ColorRole::PlaceholderText)));
    h.type_text("A");
    let text = glyph_colors(&h, "name");
    assert!(!text.is_empty());
    assert!(text.iter().all(|&c| c == normal(ColorRole::Text)));
}

#[test]
fn the_caret_follows_typing_and_gives_way_to_the_selection() {
    let mut h = focused_name();
    let start = caret(&h).expect("a focused empty field shows its caret");
    h.type_text("Ada");
    let end = caret(&h).unwrap();
    assert!(end.min_x() > start.min_x());
    assert_eq!(
        h.rt.window_system()
            .text_input(h.window)
            .unwrap()
            .unwrap()
            .cursor_rect
            .map(|r| r.min_x()),
        Some(h.rect("name").min_x() + end.min_x()),
        "the input method learns where the caret is"
    );

    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    assert_eq!(caret(&h), None);
    let selected: Vec<_> = rects(&h, "name")
        .into_iter()
        .filter(|(_, c)| *c == normal(ColorRole::SelectedTextBackground))
        .collect();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].0.min_x(), start.min_x());
    assert_eq!(selected[0].0.max_x(), end.min_x());
}

struct Rename;

impl Application for Rename {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let model = Model {
            text: "notes.txt".into(),
            ..Model::default()
        };
        (model, Vec::new())
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        App::update(model, message)
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(
            Element::new(TextField {
                label: "Name".into(),
                placeholder: String::new(),
                value: model.text.clone(),
                revision: model.revision,
                purpose: ContentPurpose::Normal,
                enabled: true,
                on_change: Some(Box::new(Message::Changed)),
                on_submit: None,
                selection: Some(0..5),
                initial: false,
            })
            .key("name"),
        )
    }
}

#[test]
fn a_field_shows_its_initial_selection_and_the_next_key_replaces_only_it() {
    let mut h = Harness::<Rename>::new();
    h.tap(TAB);
    let selected = h
        .ops("name")
        .into_iter()
        .filter(|op| {
            matches!(op, PaintOp::Fill { paint: Paint::Solid(c), .. }
                if *c == normal(ColorRole::SelectedTextBackground))
        })
        .count();
    assert_eq!(selected, 1, "the stem is highlighted");

    h.type_text("x");
    assert_eq!(h.model().text, "x.txt");
}

#[test]
fn a_click_puts_the_caret_at_the_nearest_position() {
    let mut h = focused_name();
    h.type_text("hello");
    let field = h.rect("name");
    h.click(point(field.min_x() + 6.5, field.center().y));
    h.type_text("X");
    assert_eq!(h.model().text, "Xhello");
}

#[test]
fn long_text_scrolls_to_keep_the_caret_inside_the_field() {
    let mut h = focused_name();
    h.type_text(&"w".repeat(60));
    let width = h.rect("name").width();
    let end = caret(&h).unwrap();
    assert_eq!(
        end.max_x(),
        width - 6.0,
        "the whole caret shows, at the interior's end"
    );
    assert_eq!(
        h.rt.window_system()
            .text_input(h.window)
            .unwrap()
            .unwrap()
            .cursor_rect
            .map(|r| r.min_x()),
        Some(h.rect("name").min_x() + end.min_x()),
        "the input method sees the caret where the scrolled text puts it"
    );
    h.press_key(102, 0xff50, true, false, Mods::NONE);
    assert_eq!(
        caret(&h).unwrap().min_x(),
        6.0,
        "Home scrolls back to the start"
    );
}

#[test]
fn right_to_left_fields_keep_their_text_placeholder_and_caret_at_the_leading_edge() {
    let mut h = Harness::<App>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    let field = h.rect("name");
    let width = field.width();
    let glyphs_x = |h: &Harness<App>| {
        h.ops("name")
            .into_iter()
            .find_map(|op| match op {
                PaintOp::Glyphs { origin, .. } => Some(origin.x),
                _ => None,
            })
            .unwrap()
    };
    assert!(glyphs_x(&h) > width / 2.0, "the placeholder sits right");
    let empty = caret(&h).unwrap();
    assert_eq!(empty.max_x(), width - 6.0);

    h.type_text("Ada");
    assert!(glyphs_x(&h) > width / 2.0, "the text sits right");
    let end = caret(&h).unwrap();
    assert_eq!(end, empty, "the text grows leftward from the caret");
    assert_eq!(
        h.rt.window_system()
            .text_input(h.window)
            .unwrap()
            .unwrap()
            .cursor_rect
            .map(|r| r.min_x()),
        Some(field.min_x() + end.min_x())
    );

    h.click(point(field.min_x() + 6.5, field.center().y));
    h.type_text("X");
    assert_eq!(
        h.model().text,
        "XAda",
        "a click left of the text lands at its start"
    );
}

/// Types `text`, too long for the field, and moves focus away: the harness, where the glyphs start, and the field's
/// width.
fn blurred_overlong(direction: LayoutDirection, text: &str) -> (Harness<App>, f64, f64) {
    let mut h = Harness::<App>::with_direction(direction);
    h.tap(TAB);
    h.type_text(text);
    h.tap(TAB);
    assert!(!h.focused("name"));
    assert_eq!(
        editing(&h).1.caret,
        text.len(),
        "blur leaves the caret where it was"
    );
    let origin = h
        .ops("name")
        .into_iter()
        .find_map(|op| match op {
            PaintOp::Glyphs { origin, .. } => Some(origin.x),
            _ => None,
        })
        .unwrap();
    let width = h.rect("name").width();
    (h, origin, width)
}

#[test]
fn an_unfocused_overlong_field_shows_the_start_of_its_text() {
    let text = "the quick brown fox jumps over the lazy dog again and again";
    let (mut h, origin, width) = blurred_overlong(LayoutDirection::Ltr, text);
    assert!(label_width(text) > width);
    assert_eq!(origin, 6.0, "the first letter sits at the leading edge");

    h.tap(TAB);
    assert!(h.focused("name"));
    assert_eq!(
        caret(&h).unwrap().max_x(),
        width - 6.0,
        "a focused field follows its caret back to the end, all of it showing"
    );
}

#[test]
fn an_unfocused_overlong_right_to_left_field_shows_the_start_of_its_text() {
    let text = "שלום עולם שלום עולם שלום עולם שלום עולם שלום עולם";
    let (_, origin, width) = blurred_overlong(LayoutDirection::Rtl, text);
    let text_width = label_width(text);
    assert!(text_width > width);
    assert_eq!(
        origin + text_width,
        width - 6.0,
        "the first letter, at the text's right end, sits at the leading edge"
    );
}

#[test]
fn a_composition_is_underlined_one_pixel_below_the_baseline() {
    let mut h = focused_name();
    h.send(batch(Some("ka"), None, true));
    let underline = |h: &Harness<App>| {
        rects(h, "name")
            .into_iter()
            .find(|(r, _)| r.height() == 1.0)
    };
    let (rect, c) = underline(&h).expect("the composition is underlined");
    assert_eq!(c, normal(ColorRole::Text));
    assert!(rect.width() > 0.0);
    let baseline = h.rect("name").height() / 2.0 + 13.0 * 0.36;
    assert!((rect.min_y() - (baseline + 1.5)).abs() < 1e-9);

    h.send(batch(None, Some("か"), true));
    assert_eq!(underline(&h), None);
}

#[test]
fn a_password_paints_one_identical_bullet_per_grapheme() {
    let mut h = Harness::<App>::new();
    h.tap(TAB);
    h.tap(TAB);
    assert!(h.focused("password"));
    h.type_text("hunter2");
    let glyphs: Vec<_> = h
        .ops("password")
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { run, .. } => Some(run.glyphs),
            _ => None,
        })
        .flatten()
        .map(|g| g.id)
        .collect();
    assert_eq!(glyphs.len(), 7);
    assert!(glyphs.iter().all(|&id| id == glyphs[0]));
}

/// A form whose name field starts with `N` characters.
struct Long<const N: usize>;

impl<const N: usize> Application for Long<N> {
    type Model = String;
    type Message = Message;

    fn init() -> (String, Vec<Command<Message>>) {
        ("a".repeat(N), Vec::new())
    }

    fn update(text: &mut String, message: Message) -> Vec<Command<Message>> {
        if let Message::Changed(change) = message {
            *text = change.text;
        }
        Vec::new()
    }

    fn view(text: &String) -> Vec<Window<Message>> {
        window(Element::new(TextField {
            label: "Name".into(),
            placeholder: String::new(),
            value: text.clone(),
            revision: 0,
            purpose: ContentPurpose::Normal,
            enabled: true,
            on_change: Some(Box::new(Message::Changed)),
            on_submit: None,
            selection: None,
            initial: false,
        }))
    }
}

/// The best of several keystrokes into a focused field, each shaped and painted.
fn keystroke<const N: usize>() -> std::time::Duration {
    let mut h = Harness::<Long<N>>::new();
    h.tap(TAB);
    (0..9)
        .map(|_| {
            let start = std::time::Instant::now();
            h.type_text("x");
            start.elapsed()
        })
        .min()
        .unwrap()
}

#[test]
fn a_keystroke_costs_no_more_than_linear_in_the_text() {
    let (short, long) = (keystroke::<500>(), keystroke::<2000>());
    let ratio = long.as_secs_f64() / short.as_secs_f64().max(1e-9);
    assert!(
        ratio < 10.0,
        "a keystroke into 2000 characters took {long:?} against {short:?} for 500, {ratio:.1}x for 4x the text"
    );
}

const MS: u64 = 1_000_000;

/// Moves the clock to `ms` milliseconds and lets the runtime deliver what is due.
fn at(h: &mut Harness<App>, ms: u64) {
    h.clock().advance_to(Timestamp(ms * MS)).unwrap();
    h.settle();
}

fn next_wake(h: &Harness<App>) -> Option<u64> {
    h.snapshot().pending.next_wake.map(|t| t.0 / MS)
}

#[test]
fn the_caret_blinks_in_phases_from_its_last_move() {
    let mut h = focused_name();
    assert!(caret(&h).is_some());
    assert_eq!(next_wake(&h), Some(500));
    at(&mut h, 500);
    assert!(caret(&h).is_none(), "off for the second phase");
    at(&mut h, 1000);
    assert!(caret(&h).is_some());

    at(&mut h, 1200);
    h.type_text("A");
    at(&mut h, 1600);
    assert!(caret(&h).is_some(), "a key restarts the on phase");
    at(&mut h, 1700);
    assert!(caret(&h).is_none());
}

#[test]
fn an_idle_field_stops_blinking_with_the_caret_on_and_a_key_starts_it_again() {
    let mut h = focused_name();
    for ms in (500..=10_000).step_by(500) {
        at(&mut h, ms);
    }
    assert!(caret(&h).is_some());
    assert_eq!(next_wake(&h), None, "an idle focused window settles");
    h.type_text("A");
    assert_eq!(next_wake(&h), Some(10_500));
}

#[test]
fn no_wake_is_left_while_text_is_selected_or_the_window_is_inactive() {
    let mut h = focused_name();
    h.type_text("Ada");
    h.press_key(A, 'a' as u32, true, false, Mods::PRIMARY);
    at(&mut h, 500);
    assert_eq!(next_wake(&h), None, "a selection has no caret to blink");

    h.press_key(LEFT.0, LEFT.1, true, false, Mods::NONE);
    assert_eq!(next_wake(&h), Some(1000));
    h.send(WindowEvent::Focus(false));
    at(&mut h, 1000);
    assert!(caret(&h).is_none(), "an inactive window shows no caret");
    assert_eq!(next_wake(&h), None);

    h.send(WindowEvent::Focus(true));
    assert!(caret(&h).is_some(), "activation shows the caret again");
    assert_eq!(next_wake(&h), Some(1500));
}

#[test]
fn an_unfocused_field_asks_for_no_wake() {
    let h = Harness::<App>::new();
    assert_eq!(next_wake(&h), None);
}

/// The point inside the name field just after `prefix`, on the text's line.
fn after(h: &Harness<App>, prefix: &str) -> cupertino_geometry::Point {
    let field = h.rect("name");
    point(
        field.min_x() + 6.0 + label_width(prefix) - 1.0,
        field.center().y,
    )
}

#[test]
fn a_second_and_third_click_select_the_word_and_then_everything() {
    let mut h = focused_name();
    h.type_text("hello world");
    let world = after(&h, "hello wo");
    h.click(world);
    assert_eq!(editing(&h).1.selection, 8..8);
    h.click(world);
    assert_eq!(editing(&h).1.selection, 6..11, "the word");
    h.click(world);
    assert_eq!(editing(&h).1.selection, 0..11, "everything");
}

#[test]
fn clicks_further_apart_than_the_interval_only_move_the_caret() {
    let mut h = focused_name();
    h.type_text("hello world");
    let world = after(&h, "hello wo");
    h.click(world);
    h.clock().advance_to(Timestamp(600 * MS)).unwrap();
    h.click(world);
    assert_eq!(editing(&h).1.selection, 8..8);
}

#[test]
fn a_drag_selects_from_the_press_to_the_pointer_even_past_the_field() {
    let mut h = focused_name();
    h.type_text("hello world");
    let start = after(&h, "he");
    h.pointer(PointerEvent::Motion { position: start });
    h.button(start, true);
    let past = point(h.rect("name").max_x() + 40.0, start.y);
    h.pointer(PointerEvent::Motion { position: past });
    h.button(past, false);
    assert_eq!(editing(&h).1.selection, 2..11);
}

/// Where each glyph of `key` starts, in node coordinates, by the byte its cluster starts at.
fn glyph_starts(h: &Harness<App>, key: &'static str) -> Vec<(usize, f64)> {
    h.ops(key)
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Glyphs { origin, run, .. } => Some(
                run.glyphs
                    .into_iter()
                    .map(move |g| (g.cluster, origin.x + f64::from(g.x))),
            ),
            _ => None,
        })
        .flatten()
        .collect()
}

#[test]
fn a_right_to_left_field_lays_its_text_out_right_to_left() {
    let mut h = Harness::<App>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    h.type_text("abc!");
    let x = |byte| {
        glyph_starts(&h, "name")
            .into_iter()
            .find(|(cluster, _)| *cluster == byte)
            .unwrap()
            .1
    };
    assert!(
        x(3) < x(0),
        "the trailing mark takes the paragraph's direction and sits left of the letters"
    );
}

#[test]
fn a_click_at_a_direction_change_puts_the_caret_at_the_edge_it_hit() {
    let mut h = focused_name();
    h.type_text("abc אבג");
    let hebrew = glyph_starts(&h, "name")
        .into_iter()
        .filter(|(cluster, _)| *cluster >= 4)
        .map(|(_, x)| x)
        .fold(f64::INFINITY, f64::min);
    let field = h.rect("name");
    h.click(point(field.min_x() + hebrew - 1.0, field.center().y));
    let (at, _) = rects(&h, "name")
        .into_iter()
        .find(|(r, _)| r.width() == CARET_WIDTH)
        .unwrap();
    assert!(
        (at.min_x() - hebrew).abs() < 1.0,
        "the caret is drawn after the space at {hebrew}, not at the far end of the Hebrew: {at:?}"
    );
    h.type_text("X");
    assert_eq!(h.model().text, "abc Xאבג");
}
