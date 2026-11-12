use crate::support;

use cupertino_geometry::{Insets, Rect};
use cupertino_ui_controls::{Icon, IconDirection, Stack};
use cupertino_ui_core::{
    Application, Command, Element, IconImage, Icons, PlaceholderIcons, Role, Window,
};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_platform::WindowEvent;
use cupertino_ui_render::PaintOp;
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use std::sync::{Arc, Mutex};
use support::{color, window, Harness};

/// Placeholders, with every request kept: the name, the device pixels, the colour and whether it was mirrored.
#[derive(Debug, Default)]
struct Asked {
    requests: Mutex<Vec<(String, u32, Color, bool)>>,
    draw: PlaceholderIcons,
}

impl Icons for Asked {
    fn symbol(&self, name: &str, px: u32, color: Color, mirrored: bool) -> IconImage {
        self.requests
            .lock()
            .unwrap()
            .push((name.to_owned(), px, color, mirrored));
        self.draw.symbol(name, px, color, mirrored)
    }
}

impl Asked {
    fn of(&self, name: &str) -> Vec<(u32, Color, bool)> {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, ..)| n == name)
            .map(|(_, px, color, mirrored)| (*px, *color, *mirrored))
            .collect()
    }
}

struct Icons3;

impl Application for Icons3 {
    type Model = ();
    type Message = ();

    fn init() -> ((), Vec<Command<()>>) {
        ((), Vec::new())
    }

    fn update(_: &mut (), _: ()) -> Vec<Command<()>> {
        Vec::new()
    }

    fn view(_: &()) -> Vec<Window<()>> {
        let icon = |key: &'static str, direction, label: Option<&str>, tint| {
            Element::new(Icon {
                name: format!("symbol.{key}"),
                size: 16.0,
                direction,
                label: label.map(str::to_owned),
                tint,
            })
            .key(key)
        };
        window(Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: 8.0,
            align: Align::Start,
            children: vec![
                icon("house", IconDirection::Fixed, Some("Home"), None),
                icon("back", IconDirection::Semantic, Some("Back"), None),
                icon(
                    "star",
                    IconDirection::Fixed,
                    None,
                    Some(ColorRole::PlaceholderText),
                ),
            ],
        }))
    }
}

fn harness(direction: LayoutDirection) -> (Harness<Icons3>, Arc<Asked>) {
    let asked = Arc::new(Asked::default());
    (Harness::with_icons(direction, asked.clone()), asked)
}

#[test]
fn an_icon_takes_its_size_and_draws_its_symbol_at_the_devices_pixels_once_per_scale() {
    let (mut h, asked) = harness(LayoutDirection::Ltr);
    let rect = h.rect("house");
    assert_eq!((rect.width(), rect.height()), (16.0, 16.0));
    let text = color(ColorRole::Text, InteractionVariant::Normal, false);
    for (scale, px) in [(1.0, 16), (1.5, 24), (2.0, 32)] {
        h.send(WindowEvent::Scale(scale));
        h.settle();
        let [PaintOp::Image { src, dst, .. }] = &h.ops("house")[..] else {
            panic!("one image at {scale}x: {:?}", h.ops("house"));
        };
        assert_eq!(*src, Rect::new(0.0, 0.0, f64::from(px), f64::from(px)));
        assert_eq!(*dst, Rect::new(0.0, 0.0, 16.0, 16.0), "{scale}x");
        assert_eq!(
            asked.of("symbol.house").last(),
            Some(&(px, text, false)),
            "{scale}x asks for {px} px in the label colour"
        );
    }
    let before = asked.of("symbol.house").len();
    h.send(WindowEvent::Scale(2.0));
    h.settle();
    assert_eq!(
        asked.of("symbol.house").len(),
        before,
        "an unchanged icon is not drawn again"
    );
}

#[test]
fn an_icon_is_drawn_in_its_tint_role() {
    let (_h, asked) = harness(LayoutDirection::Ltr);
    let tint = color(
        ColorRole::PlaceholderText,
        InteractionVariant::Normal,
        false,
    );
    assert_eq!(asked.of("symbol.star").last().map(|r| r.1), Some(tint));
}

#[test]
fn artwork_along_the_reading_direction_is_mirrored_right_to_left_and_fixed_artwork_is_not() {
    for (direction, mirrored) in [(LayoutDirection::Ltr, false), (LayoutDirection::Rtl, true)] {
        let (_h, asked) = harness(direction);
        assert_eq!(
            asked.of("symbol.back").last().map(|r| r.2),
            Some(mirrored),
            "{direction:?}"
        );
        assert_eq!(
            asked.of("symbol.house").last().map(|r| r.2),
            Some(false),
            "{direction:?}"
        );
    }
}

#[test]
fn a_labelled_icon_is_an_image_and_a_decorative_one_is_not_in_the_accessibility_tree() {
    let (h, _) = harness(LayoutDirection::Ltr);
    let house = h.accessible("house");
    assert_eq!(house.role, Role::Image);
    assert_eq!(house.name.as_deref(), Some("Home"));
    let star =
        h.rt.find(h.window, &cupertino_ui_core::Key::from("star"))
            .unwrap();
    assert!(
        h.snapshot().accessibility.iter().all(|(id, _)| *id != star),
        "the decorative icon has no node"
    );
}
