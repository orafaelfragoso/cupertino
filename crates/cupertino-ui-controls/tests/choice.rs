use crate::support;

use cupertino_geometry::Insets;
use cupertino_ui_controls::{RadioGroup, SegmentedControl, Stack};
use cupertino_ui_core::{AccessibleValue, Application, Checked, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use support::{window, Harness, SPACE, TAB};

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;
const DOWN: u32 = 0xff54;

/// A radio group of three sizes and a segmented control of three views; each keeps its own choice.
struct Choices;

#[derive(Default)]
struct Model {
    size: Option<usize>,
    view: Option<usize>,
}

#[derive(Clone, Debug, PartialEq)]
enum Message {
    Size(usize),
    View(usize),
}

impl Application for Choices {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                size: None,
                view: Some(0),
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::Size(i) => model.size = Some(i),
            Message::View(i) => model.view = Some(i),
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let strings = |s: &[&str]| s.iter().map(|s| s.to_string()).collect();
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 4.0,
            align: Align::Start,
            children: vec![
                Element::new(RadioGroup {
                    label: "Size".into(),
                    options: strings(&["Small", "Medium", "Large"]),
                    selected: model.size,
                    axis: Axis::Vertical,
                    spacing: 0.0,
                    enabled: true,
                    on_select: Some(Box::new(Message::Size)),
                })
                .key("size"),
                Element::new(SegmentedControl {
                    label: "View".into(),
                    segments: strings(&["Icons", "List", "Columns"]),
                    selected: model.view,
                    enabled: true,
                    on_select: Some(Box::new(Message::View)),
                })
                .key("view"),
            ],
        }))
    }
}

/// The accessibility node of the `index`-th item under `group`.
fn item(
    h: &Harness<Choices>,
    group: &'static str,
    index: usize,
) -> cupertino_ui_core::AccessibilityNode {
    let snapshot = h.snapshot();
    let id = h.rt.find(h.window, &group.into()).unwrap();
    let parent = snapshot
        .accessibility
        .iter()
        .find(|(n, _)| *n == id)
        .unwrap();
    let child = parent.1.children[index];
    snapshot
        .accessibility
        .into_iter()
        .find(|(n, _)| *n == child)
        .unwrap()
        .1
}

#[test]
fn a_group_is_one_tab_stop_and_the_arrows_choose_and_move_the_focus() {
    let mut h = Harness::<Choices>::new();
    h.tap(TAB);
    assert!(
        item(&h, "size", 0).states.focused,
        "with nothing chosen, the first is the stop"
    );
    h.tap(DOWN);
    assert_eq!(h.model().size, Some(1));
    assert!(item(&h, "size", 1).states.focused);
    h.tap(TAB);
    assert!(
        item(&h, "view", 0).states.focused,
        "Tab leaves the group for the next one"
    );
    h.tap(RIGHT);
    h.tap(RIGHT);
    h.tap(RIGHT);
    assert_eq!(h.model().view, Some(0), "the arrows wrap");
    assert!(item(&h, "view", 0).states.focused);
}

#[test]
fn in_right_to_left_the_left_arrow_chooses_the_next_segment() {
    let mut h = Harness::<Choices>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(h.model().view, Some(1));
}

#[test]
fn a_click_or_space_chooses_an_item() {
    let mut h = Harness::<Choices>::new();
    let large = item(&h, "size", 2).bounds;
    h.click(large.center());
    assert_eq!(h.model().size, Some(2));
    h.tap(TAB);
    h.tap(TAB);
    h.tap(RIGHT);
    h.tap(SPACE);
    assert_eq!(h.model().view, Some(1));
}

#[test]
fn segments_are_equally_wide_and_groups_are_radio_groups() {
    let h = Harness::<Choices>::new();
    let widths: Vec<f64> = (0..3).map(|i| item(&h, "view", i).bounds.width()).collect();
    assert!(widths.iter().all(|w| *w == widths[0]), "{widths:?}");
    let chrome = h
        .ops("view")
        .into_iter()
        .find_map(|op| match op {
            cupertino_ui_render::PaintOp::Fill {
                shape: cupertino_ui_render::Shape::RoundedRect { rect, .. },
                ..
            } => Some(rect),
            _ => None,
        })
        .unwrap();
    assert_eq!(
        chrome.width(),
        3.0 * widths[0],
        "the chrome holds the segments, not the row"
    );
    assert_eq!(h.accessible("size").role, Role::RadioGroup);
    let chosen = item(&h, "view", 0);
    assert_eq!(chosen.role, Role::RadioButton);
    assert_eq!(chosen.value, Some(AccessibleValue::Toggle(Checked::True)));
    assert_eq!(chosen.name.as_deref(), Some("Icons"));
}

