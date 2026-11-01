use crate::icon::symbol;
use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{BatteryMeter, ModuleLayout, ModuleToggle, Tile};
use cupertino_geometry::{Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Element, Environment,
    Event, EventCx, MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Constraints, ContainerKind, LayoutDirection, LayoutSpec, Measured, SizeSpec,
};
use cupertino_ui_text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle, Wrap,
};
use cupertino_ui_theme::{ColorRole, StateVariant};

/// The round button of the row layout.
const CIRCLE: f64 = 40.0;
const ROW_SYMBOL: f64 = 18.0;
/// Between the circle and the text, and after the text.
const TEXT_GAP: f64 = 8.0;
const TEXT_END: f64 = 2.0;
const TITLE: (f32, u16) = (12.0, 500);
const CAPTION: (f32, u16) = (10.0, 400);
const LINE_HEIGHT: f64 = 1.25;
const COMPACT: f64 = 62.0;
const COMPACT_SYMBOL: f64 = 22.0;
/// The compact symbol's centre below the top, and the title's line box above the bottom.
const COMPACT_SYMBOL_CENTRE: f64 = 22.0;
const COMPACT_TITLE_BOTTOM: f64 = 8.0;
/// Between the compact title and the module's sides.
const COMPACT_TITLE_SIDE: f64 = 4.0;
const TILE_RADIUS: f64 = 12.0;
/// The battery's body, its corner and stroke, and the terminal on its trailing side.
const BATTERY_BODY: Size = Size {
    width: 23.0,
    height: 12.0,
};
const BATTERY_RADIUS: f64 = 3.5;
const BATTERY_TERMINAL: Size = Size {
    width: 1.5,
    height: 4.0,
};
const BATTERY_FILL_INSET: f64 = 2.0;
const BATTERY_FILL_HEIGHT: f64 = 8.0;
const BATTERY_FILL_RADIUS: f64 = 1.5;
const BATTERY_FILL_MIN: f64 = 2.0;
/// At or below this level the fill turns red.
const BATTERY_LOW: f64 = 0.1;

/// `text` on one line in `size` and `weight`, ending in an ellipsis past `width`.
fn line(
    env: &Environment,
    text: &str,
    (size, weight): (f32, u16),
    width: f64,
) -> Option<ParagraphLayout> {
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle {
                wrap: Wrap::None,
                ellipsis: true,
                ..ParagraphStyle::default()
            },
        )
        .ok()?;
    Some(look::one_line(&paragraph, width.max(0.0)))
}

/// The line box of text in `size`.
fn line_box((size, _): (f32, u16)) -> f64 {
    f64::from(size) * LINE_HEIGHT
}

/// The top that centres `layout` in a line box at `top`.
fn centred_in_line(layout: &ParagraphLayout, top: f64, style: (f32, u16)) -> f64 {
    top + (line_box(style) - layout.size().height) / 2.0
}

