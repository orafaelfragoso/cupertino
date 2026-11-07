//! The banners and the notification centre as CupertinoKit windows, built from the [`NotificationCenter`] model.
//!
//! Each banner on screen is its own window: a strip as wide as the card plus its swipe travel and shadow, as tall as
//! the card plus its shadow, which the process puts on a layer surface at the top right of the output. The card moves
//! inside its window as the model's slide and drag offsets say, so a swipe's pointer positions stay in one frame of
//! reference while the card moves under them. The notification centre is one window, the full height of the output's
//! right edge: its header panel over a scroll view of the groups of the history, stacked as [`GroupStack`] lays them
//! out.
//!
//! A card's text is measured with the text system the windows draw with ([`CardMetrics`]): the summary wraps to
//! [`SUMMARY_LINES`] and the body to [`BODY_LINES`], each ending in an ellipsis where it is cut. A centre card whose body
//! is cut has a disclosure button that unfolds it to [`UNFOLDED_BODY_LINES`]. Every card is exactly as tall as its
//! text, so nothing in it is clipped.
//!
//! Each card asks the compositor for the popover material under itself ([`PaintCx::material`]) and paints the opaque
//! fill in its place where the window cannot show it: without the desktop's effects, with Reduce Transparency or
//! Increase Contrast.
//!
//! The windows report what the user did as [`Input`]; the host hands it to the model (see [`apply`]).
//!
//! A banner is a polite live region (an assertive one when it is critical), so a screen reader reads a new banner as
//! it arrives; the cards of the centre are not live. They are the items of a list, Tab stops that Return or Space
//! activates and Delete clears.

use std::collections::BTreeSet;
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;

use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, DisclosureButton, DisclosureStyle, Icon, IconDirection,
    RichText, ScrollView, Scroller, Spacer, Stack, Text, TextLink, TextRole,
};
use cupertino_kit::core::{
    Action, Change, CommandRegistry, Element, Event, EventCx, Key, Live, MeasureCx, PaintCx, Phase,
    Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_kit::geometry::{Insets, Point, Rect, Size};
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Flex, Justify, LayoutSpec, Measured, Overflow,
    ScrollState, SizeSpec,
};
use cupertino_kit::platform::{DecorationMode, Keysym, MouseButton, PointerEvent, WindowSpec};
use cupertino_kit::render::{Effect, FillRule, Paint, PaintOp, PathVerb, Shape};
use cupertino_kit::text::{
    AttributedText, FontStyle, FontWeight, ParagraphLayout, ParagraphStyle, TextAlign, TextRange,
    TextStyle, TextSystem, Wrap,
};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface, ThemeTokens, TypeRole};
use cupertino_theme::metrics::{alignment_height, Control};

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
/// Space between the summary, the body and the row of buttons.
pub const LINE_GAP: f64 = 2.0;
/// The summary wraps to at most this many lines, the last ending in an ellipsis.
pub const SUMMARY_LINES: u32 = 2;
/// A banner and a folded centre card show at most this many lines of the body, the last ending in an ellipsis.
pub const BODY_LINES: u32 = 4;
/// An unfolded centre card shows at most this many lines of the body, so one message never fills the list; assistive
/// technology reads all of it from the card's description.
pub const UNFOLDED_BODY_LINES: u32 = 20;
/// The most of a body that is shaped and laid out, in bytes: more never shows within [`UNFOLDED_BODY_LINES`], and a
/// huge message costs no more than this.
pub const MAX_BODY_BYTES: usize = 4096;
/// Room for the row of action buttons and the gap above it.
pub const BUTTON_ROW: f64 = 24.0;
pub const BUTTON_TOP_GAP: f64 = 8.0;
pub const BUTTON_GAP: f64 = 8.0;
/// Action buttons shown, at most.
pub const MAX_BUTTONS: usize = 3;
/// Diameter of a card's close button.
pub const CLOSE_DIAMETER: f64 = 18.0;
/// The close button's distance from the card's top and leading edges, which keeps it inside the corner's curve.
pub const CLOSE_INSET: f64 = 4.0;
/// Outline of a card: 0.5 pt.
pub const OUTLINE_WIDTH: f64 = 0.5;
/// The centre's header panel (title and Clear), the gap under it (the one between groups) and the Clear button's inset
/// from the panel's right edge.
pub const HEADER_HEIGHT: f64 = 36.0;
pub const HEADER_GAP: f64 = 6.0;
pub const HEADER_TRAILING: f64 = 6.0;
/// Space between the centre's list and the bottom edge of the output.
pub const BOTTOM_MARGIN: f64 = 16.0;
/// The Clear menu's width and row height.
pub const MENU_WIDTH: f64 = 180.0;
pub const MENU_ROW: f64 = 28.0;
/// Shadow under a card.
const SHADOW_BLUR: f64 = 8.0;
const SHADOW_ALPHA: f32 = 0.2;
/// How far beyond a card its shadow can reach: the area the shadow is drawn in.
const SHADOW_REACH: f64 = 4.0 * SHADOW_BLUR;
/// What ends a line of text that is cut.
const ELLIPSIS: char = '\u{2026}';