#[test]
fn no_divider_sits_beside_the_chosen_segment() {
    let h = Harness::<Choices>::new();
    let dividers = h
        .ops("view")
        .into_iter()
        .filter(|op| {
            matches!(
                op,
                cupertino_ui_render::PaintOp::Fill {
                    shape: cupertino_ui_render::Shape::Rect(_),
                    ..
                }
            )
        })
        .count();
    assert_eq!(
        dividers, 1,
        "the first segment is chosen: only the divider between the other two"
    );
}

/// A radio group with one option, counting what it emits.
struct Alone;

impl Application for Alone {
    type Model = usize;
    type Message = ();

    fn init() -> (usize, Vec<Command<()>>) {
        (0, Vec::new())
    }

    fn update(model: &mut usize, _: ()) -> Vec<Command<()>> {
        *model += 1;
        Vec::new()
    }

    fn view(_: &usize) -> Vec<Window<()>> {
        window(Element::new(RadioGroup {
            label: "Only".into(),
            options: vec!["One".into()],
            selected: Some(0),
            axis: Axis::Vertical,
            spacing: 0.0,
            enabled: true,
            on_select: Some(Box::new(|_| ())),
        }))
    }
}

#[test]
fn an_arrow_in_a_group_of_one_emits_nothing() {
    let mut h = Harness::<Alone>::new();
    h.tap(TAB);
    h.tap(DOWN);
    h.tap(RIGHT);
    assert_eq!(*h.model(), 0);
}

#[test]
fn dragging_across_segments_keeps_the_choice_marked_until_the_release_chooses() {
    use cupertino_ui_platform::PointerEvent;
    let mut h = Harness::<Choices>::new();
    let first = item(&h, "view", 0).bounds;
    let last = item(&h, "view", 2).bounds;
    h.pointer(PointerEvent::Motion {
        position: first.center(),
    });
    h.button(first.center(), true);
    h.pointer(PointerEvent::Motion {
        position: item(&h, "view", 1).bounds.center(),
    });
    h.pointer(PointerEvent::Motion {
        position: last.center(),
    });
    assert_eq!(h.model().view, Some(0), "nothing is chosen during the drag");
    assert_eq!(
        item(&h, "view", 0).value,
        Some(AccessibleValue::Toggle(Checked::True)),
        "the original choice stays marked"
    );
    h.button(last.center(), false);
    assert_eq!(
        h.model().view,
        Some(2),
        "the release chooses the segment it ends over"
    );

    // let go past the control's end, the drag chooses nothing
    let chosen = item(&h, "view", 2).bounds;
    h.pointer(PointerEvent::Motion {
        position: chosen.center(),
    });
    h.button(chosen.center(), true);
    let beyond = cupertino_geometry::Point::new(chosen.max_x() + 200.0, chosen.center().y);
    h.pointer(PointerEvent::Motion { position: beyond });
    h.button(beyond, false);
    assert_eq!(h.model().view, Some(2));
    // and let go below it, too
    h.button(first.center(), true);
    let below = cupertino_geometry::Point::new(first.center().x, first.max_y() + 40.0);
    h.pointer(PointerEvent::Motion { position: below });
    h.button(below, false);
    assert_eq!(h.model().view, Some(2));
}

#[test]
fn a_segment_under_the_pointer_is_found_along_the_row_in_either_direction() {
    use cupertino_geometry::Rect;
    let second = Rect::new(100.0, 0.0, 50.0, 24.0);
    let at = |x, rtl| cupertino_ui_controls::segment_at(x, second, 1, 3, 50.0, rtl);
    assert_eq!(at(125.0, false), Some(1));
    assert_eq!(at(175.0, false), Some(2));
    assert_eq!(at(60.0, false), Some(0));
    assert_eq!(at(210.0, false), None, "past the last segment");
    assert_eq!(at(40.0, false), None, "before the first");
    assert_eq!(
        at(175.0, true),
        Some(0),
        "right to left, the first is on the right"
    );
    assert_eq!(at(60.0, true), Some(2));
}
