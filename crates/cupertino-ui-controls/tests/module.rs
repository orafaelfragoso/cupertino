use crate::support;

use cupertino_geometry::{Insets, Point, Rect};
use cupertino_ui_controls::{
    BatteryMeter, ModuleLayout, ModuleToggle, Slider, SliderStyle, Stack, Tile,
};
use cupertino_ui_core::{AccessibleValue, Application, Checked, Command, Element, Role, Window};
use cupertino_ui_layout::{Align, Axis, LayoutDirection};
use cupertino_ui_render::{Paint, PaintOp, Shape};
use cupertino_ui_theme::{Color, ColorRole, InteractionVariant};
use support::{color, window, Harness, RETURN, SPACE, TAB};

/// A Control Center's modules: Wi-Fi as a row, Stage as a compact tile, a brightness slider and two batteries.
struct Panel;

#[derive(Clone, Debug, PartialEq)]
enum Message {
    WiFi(bool),
    Stage(bool),
    Brightness(f64),
}

#[derive(Default)]
struct Model {
    wifi: bool,
    stage: bool,
    brightness: f64,
}

impl Application for Panel {
    type Model = Model;
    type Message = Message;

    fn init() -> (Model, Vec<Command<Message>>) {
        (
            Model {
                brightness: 0.5,
                ..Model::default()
            },
            Vec::new(),
        )
    }

    fn update(model: &mut Model, message: Message) -> Vec<Command<Message>> {
        match message {
            Message::WiFi(on) => model.wifi = on,
            Message::Stage(on) => model.stage = on,
            Message::Brightness(value) => model.brightness = value,
        }
        Vec::new()
    }

    fn view(model: &Model) -> Vec<Window<Message>> {
        let battery = |level| {
            Element::new(BatteryMeter {
                label: "Battery".into(),
                level,
                charging: false,
            })
        };
        window(Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children: vec![
                Element::new(ModuleToggle {
                    label: "Wi-Fi".into(),
                    caption: "Home Network".into(),
                    symbol: "symbol.wifi".into(),
                    layout: ModuleLayout::Row,
                    on: model.wifi,
                    enabled: true,
                    on_toggle: Some(Box::new(Message::WiFi)),
                })
                .key("wifi"),
                Element::from(Tile {
                    label: "Stage".into(),
                    content: Element::new(ModuleToggle {
                        label: "Stage".into(),
                        caption: String::new(),
                        symbol: "symbol.stage".into(),
                        layout: ModuleLayout::Compact,
                        on: model.stage,
                        enabled: true,
                        on_toggle: Some(Box::new(Message::Stage)),
                    })
                    .key("stage"),
                })
                .key("tile"),
                Element::new(Slider {
                    label: "Display".into(),
                    value: model.brightness,
                    min: 0.0,
                    max: 1.0,
                    step: None,
                    style: SliderStyle::PanelLevel("symbol.sun"),
                    ticks: 0,
                    enabled: true,
                    on_change: Some(Box::new(Message::Brightness)),
                    on_end: None,
                })
                .key("display"),
                battery(0.0).key("empty"),
                battery(0.1).key("low"),
                battery(0.11).key("high"),
            ],
        }))
    }
}

fn fills(h: &Harness<Panel>, key: &'static str) -> Vec<(Rect, Color)> {
    h.ops(key)
        .into_iter()
        .filter_map(|op| match op {
            PaintOp::Fill {
                shape: Shape::RoundedRect { rect, .. },
                paint: Paint::Solid(c),
            } => Some((rect, c)),
            _ => None,
        })
        .collect()
}

#[test]
fn a_row_module_toggles_from_its_text_and_with_space_but_not_return() {
    let mut h = Harness::<Panel>::new();
    let row = h.rect("wifi");
    h.click(Point::new(row.max_x() - 4.0, row.center().y));
    assert!(
        h.model().wifi,
        "the text beside the circle is part of the target"
    );
    assert_eq!(
        fills(&h, "wifi")[0].1,
        color(ColorRole::ControlFill, InteractionVariant::Hover, true),
        "on is the accent circle"
    );

    h.tap(TAB);
    assert!(h.focused("wifi"));
    h.tap(RETURN);
    assert!(h.model().wifi);
    h.tap(SPACE);
    assert!(!h.model().wifi);
}

#[test]
fn a_row_module_is_a_switch_named_by_its_title_and_described_by_its_caption() {
    let mut h = Harness::<Panel>::new();
    let node = h.accessible("wifi");
    assert_eq!(node.role, Role::Switch);
    assert_eq!(node.name.as_deref(), Some("Wi-Fi"));
    assert_eq!(node.description.as_deref(), Some("Home Network"));
    assert_eq!(node.value, Some(AccessibleValue::Toggle(Checked::False)));
    h.click(h.center("wifi"));
    assert_eq!(
        h.accessible("wifi").value,
        Some(AccessibleValue::Toggle(Checked::True))
    );
}

#[test]
fn in_right_to_left_the_circle_moves_to_the_right() {
    let h = Harness::<Panel>::with_direction(LayoutDirection::Rtl);
    let row = h.rect("wifi");
    let circle = fills(&h, "wifi")[0].0;
    assert_eq!(circle.max_x(), row.width());
    assert_eq!(circle.width(), 40.0);
}

#[test]
fn a_tile_is_a_group_named_by_its_label_holding_its_compact_module() {
    let mut h = Harness::<Panel>::new();
    let tile = h.accessible("tile");
    assert_eq!(tile.role, Role::Group);
    assert_eq!(tile.name.as_deref(), Some("Stage"));
    assert!(h.rect("tile").contains(h.center("stage")));
    h.click(h.center("stage"));
    assert!(h.model().stage);
}

#[test]
fn a_panel_level_slider_keeps_its_own_track_grey_and_fills_with_the_thumb_colour() {
    let h = Harness::<Panel>::new();
    assert_eq!(
        fills(&h, "display")[0].1,
        color(ColorRole::PanelTrack, InteractionVariant::Normal, false)
    );
    assert_eq!(
        fills(&h, "display")[1].1,
        color(ColorRole::SwitchThumb, InteractionVariant::Normal, false)
    );
}

#[test]
fn a_battery_at_or_below_a_tenth_fills_red_never_narrower_than_a_sliver_and_reports_its_level() {
    let h = Harness::<Panel>::new();
    let low = fills(&h, "low")[1];
    assert_eq!(
        low.1,
        color(
            ColorRole::DestructiveFill,
            InteractionVariant::Normal,
            false
        )
    );
    assert_eq!(low.0.width(), 2.0, "a tenth of the 19 pt span, rounded");
    let empty = fills(&h, "empty")[1];
    assert_eq!(empty.0.width(), 2.0, "an empty battery keeps a sliver");
    assert_eq!(empty.1, low.1, "in red");
    assert_eq!(
        fills(&h, "high")[1].1,
        color(ColorRole::Text, InteractionVariant::Normal, false)
    );
    let node = h.accessible("low");
    assert_eq!(node.role, Role::ProgressIndicator);
    assert_eq!(
        node.value,
        Some(AccessibleValue::Number {
            value: 0.1,
            min: 0.0,
            max: 1.0,
            step: None
        })
    );
}
