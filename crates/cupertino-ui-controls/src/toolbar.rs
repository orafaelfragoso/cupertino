//! The floating toolbar controls: glass capsules of buttons ([`ToolbarGroup`]), a menu button with a chevron
//! ([`ToolbarMenuButton`]), a circular button ([`ToolbarButton`]), a search field that rests as a circular button
//! ([`ToolbarSearch`]) and the large title ([`ToolbarTitle`]).
//!
//! Every control is 36 pt tall and lies in the middle of the 52 pt toolbar row. A group's segments are 36.5 pt wide, a
//! choice's 40 pt, and the chosen segment shows a pill 4 pt inside it whose corner radius is the group's less that
//! inset ([`nested_radius`]). Thin dividers 20 pt tall separate the segments of a group that asks for them, except
//! beside the chosen one. Each control's glass is the [`GlassRole::ToolbarGroup`] role.

use crate::glass::{nested_radius, paint_glass, GlassRole};
use crate::icon::symbol;
use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, AccessibleValue, Action, Change, Checked, ControlState, Element, Event,
    EventCx, Key, MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget,
};
use cupertino_ui_layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutDirection, LayoutSpec, Measured,
    SizeSpec,
};
use cupertino_ui_platform::{MouseButton, PointerEvent};
use cupertino_ui_render::{Paint, PaintOp, PathVerb, Shape};
use cupertino_ui_text::{AttributedText, FontWeight, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{Color, ColorRole};

/// The toolbar row's height: the controls lie in its middle.
pub const TOOLBAR_ROW: f64 = 52.0;
/// The compact toolbar row's height.
pub const COMPACT_TOOLBAR_ROW: f64 = 40.0;

/// How far the chrome covers the top of a window's content: the unobscured rectangle is the content less these insets.
/// A window whose content fills it ([`DecorationMode::content_fills_window`]) draws its toolbar row over its top, 52 pt
/// ([`TOOLBAR_ROW`]) or 40 pt compact ([`COMPACT_TOOLBAR_ROW`]); the other modes put the title bar above the content and
/// cover none of it.
pub fn unobscured_insets(mode: cupertino_ui_platform::DecorationMode) -> Insets {
    use cupertino_ui_platform::DecorationMode;
    let top = match mode {
        DecorationMode::Unified | DecorationMode::Sidebar => TOOLBAR_ROW,
        DecorationMode::UnifiedCompact => COMPACT_TOOLBAR_ROW,
        DecorationMode::Preferred | DecorationMode::Client | DecorationMode::None => 0.0,
    };
    Insets {
        top,
        left: 0.0,
        bottom: 0.0,
        right: 0.0,
    }
}
/// The room between the toolbar's controls and the edges of the pane it floats over.
pub const TOOLBAR_EDGE: f64 = 8.0;
/// The room between neighbouring toolbar controls.
pub const TOOLBAR_GAP: f64 = 8.0;
/// The room before the large title, from the control before it.
pub const TOOLBAR_TITLE_GAP: f64 = 12.0;
/// A toolbar control's height, and the side of a circular button.
pub const TOOLBAR_CONTROL: f64 = 36.0;
/// A button segment's width in a group.
pub const TOOLBAR_SEGMENT: f64 = 36.5;
/// A choice segment's width.
pub const TOOLBAR_CHOICE_SEGMENT: f64 = 40.0;
/// How far inside its segment the chosen segment's pill lies.
pub const TOOLBAR_CHOICE_INSET: f64 = 4.0;
/// A divider's length; it is one device pixel wide.
pub const TOOLBAR_DIVIDER: f64 = 20.0;
/// A menu button's width: its symbol and its chevron between 8 pt margins.
pub const TOOLBAR_MENU_BUTTON: f64 = 47.0;
/// The side of a toolbar symbol.
pub const TOOLBAR_SYMBOL: f64 = 18.0;
/// The large title's type.
pub const TOOLBAR_TITLE_SIZE: f32 = 15.0;
pub const TOOLBAR_TITLE_WEIGHT: u16 = 700;

/// The back and forward chevrons: their width, height and stroke.
const CHEVRON: Size = Size {
    width: 8.0,
    height: 14.0,
};
const CHEVRON_STROKE: f64 = 1.75;
/// A menu button's chevron, after its symbol: width, height and stroke, and the gap before it.
const MENU_CHEVRON: Size = Size {
    width: 7.0,
    height: 4.0,
};
const MENU_CHEVRON_STROKE: f64 = 1.5;
const MENU_CHEVRON_GAP: f64 = 6.0;
/// The room between the two chevrons of a picker.
const PICKER_CHEVRON_GAP: f64 = 2.0;
const MENU_MARGIN: f64 = 8.0;
/// The "more" glyph: three dots this wide, this far apart.
/// The resting search field's symbol.
const SEARCH_SYMBOL: &str = "symbol.magnifyingglass";
const DOT: f64 = 3.25;
const DOT_PITCH: f64 = 6.25;

/// What a toolbar button shows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum ToolbarGlyph {
    /// A system symbol.
    Symbol(String),
    /// The back chevron, pointing against the reading direction.
    Back,
    /// The forward chevron, pointing along the reading direction.
    Forward,
    /// Three dots: more actions.
    More,
    /// A tag outline pointing up and away from the reading direction's start, with its hole: the item's tags.
    Tag,
}

