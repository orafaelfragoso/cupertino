mod support;

use cupertino_geometry::{Insets, Point, Size};
use cupertino_ui_controls::{ScrollView, Scroller, Stack, TextChange, TextEditor, TextField};
use cupertino_ui_core::{AccessibleValue, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, ScrollState};
use cupertino_ui_platform::Mods;
use cupertino_ui_render::PaintOp;
use cupertino_ui_text::{ContentPurpose, TextWork};
use support::{window, Harness, TAB};

const RETURN: (u32, u32) = (28, 0xff0d);
const UP: (u32, u32) = (103, 0xff52);
const DOWN: (u32, u32) = (108, 0xff54);
const TAB_KEY: (u32, u32) = (15, TAB);

/// A scrolled editor of `LINES` numbered lines, as a file with CRLF line ends holds them, over a text field.
struct Editor<const LINES: usize>;

struct Model {
    text: String,
    revision: u64,
    scroll: ScrollState,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Changed(TextChange),
    Scrolled(ScrollState),
    Other(TextChange),
}

impl<const LINES: usize> Application for Editor<LINES> {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        let text = (0..LINES)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\r\n");
        let scroll = ScrollState {
            offset: Point::ZERO,
            content: Size::ZERO,
            viewport: Size::ZERO,
        };
        (
            Model {
                text,
                revision: 0,
                scroll,
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Changed(change) => {
                model.text = change.text;
                model.revision = change.revision;
            }
            Message::Scrolled(scroll) => model.scroll = scroll,
            Message::Other(_) => {}
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                Element::from(ScrollView {
                    content: Element::new(TextEditor {
                        label: "Notes".into(),
                        placeholder: String::new(),
                        value: model.text.clone(),
                        revision: model.revision,
                        enabled: true,
                        on_change: Some(Box::new(Message::Changed)),
                    })
                    .key("editor"),
                    state: model.scroll,
                    horizontal: false,
                    vertical: true,
                    scroller: Scroller::Always,
                    on_scroll: Box::new(Message::Scrolled),
                })
                .key("scroll"),
                Element::new(TextField {
                    label: "After".into(),
                    placeholder: String::new(),
                    value: String::new(),
                    revision: 0,
                    purpose: ContentPurpose::Normal,
                    enabled: true,
                    on_change: Some(Box::new(Message::Other)),
                    on_submit: None,
                    selection: None,
                })
                .key("after"),
            ],
        }))
    }
}

fn focused<const LINES: usize>() -> Harness<Editor<LINES>> {
    let mut h = Harness::<Editor<LINES>>::new();
    h.tap(TAB);
    assert!(h.focused("editor"));
    h
}

fn press<const LINES: usize>(
    h: &mut Harness<Editor<LINES>>,
    (code, keysym): (u32, u32),
    mods: Mods,
) {
    h.press_key(code, keysym, true, false, mods);
    h.press_key(code, keysym, false, false, mods);
}

fn caret<const LINES: usize>(h: &Harness<Editor<LINES>>) -> usize {
    match h.accessible("editor").value {
        Some(AccessibleValue::Text {
            editing: Some(editing),
            ..
        }) => editing.caret,
        other => panic!("not an editable text: {other:?}"),
    }
}

#[test]
fn return_breaks_the_line_and_up_and_down_keep_the_column_across_a_short_line() {
    let mut h = focused::<0>();
    h.type_text("abcdef");
    press(&mut h, RETURN, Mods::NONE);
    press(&mut h, RETURN, Mods::NONE);
    h.type_text("abcd");
    assert_eq!(h.model().text, "abcdef\n\nabcd");
    assert_eq!(h.accessible("editor").role, Role::MultilineTextInput);

    let mut carets = Vec::new();
    for key in [UP, UP, DOWN, DOWN] {
        press(&mut h, key, Mods::NONE);
        carets.push(caret(&h));
    }
    assert_eq!(carets, [7, 4, 7, 12]);
}

#[test]
fn a_line_break_at_the_start_moves_every_line_after_it() {
    let mut h = focused::<0>();
    h.type_text("ab");
    press(&mut h, RETURN, Mods::NONE);
    h.type_text("cd");
    press(&mut h, RETURN, Mods::NONE);
    h.type_text("ef");
    press(&mut h, UP, Mods::PRIMARY);
    press(&mut h, RETURN, Mods::NONE);
    assert_eq!(h.model().text, "\nab\ncd\nef");

    let mut carets = vec![caret(&h)];
    for _ in 0..2 {
        press(&mut h, DOWN, Mods::NONE);
        carets.push(caret(&h));
    }
    assert_eq!(carets, [1, 4, 7]);
}

