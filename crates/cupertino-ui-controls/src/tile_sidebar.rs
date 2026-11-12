//! Tile sidebar: an optional header (a search field) over a scroll view of row groups. The rows are one roving group,
//! keyed by their items' keys; the model keeps the selection, counted over every group, which keys and clicks report.

use crate::form::paint_tile;
use crate::look::{self, paint_focus_ring};
use crate::scroll;
use crate::{IconTile, Scroller, Stack, TileSidebar, TileSidebarItem};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Element, Event, EventCx, FocusTarget, Key,
    MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutDirection, LayoutSpec, Measured,
    SizeSpec,
};
use cupertino_ui_platform::{KeyEvent, Keysym, Mods, MouseButton, PointerEvent};
use cupertino_ui_render::{PaintOp, Sampling, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::rc::Rc;

const HOME: Keysym = Keysym(0xff50);
const UP: Keysym = Keysym(0xff52);
const DOWN: Keysym = Keysym(0xff54);
const END: Keysym = Keysym(0xff57);

/// A row's height; rows of a group touch.
pub const TILE_SIDEBAR_ROW_HEIGHT: f64 = 32.0;

/// The height of a row and the side of its tile at the sidebar icon size the person chose: the medium size is
/// [`TILE_SIDEBAR_ROW_HEIGHT`] and a 20 pt tile.
/// The height of the account row, and the side of its picture.
const ACCOUNT_ROW_HEIGHT: f64 = 64.0;
const ACCOUNT_PICTURE: f64 = 38.0;
/// The account picture's distance from the row's leading selection edge, and from the label.
const ACCOUNT_PICTURE_X: f64 = 5.5;
const ACCOUNT_LABEL_GAP: f64 = 9.0;
/// The size of the account's second line, and of the initials in the disc.
const ACCOUNT_SUBTITLE_SIZE: f32 = 11.0;
const ACCOUNT_INITIALS_SIZE: f32 = 16.0;
/// How much of the secondary label's strength the second line keeps on the selected row.
const SELECTED_SUBTITLE_ALPHA: f32 = 0.75;

fn row_metrics() -> (f64, f64) {
    use cupertino_ui_core::SidebarIcons;
    match cupertino_ui_core::personal().sidebar_icons {
        SidebarIcons::Small => (28.0, 16.0),
        SidebarIcons::Medium => (TILE_SIDEBAR_ROW_HEIGHT, 20.0),
        SidebarIcons::Large => (36.0, 24.0),
    }
}
/// Between two groups, and below the last.
const GROUP_GAP: f64 = 13.0;
/// Between the sidebar's leading and trailing edges and a row's selection.
const SELECTION_LEADING: f64 = 10.5;
const SELECTION_TRAILING: f64 = 9.5;
/// Between the sidebar's leading and trailing edges and the header.
const HEADER_LEADING: f64 = 10.0;
const HEADER_TRAILING: f64 = 9.0;
/// Above the header, and between the header and the first row.
const HEADER_TOP: f64 = 9.0;
const HEADER_GAP: f64 = 12.0;
/// The selection's corner radius.
const SELECTION_RADIUS: f64 = 7.5;
/// The tile: its side and its leading edge from the selection's.
const TILE_X: f64 = 9.0;
/// The label's leading edge from the selection's, and the room kept after it.
/// The space between a row's tile and its label.
const TILE_LABEL_GAP: f64 = 7.0;
const LABEL_TRAILING: f64 = 6.0;
/// The selected row's label weight.
const SELECTED_WEIGHT: u16 = 700;

type Select<M> = Option<Rc<crate::Handler<usize, M>>>;

impl<M: 'static> From<TileSidebar<M>> for Element<M> {
    fn from(sidebar: TileSidebar<M>) -> Self {
        let count = sidebar.groups.iter().map(Vec::len).sum();
        let selected = sidebar.selected.filter(|&s| s < count);
        let cursor = (count > 0).then(|| selected.unwrap_or(0));
        let on_select = sidebar.on_select.map(Rc::new);
        let mut keys = Vec::with_capacity(count);
        let groups = sidebar
            .groups
            .into_iter()
            .map(|group| {
                let rows = group
                    .into_iter()
                    .map(|item| {
                        let index = keys.len();
                        let key = item.key.clone();
                        keys.push(key.clone());
                        Element::new(TileRow {
                            index,
                            selected: selected == Some(index),
                            cursor: cursor == Some(index),
                            item,
                            on_select: on_select.clone(),
                        })
                        .key(key)
                    })
                    .collect();
                Element::new(column(Insets::ZERO, 0.0, rows))
            })
            .collect();
        let rows = Rows {
            label: sidebar.label,
            keys,
            selected,
            on_select,
            children: groups,
        };
        let list = scroll::frame(
            Element::new(rows),
            sidebar.scroll,
            (false, true),
            Scroller::Overlay,
            Rc::new(sidebar.on_scroll),
        );
        let mut children = Vec::new();
        if let Some(header) = sidebar.header {
            children.push(Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets {
                    top: HEADER_TOP,
                    left: HEADER_LEADING,
                    bottom: HEADER_GAP,
                    right: HEADER_TRAILING,
                },
                spacing: 0.0,
                align: Align::Stretch,
                children: vec![header],
            }));
        }
        children.push(list.fill());
        Element::new(Stack {
            axis: Axis::Vertical,
            padding: Insets::ZERO,
            spacing: 0.0,
            align: Align::Stretch,
            children,
        })
    }
}