impl<M> ModuleToggle<M> {
    fn control_state(&self, press: &Press, cx: &PaintCx<'_>) -> ControlState {
        ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            selected: self.on,
            window_active: cx.window_active(),
            ..ControlState::default()
        }
    }

    /// The symbol's colour: the selected control text's on the accent, else the label's.
    fn symbol_role(&self) -> ColorRole {
        match (self.enabled, self.on) {
            (false, _) => ColorRole::DisabledControlText,
            (true, true) => ColorRole::ControlText,
            (true, false) => ColorRole::Text,
        }
    }

    /// On is the accent, a selected control's fill; off is the switch track's system fill.
    fn circle_role(&self) -> ColorRole {
        if self.on {
            ColorRole::ControlFill
        } else {
            ColorRole::SwitchTrack
        }
    }

    fn text_role(&self, enabled: ColorRole) -> ColorRole {
        if self.enabled {
            enabled
        } else {
            ColorRole::DisabledControlText
        }
    }

    fn paint_row(&self, state: &ControlState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let size = cx.size();
        let x = if rtl { size.width - CIRCLE } else { 0.0 };
        let circle = Rect::new(x, (size.height - CIRCLE) / 2.0, CIRCLE, CIRCLE);
        look::fill(
            cx,
            circle,
            CIRCLE / 2.0,
            look::color(&theme, self.circle_role(), state),
        );
        if let Some(color) = look::color(&theme, self.symbol_role(), state) {
            symbol(cx, &self.symbol, ROW_SYMBOL, color, false, circle);
        }
        let width = size.width - CIRCLE - TEXT_GAP - TEXT_END;
        let top = (size.height - line_box(TITLE) - line_box(CAPTION)) / 2.0;
        let lines = [
            (&self.label, TITLE, ColorRole::Text, top),
            (
                &self.caption,
                CAPTION,
                ColorRole::SecondaryText,
                top + line_box(TITLE),
            ),
        ];
        for (text, style, role, line_top) in lines {
            let Some(layout) = line(cx.env(), text, style, width) else {
                continue;
            };
            let x = if rtl {
                circle.min_x() - TEXT_GAP - layout.size().width
            } else {
                circle.max_x() + TEXT_GAP
            };
            let origin = Point::new(x, centred_in_line(&layout, line_top, style));
            look::glyphs(
                cx,
                &layout,
                origin,
                look::color(&theme, self.text_role(role), state),
            );
        }
        paint_focus_ring(cx, circle, CIRCLE / 2.0);
    }

    fn paint_compact(&self, state: &ControlState, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let size = cx.size();
        let symbol_role = match (self.enabled, self.on) {
            (false, _) => ColorRole::DisabledControlText,
            (true, true) => ColorRole::SelectedControlFill,
            (true, false) => ColorRole::Text,
        };
        let area = Rect::new(
            0.0,
            COMPACT_SYMBOL_CENTRE - COMPACT_SYMBOL / 2.0,
            size.width,
            COMPACT_SYMBOL,
        );
        if let Some(color) = look::color(&theme, symbol_role, state) {
            symbol(cx, &self.symbol, COMPACT_SYMBOL, color, false, area);
        }
        let width = size.width - 2.0 * COMPACT_TITLE_SIDE;
        if let Some(layout) = line(cx.env(), &self.label, TITLE_COMPACT, width) {
            let top = size.height - COMPACT_TITLE_BOTTOM - line_box(TITLE_COMPACT);
            let origin = Point::new(
                (size.width - layout.size().width) / 2.0,
                centred_in_line(&layout, top, TITLE_COMPACT),
            );
            let color = look::color(&theme, self.text_role(ColorRole::Text), state);
            look::glyphs(cx, &layout, origin, color);
        }
        paint_focus_ring(
            cx,
            Rect::new(0.0, 0.0, size.width, size.height),
            TILE_RADIUS,
        );
    }
}

/// The compact layout's title: the caption's size in the title's weight.
const TITLE_COMPACT: (f32, u16) = (CAPTION.0, TITLE.1);

