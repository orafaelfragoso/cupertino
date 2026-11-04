//! The banners and the notification centre as CupertinoKit windows, built from the [`NotificationCenter`] model.
//!
//! Each banner on screen is its own window: a strip as wide as the card plus its swipe travel and shadow, as tall as
//! the card plus its shadow, which the process puts on a layer surface at the top right of the output. The card moves
//! inside its window as the model's slide and drag offsets say, so a swipe's pointer positions stay in one frame of
//! reference while the card moves under them. The notification centre is one window, the full height of the output's
//! right edge, with the groups of the history stacked as [`GroupStack`] lays them out.
//!
//! The windows report what the user did as [`Input`]; the host hands it to the model (see [`apply`]). Text sizes,
//! colours and buttons come from the Kit theme; the card metrics below are approximate.
//!
//! A banner is a polite live region (an assertive one when it is critical), so a screen reader reads a new banner as
//! it arrives; the cards of the centre are not live.

use std::fmt::Debug;

use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, Icon, IconDirection, RichText, Stack, Text, TextLink, TextRole,
};
use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, Live, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, Overflow,
    SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, MouseButton, PointerEvent, WindowSpec};
use cupertino_kit::render::{Effect, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{AttributedText, FontStyle, FontWeight, TextRange, TextStyle};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};

use crate::center::{
    BannerMeasure, ClearScope, NotificationCenter, StackLayout, SwipeConfig, SwipeOutcome,
};
use crate::notification::{Notification, Urgency};
use crate::stack::GroupStack;

/// The `app_id` of every window of the process.
pub const APP_ID: &str = "cupertino-notifications";
/// The title of the notification centre's window, which tells it from the banners' windows.
pub const CENTRE_TITLE: &str = "Notification Centre";
/// Room around a banner card in its window for the card's shadow, in points.
pub const SHADOW_MARGIN: f64 = 12.0;
/// Banners below this opacity get no window.
pub const MIN_OPACITY: f64 = 0.002;

/// Card corner radius: 16 pt, as the banner platter.
pub const CORNER_RADIUS: f64 = 16.0;
/// Space between a card's edge and its content.
pub const PADDING: f64 = 12.0;
/// The application icon at the card's leading edge.
pub const ICON_SIZE: f64 = 32.0;
pub const ICON_GAP: f64 = 10.0;
/// The summary's line (approximate).
pub const TITLE_LINE: f64 = 18.0;
/// The body's line (approximate); the body shows one line, ending in an ellipsis.
pub const BODY_LINE: f64 = 17.0;
pub const LINE_GAP: f64 = 2.0;
/// Room for the row of action buttons and the gap above it.
pub const BUTTON_ROW: f64 = 24.0;
pub const BUTTON_TOP_GAP: f64 = 8.0;
pub const BUTTON_GAP: f64 = 8.0;
/// Action buttons shown, at most.
pub const MAX_BUTTONS: usize = 3;
/// Diameter of a card's close button.
pub const CLOSE_DIAMETER: f64 = 18.0;
/// Outline of a card: 0.5 pt.
pub const OUTLINE_WIDTH: f64 = 0.5;
/// The centre's header row (title and Clear) and the gap under it.
pub const HEADER_HEIGHT: f64 = 28.0;
pub const HEADER_GAP: f64 = 8.0;
/// The Clear menu's width and row height.
pub const MENU_WIDTH: f64 = 180.0;
pub const MENU_ROW: f64 = 28.0;
/// Shadow under a card (approximate).
const SHADOW_BLUR: f64 = 8.0;
const SHADOW_ALPHA: f32 = 0.2;

/// What the user did in a window, for [`apply`] to carry out on the model.
#[derive(Clone, Debug, PartialEq)]
pub enum Input {
    /// The pointer is over this banner, or over none.
    Hover(Option<u32>),
    /// A press on a banner's body, which becomes a swipe or a click.
    SwipeBegin(u32),
    /// The pointer moved by `dx` points since the press.
    SwipeMove(u32, f64),
    SwipeEnd(u32),
    /// The press ended without effect (the pointer or the surface was lost).
    SwipeCancel(u32),
    /// The close button of a banner.
    Dismiss(u32),
    /// The close button of a card of the centre.
    Clear(u32),
    /// An action button.
    Invoke(u32, String),
    /// A click on a card of the centre, or assistive technology's click on a card.
    Activate(u32),
    /// A link of a body.
    OpenLink(String),
    ToggleClearMenu,
    ClearScope(ClearScope),
    /// Escape, or a press outside the cards.
    CloseCentre,
}