/// One button of a [`ToolbarGroup`].
pub struct ToolbarItem<M> {
    /// The node's key, which a menu anchored below the button names.
    pub key: Option<Key>,
    pub glyph: ToolbarGlyph,
    /// What assistive technology calls it.
    pub label: String,
    /// Sent when it is pressed; `None` disables it.
    pub on_press: Option<M>,
    /// The button opens a menu: it acts on the press, reads as a pop-up button, and the menu is open while this is
    /// `Some(true)`.
    pub menu: Option<bool>,
}

/// A floating glass capsule of buttons. With `selected` it is a choice: its segments are 40 pt wide, the chosen one
/// shows a pill, and it reads as a radio group. Build its element with `Element::from`.
pub struct ToolbarGroup<M> {
    /// What assistive technology calls the group.
    pub label: String,
    pub items: Vec<ToolbarItem<M>>,
    pub selected: Option<usize>,
    /// Thin dividers between the segments (none beside the chosen one).
    pub dividers: bool,
}

/// A floating glass capsule that opens a menu: a symbol and a chevron. Build its element with `Element::from`.
pub struct ToolbarMenuButton<M> {
    pub key: Option<Key>,
    pub symbol: String,
    pub label: String,
    /// Sent on the press; `None` disables it.
    pub on_press: Option<M>,
    /// Its menu is open.
    pub expanded: bool,
}

/// A floating glass capsule that opens a menu to pick one of several, as the view's mode: the pick's symbol and a pair
/// of chevrons, up over down. Build its element with `Element::from`.
pub struct ToolbarPicker<M> {
    pub key: Option<Key>,
    /// The symbol of what is picked now.
    pub symbol: String,
    pub label: String,
    /// Sent on the press; `None` disables it.
    pub on_press: Option<M>,
    /// Its menu is open.
    pub expanded: bool,
}

/// A floating circular glass button, as the search button. Build its element with `Element::from`.
pub struct ToolbarButton<M> {
    pub key: Option<Key>,
    pub glyph: ToolbarGlyph,
    pub label: String,
    /// Sent when it is pressed; `None` disables it.
    pub on_press: Option<M>,
}

/// A search field that rests as a circular glass button with a magnifying glass: while `collapsed` the button covers
/// the field, which still takes the presses, so a click on it gives the field the keyboard. The application opens it
/// (clears `collapsed`) while the field has the keyboard or holds text, and gives it its width: [`TOOLBAR_CONTROL`]
/// at rest. Build its element with `Element::from`.
pub struct ToolbarSearch<M> {
    /// The field: a [`crate::TextField`].
    pub field: Element<M>,
    pub collapsed: bool,
}

