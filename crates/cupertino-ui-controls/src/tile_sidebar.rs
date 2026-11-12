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
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphLayout, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::rc::Rc;

const HOME: Keysym = Keysym(0xff50);
const UP: Keysym = Keysym(0xff52);
const DOWN: Keysym = Keysym(0xff54);
const END: Keysym = Keysym(0xff57);

/// A row's height; rows of a group touch.
pub const TILE_SIDEBAR_ROW_HEIGHT: f64 = 32.0;
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
const TILE: f64 = 20.0;
const TILE_X: f64 = 9.0;
/// The label's leading edge from the selection's, and the room kept after it.
const LABEL_X: f64 = 36.0;
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

impl<M: 'static> Widget<M> for TileRow<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(TILE_SIDEBAR_ROW_HEIGHT),
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
            size: Size::new(0.0, TILE_SIDEBAR_ROW_HEIGHT),
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
        let tile = IconTile {
            symbol: self.item.symbol.clone(),
            color: self.item.tile,
            size: TILE,
        };
        let area = Rect::new(
            leading(TILE_X, TILE),
            (size.height - TILE) / 2.0,
            TILE,
            TILE,
        );
        paint_tile(cx, &tile, area);
        // the label at full strength: the sidebar's labels stand out more than the window's text; in a window that is
        // not active every label, the selected row's too, is the inactive sidebar text (the tiles keep their colours)
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
        if let Some(layout) = label(cx, &self.item.label, self.selected) {
            let room = (chrome.width() - LABEL_X - LABEL_TRAILING).max(0.0);
            let clip = Rect::new(leading(LABEL_X, room), 0.0, room, size.height);
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
