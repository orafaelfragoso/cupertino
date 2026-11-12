//! A settings form's look: icon tiles, groups of rows on a rounded fill with hairlines between them, the rows, the
//! navigation rows that open a page, the header card at the top of a page and the colour behind it all.

use crate::icon;
use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{
    AppIcon, AppIconRow, FormBackground, FormGroup, FormHeader, FormRow, IconTile, NavigationRow,
    PaneHeader, Stack, Text, TextRole, TileColor,
};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Element, Environment, Event, EventCx,
    MeasureCx, PaintCx, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    SizeSpec,
};
use cupertino_ui_render::{GradientStop, Paint, PaintOp, Shape};
use cupertino_ui_text::{
    AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextStyle, Wrap,
};
use cupertino_ui_theme::{Color, ColorRole, StateVariant, Surface, ThemeTokens};

/// A tile's corner radius and its symbol's size, as shares of the tile's side.
const TILE_RADIUS: f64 = 0.225;
const TILE_SYMBOL: f64 = 0.6;
/// The hairline just inside a tile's edge, white at this opacity, which keeps a tile apart from a dark window; a
/// graphite tile's is stronger.
const TILE_RIM_ALPHA: f32 = 0.15;
const GRAPHITE_RIM_ALPHA: f32 = 0.3;

/// A group's corner radius.
pub(crate) const GROUP_RADIUS: f64 = 12.0;
/// A group's fill and the hairline between its rows: the label colour at these opacities.
const GROUP_FILL_ALPHA: f32 = 0.027;
const SEPARATOR_ALPHA: f32 = 0.047;
/// The hairline's thickness, and how far it stops short of the group's edges.
const SEPARATOR: f64 = 1.0;
const SEPARATOR_INSET: f64 = 10.0;

/// A row's least height, and with an icon tile of the standard size.
pub const FORM_ROW_HEIGHT: f64 = 36.5;
pub const FORM_TILE_ROW_HEIGHT: f64 = 48.0;
/// The least height of a row with a 22 pt icon, and with a 20 pt one.
pub const FORM_ICON_ROW_HEIGHT: f64 = 43.0;
pub const FORM_SMALL_ICON_ROW_HEIGHT: f64 = 42.0;
/// The least height of a row with an icon and a second line of text.
pub const FORM_ICON_TWO_LINE_ROW_HEIGHT: f64 = 53.0;
/// A row's padding: above and below its content (a control padded out to the least pointer target, as a pop-up
/// button is, still leaves the row at the least height), at its edges, and at the leading edge before a tile.
const ROW_PADDING_Y: f64 = 6.0;
const ROW_PADDING: f64 = 10.0;
const TILE_ROW_LEADING: f64 = 12.0;
/// The tile of a row, and the space between it and the content.
const ROW_TILE: f64 = 24.0;
const ROW_TILE_GAP: f64 = 12.0;
/// The sides of the smaller icons a row may carry.
const ICON_TILE: f64 = 22.0;
const SMALL_ICON_TILE: f64 = 20.0;
/// A smaller icon's row: the space before it and between it and the content.
const ICON_ROW_LEADING: f64 = 9.0;
const ICON_ROW_GAP: f64 = 10.0;
/// The chevron of a navigation row: its symbol's size and the slot it is centred in at the trailing edge.
const CHEVRON: f64 = 11.0;
const CHEVRON_SLOT: f64 = 12.0;
/// The press highlight of a navigation row: the label colour at this opacity.
const PRESSED_ALPHA: f32 = 0.06;

/// The header card: its padding above the tile, at its sides and below the text; the tile; the space between the tile
/// and the title; the title's size and weight.
const HEADER_TOP: f64 = 26.0;
const HEADER_SIDE: f64 = 12.0;
const HEADER_BOTTOM: f64 = 20.0;
const HEADER_TILE: f64 = 48.0;
const HEADER_TILE_GAP: f64 = 12.0;
const HEADER_TITLE_SIZE: f32 = 22.0;
const HEADER_TITLE_WEIGHT: u16 = 700;