const RETURN: Keysym = Keysym(0xff0d);
const KEYPAD_ENTER: Keysym = Keysym(0xff8d);
const SPACE: Keysym = Keysym(0x20);
const DELETE: Keysym = Keysym(0xffff);
const BACKSPACE: Keysym = Keysym(0xff08);

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
    /// The close button of a card of the centre, or Delete on it.
    Clear(u32),
    /// An action button.
    Invoke(u32, String),
    /// A click on a card of the centre, Return or Space on it, or assistive technology's click on a card.
    Activate(u32),
    /// A link of a body.
    OpenLink(String),
    /// A centre card's disclosure button: unfold (`true`) or fold its body.
    Unfold(u32, bool),
    /// The centre's list moved, or its viewport or content changed size.
    Scroll(ScrollState),
    ToggleClearMenu,
    ClearScope(ClearScope),
    /// Escape, or a press outside the cards.
    CloseCentre,
}

/// What the windows show beyond the model: whether the centre and its Clear menu are open, where its list is
/// scrolled to and which of its cards are unfolded.
#[derive(Clone, Debug, PartialEq)]
pub struct Ui {
    pub centre_open: bool,
    pub menu_open: bool,
    pub scroll: ScrollState,
    pub unfolded: BTreeSet<u32>,
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            centre_open: false,
            menu_open: false,
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            unfolded: BTreeSet::new(),
        }
    }
}

/// What [`apply`] leaves to the host, which needs an activation token or a process for it.
#[derive(Clone, Debug, PartialEq)]
pub enum HostAction {
    /// Activate the notification (its default action, or a dismissal).
    Activate(u32),
    Invoke(u32, String),
    OpenLink(String),
}

/// How much of a card's body shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    /// A banner: [`BODY_LINES`], no disclosure button.
    Banner,
    /// A centre card: [`BODY_LINES`], with a disclosure button when that cuts the body.
    Folded,
    /// A centre card the user unfolded: [`UNFOLDED_BODY_LINES`].
    Unfolded,
}

/// A body as a card shows it: its text runs and the links in it.
#[derive(Clone, Debug, PartialEq)]
pub struct BodyText {
    pub content: AttributedText,
    pub links: Vec<(Range<usize>, String)>,
}

/// What a card shows and how tall it is, from [`CardMetrics::card`].
#[derive(Clone, Debug, PartialEq)]
pub struct CardLayout {
    /// The summary, cut to [`SUMMARY_LINES`].
    pub summary: AttributedText,
    /// The width the summary is laid out at.
    pub summary_width: f64,
    /// The body, cut to the lines its fold shows; `None` for an empty one.
    pub body: Option<BodyText>,
    /// The body is cut at [`BODY_LINES`]: a centre card offers its disclosure button.
    pub foldable: bool,
    /// The action buttons' keys and labels, the labels cut so the row fits the card.
    pub actions: Vec<(String, String)>,
    /// The card's height, in whole points.
    pub height: f64,
}

/// A paragraph cut to its lines.
struct Fitted {
    content: AttributedText,
    height: f64,
    cut: bool,
}

/// Measures cards with the text system and the type the windows draw with, so a card is exactly as tall as its text.
#[derive(Clone)]
pub struct CardMetrics {
    theme: Arc<ThemeTokens>,
    text: Arc<TextSystem>,
}

impl CardMetrics {
    /// The metrics of windows drawn with `theme`'s type by `text`, as the runtime's options give them.
    pub fn new(theme: Arc<ThemeTokens>, text: Arc<TextSystem>) -> Self {
        CardMetrics { theme, text }
    }

    /// The width of a card's text column.
    pub fn column_width(width: f64) -> f64 {
        width - 2.0 * PADDING - ICON_SIZE - ICON_GAP
    }

    /// The side of a disclosure button, which Kit takes from the theme's control size.
    pub fn disclosure_side(&self) -> f64 {
        alignment_height(Control::DisclosureButton, self.theme.metrics.size)
            .or_else(|| {
                alignment_height(
                    Control::DisclosureButton,
                    cupertino_kit::theme::ControlSize::Regular,
                )
            })
            .unwrap_or_default()
    }

    /// `content` in `role`'s size, and its weight where a run keeps the regular one, as Kit's text sets it.
    fn styled(content: &AttributedText, role: TypeRole) -> AttributedText {
        let mut content = content.clone();
        for (_, style) in &mut content.runs {
            style.size = role.size;
            if style.weight == FontWeight::REGULAR {
                style.weight = FontWeight(role.weight);
            }
        }
        content
    }