/// The large bold title after the back and forward buttons. It takes the room the controls leave, its text ending in
/// an ellipsis when it does not fit. Build its element with `Element::new`.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolbarTitle {
    pub text: String,
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn keyed<M: 'static>(element: Element<M>, key: Option<Key>) -> Element<M> {
    match key {
        Some(key) => element.key(key),
        None => element,
    }
}

impl<M: Clone + 'static> From<ToolbarGroup<M>> for Element<M> {
    fn from(group: ToolbarGroup<M>) -> Self {
        let choice = group.selected.is_some();
        let width = if choice {
            TOOLBAR_CHOICE_SEGMENT
        } else {
            TOOLBAR_SEGMENT
        };
        let enabled = group.items.iter().map(|i| i.on_press.is_some()).collect();
        let count = group.items.len();
        let children = group
            .items
            .into_iter()
            .enumerate()
            .map(|(i, item)| {
                let key = item.key.clone();
                keyed(
                    Element::new(Segment {
                        glyph: item.glyph,
                        label: item.label,
                        message: item.on_press,
                        menu: item.menu,
                        width,
                        chosen: choice.then_some(group.selected == Some(i)),
                        picker: false,
                    }),
                    key,
                )
            })
            .collect();
        Element::new(Capsule {
            label: group.label,
            width: width * count as f64,
            selected: group.selected,
            segment: width,
            dividers: group.dividers,
            enabled,
            children,
        })
    }
}

impl<M: Clone + 'static> From<ToolbarMenuButton<M>> for Element<M> {
    fn from(button: ToolbarMenuButton<M>) -> Self {
        let key = button.key.clone();
        let segment = Element::new(Segment {
            glyph: ToolbarGlyph::Symbol(button.symbol),
            label: button.label,
            message: button.on_press,
            menu: Some(button.expanded),
            width: TOOLBAR_MENU_BUTTON,
            chosen: None,
            picker: false,
        });
        Element::new(Capsule {
            label: String::new(),
            width: TOOLBAR_MENU_BUTTON,
            selected: None,
            segment: TOOLBAR_MENU_BUTTON,
            dividers: false,
            enabled: Vec::new(),
            children: vec![keyed(segment, key)],
        })
    }
}

impl<M: Clone + 'static> From<ToolbarPicker<M>> for Element<M> {
    fn from(picker: ToolbarPicker<M>) -> Self {
        let key = picker.key.clone();
        let segment = Element::new(Segment {
            glyph: ToolbarGlyph::Symbol(picker.symbol),
            label: picker.label,
            message: picker.on_press,
            menu: Some(picker.expanded),
            width: TOOLBAR_MENU_BUTTON,
            chosen: None,
            picker: true,
        });
        Element::new(Capsule {
            label: String::new(),
            width: TOOLBAR_MENU_BUTTON,
            selected: None,
            segment: TOOLBAR_MENU_BUTTON,
            dividers: false,
            enabled: Vec::new(),
            children: vec![keyed(segment, key)],
        })
    }
}

impl<M: Clone + 'static> From<ToolbarButton<M>> for Element<M> {
    fn from(button: ToolbarButton<M>) -> Self {
        let key = button.key.clone();
        let segment = Element::new(Segment {
            glyph: button.glyph,
            label: button.label,
            message: button.on_press,
            menu: None,
            width: TOOLBAR_CONTROL,
            chosen: None,
            picker: false,
        });
        Element::new(Capsule {
            label: String::new(),
            width: TOOLBAR_CONTROL,
            selected: None,
            segment: TOOLBAR_CONTROL,
            dividers: false,
            enabled: Vec::new(),
            children: vec![keyed(segment, key)],
        })
    }
}

impl<M: 'static> From<ToolbarSearch<M>> for Element<M> {
    fn from(search: ToolbarSearch<M>) -> Self {
        Element::new(SearchHolder {
            children: vec![
                Element::new(SearchField {
                    collapsed: search.collapsed,
                    children: vec![search.field],
                }),
                Element::new(SearchCover {
                    collapsed: search.collapsed,
                }),
            ],
        })
    }
}