/// The top and bottom colours of each tile, in sRGB.
pub(crate) fn tile_shades(color: TileColor) -> ([u8; 3], [u8; 3]) {
    match color {
        TileColor::Blue => ([74, 160, 250], [52, 122, 232]),
        TileColor::Gray => ([170, 171, 176], [130, 131, 136]),
        TileColor::Graphite => ([52, 52, 54], [16, 16, 17]),
        TileColor::Green => ([114, 212, 117], [94, 184, 98]),
        TileColor::Indigo => ([114, 112, 240], [84, 80, 214]),
        TileColor::Orange => ([255, 172, 64], [245, 136, 22]),
        TileColor::Pink => ([255, 100, 140], [232, 58, 100]),
        TileColor::Purple => ([198, 122, 242], [160, 80, 214]),
        TileColor::Red => ([255, 102, 98], [232, 60, 56]),
        TileColor::Teal => ([112, 206, 232], [70, 170, 200]),
        TileColor::Yellow => ([255, 214, 64], [245, 186, 12]),
    }
}

pub(crate) fn rgb(c: [u8; 3]) -> Color {
    Color::from_rgba8([c[0], c[1], c[2], 255])
}

/// Draws `tile` filling the square `area`: the shaded fill, the rim and the symbol in white.
pub(crate) fn paint_tile(cx: &mut PaintCx<'_>, tile: &IconTile, area: Rect) {
    let radius = area.width() * TILE_RADIUS;
    let (top, bottom) = tile_shades(tile.color);
    cx.push(PaintOp::Fill {
        shape: Shape::RoundedRect { rect: area, radius },
        paint: Paint::Linear {
            start: Point::new(area.min_x(), area.min_y()),
            end: Point::new(area.min_x(), area.max_y()),
            stops: vec![
                GradientStop {
                    offset: 0.0,
                    color: rgb(top),
                },
                GradientStop {
                    offset: 1.0,
                    color: rgb(bottom),
                },
            ],
        },
    });
    let white = Color::new(1.0, 1.0, 1.0, 1.0);
    look::stroke(
        cx,
        area.inset(Insets::uniform(0.25)),
        radius - 0.25,
        0.5,
        Some(white.with_alpha(if tile.color == TileColor::Graphite {
            GRAPHITE_RIM_ALPHA
        } else {
            TILE_RIM_ALPHA
        })),
    );
    icon::symbol(
        cx,
        &tile.symbol,
        area.width() * TILE_SYMBOL,
        white,
        false,
        area,
    );
}

/// The label colour at `alpha` times its own opacity's share of full: a fill or line that follows the appearance.
fn label_tint(theme: &ThemeTokens, alpha: f32) -> Option<Color> {
    let label = theme.colors.get(ColorRole::Text, StateVariant::NORMAL)?;
    Some(label.with_alpha(alpha))
}

/// A group's rounded fill over `rect`.
pub(crate) fn paint_group(cx: &mut PaintCx<'_>, rect: Rect) {
    let theme = cx.env().theme.clone();
    look::fill(cx, rect, GROUP_RADIUS, label_tint(&theme, GROUP_FILL_ALPHA));
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

impl<M: 'static> Widget<M> for IconTile {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.size, self.size),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let side = size.width.min(size.height);
        let area = Rect::new(
            (size.width - side) / 2.0,
            (size.height - side) / 2.0,
            side,
            side,
        );
        paint_tile(cx, self, area);
    }

    /// Decorative: the row it stands in names it.
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The rows of a group, stacked, on the group's fill.
struct Group<M> {
    rows: Vec<Element<M>>,
}

impl<M: Clone + 'static> From<FormGroup<M>> for Element<M> {
    fn from(group: FormGroup<M>) -> Self {
        let count = group.rows.len();
        let mut rows = Vec::with_capacity(count * 2);
        for (i, row) in group.rows.into_iter().enumerate() {
            if i > 0 {
                rows.push(Element::new(Hairline));
            }
            // a navigation row's press fill follows the group's rounded corners where the row meets them
            let corners = GroupCorners {
                top: i == 0,
                bottom: i + 1 == count,
            };
            rows.push(
                row.map_widget(|row: NavigationRow<M>| GroupedNavigationRow { row, corners }),
            );
        }
        Element::new(Group { rows })
    }
}

impl<M: 'static> Widget<M> for Group<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: 0.0,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.rows)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        paint_group(cx, Rect::new(0.0, 0.0, size.width, size.height));
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The line between two rows of a group.
struct Hairline;