/// What the windows show beyond the model: whether the centre and its Clear menu are open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Ui {
    pub centre_open: bool,
    pub menu_open: bool,
}

/// What [`apply`] leaves to the host, which needs an activation token or a process for it.
#[derive(Clone, Debug, PartialEq)]
pub enum HostAction {
    /// Activate the notification (its default action, or a dismissal).
    Activate(u32),
    Invoke(u32, String),
    OpenLink(String),
}

/// Heights of the cards, from the metrics above: one summary line, one body line and a row of buttons when there
/// are actions.
#[derive(Clone, Copy, Debug, Default)]
pub struct CardMetrics;

impl CardMetrics {
    pub fn height(n: &Notification) -> f64 {
        let mut text = TITLE_LINE;
        if !n.body_text().trim().is_empty() {
            text += LINE_GAP + BODY_LINE;
        }
        if n.button_actions().next().is_some() {
            text += BUTTON_TOP_GAP + BUTTON_ROW;
        }
        2.0 * PADDING + text.max(ICON_SIZE)
    }
}

impl BannerMeasure for CardMetrics {
    fn banner_height(&mut self, n: &Notification, _width: f64) -> f64 {
        Self::height(n)
    }
}

/// Width of every window: the card, its right margin, the leftward swipe travel and the shadow on the left.
pub fn strip_width(layout: &StackLayout, swipe: &SwipeConfig) -> f64 {
    (layout.width + layout.right_margin + swipe.left_limit.abs() + SHADOW_MARGIN).ceil()
}

/// The key of a banner's window: a replaced notification may change height, so it gets a window of its own.
pub fn banner_key(n: &Notification) -> WindowKey {
    WindowKey(Key::from(format!("banner-{}-{}", n.id, n.revision)))
}

pub fn centre_key() -> WindowKey {
    WindowKey(Key::from("centre"))
}

/// The windows for the model at `now`: the banners on screen, or the open centre instead of them (they stay in the
/// model and show again when it closes).
pub fn windows<M: Clone + 'static>(
    center: &mut NotificationCenter,
    ui: &Ui,
    now: f64,
    map: fn(Input) -> M,
) -> Vec<Window<M>> {
    let width = strip_width(&center.config.layout, &center.config.swipe);
    if ui.centre_open {
        return vec![centre_window(center, ui, width, map)];
    }
    let placements = center.placements(Size::new(width, 0.0), &mut CardMetrics, now);
    placements
        .into_iter()
        .filter(|p| p.opacity > MIN_OPACITY)
        .filter_map(|p| {
            let n = center.get(p.id)?.clone();
            let card = Rect::new(p.rect.x(), SHADOW_MARGIN, p.rect.width(), p.rect.height());
            Some(banner_window(&n, card, p.opacity, p.hovered, width, map))
        })
        .collect()
}

/// Carries `input` out on the model; what needs the host comes back.
pub fn apply(
    center: &mut NotificationCenter,
    ui: &mut Ui,
    input: Input,
    now: f64,
) -> Option<HostAction> {
    match input {
        Input::Hover(id) => center.set_hover(id, now),
        Input::SwipeBegin(id) => {
            center.swipe_begin(id, now);
        }
        Input::SwipeMove(id, dx) => center.swipe_update(id, dx, now),
        Input::SwipeEnd(id) => {
            if center.swipe_end(id, now) == SwipeOutcome::Click {
                return Some(HostAction::Activate(id));
            }
        }
        Input::SwipeCancel(id) => {
            center.swipe_update(id, 0.0, now);
            center.swipe_end(id, now);
        }
        Input::Dismiss(id) => center.dismiss_banner(id, now),
        Input::Clear(id) => {
            center.clear(id, now);
        }
        Input::Invoke(id, key) => return Some(HostAction::Invoke(id, key)),
        // the host closes the centre once it has the activation token, which needs the centre's window
        Input::Activate(id) => return Some(HostAction::Activate(id)),
        Input::OpenLink(href) => return Some(HostAction::OpenLink(href)),
        Input::ToggleClearMenu => ui.menu_open = !ui.menu_open,
        Input::ClearScope(scope) => {
            center.clear_scope(scope, now);
            ui.menu_open = false;
        }
        Input::CloseCentre if ui.menu_open => ui.menu_open = false,
        Input::CloseCentre => *ui = Ui::default(),
    }
    None
}