/// The field and the button over it, as tall as a toolbar control.
struct SearchHolder<M> {
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for SearchHolder<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(TOOLBAR_CONTROL),
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), TOOLBAR_CONTROL),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The field under the search button: unseen while the button covers it, so the button's glass shows what lies below
/// the field and not the field; it still takes the presses.
struct SearchField<M> {
    collapsed: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for SearchField<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.collapsed != old.collapsed {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(constraints.width.ideal(), TOOLBAR_CONTROL),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if self.collapsed {
            cx.set_group(0.0, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// The resting search button: glass and a magnifying glass over the field, which the pointer passes through.
struct SearchCover {
    collapsed: bool,
}

impl<M: 'static> Widget<M> for SearchCover {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.collapsed != old.collapsed {
            Change::Paint
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

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        if !self.collapsed {
            return;
        }
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = size.height.min(size.width) / 2.0;
        // the field under the button is unseen meanwhile ([`SearchField`])
        paint_glass(cx, GlassRole::ToolbarGroup, rect, radius);
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let color = glyph_color(&cx.env().theme.clone(), &state);
        symbol(cx, SEARCH_SYMBOL, TOOLBAR_SYMBOL, color, false, rect);
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}

    fn hit(&self, _: &(), _: Point) -> bool {
        false
    }
}

/// The glass under a group's segments, the chosen segment's pill and the dividers.
struct Capsule<M> {
    /// Empty for a single control, which needs no group of its own.
    label: String,
    width: f64,
    selected: Option<usize>,
    segment: f64,
    dividers: bool,
    /// Which segments are enabled, for the dividers.
    enabled: Vec<bool>,
    children: Vec<Element<M>>,
}

impl<M> Capsule<M> {
    fn radius() -> f64 {
        TOOLBAR_CONTROL / 2.0
    }

    /// The x of each divider: between two segments, unless one of them is chosen.
    fn divider_positions(&self) -> Vec<f64> {
        if !self.dividers {
            return Vec::new();
        }
        let count = self.enabled.len();
        (1..count)
            .filter(|&i| self.selected != Some(i) && self.selected != Some(i - 1))
            .map(|i| self.segment * i as f64)
            .collect()
    }
}

impl<M: 'static> Widget<M> for Capsule<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            height: SizeSpec::Fixed(TOOLBAR_CONTROL),
            flex: Flex::NONE,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: 0.0,
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
        if self.width != old.width || self.segment != old.segment {
            Change::Layout
        } else if self.selected != old.selected
            || self.dividers != old.dividers
            || self.enabled != old.enabled
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.width, TOOLBAR_CONTROL),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let radius = Self::radius();
        paint_glass(cx, GlassRole::ToolbarGroup, rect, radius);
        let theme = cx.env().theme.clone();
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let mirror = |x: f64, w: f64| if rtl { size.width - x - w } else { x };
        if let Some(i) = self.selected {
            let pill = Rect::new(
                mirror(self.segment * i as f64, self.segment),
                0.0,
                self.segment,
                size.height,
            )
            .inset(Insets::uniform(TOOLBAR_CHOICE_INSET));
            look::fill(
                cx,
                pill,
                nested_radius(radius, TOOLBAR_CHOICE_INSET),
                Some(pill_color(&theme, true)),
            );
        }
        let px = 1.0 / cx.env().scale.max(1.0);
        let color = theme
            .colors
            .get(
                ColorRole::Separator,
                cupertino_ui_theme::StateVariant::NORMAL,
            )
            .map(|c| divider_color(&theme, c));
        for x in self.divider_positions() {
            let x = mirror(x, 0.0);
            look::fill(
                cx,
                Rect::new(
                    x - px / 2.0,
                    (size.height - TOOLBAR_DIVIDER) / 2.0,
                    px,
                    TOOLBAR_DIVIDER,
                ),
                0.0,
                color,
            );
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.label.is_empty() {
            return;
        }
        cx.role(if self.selected.is_some() {
            Role::RadioGroup
        } else {
            Role::Group
        });
        cx.name(self.label.clone());
    }
}

