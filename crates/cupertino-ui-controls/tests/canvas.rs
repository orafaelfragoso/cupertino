use crate::support;

use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_controls::{Button, ButtonRole, ButtonStyle, Canvas, Stack};
use cupertino_ui_core::{Application, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::Color;
use support::{window, Harness, TAB};

const INK: Color = Color::new(0.1, 0.2, 0.3, 1.0);

struct Drawing;

impl Application for Drawing {
    type Model = u32;
    type Message = ();

    fn init() -> (u32, Vec<Command<()>>) {
        (0, Vec::new())
    }

    fn update(presses: &mut u32, _: ()) -> Vec<Command<()>> {
        *presses += 1;
        Vec::new()
    }

    fn view(_: &u32) -> Vec<Window<()>> {
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Start,
            children: vec![Element::new(Canvas {
                size: Some(Size::new(120.0, 60.0)),
                label: "Chart".into(),
                paint: Box::new(|cx| {
                    let size = cx.size();
                    cx.push(PaintOp::Fill {
                        shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height / 2.0)),
                        paint: Paint::Solid(INK),
                    });
                }),
                children: vec![Element::new(Button {
                    label: "Bar".into(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Borderless,
                    enabled: true,
                    busy: false,
                    on_press: Some(()),
                })
                .key("part")],
            })
            .key("canvas")],
        }))
    }
}

#[test]
fn a_canvas_draws_what_its_closure_paints_at_its_own_size() {
    let h = Harness::<Drawing>::new();
    assert_eq!(h.rect("canvas"), Rect::new(0.0, 0.0, 120.0, 60.0));
    assert_eq!(
        h.ops("canvas"),
        [PaintOp::Fill {
            shape: Shape::Rect(Rect::new(0.0, 0.0, 120.0, 30.0)),
            paint: Paint::Solid(INK),
        }]
    );
}

#[test]
fn its_interactive_parts_are_its_children_in_a_named_group() {
    let mut h = Harness::<Drawing>::assistive();
    let canvas = h.accessible("canvas");
    assert_eq!(
        (canvas.role, canvas.name.as_deref()),
        (Role::Group, Some("Chart"))
    );
    h.tap(TAB);
    assert!(h.focused("part"));
    h.tap(support::RETURN);
    assert_eq!(*h.model(), 1);
}
