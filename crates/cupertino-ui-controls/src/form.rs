//! A settings form's look: icon tiles, groups of rows on a rounded fill with hairlines between them, the rows, the
//! navigation rows that open a page, the header card at the top of a page and the colour behind it all.

use crate::icon;
use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{FormBackground, FormGroup, FormHeader, FormRow, IconTile, NavigationRow, TileColor};
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

/// A row's least height, and with an icon tile.
pub const FORM_ROW_HEIGHT: f64 = 36.5;
pub const FORM_TILE_ROW_HEIGHT: f64 = 48.0;
/// A row's padding: above and below its content (a control padded out to the least pointer target, as a pop-up
/// button is, still leaves the row at the least height), at its edges, and at the leading edge before a tile.
const ROW_PADDING_Y: f64 = 6.0;
const ROW_PADDING: f64 = 10.0;
const TILE_ROW_LEADING: f64 = 12.0;
/// The tile of a row, and the space between it and the content.
const ROW_TILE: f64 = 24.0;
const ROW_TILE_GAP: f64 = 12.0;
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
fn tile_shades(color: TileColor) -> ([u8; 3], [u8; 3]) {
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

fn rgb(c: [u8; 3]) -> Color {
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

impl<M: 'static> From<FormGroup<M>> for Element<M> {
    fn from(group: FormGroup<M>) -> Self {
        let mut rows = Vec::with_capacity(group.rows.len() * 2);
        for (i, row) in group.rows.into_iter().enumerate() {
            if i > 0 {
                rows.push(Element::new(Hairline));
            }
            rows.push(row);
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

/// The padding of a row: a tile row starts further in.
fn row_insets(tiled: bool) -> Insets {
    Insets {
        top: ROW_PADDING_Y,
        left: if tiled { TILE_ROW_LEADING } else { ROW_PADDING },
        bottom: ROW_PADDING_Y,
        right: ROW_PADDING,
    }
}

fn row_height(tiled: bool) -> f64 {
    if tiled {
        FORM_TILE_ROW_HEIGHT
    } else {
        FORM_ROW_HEIGHT
    }
}

/// A row's tile, content and strut side by side; the content takes what the others leave.
struct Row<M> {
    tiled: bool,
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
        let tiled = row.tile.is_some();
        let mut children = vec![Element::new(Strut(row_height(tiled) - 2.0 * ROW_PADDING_Y))];
        if let Some(tile) = row.tile {
            children.push(Element::new(IconTile {
                size: ROW_TILE,
                ..tile
            }));
            children.push(Element::new(Gap(ROW_TILE_GAP)));
        }
        children.push(row.content.fill());
        Element::new(Row { tiled, children })
    }
}

impl<M: 'static> Widget<M> for Row<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            insets: row_insets(self.tiled),
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
        if self.tiled != old.tiled {
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
        let insets = row_insets(self.tile.is_some());
        let mut start = insets.left;
        let tile = self.tile.as_ref().map(|_| {
            let rect = Rect::new(start, (size.height - ROW_TILE) / 2.0, ROW_TILE, ROW_TILE);
            start += ROW_TILE + ROW_TILE_GAP;
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
        if self.tile.is_some() != old.tile.is_some() {
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
            size: Size::new(constraints.width.ideal(), row_height(self.tile.is_some())),
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
        let env = cx.env().clone();
        let theme = env.theme.clone();
        let size = cx.size();
        let whole = Rect::new(0.0, 0.0, size.width, size.height);
        if press.pressed() {
            look::fill(cx, whole, 0.0, label_tint(&theme, PRESSED_ALPHA));
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

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
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