impl<M: 'static> Widget<M> for Hairline {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(SEPARATOR),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, SEPARATOR),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let size = cx.size();
        let line = Rect::new(
            SEPARATOR_INSET,
            0.0,
            (size.width - 2.0 * SEPARATOR_INSET).max(0.0),
            size.height,
        );
        if let Some(color) = label_tint(&theme, SEPARATOR_ALPHA) {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(line),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Holds a row open to its least height: an empty leaf of that height and no width.
struct Strut(f64);

impl<M: 'static> Widget<M> for Strut {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.0 != old.0 {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, self.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// What a row's icon asks of the row: its side, the space before it and after it, and the row's least height.
#[derive(Clone, Copy, Debug, PartialEq)]
struct IconRow {
    side: f64,
    leading: f64,
    gap: f64,
    height: f64,
}

/// The row of an icon `size` points across: a 20 or 22 pt icon sits in a shorter row, closer to the edge, than the
/// standard 24 pt tile. A size in between takes the next one up.
fn icon_row(size: f64) -> IconRow {
    if size <= SMALL_ICON_TILE + 0.5 {
        IconRow {
            side: SMALL_ICON_TILE,
            leading: ICON_ROW_LEADING,
            gap: ICON_ROW_GAP,
            height: FORM_SMALL_ICON_ROW_HEIGHT,
        }
    } else if size <= ICON_TILE + 0.5 {
        IconRow {
            side: ICON_TILE,
            leading: ICON_ROW_LEADING,
            gap: ICON_ROW_GAP,
            height: FORM_ICON_ROW_HEIGHT,
        }
    } else {
        IconRow {
            side: ROW_TILE,
            leading: TILE_ROW_LEADING,
            gap: ROW_TILE_GAP,
            height: FORM_TILE_ROW_HEIGHT,
        }
    }
}

/// The padding of a row: a row with an icon starts further in or less far, by the icon's size.
fn row_insets(icon: Option<IconRow>) -> Insets {
    Insets {
        top: ROW_PADDING_Y,
        left: icon.map_or(ROW_PADDING, |i| i.leading),
        bottom: ROW_PADDING_Y,
        right: ROW_PADDING,
    }
}

fn row_height(icon: Option<IconRow>) -> f64 {
    icon.map_or(FORM_ROW_HEIGHT, |i| i.height)
}

/// A row's tile, content and strut side by side; the content takes what the others leave.
struct Row<M> {
    icon: Option<IconRow>,
    children: Vec<Element<M>>,
}

/// The space between a row's tile and its content.
struct Gap(f64);

impl<M: 'static> Widget<M> for Gap {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.0 != old.0 {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.0, 0.0),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl<M: 'static> From<FormRow<M>> for Element<M> {
    fn from(row: FormRow<M>) -> Self {
        let icon = row.tile.as_ref().map(|t| icon_row(t.size));
        let mut children = vec![Element::new(Strut(row_height(icon) - 2.0 * ROW_PADDING_Y))];
        if let (Some(tile), Some(icon)) = (row.tile, icon) {
            children.push(Element::new(IconTile {
                size: icon.side,
                ..tile
            }));
            children.push(Element::new(Gap(icon.gap)));
        }
        children.push(row.content.fill());
        Element::new(Row { icon, children })
    }
}

impl<M: 'static> Widget<M> for Row<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            insets: row_insets(self.icon),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.icon != old.icon {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

impl<M: Clone + 'static> NavigationRow<M> {
    /// The tile's square, the label's room and the chevron's slot of a row `size` big, from its leading edge; mirror
    /// them for a right-to-left layout.
    fn places(&self, size: Size) -> (Option<Rect>, Rect, Rect) {
        let icon = self.tile.as_ref().map(|t| icon_row(t.size));
        let insets = row_insets(icon);
        let mut start = insets.left;
        let tile = icon.map(|icon| {
            let rect = Rect::new(start, (size.height - icon.side) / 2.0, icon.side, icon.side);
            start += icon.side + icon.gap;
            rect
        });
        let chevron = Rect::new(
            size.width - insets.right - CHEVRON_SLOT,
            0.0,
            CHEVRON_SLOT,
            size.height,
        );
        let label = Rect::new(
            start,
            0.0,
            (chevron.min_x() - ROW_PADDING - start).max(0.0),
            size.height,
        );
        (tile, label, chevron)
    }
}

/// `rect` of a node `width` wide, mirrored when the layout runs right to left.
fn mirrored(rect: Rect, width: f64, direction: LayoutDirection) -> Rect {
    match direction {
        LayoutDirection::Ltr => rect,
        LayoutDirection::Rtl => Rect::new(
            width - rect.max_x(),
            rect.min_y(),
            rect.width(),
            rect.height(),
        ),
    }
}