#[test]
fn a_rebuild_with_the_models_crlf_text_keeps_the_caret() {
    let mut h = focused::<3>();
    press(&mut h, UP, Mods::PRIMARY);
    press(&mut h, TAB_KEY, Mods::NONE);
    h.type_text("x");
    assert!(h.focused("after"));
    assert_eq!(h.model().text, "line 0\r\nline 1\r\nline 2");
    assert_eq!(
        caret(&h),
        0,
        "the view was rebuilt with the same text and revision"
    );
}

#[test]
fn down_moves_by_visual_line_inside_a_wrapped_paragraph() {
    let mut h = focused::<0>();
    let text = "word ".repeat(30);
    h.type_text(&text);
    press(&mut h, UP, Mods::PRIMARY);
    assert_eq!(caret(&h), 0);

    press(&mut h, DOWN, Mods::NONE);
    let second = caret(&h);
    assert!(
        0 < second && second < 30,
        "the caret went to the second visual line, not the paragraph's end: {second}"
    );
}

#[test]
fn tab_moves_the_focus_on_instead_of_typing_a_tab() {
    let mut h = focused::<0>();
    assert_eq!(
        h.rect("editor").height(),
        h.rect("scroll").height(),
        "a short text still fills its scroll view, so a press below it focuses the editor"
    );
    h.type_text("a");
    press(&mut h, TAB_KEY, Mods::NONE);
    assert!(h.focused("after"));
    assert_eq!(h.model().text, "a");
}

fn glyph_runs<const LINES: usize>(h: &Harness<Editor<LINES>>) -> usize {
    h.ops("editor")
        .iter()
        .filter(|op| matches!(op, PaintOp::Glyphs { .. }))
        .count()
}

#[test]
fn the_caret_scrolls_into_view_and_only_the_lines_that_show_are_painted() {
    let mut h = focused::<1000>();
    let scroll = h.model().scroll;
    assert!(scroll.content.height > 100.0 * scroll.viewport.height);
    assert_eq!(
        scroll.offset.y,
        scroll.content.height - scroll.viewport.height,
        "focus shows the caret at the text's end"
    );
    let painted = glyph_runs(&h);
    assert!((1..20).contains(&painted), "painted {painted} lines");
    let bottom = h.ops("editor");

    press(&mut h, UP, Mods::PRIMARY);
    assert_eq!((caret(&h), h.model().scroll.offset.y), (0, 0.0));
    assert_eq!(glyph_runs(&h), painted);
    assert_ne!(
        h.ops("editor"),
        bottom,
        "scrolling paints the lines that show now"
    );
}

/// The text work of typing one character at the end of a `LINES`-line document.
fn work_of_one_edit<const LINES: usize>() -> TextWork {
    let mut h = focused::<LINES>();
    let before = TextWork::current();
    h.type_text("x");
    let after = TextWork::current();
    assert!(h.model().text.ends_with('x'));
    TextWork {
        shaped: after.shaped - before.shaped,
        laid_out: after.laid_out - before.laid_out,
    }
}

#[test]
fn a_one_character_edit_shapes_and_lays_out_as_much_in_10_000_lines_as_in_1_000() {
    let small = work_of_one_edit::<1_000>();
    assert_eq!(small, work_of_one_edit::<10_000>());
    assert!(
        (1..=4).contains(&small.laid_out),
        "an edit lays out only its paragraph: {small:?}"
    );
}

#[test]
fn a_new_width_breaks_the_lines_again_without_shaping_them_again() {
    let mut h = focused::<10_000>();
    let height = h.rect("editor").height();
    let before = TextWork::current();
    h.resize(Size::new(60.0, 400.0));
    let after = TextWork::current();
    assert_eq!(
        after.shaped - before.shaped,
        0,
        "the paragraphs keep their shaping"
    );
    assert_eq!(
        after.laid_out - before.laid_out,
        10_001,
        "each paragraph breaks once, and the field below"
    );
    assert!(
        h.rect("editor").height() > height,
        "the lines wrap at the new width"
    );
}