    /// `content` wrapped within `width` as Kit's wrapping text lays it out, or cut to `max_lines` with an ellipsis.
    fn wrapped(
        &self,
        content: &AttributedText,
        role: TypeRole,
        width: f64,
        max_lines: Option<u32>,
    ) -> Option<ParagraphLayout> {
        let style = ParagraphStyle {
            align: TextAlign::Left,
            wrap: Wrap::Word,
            max_lines,
            ellipsis: max_lines.is_some(),
            ..ParagraphStyle::default()
        };
        let paragraph = self
            .text
            .paragraph(&Self::styled(content, role), &style)
            .ok()?;
        Some(paragraph.layout(Some(width as f32)))
    }

    /// `content` cut to at most `lines` lines within `width`: the text that shows, ending in an ellipsis where it was
    /// cut, and its height.
    fn fit(&self, content: &AttributedText, role: TypeRole, lines: u32, width: f64) -> Fitted {
        let lines = lines.max(1);
        let full = self.wrapped(content, role, width, None);
        let height =
            |layout: &Option<ParagraphLayout>| layout.as_ref().map_or(0.0, |l| l.size().height);
        if full
            .as_ref()
            .is_none_or(|l| l.lines().len() <= lines as usize)
        {
            return Fitted {
                content: content.clone(),
                height: height(&full),
                cut: false,
            };
        }
        // the bytes the last line shows when the text is cut there with an ellipsis
        let mut end = self
            .wrapped(content, role, width, Some(lines))
            .and_then(|l| l.lines().last().map(|line| line.range.0.end))
            .unwrap_or(0)
            .min(content.text.len());
        loop {
            let cut = cut_with_ellipsis(content, end);
            let layout = self.wrapped(&cut, role, width, None);
            if end == 0
                || layout
                    .as_ref()
                    .is_some_and(|l| l.lines().len() <= lines as usize)
            {
                return Fitted {
                    height: height(&layout),
                    content: cut,
                    cut: true,
                };
            }
            // the ellipsis wrapped onto a line of its own: give it a character more room
            end = content.text[..end]
                .char_indices()
                .next_back()
                .map_or(0, |(at, _)| at);
        }
    }

    /// The action labels, each cut to an equal share of `width` when the row would not fit it.
    fn fit_labels(&self, labels: Vec<(String, String)>, width: f64) -> Vec<(String, String)> {
        if labels.is_empty() {
            return labels;
        }
        let role = self.theme.type_scale.body;
        let inset = self.theme.metrics.button_inset;
        let natural = |label: &str| {
            self.wrapped(
                &plain(label, FontWeight::REGULAR),
                role,
                f64::INFINITY,
                None,
            )
            .map_or(0.0, |l| l.size().width)
                + 2.0 * inset
        };
        let gaps = BUTTON_GAP * (labels.len() - 1) as f64;
        let total: f64 = labels.iter().map(|(_, label)| natural(label)).sum::<f64>() + gaps;
        if total <= width {
            return labels;
        }
        let share = (width - gaps) / labels.len() as f64;
        labels
            .into_iter()
            .map(|(key, label)| {
                if natural(&label) <= share {
                    return (key, label);
                }
                let text = plain(&label, FontWeight::REGULAR);
                let fitted = self.fit(&text, role, 1, (share - 2.0 * inset).max(1.0));
                (key, fitted.content.text)
            })
            .collect()
    }

    /// What `n`'s card shows `width` points wide in `fold`, and its height.
    pub fn card(&self, n: &Notification, width: f64, fold: Fold) -> CardLayout {
        let column = Self::column_width(width);
        let role = self.theme.type_scale.body;
        let body = body_text(n);
        let folded = body
            .as_ref()
            .map(|b| self.fit(&b.content, role, BODY_LINES, column));
        let foldable = fold != Fold::Banner && folded.as_ref().is_some_and(|f| f.cut);
        let shown = match (fold, &body) {
            (Fold::Unfolded, Some(b)) if foldable => {
                Some(self.fit(&b.content, role, UNFOLDED_BODY_LINES, column))
            }
            _ => folded,
        };
        let side = self.disclosure_side();
        let summary_width = if foldable {
            column - side - BUTTON_GAP
        } else {
            column
        };
        let summary = self.fit(
            &plain(&n.summary, FontWeight::SEMIBOLD),
            role,
            SUMMARY_LINES,
            summary_width,
        );
        let mut text = if foldable {
            summary.height.max(side)
        } else {
            summary.height
        };
        let body = body.zip(shown).map(|(b, shown)| {
            text += LINE_GAP + shown.height;
            // a link ends where the kept text does, before the ellipsis
            let end = shown.content.text.len() - if shown.cut { ELLIPSIS.len_utf8() } else { 0 };
            let links = b
                .links
                .into_iter()
                .filter_map(|(range, href)| {
                    let range = range.start..range.end.min(end);
                    (range.start < range.end && shown.content.text.is_char_boundary(range.end))
                        .then_some((range, href))
                })
                .collect();
            BodyText {
                content: shown.content,
                links,
            }
        });
        let labels = n
            .button_actions()
            .take(MAX_BUTTONS)
            .map(|a| (a.key.clone(), a.label.clone()))
            .collect::<Vec<_>>();
        if !labels.is_empty() {
            text += BUTTON_TOP_GAP + BUTTON_ROW;
        }
        CardLayout {
            summary: summary.content,
            summary_width,
            body,
            foldable,
            actions: self.fit_labels(labels, column),
            height: (2.0 * PADDING + text.max(ICON_SIZE)).ceil(),
        }
    }
}