impl<M: Clone + 'static> Widget<M> for NavigationRow<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.tile.as_ref().map(|t| icon_row(t.size))
            != old.tile.as_ref().map(|t| icon_row(t.size))
        {
            Change::Layout
        } else if self.label != old.label
            || self.tile != old.tile
            || self.on_open.is_some() != old.on_open.is_some()
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(
                constraints.width.ideal(),
                row_height(self.tile.as_ref().map(|t| icon_row(t.size))),
            ),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let enabled = self.on_open.is_some();
        match press.event(event, enabled, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                if let Some(message) = &self.on_open {
                    cx.emit(message.clone());
                }
                cx.request_paint();
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        self.paint_in_group(press, cx, GroupCorners::default());
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.describe(press, cx);
    }
}

/// Which of a group's rounded corners a row meets: the first row the top ones, the last the bottom ones.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct GroupCorners {
    top: bool,
    bottom: bool,
}

/// A [`NavigationRow`] that knows where it stands in its group, so its press fill takes the group's corners.
struct GroupedNavigationRow<M> {
    row: NavigationRow<M>,
    corners: GroupCorners,
}

impl<M: Clone + 'static> Widget<M> for GroupedNavigationRow<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        Widget::<M>::layout(&self.row)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, press: &mut Press) -> Change {
        match self.row.update(&old.row, press) {
            Change::None if self.corners != old.corners => Change::Paint,
            change => change,
        }
    }

    fn measure(&self, press: &Press, constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        self.row.measure(press, constraints, cx)
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        self.row.event(press, event, cx)
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        self.row.paint_in_group(press, cx, self.corners);
    }

    fn semantics(&self, press: &Press, cx: &mut SemanticsCx<'_>) {
        self.row.describe(press, cx);
    }
}

/// The press fill of a row `whole` big: square where the row meets the next one, the group's radius where it meets
/// the group's edge.
fn paint_row_press(cx: &mut PaintCx<'_>, whole: Rect, corners: GroupCorners, color: Option<Color>) {
    let r = GROUP_RADIUS;
    let (top, bottom) = (corners.top, corners.bottom);
    if !top && !bottom {
        look::fill(cx, whole, 0.0, color);
        return;
    }
    // a rounded rectangle reaching a radius past each square edge, clipped to the row
    let y0 = if top {
        whole.min_y()
    } else {
        whole.min_y() - r
    };
    let y1 = if bottom {
        whole.max_y()
    } else {
        whole.max_y() + r
    };
    let shape = Rect::new(whole.min_x(), y0, whole.width(), y1 - y0);
    cx.push(PaintOp::PushClip(Shape::Rect(whole)));
    look::fill(cx, shape, r, color);
    cx.push(PaintOp::PopClip);
}

impl<M: Clone + 'static> NavigationRow<M> {
    fn paint_in_group(&self, press: &Press, cx: &mut PaintCx<'_>, corners: GroupCorners) {
        let env = cx.env().clone();
        let theme = env.theme.clone();
        let size = cx.size();
        let whole = Rect::new(0.0, 0.0, size.width, size.height);
        if press.pressed() {
            paint_row_press(cx, whole, corners, label_tint(&theme, PRESSED_ALPHA));
        }
        let (tile_area, label_area, chevron) = self.places(size);
        let rtl = env.direction == LayoutDirection::Rtl;
        if let (Some(tile), Some(area)) = (&self.tile, tile_area) {
            paint_tile(cx, tile, mirrored(area, size.width, env.direction));
        }
        let state = ControlState {
            enabled: self.on_open.is_some(),
            ..ControlState::default()
        };
        let text = if state.enabled {
            theme.colors.get(ColorRole::Text, StateVariant::NORMAL)
        } else {
            look::color(&theme, ColorRole::DisabledControlText, &state)
        };
        if let Some(layout) = look::label(&env, &self.label) {
            let clip = mirrored(label_area, size.width, env.direction);
            let x = if rtl {
                clip.max_x() - layout.size().width
            } else {
                clip.min_x()
            };
            let origin = Point::new(x, look::top(&layout, whole, &env));
            cx.push(PaintOp::PushClip(Shape::Rect(clip)));
            look::glyphs(cx, &layout, origin, text);
            cx.push(PaintOp::PopClip);
        }
        let chevron = mirrored(chevron, size.width, env.direction);
        let chevron_color = theme
            .colors
            .get(ColorRole::PlaceholderText, StateVariant::NORMAL);
        if let Some(color) = chevron_color {
            icon::symbol(cx, "symbol.chevron.right", CHEVRON, color, rtl, chevron);
        }
        paint_focus_ring(cx, whole.inset(Insets::uniform(2.0)), GROUP_RADIUS / 2.0);
    }

    fn describe(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        let enabled = self.on_open.is_some();
        cx.states(AccessibleStates {
            disabled: !enabled,
            ..AccessibleStates::default()
        });
        if enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

impl<M: 'static> From<AppIconRow<M>> for Element<M> {
    fn from(row: AppIconRow<M>) -> Self {
        let icon = icon_row(row.size);
        let height = if row.two_line {
            FORM_ICON_TWO_LINE_ROW_HEIGHT
        } else {
            icon.height
        };
        let children = vec![
            Element::new(Strut(height - 2.0 * ROW_PADDING_Y)),
            Element::new(AppIcon {
                name: row.icon,
                size: icon.side,
            }),
            Element::new(Gap(icon.gap)),
            row.content.fill(),
        ];
        Element::new(Row {
            icon: Some(icon),
            children,
        })
    }
}