/// The colour of a chosen segment's pill, or of a pressed one's.
fn pill_color(theme: &cupertino_ui_theme::ThemeTokens, chosen: bool) -> Color {
    let dark = theme.preferences.appearance.is_dark();
    match (dark, chosen) {
        (true, true) => Color::new(1.0, 1.0, 1.0, 0.118),
        (true, false) => Color::new(1.0, 1.0, 1.0, 0.08),
        (false, true) => Color::new(0.0, 0.0, 0.0, 0.08),
        (false, false) => Color::new(0.0, 0.0, 0.0, 0.05),
    }
}

/// A divider of the light appearance: a share of black, so it reads as strong over the glass's light face as the
/// disabled glyphs beside it.
const DIVIDER_LIGHT: f32 = 0.4;

/// A divider is drawn as strong as the disabled glyphs: a share of white over the dark glass, of black over the light
/// one; with Increase Contrast the theme's separator, made a little stronger over glass.
fn divider_color(theme: &cupertino_ui_theme::ThemeTokens, separator: Color) -> Color {
    let appearance = theme.preferences.appearance;
    if appearance.is_dark() {
        Color::new(1.0, 1.0, 1.0, 0.25)
    } else if appearance.is_high_contrast() {
        Color {
            a: (separator.a * 1.5).min(1.0),
            ..separator
        }
    } else {
        Color::new(0.0, 0.0, 0.0, DIVIDER_LIGHT)
    }
}

/// One button of a capsule.
struct Segment<M> {
    glyph: ToolbarGlyph,
    label: String,
    message: Option<M>,
    menu: Option<bool>,
    width: f64,
    /// In a choice: whether it is the chosen one.
    chosen: Option<bool>,
    /// A menu button's chevron is a pair, up over down, for a button that picks one of several: the pick's own symbol
    /// shows.
    picker: bool,
}