fn spec(title: String, size: Size) -> WindowSpec {
    WindowSpec {
        title,
        app_id: APP_ID.to_owned(),
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region: None,
    }
}

fn window<M: Clone + 'static>(key: WindowKey, spec: WindowSpec, content: Element<M>) -> Window<M> {
    Window {
        key,
        spec,
        shortcuts: CommandRegistry::new(),
        on_close: None,
        // the cards paint their own platter
        background: WindowBackground::None,
        content,
        popup: None,
        menus: None,
    }
}

/// What assistive technology calls a card.
pub fn accessible_name(n: &Notification) -> String {
    match (n.app_name.trim(), n.summary.trim()) {
        ("", summary) => summary.to_owned(),
        (app, "") => app.to_owned(),
        (app, summary) => format!("{app}: {summary}"),
    }
}

fn banner_window<M: Clone + 'static>(
    n: &Notification,
    card: Rect,
    opacity: f64,
    hovered: bool,
    width: f64,
    map: fn(Input) -> M,
) -> Window<M> {
    let size = Size::new(width, card.height() + 2.0 * SHADOW_MARGIN);
    let live = if n.urgency == Urgency::Critical {
        Live::Assertive
    } else {
        Live::Polite
    };
    let content = Element::new(Strip {
        id: n.id,
        card,
        map,
        children: vec![Element::new(Card {
            id: n.id,
            rect: card,
            opacity,
            brightness: 0.0,
            name: accessible_name(n),
            description: n.body_text(),
            live: Some(live),
            clickable: false,
            map,
            children: card_content(n, hovered.then_some(Input::Dismiss(n.id)), map),
        })
        .key(format!("card-{}", n.id))],
    });
    window(banner_key(n), spec(accessible_name(n), size), content)
}

fn centre_window<M: Clone + 'static>(
    center: &NotificationCenter,
    ui: &Ui,
    width: f64,
    map: fn(Input) -> M,
) -> Window<M> {
    let layout = &center.config.layout;
    let stack = GroupStack::default();
    let x = width - layout.right_margin - layout.width;
    let mut y = layout.top_inset;
    let mut children = Vec::new();
    let groups = center.groups();
    let scopes = center.clear_scopes();
    let mut header = vec![Element::new(Text {
        content: plain(CENTRE_TITLE, FontWeight::SEMIBOLD),
        role: TextRole::Section,
        wrap: false,
        secondary: false,
        selected: false,
    })
    .fill()];
    if !scopes.is_empty() {
        header.push(
            Element::new(Button {
                label: "Clear".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Bordered,
                enabled: true,
                busy: false,
                on_press: Some(map(Input::ToggleClearMenu)),
            })
            .key("clear"),
        );
    }
    children.push(
        Element::new(Place {
            rect: Rect::new(x, y, layout.width, HEADER_HEIGHT),
            axis: Axis::Horizontal,
            padding: Insets {
                left: PADDING,
                right: 0.0,
                top: 0.0,
                bottom: 0.0,
            },
            spacing: BUTTON_GAP,
            children: header,
        })
        .key("header"),
    );
    y += HEADER_HEIGHT + HEADER_GAP;
    for group in &groups {
        let front = group.notifications[0];
        let count = group.notifications.len();
        let height = CardMetrics::height(front);
        let rect = Rect::new(x, y, layout.width, height);
        // the cards behind first, the farthest first, so the front card covers them
        for behind in stack.cards(rect, count).into_iter().rev() {
            children.push(Element::new(Card {
                id: front.id,
                rect: behind.rect,
                opacity: 1.0,
                brightness: behind.brightness,
                name: String::new(),
                description: String::new(),
                live: None,
                clickable: false,
                map,
                children: Vec::new(),
            }));
        }
        let mut description = front.body_text();
        if count > 1 {
            let more = format!("{} more from {}", count - 1, group.app_name);
            description = if description.is_empty() {
                more
            } else {
                format!("{description}; {more}")
            };
        }
        children.push(
            Element::new(Card {
                id: front.id,
                rect,
                opacity: 1.0,
                brightness: 0.0,
                name: accessible_name(front),
                description,
                live: None,
                clickable: true,
                map,
                children: card_content(front, Some(Input::Clear(front.id)), map),
            })
            .key(format!("card-{}", front.id)),
        );
        y += stack.height(height, count);
    }
    if ui.menu_open && !scopes.is_empty() {
        let rows = scopes
            .iter()
            .map(|&scope| {
                Element::new(Button {
                    label: scope.label().into(),
                    role: ButtonRole::Normal,
                    style: ButtonStyle::Borderless,
                    enabled: true,
                    busy: false,
                    on_press: Some(map(Input::ClearScope(scope))),
                })
                .key(format!("scope-{scope:?}"))
            })
            .collect();
        let height = scopes.len() as f64 * MENU_ROW + 2.0 * (PADDING / 2.0);
        let top = layout.top_inset + HEADER_HEIGHT;
        children.push(
            Element::new(Card {
                id: 0,
                rect: Rect::new(x + layout.width - MENU_WIDTH, top, MENU_WIDTH, height),
                opacity: 1.0,
                brightness: 0.0,
                name: "Clear".into(),
                description: String::new(),
                live: None,
                clickable: false,
                map,
                children: vec![Element::new(Stack {
                    axis: Axis::Vertical,
                    padding: Insets {
                        top: PADDING / 2.0,
                        bottom: PADDING / 2.0,
                        left: PADDING / 2.0,
                        right: PADDING / 2.0,
                    },
                    spacing: 0.0,
                    align: Align::Stretch,
                    children: rows,
                })],
            })
            .key("clear-menu"),
        );
    }
    let content = Element::new(CentreRoot {
        map,
        menu_open: ui.menu_open,
        children,
    });
    // the height is the output's: the layer surface spans the right edge from top to bottom
    window(
        centre_key(),
        spec(CENTRE_TITLE.to_owned(), Size::new(width, y.max(1.0))),
        content,
    )
}

