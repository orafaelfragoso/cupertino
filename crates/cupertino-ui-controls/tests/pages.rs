use crate::support;

use cupertino_geometry::{Insets, Point};
use cupertino_ui_controls::{PageDots, Stack};
use cupertino_ui_core::{AccessibleValue, Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, window, Harness, TAB};

const LEFT: u32 = 0xff51;
const RIGHT: u32 = 0xff53;

/// Page dots for `PAGES` pages, starting on the first; the model takes every choice.
struct Pages<const PAGES: usize>;

impl<const PAGES: usize> Application for Pages<PAGES> {
    type Model = usize;
    type Message = usize;

    fn init() -> (usize, Vec<Command<usize>>) {
        (0, Vec::new())
    }

    fn update(current: &mut usize, new: usize) -> Vec<Command<usize>> {
        *current = new;
        Vec::new()
    }

    fn view(current: &usize) -> Vec<Window<usize>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(PageDots {
                label: "Photos".into(),
                count: PAGES,
                current: *current,
                enabled: true,
                on_select: Some(Box::new(|page| page)),
            })
            .key("dots")],
        }))
    }
}

/// The tabs, in order, as (name, selected, focusable).
fn tabs<A: Application>(h: &Harness<A>) -> Vec<(String, bool, bool)> {
    let snapshot = h.snapshot();
    let mut tabs: Vec<_> = snapshot
        .nodes
        .iter()
        .filter_map(|n| {
            let (_, node) = snapshot.accessibility.iter().find(|(id, _)| *id == n.id)?;
            (node.role == Role::Tab).then(|| {
                (
                    n.rect.min_x(),
                    (
                        node.name.clone().unwrap_or_default(),
                        node.states.selected,
                        node.states.focusable,
                    ),
                )
            })
        })
        .collect();
    tabs.sort_by(|a, b| a.0.total_cmp(&b.0));
    tabs.into_iter().map(|(_, tab)| tab).collect()
}

/// Each dot's colour, in paint order.
fn dots<A: Application>(h: &Harness<A>) -> Vec<Color> {
    h.snapshot()
        .display_list
        .nodes
        .into_iter()
        .flat_map(|n| n.ops)
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                paint: Paint::Solid(color),
            } if rect.width() == 8.0 => Some(color),
            _ => None,
        })
        .collect()
}

#[test]
fn up_to_ten_pages_are_tabs_with_the_shown_page_selected_and_the_only_tab_stop() {
    let h = Harness::<Pages<3>>::new();
    let list = h.accessible("dots");
    assert_eq!(
        (list.role, list.name.as_deref()),
        (Role::TabList, Some("Photos"))
    );
    assert_eq!(
        tabs(&h),
        [
            ("1".to_owned(), true, true),
            ("2".to_owned(), false, false),
            ("3".to_owned(), false, false),
        ]
    );
    let (shown, other) = (
        color(ColorRole::Text, InteractionVariant::Normal, false),
        color(ColorRole::PageDot, InteractionVariant::Normal, false),
    );
    assert_eq!(dots(&h), [shown, other, other]);
}

#[test]
fn the_arrows_choose_the_next_page_and_take_focus_with_them_but_stop_at_either_end() {
    let mut h = Harness::<Pages<3>>::new();
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(*h.model(), 0);
    h.tap(RIGHT);
    h.tap(RIGHT);
    assert_eq!(*h.model(), 2);
    let snapshot = h.snapshot();
    let focused = snapshot
        .accessibility
        .iter()
        .find(|(id, _)| Some(*id) == snapshot.focus)
        .and_then(|(_, n)| n.name.clone());
    assert_eq!(focused.as_deref(), Some("3"), "focus follows the selection");
    h.tap(RIGHT);
    assert_eq!(*h.model(), 2);

    let mut h = Harness::<Pages<3>>::with_direction(LayoutDirection::Rtl);
    h.tap(TAB);
    h.tap(LEFT);
    assert_eq!(*h.model(), 1, "left is forward right to left");
}

#[test]
fn a_click_on_a_dot_shows_its_page() {
    let mut h = Harness::<Pages<3>>::new();
    let row = h.rect("dots");
    let centre = h.center("dots");
    h.click(Point::new(row.max_x() - 10.0, centre.y));
    assert_eq!(*h.model(), 2);
}

#[test]
fn more_than_ten_pages_are_one_slider_of_page_numbers() {
    let mut h = Harness::<Pages<12>>::new();
    assert!(tabs(&h).is_empty());
    let value = |h: &Harness<Pages<12>>| h.accessible("dots").value;
    let node = h.accessible("dots");
    assert_eq!(node.role, Role::Slider);
    assert!(node.states.focusable);
    assert_eq!(
        value(&h),
        Some(AccessibleValue::Number {
            value: 1.0,
            min: 1.0,
            max: 12.0,
            step: Some(1.0),
        })
    );
    h.tap(TAB);
    h.tap(RIGHT);
    assert_eq!(*h.model(), 1);
    assert!(matches!(
        value(&h),
        Some(AccessibleValue::Number { value, .. }) if value == 2.0
    ));
}