fn select<M>(on_select: &Select<M>, index: usize, cx: &mut EventCx<'_, M>) {
    if let Some(on_select) = on_select {
        cx.emit(on_select(index));
    }
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

/// A vertical stack of `children` that fills the width, as a group of rows.
fn column<M>(insets: Insets, spacing: f64, children: Vec<Element<M>>) -> Stack<M> {
    Stack {
        axis: Axis::Vertical,
        padding: insets,
        spacing,
        align: Align::Stretch,
        children,
    }
}

/// The content of the sidebar's scroll view: the groups, and the keys that move the selection across them.
struct Rows<M> {
    label: String,
    keys: Vec<Key>,
    selected: Option<usize>,
    on_select: Select<M>,
    children: Vec<Element<M>>,
}

impl<M: 'static> Rows<M> {
    fn key(&self, key: &KeyEvent, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(last) = self.keys.len().checked_sub(1) else {
            return Propagation::Continue;
        };
        if [Mods::CTRL, Mods::ALT, Mods::PRIMARY, Mods::SHIFT]
            .iter()
            .any(|&m| key.mods.contains(m))
        {
            return Propagation::Continue;
        }
        let to = match (key.keysym, self.selected) {
            (UP, Some(s)) => s.saturating_sub(1),
            (DOWN, Some(s)) => (s + 1).min(last),
            (UP | DOWN | HOME, None) | (HOME, Some(_)) => 0,
            (END, _) => last,
            _ => return Propagation::Continue,
        };
        if self.selected != Some(to) {
            select(&self.on_select, to, cx);
        }
        cx.focus_item(FocusTarget::Key(self.keys[to].clone()));
        Propagation::Stop
    }
}

impl<M: 'static> Widget<M> for Rows<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: 0.0,
                left: 0.0,
                bottom: GROUP_GAP,
                right: 0.0,
            },
            width: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
                spacing: GROUP_GAP,
                align: Align::Stretch,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.keys.len() != old.keys.len() {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(key, _) if key.pressed && cx.phase() != Phase::Capture => self.key(key, cx),
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name(self.label.clone());
        cx.item_count(self.keys.len());
        cx.roving_group();
    }
}

/// One row: its selection inset from the sidebar's edges, its tile and its label, and the focus at the cursor.
struct TileRow<M> {
    index: usize,
    item: TileSidebarItem,
    selected: bool,
    /// The sidebar's one Tab stop.
    cursor: bool,
    on_select: Select<M>,
}

fn label(cx: &PaintCx<'_>, text: &str, selected: bool) -> Option<ParagraphLayout> {
    let env = cx.env();
    let body = env.theme.type_scale.body;
    let weight = if selected {
        SELECTED_WEIGHT
    } else {
        body.weight
    };
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(body.size)
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
}