impl<M> Segment<M> {
    fn state(&self, press: &Press, cx: &PaintCx<'_>) -> ControlState {
        ControlState {
            enabled: self.message.is_some(),
            hovered: press.hovered(),
            pressed: press.pressed(),
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            selected: self.chosen == Some(true),
            ..ControlState::default()
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Segment<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            height: SizeSpec::Fixed(TOOLBAR_CONTROL),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.width != old.width {
            Change::Layout
        } else if self.message.is_some() != old.message.is_some()
            || self.glyph != old.glyph
            || self.menu != old.menu
            || self.chosen != old.chosen
            || self.picker != old.picker
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(self.width, TOOLBAR_CONTROL),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let enabled = self.message.is_some();
        // a menu opens on the press, as a pop-up button's does
        if self.menu.is_some() && enabled && cx.phase() != Phase::Capture {
            if let Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) = event
            {
                if let Some(message) = &self.message {
                    cx.emit(message.clone());
                }
                cx.request_paint();
                return Propagation::Stop;
            }
        }
        match press.event(event, enabled, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                // a menu opened on the press already
                let pointer = matches!(event, Event::Pointer(..));
                if !(self.menu.is_some() && pointer) {
                    if let Some(message) = &self.message {
                        cx.emit(message.clone());
                    }
                }
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let state = self.state(press, cx);
        let theme = cx.env().theme.clone();
        let size = cx.size();
        let area = Rect::new(0.0, 0.0, size.width, size.height);
        let pill = area.inset(Insets::uniform(TOOLBAR_CHOICE_INSET));
        let radius = nested_radius(TOOLBAR_CONTROL / 2.0, TOOLBAR_CHOICE_INSET);
        if state.enabled && (state.pressed || self.menu == Some(true)) && !state.selected {
            look::fill(cx, pill, radius, Some(pill_color(&theme, false)));
        }
        let color = glyph_color(&theme, &state);
        let direction = cx.env().direction;
        match &self.glyph {
            ToolbarGlyph::Symbol(name) => {
                // a menu button's symbol sits before its chevron, the pair centred
                let area = if self.menu.is_some() && self.width >= TOOLBAR_MENU_BUTTON {
                    let rtl = direction == LayoutDirection::Rtl;
                    let x = if rtl {
                        size.width - MENU_MARGIN - TOOLBAR_SYMBOL
                    } else {
                        MENU_MARGIN
                    };
                    Rect::new(x, 0.0, TOOLBAR_SYMBOL, size.height)
                } else {
                    area
                };
                symbol(cx, name, TOOLBAR_SYMBOL, color, false, area);
            }
            ToolbarGlyph::Back | ToolbarGlyph::Forward => {
                let forward =
                    (self.glyph == ToolbarGlyph::Forward) == (direction == LayoutDirection::Ltr);
                cx.push(PaintOp::Stroke {
                    shape: chevron(area, forward),
                    width: CHEVRON_STROKE,
                    paint: Paint::Solid(color),
                });
            }
            ToolbarGlyph::Tag => {
                let rtl = direction == LayoutDirection::Rtl;
                let (outline, hole) = tag(area, rtl);
                cx.push(PaintOp::Stroke {
                    shape: outline,
                    width: TAG_STROKE,
                    paint: Paint::Solid(color),
                });
                look::fill(
                    cx,
                    Rect::new(
                        hole.x - TAG_HOLE / 2.0,
                        hole.y - TAG_HOLE / 2.0,
                        TAG_HOLE,
                        TAG_HOLE,
                    ),
                    TAG_HOLE / 2.0,
                    Some(color),
                );
            }
            ToolbarGlyph::More => {
                let centre = Point::new(area.width() / 2.0, area.height() / 2.0);
                for i in [-1.0, 0.0, 1.0] {
                    let dot = Rect::new(
                        centre.x + i * DOT_PITCH - DOT / 2.0,
                        centre.y - DOT / 2.0,
                        DOT,
                        DOT,
                    );
                    look::fill(cx, dot, DOT / 2.0, Some(color));
                }
            }
        }
        if self.menu.is_some() && self.width >= TOOLBAR_MENU_BUTTON {
            let rtl = direction == LayoutDirection::Rtl;
            let x = MENU_MARGIN + TOOLBAR_SYMBOL + MENU_CHEVRON_GAP;
            let x = if rtl {
                size.width - x - MENU_CHEVRON.width
            } else {
                x
            };
            let y = (size.height - MENU_CHEVRON.height) / 2.0;
            let mut path = |y: f64, down: bool| {
                let (near, far) = if down {
                    (y, y + MENU_CHEVRON.height)
                } else {
                    (y + MENU_CHEVRON.height, y)
                };
                cx.push(PaintOp::Stroke {
                    shape: Shape::Path {
                        verbs: vec![
                            PathVerb::MoveTo(Point::new(x, near)),
                            PathVerb::LineTo(Point::new(x + MENU_CHEVRON.width / 2.0, far)),
                            PathVerb::LineTo(Point::new(x + MENU_CHEVRON.width, near)),
                        ],
                        fill_rule: Default::default(),
                    },
                    width: MENU_CHEVRON_STROKE,
                    paint: Paint::Solid(color),
                });
            };
            if self.picker {
                // up over down, the pair as tall as the chevron and the gap between them
                let pair = MENU_CHEVRON.height * 2.0 + PICKER_CHEVRON_GAP;
                let top = (size.height - pair) / 2.0;
                path(top, false);
                path(top + MENU_CHEVRON.height + PICKER_CHEVRON_GAP, true);
            } else {
                path(y, true);
            }
        }
        paint_focus_ring(cx, pill, radius);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        let enabled = self.message.is_some();
        cx.role(match (self.menu, self.chosen) {
            (Some(_), _) => Role::PopUpButton,
            (None, Some(_)) => Role::RadioButton,
            (None, None) => Role::Button,
        });
        cx.name(self.label.clone());
        if let (None, Some(chosen)) = (self.menu, self.chosen) {
            cx.value(AccessibleValue::Toggle(if chosen {
                Checked::True
            } else {
                Checked::False
            }));
        }
        cx.states(AccessibleStates {
            disabled: !enabled,
            selected: self.chosen == Some(true),
            expanded: self.menu,
            ..AccessibleStates::default()
        });
        if enabled {
            cx.focusable();
            cx.action(Action::Click);
        }
    }
}

/// The tag glyph's body: its length and width before it is turned 45°, the length of its point, its stroke, and the
/// side of its hole.
const TAG: Size = Size {
    width: 15.0,
    height: 8.5,
};
const TAG_POINT: f64 = 4.25;
const TAG_STROKE: f64 = 1.5;
const TAG_HOLE: f64 = 2.25;

/// The tag in the middle of `area`: its outline, the point towards the top right (top left when `mirrored`), and the
/// centre of its hole near the point.
fn tag(area: Rect, mirrored: bool) -> (Shape, Point) {
    let centre = Point::new(
        area.min_x() + area.width() / 2.0,
        area.min_y() + area.height() / 2.0,
    );
    // laid out pointing right, then turned a quarter of a half turn up
    let k = std::f64::consts::FRAC_1_SQRT_2;
    let turn = |x: f64, y: f64| {
        let (x, y) = ((x + y) * k, (y - x) * k);
        let x = if mirrored { -x } else { x };
        Point::new(centre.x + x, centre.y + y)
    };
    let (w, h) = (TAG.width / 2.0, TAG.height / 2.0);
    let corners = [
        (-w, -h),
        (w - TAG_POINT, -h),
        (w, 0.0),
        (w - TAG_POINT, h),
        (-w, h),
    ];
    let mut verbs: Vec<PathVerb> = corners
        .iter()
        .enumerate()
        .map(|(i, &(x, y))| {
            let p = turn(x, y);
            if i == 0 {
                PathVerb::MoveTo(p)
            } else {
                PathVerb::LineTo(p)
            }
        })
        .collect();
    verbs.push(PathVerb::Close);
    let hole = turn(w - TAG_POINT - TAG_HOLE / 2.0, 0.0);
    (
        Shape::Path {
            verbs,
            fill_rule: Default::default(),
        },
        hole,
    )
}

/// The chevron in the middle of `area`, pointing right when `forward` and left otherwise.
fn chevron(area: Rect, right: bool) -> Shape {
    let centre = Point::new(
        area.min_x() + area.width() / 2.0,
        area.min_y() + area.height() / 2.0,
    );
    let reach = if right {
        CHEVRON.width / 2.0
    } else {
        -CHEVRON.width / 2.0
    };
    Shape::Path {
        verbs: vec![
            PathVerb::MoveTo(Point::new(
                centre.x - reach,
                centre.y - CHEVRON.height / 2.0,
            )),
            PathVerb::LineTo(Point::new(centre.x + reach, centre.y)),
            PathVerb::LineTo(Point::new(
                centre.x - reach,
                centre.y + CHEVRON.height / 2.0,
            )),
        ],
        fill_rule: Default::default(),
    }
}

/// How much of the label colour the title of an active window's toolbar keeps.
const TOOLBAR_TITLE_ALPHA: f32 = 0.76;

/// A glyph's colour, and the title's: the label colour; dimmed when disabled, and the faintest text colour in a window
/// that is not active, where the toolbar recedes.
fn glyph_color(theme: &cupertino_ui_theme::ThemeTokens, state: &ControlState) -> Color {
    let normal = cupertino_ui_theme::StateVariant::NORMAL;
    let role = if !state.enabled {
        ColorRole::DisabledControlText
    } else if !state.window_active {
        ColorRole::TertiaryText
    } else {
        ColorRole::Text
    };
    theme
        .colors
        .get(role, normal)
        .or_else(|| theme.colors.get(ColorRole::Text, normal))
        .unwrap_or(Color::new(0.5, 0.5, 0.5, 1.0))
}

impl ToolbarTitle {
    fn style() -> TextStyle {
        TextStyle {
            weight: FontWeight(TOOLBAR_TITLE_WEIGHT),
            ..TextStyle::new(TOOLBAR_TITLE_SIZE)
        }
    }