impl<M: 'static> Widget<M> for AppIcon {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.size, self.size),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    /// Drawn at the device's pixels, centred in the node.
    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env();
        let scale = env.scale;
        let px = (self.size * scale).round().max(1.0) as u32;
        let Some(icon) = env.icons.named(&self.name, px) else {
            return;
        };
        let size = cx.size();
        let side = self.size.min(size.width).min(size.height);
        let dst = Rect::new(
            (size.width - side) / 2.0,
            (size.height - side) / 2.0,
            side,
            side,
        );
        cx.image(
            icon.id,
            &icon.image,
            dst,
            cupertino_ui_render::Sampling::Linear,
        );
    }

    /// Decorative: the row it stands in names it.
    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The tile of a compact pane header, and the space between it and the text; the header's padding.
const PANE_HEADER_TILE: f64 = 26.0;
const PANE_HEADER_GAP: f64 = 10.0;
const PANE_HEADER_PADDING: Insets = Insets {
    top: 12.0,
    left: 12.0,
    bottom: 12.0,
    right: 12.0,
};
/// A compact pane header's description, in points, and the space between the title and it.
const PANE_HEADER_DESCRIPTION: f32 = 11.0;
const PANE_HEADER_LINE_GAP: f64 = 2.0;

impl<M: 'static> From<PaneHeader<M>> for Element<M> {
    fn from(header: PaneHeader<M>) -> Self {
        let text = |content: &str, style: TextStyle, secondary: bool| {
            Element::new(Text {
                content: AttributedText::plain(content, style),
                role: TextRole::Body,
                wrap: true,
                secondary,
                selected: false,
            })
        };
        let body = TextStyle::new(13.0);
        let mut lines = vec![text(
            &header.title,
            TextStyle {
                weight: FontWeight(700),
                ..body
            },
            false,
        )];
        if !header.description.is_empty() {
            lines.push(text(
                &header.description,
                TextStyle::new(PANE_HEADER_DESCRIPTION),
                true,
            ));
        }
        let mut children = vec![
            Element::new(IconTile {
                size: PANE_HEADER_TILE,
                ..header.tile
            }),
            Element::new(Gap(PANE_HEADER_GAP)),
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: PANE_HEADER_LINE_GAP,
                align: Align::Stretch,
                children: lines,
            })
            .fill(),
        ];
        children.extend(header.switch.map(|s| {
            Element::new(Stack {
                axis: Axis::Horizontal,
                padding: Insets {
                    left: PANE_HEADER_GAP,
                    ..Insets::ZERO
                },
                spacing: 0.0,
                align: Align::Center,
                children: vec![s],
            })
        }));
        Element::new(Group {
            rows: vec![Element::new(HeaderRow {
                children,
                insets: PANE_HEADER_PADDING,
            })],
        })
    }
}

/// The compact header's tile, text and switch side by side, the tile and switch centred on the text's height.
struct HeaderRow<M> {
    children: Vec<Element<M>>,
    insets: Insets,
}

impl<M: 'static> Widget<M> for HeaderRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            insets: self.insets,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
                align: Align::Center,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The header's tile, title and description laid out for a card `width` wide.
struct HeaderLayout {
    title: Option<ParagraphLayout>,
    description: Option<ParagraphLayout>,
}