impl BannerMeasure for CardMetrics {
    fn banner_height(&mut self, n: &Notification, width: f64) -> f64 {
        self.card(n, width, Fold::Banner).height
    }
}

/// `content` up to byte `end` without the spaces before it, followed by an ellipsis in the style of the run it ends.
fn cut_with_ellipsis(content: &AttributedText, end: usize) -> AttributedText {
    let end = content.text[..end].trim_end().len();
    let mut text = content.text[..end].to_owned();
    text.push(ELLIPSIS);
    let mut runs: Vec<(TextRange, TextStyle)> = content
        .runs
        .iter()
        .filter(|(range, _)| range.0.start < end)
        .map(|(range, style)| {
            (
                TextRange(range.0.start..range.0.end.min(end)),
                style.clone(),
            )
        })
        .collect();
    match runs.last_mut() {
        Some((range, _)) => range.0.end = text.len(),
        None => {
            let style = content
                .runs
                .first()
                .map_or_else(|| TextStyle::new(13.0), |(_, style)| style.clone());
            runs.push((TextRange(0..text.len()), style));
        }
    }
    AttributedText { text, runs }
}

/// Width of every window: the card, its right margin, the leftward swipe travel and the shadow on the left.
pub fn strip_width(layout: &StackLayout, swipe: &SwipeConfig) -> f64 {
    (layout.width + layout.right_margin + swipe.left_limit.abs() + SHADOW_MARGIN).ceil()
}

/// The key of a banner's window: a replaced notification keeps its window unless its height changed, as a layer
/// surface keeps the size it was made with.
pub fn banner_key(n: &Notification, height: f64) -> WindowKey {
    WindowKey(Key::from(format!(
        "banner-{}-{}",
        n.id,
        height.ceil() as u32
    )))
}

pub fn centre_key() -> WindowKey {
    WindowKey(Key::from("centre"))
}

/// The windows for the model at `now`: the banners on screen, or the open centre instead of them (they stay in the
/// model and show again when it closes).
pub fn windows<M: Clone + 'static>(
    center: &mut NotificationCenter,
    ui: &Ui,
    metrics: &CardMetrics,
    now: f64,
    map: fn(Input) -> M,
) -> Vec<Window<M>> {
    let width = strip_width(&center.config.layout, &center.config.swipe);
    if ui.centre_open {
        return vec![centre_window(center, ui, metrics, width, map)];
    }
    let placements = center.placements(Size::new(width, 0.0), &mut metrics.clone(), now);
    placements
        .into_iter()
        .filter(|p| p.opacity > MIN_OPACITY)
        .filter_map(|p| {
            let n = center.get(p.id)?.clone();
            let layout = metrics.card(&n, p.rect.width(), Fold::Banner);
            let card = Rect::new(p.rect.x(), SHADOW_MARGIN, p.rect.width(), p.rect.height());
            // the resting card: the swipe keeps the pointer with an implicit grab, so the rest of the strip passes
            // the pointer through to the windows under it
            let rest = &center.config.layout;
            let region = Rect::new(
                width - rest.right_margin - rest.width,
                SHADOW_MARGIN,
                rest.width,
                card.height(),
            );
            Some(banner_window(
                &n, &layout, card, region, p.opacity, p.hovered, width, map,
            ))
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
            ui.unfolded.remove(&id);
        }
        Input::Invoke(id, key) => return Some(HostAction::Invoke(id, key)),
        // the host closes the centre once it has the activation token, which needs the centre's window
        Input::Activate(id) => return Some(HostAction::Activate(id)),
        Input::OpenLink(href) => return Some(HostAction::OpenLink(href)),
        Input::Unfold(id, true) => {
            ui.unfolded.insert(id);
        }
        Input::Unfold(id, false) => {
            ui.unfolded.remove(&id);
        }
        // whole points, so the cards and the materials the compositor draws under them stay on the same pixels
        Input::Scroll(state) => {
            ui.scroll = ScrollState {
                offset: Point::new(state.offset.x.round(), state.offset.y.round()),
                ..state
            }
        }
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

fn spec(title: String, size: Size, input_region: Option<Vec<Rect>>) -> WindowSpec {
    WindowSpec {
        title,
        app_id: APP_ID.to_owned(),
        size,
        min_size: None,
        max_size: None,
        decorations: DecorationMode::None,
        foreign_parent: None,
        input_region,
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

#[allow(clippy::too_many_arguments)]
fn banner_window<M: Clone + 'static>(
    n: &Notification,
    layout: &CardLayout,
    card: Rect,
    region: Rect,
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
            material: true,
            name: accessible_name(n),
            description: n.body_text(),
            role: Role::Group,
            index: None,
            first: false,
            live: Some(live),
            clickable: false,
            map,
            children: card_content(
                n,
                layout,
                card.size,
                hovered.then_some(Input::Dismiss(n.id)),
                None,
                map,
            ),
        })
        .key(format!("card-{}", n.id))],
    });
    window(
        banner_key(n, layout.height),
        spec(accessible_name(n), size, Some(vec![region])),
        content,
    )
}