fn plain(text: &str, weight: FontWeight) -> AttributedText {
    let mut style = TextStyle::new(13.0);
    style.weight = weight;
    AttributedText::plain(text, style)
}

/// The body with its markup as text runs and its links.
fn body<M: Clone + 'static>(n: &Notification, map: fn(Input) -> M) -> RichText<M> {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links = Vec::new();
    for span in &n.spans {
        // one line: line breaks of the body read as spaces
        let piece = span.text.replace(['\n', '\r'], " ");
        if piece.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&piece);
        let mut style = TextStyle::new(13.0);
        if span.bold {
            style.weight = FontWeight::BOLD;
        }
        if span.italic {
            style.style = FontStyle::Italic;
        }
        runs.push((TextRange(start..text.len()), style));
        if let Some(href) = &span.link {
            links.push(TextLink {
                range: start..text.len(),
                url: href.clone(),
                on_open: Some(map(Input::OpenLink(href.clone()))),
            });
        }
    }
    RichText {
        content: AttributedText { text, runs },
        role: TextRole::Body,
        wrap: false,
        secondary: true,
        links,
    }
}

/// The icon, the summary, the body and the action buttons of a card, and its close button when `close` is given.
fn card_content<M: Clone + 'static>(
    n: &Notification,
    close: Option<Input>,
    map: fn(Input) -> M,
) -> Vec<Element<M>> {
    let mut lines = vec![Element::new(Text {
        content: plain(&n.summary, FontWeight::SEMIBOLD),
        role: TextRole::Body,
        wrap: false,
        secondary: false,
        selected: false,
    })];
    if !n.body_text().trim().is_empty() {
        lines.push(Element::new(body(n, map)));
    }
    let buttons: Vec<Element<M>> = n
        .button_actions()
        .take(MAX_BUTTONS)
        .map(|a| {
            Element::new(Button {
                label: a.label.clone(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Pill,
                enabled: true,
                busy: false,
                on_press: Some(map(Input::Invoke(n.id, a.key.clone()))),
            })
            .key(format!("action-{}", a.key))
        })
        .collect();
    if !buttons.is_empty() {
        lines.push(Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets {
                top: BUTTON_TOP_GAP - LINE_GAP,
                ..Insets::ZERO
            },
            spacing: BUTTON_GAP,
            align: Align::Center,
            children: buttons,
        }));
    }
    let icon = if n.icon.is_empty() || n.icon.contains('/') {
        "bell".to_owned()
    } else {
        n.icon.clone()
    };
    let mut content = vec![Element::new(Stack {
        axis: Axis::Horizontal,
        padding: Insets {
            top: PADDING,
            left: PADDING,
            bottom: PADDING,
            right: PADDING,
        },
        spacing: ICON_GAP,
        align: Align::Start,
        children: vec![
            Element::new(Icon {
                name: icon,
                size: ICON_SIZE,
                direction: IconDirection::Fixed,
                label: None,
                tint: None,
            }),
            Element::new(Stack {
                axis: Axis::Vertical,
                padding: Insets::ZERO,
                spacing: LINE_GAP,
                align: Align::Start,
                children: lines,
            })
            .fill(),
        ],
    })];
    if let Some(close) = close {
        content.push(
            Element::new(CloseButton {
                message: map(close),
            })
            .key("close"),
        );
    }
    content
}