/// `text` set at `size` and `weight`, in one line.
fn sized_label(cx: &PaintCx<'_>, text: &str, size: f32, weight: u16) -> Option<ParagraphLayout> {
    let style = TextStyle {
        weight: FontWeight(weight),
        ..TextStyle::new(size)
    };
    let paragraph = cx
        .env()
        .text
        .paragraph(
            &AttributedText::plain(text, style),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
}

impl<M: 'static> TileRow<M> {
    fn height(&self) -> f64 {
        if self.item.account.is_some() {
            ACCOUNT_ROW_HEIGHT
        } else {
            row_metrics().0
        }
    }

    /// The account row's picture or initials disc, then the name over the second line.
    fn paint_account(
        &self,
        cx: &mut PaintCx<'_>,
        chrome: Rect,
        rtl: bool,
        color: cupertino_ui_theme::Color,
    ) {
        let Some(account) = &self.item.account else {
            return;
        };
        let size = cx.size();
        let x = if rtl {
            chrome.max_x() - ACCOUNT_PICTURE_X - ACCOUNT_PICTURE
        } else {
            chrome.min_x() + ACCOUNT_PICTURE_X
        };
        let disc = Rect::new(
            x,
            (size.height - ACCOUNT_PICTURE) / 2.0,
            ACCOUNT_PICTURE,
            ACCOUNT_PICTURE,
        );
        let radius = ACCOUNT_PICTURE / 2.0;
        match &account.picture {
            Some(picture) => {
                cx.push(PaintOp::PushClip(Shape::RoundedRect { rect: disc, radius }));
                let (w, h) = (
                    f64::from(picture.image.width),
                    f64::from(picture.image.height),
                );
                if w > 0.0 && h > 0.0 {
                    let scale = (disc.width() / w).max(disc.height() / h);
                    let dst = Rect::new(
                        disc.min_x() + (disc.width() - w * scale) / 2.0,
                        disc.min_y() + (disc.height() - h * scale) / 2.0,
                        w * scale,
                        h * scale,
                    );
                    cx.image(picture.id, &picture.image, dst, Sampling::Linear);
                }
                cx.push(PaintOp::PopClip);
            }
            None => {
                let (top, _) = crate::form::tile_shades(self.item.tile);
                look::fill(cx, disc, radius, Some(crate::form::rgb(top)));
                let white = cupertino_ui_theme::Color::new(1.0, 1.0, 1.0, 1.0);
                if let Some(layout) = sized_label(cx, &account.initials, ACCOUNT_INITIALS_SIZE, 600)
                {
                    let origin = Point::new(
                        disc.min_x() + (disc.width() - layout.size().width) / 2.0,
                        look::top(&layout, disc, cx.env()),
                    );
                    look::glyphs(cx, &layout, origin, Some(white));
                }
            }
        }
        let text_x = ACCOUNT_PICTURE_X + ACCOUNT_PICTURE + ACCOUNT_LABEL_GAP;
        let room = (chrome.width() - text_x - LABEL_TRAILING).max(0.0);
        let clip = Rect::new(
            if rtl {
                chrome.max_x() - text_x - room
            } else {
                chrome.min_x() + text_x
            },
            0.0,
            room,
            size.height,
        );
        let body = cx.env().theme.type_scale.body;
        let name = sized_label(cx, &self.item.label, body.size, SELECTED_WEIGHT);
        let line = sized_label(cx, &account.subtitle, ACCOUNT_SUBTITLE_SIZE, body.weight);
        let heights = name.as_ref().map_or(0.0, |l| l.size().height)
            + line.as_ref().map_or(0.0, |l| l.size().height);
        let mut y = (size.height - heights) / 2.0;
        cx.push(PaintOp::PushClip(Shape::Rect(clip)));
        for (layout, shade) in [
            (name, color),
            (
                line,
                if self.selected {
                    color.with_alpha(color.a * SELECTED_SUBTITLE_ALPHA)
                } else {
                    cx.env()
                        .theme
                        .colors
                        .get(ColorRole::SecondaryText, StateVariant::NORMAL)
                        .unwrap_or(color)
                },
            ),
        ] {
            let Some(layout) = layout else { continue };
            let h = layout.size().height;
            let x = if rtl {
                clip.max_x() - layout.size().width
            } else {
                clip.min_x()
            };
            look::glyphs(cx, &layout, Point::new(x, y), Some(shade));
            y += h;
        }
        cx.push(PaintOp::PopClip);
    }
}