impl<M: 'static> Widget<M> for ModuleToggle<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        let height = match self.layout {
            ModuleLayout::Row => SizeSpec::Fixed(CIRCLE),
            ModuleLayout::Compact => SizeSpec::Fill,
        };
        LayoutSpec {
            width: SizeSpec::Fill,
            height,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.layout != old.layout {
            Change::Layout
        } else if self.on != old.on
            || self.enabled != old.enabled
            || self.label != old.label
            || self.caption != old.caption
            || self.symbol != old.symbol
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        let size = match self.layout {
            ModuleLayout::Row => Size::new(constraints.width.ideal(), CIRCLE),
            ModuleLayout::Compact => Size::new(COMPACT, COMPACT),
        };
        Measured {
            size,
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match press.event(event, self.enabled, Keys::Space, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                if let Some(on_toggle) = &self.on_toggle {
                    cx.emit(on_toggle(!self.on));
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let state = self.control_state(press, cx);
        match self.layout {
            ModuleLayout::Row => self.paint_row(&state, cx),
            ModuleLayout::Compact => self.paint_compact(&state, cx),
        }
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Switch);
        cx.name(self.label.clone());
        if !self.caption.is_empty() {
            cx.description(self.caption.clone());
        }
        cx.value(AccessibleValue::Toggle(if self.on {
            Checked::True
        } else {
            Checked::False
        }));
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

/// A [`Tile`] in the tree: its fill behind its one child.
struct TileFrame<M> {
    label: String,
    children: Vec<Element<M>>,
}

impl<M: 'static> From<Tile<M>> for Element<M> {
    fn from(tile: Tile<M>) -> Self {
        Element::new(TileFrame {
            label: tile.label,
            children: vec![tile.content],
        })
    }
}

impl<M: 'static> Widget<M> for TileFrame<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Stack {
            axis: cupertino_ui_layout::Axis::Vertical,
            spacing: 0.0,
            align: cupertino_ui_layout::Align::Stretch,
            justify: cupertino_ui_layout::Justify::Start,
        })
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let color = cx
            .env()
            .theme
            .colors
            .get(ColorRole::GroupFill, StateVariant::NORMAL);
        look::fill(
            cx,
            Rect::new(0.0, 0.0, size.width, size.height),
            TILE_RADIUS,
            color,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
        cx.name(self.label.clone());
    }
}

impl BatteryMeter {
    /// The level in `0..=1`, with an unknown one read as empty.
    fn level(&self) -> f64 {
        if self.level.is_nan() {
            0.0
        } else {
            self.level.clamp(0.0, 1.0)
        }
    }
}

impl<M: 'static> Widget<M> for BatteryMeter {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(
                BATTERY_BODY.width + BATTERY_TERMINAL.width,
                BATTERY_BODY.height,
            ),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// The terminal sits on the trailing side, and the fill grows from the leading one.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let y = (size.height - BATTERY_BODY.height) / 2.0;
        let body_x = if rtl { BATTERY_TERMINAL.width } else { 0.0 };
        let body = Rect::new(body_x, y, BATTERY_BODY.width, BATTERY_BODY.height);
        let outline = look::color(&theme, ColorRole::SecondaryText, &state);
        look::stroke(
            cx,
            body.inset(cupertino_geometry::Insets::uniform(0.5)),
            BATTERY_RADIUS - 0.5,
            1.0,
            outline,
        );
        let terminal_x = if rtl { 0.0 } else { body.max_x() };
        look::fill(
            cx,
            Rect::new(
                terminal_x,
                y + (BATTERY_BODY.height - BATTERY_TERMINAL.height) / 2.0,
                BATTERY_TERMINAL.width,
                BATTERY_TERMINAL.height,
            ),
            BATTERY_TERMINAL.width / 2.0,
            outline,
        );
        let level = self.level();
        let span = BATTERY_BODY.width - 2.0 * BATTERY_FILL_INSET;
        // an empty battery still shows a sliver, so the red warning stays visible
        let width = (span * level).round().clamp(BATTERY_FILL_MIN, span);
        let fill_x = if rtl {
            body.max_x() - BATTERY_FILL_INSET - width
        } else {
            body.min_x() + BATTERY_FILL_INSET
        };
        let role = if level <= BATTERY_LOW {
            ColorRole::DestructiveFill
        } else {
            ColorRole::Text
        };
        look::fill(
            cx,
            Rect::new(
                fill_x,
                y + (BATTERY_BODY.height - BATTERY_FILL_HEIGHT) / 2.0,
                width,
                BATTERY_FILL_HEIGHT,
            ),
            BATTERY_FILL_RADIUS,
            look::color(&theme, role, &state),
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ProgressIndicator);
        cx.name(self.label.clone());
        if self.charging {
            cx.description("Charging".to_owned());
        }
        cx.value(AccessibleValue::Number {
            value: self.level(),
            min: 0.0,
            max: 1.0,
            step: None,
        });
    }
}