fn centre_window<M: Clone + 'static>(
    center: &NotificationCenter,
    ui: &Ui,
    metrics: &CardMetrics,
    width: f64,
    map: fn(Input) -> M,
) -> Window<M> {
    let layout = &center.config.layout;
    let stack = GroupStack::default();
    let x = width - layout.right_margin - layout.width;
    let groups = center.groups();
    let scopes = center.clear_scopes();

    let mut header = vec![
        Element::new(Text {
            content: plain(CENTRE_TITLE, FontWeight::SEMIBOLD),
            role: TextRole::Section,
            wrap: false,
            secondary: false,
            selected: false,
        }),
        Element::new(Spacer::default()),
    ];
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
    let header_rect = Rect::new(x, 0.0, layout.width, HEADER_HEIGHT);
    let header = Element::new(Layer {
        size: Size::new(width, HEADER_HEIGHT),
        role: None,
        count: 0,
        children: vec![
            // the panel keeps the title readable on any wallpaper
            Element::new(Card {
                id: 0,
                rect: header_rect,
                opacity: 1.0,
                brightness: 0.0,
                material: true,
                name: String::new(),
                description: String::new(),
                role: Role::Group,
                index: None,
                first: false,
                live: None,
                clickable: false,
                map,
                children: Vec::new(),
            }),
            Element::new(Place {
                rect: header_rect,
                axis: Axis::Horizontal,
                align: Align::Center,
                padding: Insets {
                    left: PADDING,
                    right: HEADER_TRAILING,
                    top: 0.0,
                    bottom: 0.0,
                },
                spacing: BUTTON_GAP,
                children: header,
            })
            .key("header"),
        ],
    })
    .key("header-row");

    // the list: the first card a gap below the header, each group as tall as its stack
    let mut y = HEADER_GAP;
    let mut cards = Vec::new();
    for (index, group) in groups.iter().enumerate() {
        let front = group.notifications[0];
        let count = group.notifications.len();
        let fold = if ui.unfolded.contains(&front.id) {
            Fold::Unfolded
        } else {
            Fold::Folded
        };
        let card = metrics.card(front, layout.width, fold);
        let rect = Rect::new(x, y, layout.width, card.height);
        // the cards behind first, the farthest first, so the front card covers them; they are opaque, as a material
        // under the front card would show through it
        for behind in stack.cards(rect, count).into_iter().rev() {
            cards.push(Element::new(Card {
                id: front.id,
                rect: behind.rect,
                opacity: 1.0,
                brightness: behind.brightness,
                material: false,
                name: String::new(),
                description: String::new(),
                role: Role::Group,
                index: None,
                first: false,
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
        let disclosure = card.foldable.then_some((front.id, fold == Fold::Unfolded));
        cards.push(
            Element::new(Card {
                id: front.id,
                rect,
                opacity: 1.0,
                brightness: 0.0,
                material: true,
                name: accessible_name(front),
                description,
                role: Role::ListItem,
                index: Some(index),
                first: index == 0,
                live: None,
                clickable: true,
                map,
                children: card_content(
                    front,
                    &card,
                    rect.size,
                    Some(Input::Clear(front.id)),
                    disclosure,
                    map,
                ),
            })
            .key(format!("card-{}", front.id)),
        );
        y += stack.height(card.height, count);
    }
    let content_height = y;
    let list = Element::from(ScrollView {
        content: Element::new(Layer {
            size: Size::new(width, content_height),
            role: Some(Role::List),
            count: groups.len(),
            children: cards,
        })
        .key("list-content"),
        state: ui.scroll,
        horizontal: false,
        vertical: true,
        scroller: Scroller::Overlay,
        on_scroll: Box::new(move |state| map(Input::Scroll(state))),
    })
    .fill()
    .key("list");
    let mut children = vec![Element::new(Column {
        insets: Insets {
            top: layout.top_inset,
            bottom: BOTTOM_MARGIN,
            left: 0.0,
            right: 0.0,
        },
        children: vec![header, list],
    })];

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
        let rect = Rect::new(x + layout.width - MENU_WIDTH, top, MENU_WIDTH, height);
        children.push(
            Element::new(Layer {
                size: Size::new(width, top + height),
                role: None,
                count: 0,
                children: vec![Element::new(Card {
                    id: 0,
                    rect,
                    opacity: 1.0,
                    brightness: 0.0,
                    // over the cards: the window's material lies under all of it, so the menu paints its fill
                    material: false,
                    name: "Clear".into(),
                    description: String::new(),
                    role: Role::Group,
                    index: None,
                    first: false,
                    live: None,
                    clickable: false,
                    map,
                    children: vec![Element::new(Place {
                        rect: Rect::new(0.0, 0.0, MENU_WIDTH, height),
                        axis: Axis::Vertical,
                        align: Align::Stretch,
                        padding: Insets {
                            top: PADDING / 2.0,
                            bottom: PADDING / 2.0,
                            left: PADDING / 2.0,
                            right: PADDING / 2.0,
                        },
                        spacing: 0.0,
                        children: rows,
                    })],
                })
                .key("clear-menu")],
            })
            .key("menu-layer"),
        );
    }
    let content = Element::new(CentreRoot {
        map,
        menu_open: ui.menu_open,
        children,
    });
    // the layer surface spans the output's right edge from top to bottom; this height is what the list asks for
    let natural = layout.top_inset + HEADER_HEIGHT + content_height + BOTTOM_MARGIN;
    window(
        centre_key(),
        spec(
            CENTRE_TITLE.to_owned(),
            Size::new(width, natural.ceil()),
            None,
        ),
        content,
    )
}

fn plain(text: &str, weight: FontWeight) -> AttributedText {
    let mut style = TextStyle::new(13.0);
    style.weight = weight;
    AttributedText::plain(text, style)
}

/// The body with its markup as text runs, its line breaks kept, and its links; `None` when it is empty. At most
/// [`MAX_BODY_BYTES`] of it.
fn body_text(n: &Notification) -> Option<BodyText> {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links = Vec::new();
    for span in &n.spans {
        let mut piece = span.text.replace("\r\n", "\n").replace('\r', "\n");
        let room = MAX_BODY_BYTES.saturating_sub(text.len());
        if piece.len() > room {
            let mut end = room;
            while !piece.is_char_boundary(end) {
                end -= 1;
            }
            piece.truncate(end);
        }
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
            links.push((start..text.len(), href.clone()));
        }
    }
    (!text.trim().is_empty()).then_some(BodyText {
        content: AttributedText { text, runs },
        links,
    })
}

/// The icon, the summary, the body and the action buttons of a card `size` large, its close button when `close` is
/// given, and the disclosure button of a foldable centre card (its id and whether it is unfolded).
fn card_content<M: Clone + 'static>(
    n: &Notification,
    layout: &CardLayout,
    size: Size,
    close: Option<Input>,
    disclosure: Option<(u32, bool)>,
    map: fn(Input) -> M,
) -> Vec<Element<M>> {
    let summary = Element::new(Text {
        content: layout.summary.clone(),
        role: TextRole::Body,
        wrap: true,
        secondary: false,
        selected: false,
    });
    let mut lines = vec![match disclosure {
        Some((id, unfolded)) => Element::new(Stack {
            axis: Axis::Horizontal,
            padding: Insets::ZERO,
            spacing: BUTTON_GAP,
            align: Align::Start,
            children: vec![
                summary.width(layout.summary_width),
                Element::new(DisclosureButton {
                    label: if unfolded { "Show Less" } else { "Show More" }.into(),
                    expanded: unfolded,
                    style: DisclosureStyle::Triangle,
                    enabled: true,
                    on_toggle: Some(Box::new(move |open| map(Input::Unfold(id, open)))),
                })
                .key("disclosure"),
            ],
        }),
        None => summary,
    }];
    if let Some(body) = &layout.body {
        lines.push(Element::new(RichText {
            content: body.content.clone(),
            role: TextRole::Body,
            wrap: true,
            secondary: true,
            links: body
                .links
                .iter()
                .map(|(range, href)| TextLink {
                    range: range.clone(),
                    url: href.clone(),
                    on_open: Some(map(Input::OpenLink(href.clone()))),
                })
                .collect(),
        }));
    }
    let buttons: Vec<Element<M>> = layout
        .actions
        .iter()
        .map(|(key, label)| {
            Element::new(Button {
                label: label.clone(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Pill,
                enabled: true,
                busy: false,
                on_press: Some(map(Input::Invoke(n.id, key.clone()))),
            })
            .key(format!("action-{key}"))
        })
        .collect();
    if !buttons.is_empty() {
        let top = BUTTON_TOP_GAP - LINE_GAP;
        lines.push(Element::new(Row {
            height: top + BUTTON_ROW,
            top,
            children: buttons,
        }));
    }
    let icon = if n.icon.is_empty() || n.icon.contains('/') {
        "bell".to_owned()
    } else {
        n.icon.clone()
    };
    let mut content = vec![Element::new(Place {
        rect: Rect::from_origin_size(Point::ZERO, size),
        axis: Axis::Horizontal,
        align: Align::Start,
        padding: Insets {
            top: PADDING,
            left: PADDING,
            bottom: PADDING,
            right: PADDING,
        },
        spacing: ICON_GAP,
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
                self.hover(state, self.card.contains(position), cx);
                Propagation::Stop
            }
            Event::Cancel if state.press.is_some() => {
                state.press = None;
                cx.emit((self.map)(Input::SwipeCancel(self.id)));
                self.hover(state, false, cx);
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &StripState, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &StripState, _: &mut SemanticsCx<'_>) {}
}

/// The centre's window content: a press no card, button or menu took, and Escape close the Clear menu, then the
/// centre.
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
            ..LayoutSpec::new(ContainerKind::Overlay)
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
            ) if cx.phase() != Phase::Capture => {
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
        cx.role(Role::Group);
        cx.name(CENTRE_TITLE);
        cx.dismissible();
    }
}

/// The centre's header and list, one above the other, inset from the window's top and bottom.
struct Column<M> {
    insets: Insets,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Column<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: self.insets,
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Vertical,
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
        if self.insets != old.insets {
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

/// Children at their own positions in a box `size` large: the header row, the list's content, the Clear menu's layer.
/// With a role it is the list of the centre's cards, `count` of them.
struct Layer<M> {
    size: Size,
    role: Option<Role>,
    count: usize,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Layer<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.size.width),
            height: SizeSpec::Fixed(self.size.height),
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.size != old.size {
            Change::Layout
        } else if self.role != old.role || self.count != old.count {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if let Some(role) = self.role {
            cx.role(role);
            cx.name("Notifications");
            cx.item_count(self.count);
        }
    }
}

/// A row placed at `rect` in an absolute parent.
struct Place<M> {
    rect: Rect,
    axis: Axis,
    align: Align,
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
                align: self.align,
                justify: Justify::Start,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect
            || self.padding != old.padding
            || self.spacing != old.spacing
            || self.align != old.align
        {
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

/// The row of action buttons, `height` tall with the buttons `top` below its top edge, so a card's height is known
/// before Kit measures its buttons.
struct Row<M> {
    height: f64,
    top: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Row<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets {
                top: self.top,
                ..Insets::ZERO
            },
            height: SizeSpec::Fixed(self.height),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: BUTTON_GAP,
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
        if self.height != old.height || self.top != old.top {
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

/// A card's platter at `rect` in an absolute parent, with its content over it and faded with it.
struct Card<M> {
    id: u32,
    rect: Rect,
    opacity: f64,
    /// Darkening of a card behind a group's front one, from 0 down.
    brightness: f64,
    /// The compositor's material under it where the window shows one; else, and for a card over others or behind a
    /// group's front one, the opaque fill.
    material: bool,
    /// Empty for a card behind, which is decoration only.
    name: String,
    description: String,
    role: Role,
    /// Its place in the centre's list.
    index: Option<usize>,
    /// The centre's first card, which takes the focus when the centre opens.
    first: bool,
    live: Option<Live>,
    /// A click, Return or Space on the card activates it and Delete clears it (the centre's cards; a banner's clicks
    /// come from its swipe).
    clickable: bool,
    map: fn(Input) -> M,
    children: Vec<Element<M>>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CardState {
    armed: bool,
}

/// `rect` with its corners rounded by `radius`, as a path of four quarter circles.
fn rounded_path(rect: Rect, radius: f64) -> Vec<PathVerb> {
    // a quarter circle's control points sit this far along its tangents, as a fraction of the radius
    const KAPPA: f64 = 0.552_284_75;
    let (l, t, r, b) = (rect.min_x(), rect.min_y(), rect.max_x(), rect.max_y());
    let k = radius * KAPPA;
    vec![
        PathVerb::MoveTo(Point::new(l + radius, t)),
        PathVerb::LineTo(Point::new(r - radius, t)),
        PathVerb::CubicTo(
            Point::new(r - radius + k, t),
            Point::new(r, t + radius - k),
            Point::new(r, t + radius),
        ),
        PathVerb::LineTo(Point::new(r, b - radius)),
        PathVerb::CubicTo(
            Point::new(r, b - radius + k),
            Point::new(r - radius + k, b),
            Point::new(r - radius, b),
        ),
        PathVerb::LineTo(Point::new(l + radius, b)),
        PathVerb::CubicTo(
            Point::new(l + radius - k, b),
            Point::new(l, b - radius + k),
            Point::new(l, b - radius),
        ),
        PathVerb::LineTo(Point::new(l, t + radius)),
        PathVerb::CubicTo(
            Point::new(l, t + radius - k),
            Point::new(l + radius - k, t),
            Point::new(l + radius, t),
        ),
        PathVerb::Close,
    ]
}

impl<M: 'static> Widget<M> for Card<M> {
    type State = CardState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            overflow: Overflow::Clip,
            ..LayoutSpec::new(ContainerKind::Absolute)
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
            || self.material != old.material
            || self.name != old.name
            || self.description != old.description
            || self.role != old.role
            || self.index != old.index
            || self.first != old.first
            || self.live != old.live
            || self.clickable != old.clickable
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
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        let active = self.clickable && !self.name.is_empty();
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
                if !active {
                    // a card's own area is no press outside the cards, nor is a banner's (its strip takes it)
                    return if self.clickable || self.live.is_none() {
                        Propagation::Stop
                    } else {
                        Propagation::Continue
                    };
                }
                if pressed {
                    state.armed = true;
                } else if std::mem::take(&mut state.armed) && cx.hit(position) {
                    cx.emit((self.map)(Input::Activate(self.id)));
                }
                Propagation::Stop
            }
            Event::Key(key, _) if active && key.pressed && cx.node() == cx.target() => {
                match key.keysym {
                    RETURN | KEYPAD_ENTER | SPACE => cx.emit((self.map)(Input::Activate(self.id))),
                    DELETE | BACKSPACE => cx.emit((self.map)(Input::Clear(self.id))),
                    // the arrows and page keys scroll the list
                    _ => return Propagation::Continue,
                }
                Propagation::Stop
            }
            Event::Focus { .. } if cx.node() == cx.target() => {
                cx.request_paint();
                Propagation::Continue
            }
            Event::Cancel => {
                state.armed = false;
                Propagation::Continue
            }
            Event::Action(ref request) if request.action == Action::Click && self.id != 0 => {
                cx.emit((self.map)(Input::Activate(self.id)));
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &CardState, cx: &mut PaintCx<'_>) {
        let opacity = self.opacity.clamp(0.0, 1.0);
        let rect = Rect::new(0.0, 0.0, self.rect.width(), self.rect.height());
        let radius = CORNER_RADIUS.min(rect.height() / 2.0);
        let shadow = Effect::DropShadow {
            offset: Point::new(0.0, 1.0),
            blur: SHADOW_BLUR,
            color: Color::new(0.0, 0.0, 0.0, SHADOW_ALPHA),
        };
        let shape = Shape::RoundedRect { rect, radius };
        let theme = cx.env().theme.clone();
        let Some(fill) = theme.surface(Surface::Popover).fill else {
            return;
        };
        if self.material && cx.material(Surface::Popover, radius, opacity) {
            // the compositor draws the material under the card: the shadow is drawn only outside the card, from an
            // opaque copy of its shape the clip hides, and the text casts none
            cx.set_group(opacity as f32, Vec::new());
            let mut ring = rounded_path(rect.outset(SHADOW_REACH), 0.0);
            ring.extend(rounded_path(rect, radius));
            cx.push(PaintOp::PushClip(Shape::Path {
                verbs: ring,
                fill_rule: FillRule::EvenOdd,
            }));
            cx.push(PaintOp::PushLayer {
                opacity: 1.0,
                effects: vec![shadow],
            });
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(fill),
            });
            cx.push(PaintOp::PopLayer);
            cx.push(PaintOp::PopClip);
        } else {
            cx.set_group(opacity as f32, vec![shadow]);
            let darken = (1.0 + self.brightness).clamp(0.0, 1.0) as f32;
            let fill = Color::new(fill.r * darken, fill.g * darken, fill.b * darken, fill.a);
            cx.push(PaintOp::Fill {
                shape: shape.clone(),
                paint: Paint::Solid(fill),
            });
        }
        if let Some(outline) = color(cx, ColorRole::Separator) {
            cx.push(PaintOp::Stroke {
                shape,
                width: OUTLINE_WIDTH,
                paint: Paint::Solid(outline),
            });
        }
        if cx.is_focused() && cx.focus_visible() && cx.window_active() {
            let ring = &theme.focus;
            let color = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL);
            if let Some(color) = color {
                let inset = ring.width / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(-inset),
                        radius: (radius - inset).max(0.0),
                    },
                    width: ring.width,
                    paint: Paint::Solid(color),
                });
            }
        }
    }

    fn semantics(&self, _: &CardState, cx: &mut SemanticsCx<'_>) {
        // a card behind a group's front one is decoration
        if self.name.is_empty() {
            return;
        }
        cx.role(self.role);
        cx.name(self.name.clone());
        if !self.description.is_empty() {
            cx.description(self.description.clone());
        }
        if let Some(live) = self.live {
            cx.live(live);
        }
        if let Some(index) = self.index {
            cx.item_index(index);
        }
        if self.clickable {
            cx.focusable();
            if self.first {
                cx.initial_focus();
            }
        }
        if self.id != 0 {
            cx.action(Action::Click);
        }
    }
}

/// The round close button inside a card's top-leading corner.
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
            position: Point::new(CLOSE_INSET, CLOSE_INSET),
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
        if let Some(edge) = color(cx, ColorRole::Separator) {
            cx.push(PaintOp::Stroke {
                shape: Shape::RoundedRect {
                    rect,
                    radius: CLOSE_DIAMETER / 2.0,
                },
                width: OUTLINE_WIDTH,
                paint: Paint::Solid(edge),
            });
        }
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
mod tests;