impl<M: 'static> Widget<M> for TileRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(self.height()),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (self.index, &self.item, self.selected, self.cursor)
            != (old.index, &old.item, old.selected, old.cursor)
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(0.0, self.height()),
            baseline: None,
        }
    }

    /// A press focuses the row without showing the ring and selects it. A press on the row already selected selects it
    /// again, so that a window showing a page below that row's page can go back to it.
    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture => {
                cx.request_focus();
                select(&self.on_select, self.index, cx);
                Propagation::Stop
            }
            Event::Action(request)
                if cx.node() == cx.target() && request.action == Action::Click =>
            {
                select(&self.on_select, self.index, cx);
                Propagation::Stop
            }
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let chrome = Rect::new(
            if rtl {
                SELECTION_TRAILING
            } else {
                SELECTION_LEADING
            },
            0.0,
            (size.width - SELECTION_LEADING - SELECTION_TRAILING).max(0.0),
            size.height,
        );
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: true,
            selected: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let active = cx.window_active();
        // the selection is the accent in an active window and the sidebar's faint neutral selection, laid over its
        // material, in one that is not
        if self.selected {
            let role = if active {
                ColorRole::ControlFill
            } else {
                ColorRole::SidebarSelection
            };
            look::fill(
                cx,
                chrome,
                SELECTION_RADIUS,
                look::color(&theme, role, &state),
            );
        }
        let leading = |offset: f64, width: f64| {
            if rtl {
                chrome.max_x() - offset - width
            } else {
                chrome.min_x() + offset
            }
        };
        // the label's colour first: the account row draws its own picture and lines with it
        let color = if !active {
            theme
                .colors
                .get(ColorRole::InactiveSidebarText, StateVariant::NORMAL)
        } else if self.selected {
            look::color(&theme, ColorRole::ControlText, &state)
        } else {
            theme
                .colors
                .get(ColorRole::Text, StateVariant::NORMAL)
                .map(|c| c.with_alpha(1.0))
        };
        if self.item.account.is_some() {
            if let Some(color) = color {
                self.paint_account(cx, chrome, rtl, color);
            }
            if cx.is_focused() {
                paint_focus_ring(cx, chrome, SELECTION_RADIUS);
            }
            return;
        }
        let tile_size = row_metrics().1;
        let label_x = TILE_X + tile_size + TILE_LABEL_GAP;
        let tile = IconTile {
            symbol: self.item.symbol.clone(),
            color: self.item.tile,
            size: tile_size,
        };
        let area = Rect::new(
            leading(TILE_X, tile_size),
            (size.height - tile_size) / 2.0,
            tile_size,
            tile_size,
        );
        paint_tile(cx, &tile, area);
        if let Some(layout) = label(cx, &self.item.label, self.selected) {
            let room = (chrome.width() - label_x - LABEL_TRAILING).max(0.0);
            let clip = Rect::new(leading(label_x, room), 0.0, room, size.height);
            let x = if rtl {
                clip.max_x() - layout.size().width
            } else {
                clip.min_x()
            };
            let origin = Point::new(x, look::top(&layout, chrome, cx.env()));
            cx.push(PaintOp::PushClip(Shape::Rect(clip)));
            look::glyphs(cx, &layout, origin, color);
            cx.push(PaintOp::PopClip);
        }
        if cx.is_focused() {
            paint_focus_ring(cx, chrome, SELECTION_RADIUS);
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::ListItem);
        cx.name(self.item.label.clone());
        cx.states(AccessibleStates {
            selected: self.selected,
            ..AccessibleStates::default()
        });
        cx.item_index(self.index);
        cx.action(Action::Click);
        cx.roving_item();
        if self.cursor {
            cx.focusable();
        }
    }
}