impl FormHeader {
    fn laid_out(&self, env: &Environment, width: f64) -> HeaderLayout {
        let room = (width - 2.0 * HEADER_SIDE).max(1.0) as f32;
        let centred = ParagraphStyle {
            align: TextAlign::Center,
            wrap: Wrap::Word,
            ..ParagraphStyle::default()
        };
        let body = env.theme.type_scale.body;
        let paragraph = |text: &str, style: TextStyle| {
            if text.is_empty() {
                return None;
            }
            let paragraph = env
                .text
                .paragraph(&AttributedText::plain(text, style), &centred)
                .ok()?;
            Some(paragraph.layout(Some(room)))
        };
        HeaderLayout {
            title: paragraph(
                &self.title,
                TextStyle {
                    weight: FontWeight(HEADER_TITLE_WEIGHT),
                    ..TextStyle::new(HEADER_TITLE_SIZE)
                },
            ),
            description: paragraph(
                &self.description,
                TextStyle {
                    weight: FontWeight(body.weight),
                    ..TextStyle::new(body.size)
                },
            ),
        }
    }

    fn height(layout: &HeaderLayout) -> f64 {
        let text = |l: &Option<ParagraphLayout>| l.as_ref().map_or(0.0, |l| l.size().height);
        HEADER_TOP
            + HEADER_TILE
            + HEADER_TILE_GAP
            + text(&layout.title)
            + text(&layout.description)
            + HEADER_BOTTOM
    }
}

impl<M: 'static> Widget<M> for FormHeader {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.title != old.title || self.description != old.description {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = constraints.width.max();
        let width = if width.is_finite() {
            width
        } else {
            constraints.width.ideal()
        };
        let layout = self.laid_out(cx.env(), width);
        Measured {
            size: Size::new(width, Self::height(&layout)),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let size = cx.size();
        paint_group(cx, Rect::new(0.0, 0.0, size.width, size.height));
        let tile = Rect::new(
            (size.width - HEADER_TILE) / 2.0,
            HEADER_TOP,
            HEADER_TILE,
            HEADER_TILE,
        );
        paint_tile(cx, &self.tile, tile);
        let layout = self.laid_out(&env, size.width);
        let mut y = tile.max_y() + HEADER_TILE_GAP;
        // the paragraphs centre their lines within the room between the sides
        let centred = |_: &ParagraphLayout, y: f64| Point::new(HEADER_SIDE, y);
        if let Some(title) = &layout.title {
            let color = env.theme.colors.get(ColorRole::Text, StateVariant::NORMAL);
            look::glyphs(cx, title, centred(title, y), color);
            y += title.size().height;
        }
        if let Some(description) = &layout.description {
            let color = env
                .theme
                .colors
                .get(ColorRole::SecondaryText, StateVariant::NORMAL);
            look::glyphs(cx, description, centred(description, y), color);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.title.clone());
        if !self.description.is_empty() {
            cx.description(self.description.clone());
        }
    }
}

/// The content, filling the node, over the text background.
struct Background<M> {
    content: Vec<Element<M>>,
}

/// The surface a form's pane asks the compositor to draw behind it, where it can.
pub(crate) const FORM_SURFACE: Surface = Surface::WindowBackground;
/// The colour a form's pane fills itself with where the compositor draws nothing behind it.
pub(crate) const FORM_ROLE: ColorRole = ColorRole::WindowBackground;

impl<M: 'static> From<FormBackground<M>> for Element<M> {
    fn from(background: FormBackground<M>) -> Self {
        Element::new(Background {
            content: vec![background.content.fill()],
        })
    }
}

impl<M: 'static> Widget<M> for Background<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.content)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        // where the desktop can, the compositor draws the colour, under the window's glass: a toolbar's capsules over
        // the form then show, which an opaque fill of the window's own would cover
        // the pane is the window's own background: the groups on it keep their own fill
        if cx.material(FORM_SURFACE, 0.0, 1.0) {
            return;
        }
        let theme = cx.env().theme.clone();
        let size = cx.size();
        let state = StateVariant {
            window_active: cx.window_active(),
            ..StateVariant::NORMAL
        };
        let color = theme.colors.get(FORM_ROLE, state);
        if let Some(color) = color {
            cx.push(PaintOp::Fill {
                shape: Shape::Rect(Rect::new(0.0, 0.0, size.width, size.height)),
                paint: Paint::Solid(color),
            });
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