    fn laid_out(
        &self,
        env: &cupertino_ui_core::Environment,
        width: f64,
    ) -> Option<cupertino_ui_text::ParagraphLayout> {
        let paragraph = env
            .text
            .paragraph(
                &AttributedText::plain(self.text.clone(), Self::style()),
                &ParagraphStyle {
                    ellipsis: true,
                    ..ParagraphStyle::default()
                },
            )
            .ok()?;
        Some(look::one_line(&paragraph, width))
    }
}

/// The width the title's text takes at most, from its last measure, for what assistive technology shows of it.
#[derive(Default)]
pub struct TitleState {
    text_width: std::cell::Cell<f64>,
}

impl<M: 'static> Widget<M> for ToolbarTitle {
    type State = TitleState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fixed(TOOLBAR_CONTROL),
            flex: Flex::grow(1.0),
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> TitleState {
        TitleState::default()
    }

    fn update(&self, old: &Self, _: &mut TitleState) -> Change {
        if self.text != old.text {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, state: &TitleState, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = self
            .laid_out(cx.env(), f64::INFINITY)
            .map_or(0.0, |layout| layout.size().width);
        state.text_width.set(width);
        no_measure()
    }

    fn event(&self, _: &mut TitleState, _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &TitleState, cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let inset = TOOLBAR_TITLE_GAP - TOOLBAR_GAP;
        let width = (size.width - inset).max(0.0);
        let Some(layout) = self.laid_out(cx.env(), width) else {
            return;
        };
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: true,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let color = glyph_color(&theme, &state);
        // the title is a little lighter than the label colour, in a window that is active
        let color = if state.window_active {
            color.with_alpha(color.a * TOOLBAR_TITLE_ALPHA)
        } else {
            color
        };
        let rtl = cx.env().direction == LayoutDirection::Rtl;
        let x = if rtl {
            size.width - inset - layout.size().width
        } else {
            inset
        };
        let y = (size.height - layout.size().height) / 2.0;
        look::glyphs(cx, &layout, Point::new(x, y), Some(color));
    }

    fn semantics(&self, state: &TitleState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
        // the text, not the room after it, which is the toolbar's empty space
        let size = cx.size();
        let inset = TOOLBAR_TITLE_GAP - TOOLBAR_GAP;
        let width = state.text_width.get().min((size.width - inset).max(0.0));
        if width > 0.0 {
            let x = if cx.direction() == LayoutDirection::Rtl {
                size.width - inset - width
            } else {
                inset
            };
            cx.bounds(Rect::new(x, 0.0, width, size.height));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn points(shape: &Shape) -> Vec<Point> {
        let Shape::Path { verbs, .. } = shape else {
            panic!("a path");
        };
        verbs
            .iter()
            .filter_map(|v| match v {
                PathVerb::MoveTo(p) | PathVerb::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_tag_points_up_and_to_the_trailing_side_with_its_hole_by_the_point() {
        let area = Rect::new(0.0, 0.0, 20.0, 20.0);
        let (outline, hole) = tag(area, false);
        let corners = points(&outline);
        assert_eq!(corners.len(), 5);
        // the point is the corner furthest up and right; the hole lies between it and the middle
        let point = corners[2];
        assert!(point.x > 10.0 && point.y < 10.0, "{point:?}");
        assert!(hole.x > 10.0 && hole.x < point.x && hole.y < 10.0 && hole.y > point.y);
        for p in &corners {
            assert!(
                (0.0..=20.0).contains(&p.x) && (0.0..=20.0).contains(&p.y),
                "{p:?}"
            );
        }
        // right to left it points up and to the left, the mirror image
        let (mirrored, mirrored_hole) = tag(area, true);
        for (a, b) in corners.iter().zip(points(&mirrored)) {
            assert!((a.x - (20.0 - b.x)).abs() < 1e-9 && (a.y - b.y).abs() < 1e-9);
        }
        assert!((hole.x - (20.0 - mirrored_hole.x)).abs() < 1e-9);
    }
}
