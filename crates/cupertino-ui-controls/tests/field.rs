mod support;

use cupertino_geometry::Rect;
use cupertino_ui_controls::{SecureField, Stack, TextChange, TextField};
use cupertino_ui_core::{
    AccessibleValue, Application, Command, Element, Role, TextSemantics, Window,
};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_platform::{ClipboardKind, Mods, WindowEvent};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_text::{ContentPurpose, ImeBatch, ImeEvent, Preedit, CARET_WIDTH};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, point, window, Harness, ESCAPE, RETURN, TAB};

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
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Changed(TextChange),
    Submit,
    Login(usize),
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
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
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
                })
                .key("name"),
                Element::new(SecureField {
                    label: "Password".into(),
                    placeholder: "Required".into(),
                    enabled: ENABLED,
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
fn a_password_reaches_only_the_submit_handler() {
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
    assert!(end.max_x() <= width - 6.0 + CARET_WIDTH && end.min_x() > width / 2.0);
    h.press_key(102, 0xff50, true, false, Mods::NONE);
    assert_eq!(
        caret(&h).unwrap().min_x(),
        6.0,
        "Home scrolls back to the start"
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