fn no_measure() -> Measured {
    Measured {
        size: Size::ZERO,
        baseline: None,
    }
}

fn color(cx: &PaintCx<'_>, role: ColorRole) -> Option<Color> {
    cx.env().theme.colors.get(role, StateVariant::NORMAL)
}

/// A banner's window content: the card and the pointer handling of the banner, hover and swipe.
struct Strip<M> {
    id: u32,
    /// The card's resting rect at this frame, window coordinates.
    card: Rect,
    map: fn(Input) -> M,
    children: Vec<Element<M>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct StripState {
    /// The x of the press that may become a swipe.
    press: Option<f64>,
    hovered: bool,
}

impl<M: 'static> Strip<M> {
    fn hover(&self, state: &mut StripState, over: bool, cx: &mut EventCx<'_, M>) {
        if over != state.hovered && state.press.is_none() {
            state.hovered = over;
            cx.emit((self.map)(Input::Hover(over.then_some(self.id))));
        }
    }
}

impl<M: 'static> Widget<M> for Strip<M> {
    type State = StripState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> StripState {
        StripState::default()
    }

    fn update(&self, old: &Self, _: &mut StripState) -> Change {
        if self.card != old.card {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &StripState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, state: &mut StripState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match *event {
            // hover follows the pointer before any child sees it
            Event::Pointer(
                PointerEvent::Enter { position } | PointerEvent::Motion { position },
                _,
            ) if cx.phase() == Phase::Capture || cx.node() == cx.target() => {
                if let Some(start) = state.press {
                    cx.emit((self.map)(Input::SwipeMove(self.id, position.x - start)));
                    return Propagation::Stop;
                }
                self.hover(state, self.card.contains(position), cx);
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) if cx.node() == cx.target() => {
                self.hover(state, false, cx);
                Propagation::Continue
            }
            // a press no child took: on the card it begins a swipe or a click
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.phase() != Phase::Capture && self.card.contains(position) => {
                state.press = Some(position.x);
                cx.capture_pointer();
                cx.emit((self.map)(Input::SwipeBegin(self.id)));
                Propagation::Stop
            }
            Event::Pointer(
                PointerEvent::Button {
                    position,
                    button: MouseButton::Left,
                    pressed: false,
                    ..
                },
                _,
            ) if state.press.is_some() => {
                state.press = None;
                cx.release_pointer();
                cx.emit((self.map)(Input::SwipeEnd(self.id)));
                state.hovered = self.card.contains(position);
                Propagation::Stop
            }
            Event::Cancel if state.press.is_some() => {
                state.press = None;
                cx.emit((self.map)(Input::SwipeCancel(self.id)));
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &StripState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &StripState, _: &mut SemanticsCx<'_>) {}
}

/// The centre's window content: a press outside the cards and Escape close the Clear menu, then the centre.
struct CentreRoot<M> {
    map: fn(Input) -> M,
    menu_open: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for CentreRoot<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.menu_open != old.menu_open {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed: true,
                    ..
                },
                _,
            ) if cx.node() == cx.target() => {
                cx.emit((self.map)(Input::CloseCentre));
                Propagation::Stop
            }
            Event::Dismiss => {
                cx.emit((self.map)(Input::CloseCentre));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::List);
        cx.name(CENTRE_TITLE);
        cx.dismissible();
    }
}

/// A row placed at `rect` in an absolute parent.
struct Place<M> {
    rect: Rect,
    axis: Axis,
    padding: Insets,
    spacing: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Place<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: self.padding,
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: self.axis,
                spacing: self.spacing,
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
        if self.rect != old.rect || self.padding != old.padding || self.spacing != old.spacing {
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

/// A card's platter at `rect` in an absolute parent, with its content layered over it and faded with it.
struct Card<M> {
    id: u32,
    rect: Rect,
    opacity: f64,
    /// Darkening of a card behind a group's front one, from 0 down.
    brightness: f64,
    /// Empty for a card behind, which is decoration only.
    name: String,
    description: String,
    live: Option<Live>,
    /// A click on the card activates it (the centre's cards; a banner's clicks come from its swipe).
    clickable: bool,
    map: fn(Input) -> M,
    children: Vec<Element<M>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CardState {
    armed: bool,
}

impl<M: 'static> Widget<M> for Card<M> {
    type State = CardState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Overlay)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> CardState {
        CardState::default()
    }

    fn update(&self, old: &Self, _: &mut CardState) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.opacity != old.opacity
            || self.brightness != old.brightness
            || self.name != old.name
            || self.description != old.description
            || self.live != old.live
        {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &CardState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, state: &mut CardState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture || self.name.is_empty() {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed,
                    position,
                    ..
                },
                _,
            ) if self.clickable => {
                if pressed {
                    state.armed = true;
                } else if std::mem::take(&mut state.armed) && cx.hit(position) {
                    cx.emit((self.map)(Input::Activate(self.id)));
                }
                Propagation::Stop
            }
            Event::Cancel => {
                state.armed = false;
                Propagation::Continue
            }
            Event::Action(ref request) if request.action == Action::Click => {
                cx.emit((self.map)(Input::Activate(self.id)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &CardState, cx: &mut PaintCx<'_>) {
        let shadow = Effect::DropShadow {
            offset: Point::new(0.0, 1.0),
            blur: SHADOW_BLUR,
            color: Color::new(0.0, 0.0, 0.0, SHADOW_ALPHA),
        };
        cx.set_group(self.opacity.clamp(0.0, 1.0) as f32, vec![shadow]);
        let rect = Rect::new(0.0, 0.0, self.rect.width(), self.rect.height());
        let radius = CORNER_RADIUS.min(rect.height() / 2.0);
        let theme = &cx.env().theme;
        let Some(fill) = theme.surface(Surface::Popover).fill else {
            return;
        };
        let darken = (1.0 + self.brightness).clamp(0.0, 1.0) as f32;
        let fill = Color::new(fill.r * darken, fill.g * darken, fill.b * darken, fill.a);
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect { rect, radius },
            paint: Paint::Solid(fill),
        });
        if let Some(outline) = color(cx, ColorRole::Separator) {
            cx.push(PaintOp::Stroke {
                shape: Shape::RoundedRect { rect, radius },
                width: OUTLINE_WIDTH,
                paint: Paint::Solid(outline),
            });
        }
    }

    fn semantics(&self, _: &CardState, cx: &mut SemanticsCx<'_>) {
        // a card behind a group's front one is decoration
        if self.name.is_empty() {
            return;
        }
        cx.role(Role::Group);
        cx.name(self.name.clone());
        if !self.description.is_empty() {
            cx.description(self.description.clone());
        }
        if let Some(live) = self.live {
            cx.live(live);
        }
        if self.id != 0 {
            cx.action(Action::Click);
        }
    }
}

/// The round close button at a card's top-leading corner.
struct CloseButton<M> {
    message: M,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CloseState {
    armed: bool,
}

impl<M: Clone + 'static> Widget<M> for CloseButton<M> {
    type State = CloseState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(CLOSE_DIAMETER),
            height: SizeSpec::Fixed(CLOSE_DIAMETER),
            align_self: Some(Align::Start),
            flex: Flex::NONE,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> CloseState {
        CloseState::default()
    }

    fn update(&self, _: &Self, _: &mut CloseState) -> Change {
        Change::None
    }

    fn measure(&self, _: &CloseState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(CLOSE_DIAMETER, CLOSE_DIAMETER),
            baseline: None,
        }
    }

    fn event(&self, state: &mut CloseState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left,
                    pressed,
                    position,
                    ..
                },
                _,
            ) => {
                if pressed {
                    state.armed = true;
                    cx.capture_pointer();
                } else if std::mem::take(&mut state.armed) {
                    cx.release_pointer();
                    if cx.hit(position) {
                        cx.emit(self.message.clone());
                    }
                }
                Propagation::Stop
            }
            Event::Cancel => {
                state.armed = false;
                Propagation::Continue
            }
            Event::Action(ref request) if request.action == Action::Click => {
                cx.emit(self.message.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &CloseState, cx: &mut PaintCx<'_>) {
        let (Some(fill), Some(mark)) = (
            color(cx, ColorRole::ControlFill),
            color(cx, ColorRole::SecondaryText),
        ) else {
            return;
        };
        let rect = Rect::new(0.0, 0.0, CLOSE_DIAMETER, CLOSE_DIAMETER);
        cx.push(PaintOp::Fill {
            shape: Shape::RoundedRect {
                rect,
                radius: CLOSE_DIAMETER / 2.0,
            },
            paint: Paint::Solid(fill),
        });
        let (a, b) = (CLOSE_DIAMETER * 0.32, CLOSE_DIAMETER * 0.68);
        cx.push(PaintOp::Stroke {
            shape: Shape::Path {
                verbs: vec![
                    PathVerb::MoveTo(Point::new(a, a)),
                    PathVerb::LineTo(Point::new(b, b)),
                    PathVerb::MoveTo(Point::new(b, a)),
                    PathVerb::LineTo(Point::new(a, b)),
                ],
                fill_rule: Default::default(),
            },
            width: 1.5,
            paint: Paint::Solid(mark),
        });
    }

    fn semantics(&self, _: &CloseState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name("Close");
        cx.action(Action::Click);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notification::NotifyInput;

    #[test]
    fn a_card_is_as_tall_as_its_lines_and_never_shorter_than_its_icon() {
        let mut c = NotificationCenter::default();
        let plain = c.notify(NotifyInput::new("app", "Title", ""), 0.0);
        let body = c.notify(NotifyInput::new("app", "Title", "Body"), 0.0);
        let actions = c.notify(
            NotifyInput::new("app", "Title", "Body").with_action("reply", "Reply"),
            0.0,
        );
        let h = |id| CardMetrics::height(c.get(id).unwrap());
        assert_eq!(h(plain), 2.0 * PADDING + ICON_SIZE);
        assert_eq!(
            h(body),
            2.0 * PADDING + ICON_SIZE.max(TITLE_LINE + LINE_GAP + BODY_LINE)
        );
        assert_eq!(
            h(actions),
            2.0 * PADDING + TITLE_LINE + LINE_GAP + BODY_LINE + BUTTON_TOP_GAP + BUTTON_ROW
        );
    }

    #[test]
    fn the_strip_holds_the_card_its_margin_the_swipe_travel_and_the_shadow() {
        let width = strip_width(&StackLayout::default(), &SwipeConfig::default());
        assert_eq!(width, 344.0 + 16.0 + 16.0 + SHADOW_MARGIN);
    }

    #[test]
    fn the_centre_replaces_the_banners_and_closing_it_brings_them_back() {
        let mut c = NotificationCenter::default();
        c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
        let mut ui = Ui::default();
        let banners = windows::<Input>(&mut c, &ui, 1.0, |i| i);
        assert_eq!(banners.len(), 1);
        assert_eq!(banners[0].spec.title, "app: Hello");
        ui.centre_open = true;
        let centre = windows::<Input>(&mut c, &ui, 1.0, |i| i);
        assert_eq!(centre.len(), 1);
        assert_eq!(centre[0].spec.title, CENTRE_TITLE);
        assert_eq!(apply(&mut c, &mut ui, Input::CloseCentre, 1.0), None);
        assert!(!ui.centre_open);
    }

    #[test]
    fn escape_closes_the_clear_menu_before_the_centre() {
        let mut c = NotificationCenter::default();
        let mut ui = Ui {
            centre_open: true,
            menu_open: true,
        };
        apply(&mut c, &mut ui, Input::CloseCentre, 0.0);
        assert_eq!(
            ui,
            Ui {
                centre_open: true,
                menu_open: false
            }
        );
        apply(&mut c, &mut ui, Input::CloseCentre, 0.0);
        assert_eq!(ui, Ui::default());
    }

    #[test]
    fn a_press_that_never_travels_is_a_click_and_activates() {
        let mut c = NotificationCenter::default();
        let id = c.notify(NotifyInput::new("app", "Hello", "World"), 0.0);
        let mut ui = Ui::default();
        windows::<Input>(&mut c, &ui, 1.0, |i| i);
        assert_eq!(apply(&mut c, &mut ui, Input::SwipeBegin(id), 1.0), None);
        assert_eq!(
            apply(&mut c, &mut ui, Input::SwipeEnd(id), 1.1),
            Some(HostAction::Activate(id))
        );
    }
}
