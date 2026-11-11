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
//! it arrives. A bridge to assistive technology announces a live node only as it appears or is renamed, and says its
//! name alone, so a banner is named after what it shows ([`banner_speech`]): a new banner, or one whose text changes
//! in place, is said whole. While the screen is locked no window is open, as the lock screen hides them; a banner
//! still on screen at the unlock appears then without being said again, as it was said when it arrived. A banner or
//! card of an application whose previews are off shows only "Notification" ([`NotificationCenter::shown`]). The cards
//! of the centre are not live. They are the items of
//! a list, named after their application and summary with the whole body as description, Tab stops that Return or
//! Space activates and Delete clears.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Debug;
use std::ops::Range;
use std::sync::Arc;

use cupertino_animation::{Spring, SpringParams};
use cupertino_kit::controls::{
    Button, ButtonRole, ButtonStyle, DisclosureButton, DisclosureStyle, Icon, IconDirection,
    RichText, ScrollView, Scroller, Stack, Text, TextLink, TextRole,
};
use cupertino_kit::core::{
    AccessibleStates, Action, Change, CommandRegistry, Element, Environment, Event, EventCx, Glass,
    Key, Live, MeasureCx, PaintCx, Phase, Propagation, Role, SemanticsCx, Widget, Window,
    WindowBackground, WindowKey, MAX_MATERIALS,
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

use crate::center::{
    BannerMeasure, ClearScope, NotificationCenter, StackLayout, SwipeConfig, SwipeOutcome,
};
use crate::notification::{
    cap, Notification, Urgency, ELLIPSIS, MAX_APP_NAME_CHARS, MAX_SUMMARY_CHARS,
};
use crate::stack::GroupStack;
use controls::{CardControl, Kind};
pub use motion::{Gap, Removal, Rise, Slide, SLIDE_DISTANCE};

/// The `app_id` of every window of the process.
pub const APP_ID: &str = "cupertino-notifications";
/// The title of the notification centre's window, which tells it from the banners' windows.
pub const CENTRE_TITLE: &str = "Notification Centre";
/// Room around a banner card in its window for the card's shadow, in points: the shadow's reach.
pub const SHADOW_MARGIN: f64 = SHADOW_REACH;
/// Banners below this opacity get no window.
pub const MIN_OPACITY: f64 = 0.002;

/// Card corner radius.
pub const CORNER_RADIUS: f64 = 18.0;
/// Space between a card's top and bottom edges and its content.
pub const PADDING: f64 = 12.0;
/// Space between a card's leading edge and its icon, and between its text and its trailing edge.
pub const LEADING: f64 = 13.0;
pub const TRAILING: f64 = 16.0;
/// The application icon at the card's leading edge, and the gap between it and the text.
pub const ICON_SIZE: f64 = 32.0;
pub const ICON_GAP: f64 = 14.0;
/// The least height of a card.
pub const MIN_CARD_HEIGHT: f64 = 58.0;
/// Room kept at the end of a summary's lines for the age at the card's top right, and the age's type size.
pub const AGE_WIDTH: f64 = 56.0;
pub const AGE_SIZE: f64 = 11.0;
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
pub const CLOSE_DIAMETER: f64 = 20.0;
/// How far the close button reaches past the card's leading and top edges: it is centred on the card's corner curve,
/// inside the corner point.
pub const CLOSE_REACH_X: f64 = 5.75;
pub const CLOSE_REACH_Y: f64 = 4.25;
// a banner's window holds the part of the close button past the card
const _: () = assert!(CLOSE_REACH_X <= SHADOW_MARGIN);
/// Outline of a card: 0.5 pt.
pub const OUTLINE_WIDTH: f64 = 0.5;
/// The centre's header row (title and Clear All), the gap under it and the title's inset from the cards' leading edge.
pub const HEADER_HEIGHT: f64 = 33.0;
pub const HEADER_GAP: f64 = 5.0;
pub const TITLE_INSET: f64 = 2.5;
/// The centre's title size.
pub const TITLE_SIZE: f64 = 19.0;
/// The header's clear button: a circle of [`CLOSE_DIAMETER`] that grows into a capsule this wide, naming itself, while
/// the pointer is over it or it has the keyboard.
pub const CLEAR_WIDTH: f64 = 86.0;
/// An expanded group: its header row (its application, Show Less and its clear button) and the gap between its cards.
pub const GROUP_HEADER: f64 = 30.0;
pub const CARD_GAP: f64 = 6.0;
/// How long a group takes to expand or collapse, as a spring's perceptual duration, without bounce.
pub const EXPAND_DURATION: f64 = 0.35;
/// Space between the centre's list and the bottom edge of the output.
pub const BOTTOM_MARGIN: f64 = 16.0;
/// How far left of the cards the centre takes input, and how tall (more than any output) its region is.
const CENTRE_EDGE: f64 = 28.0;
const CENTRE_REGION_HEIGHT: f64 = 32768.0;
/// A card's halo fades out over this many points as the card goes under the header or past the list's bottom edge.
const HALO_FADE: f64 = 40.0;
/// The height of the layer the halos are drawn in, more than any output.
const HALO_LAYER_HEIGHT: f64 = 8192.0;
/// The Clear menu's width and row height.
pub const MENU_WIDTH: f64 = 180.0;
pub const MENU_ROW: f64 = 28.0;
/// The halo around a card: a wide dark Gaussian, its blur twice its standard deviation of 32 pt, peaking at 0.45 black.
const SHADOW_BLUR: f64 = 64.0;
const SHADOW_ALPHA: f32 = 0.45;
/// How far beyond a card its shadow reaches, three standard deviations: the area the shadow is drawn in.
pub const SHADOW_REACH: f64 = 96.0;

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
    /// The close button of a card of the centre.
    Clear(u32),
    /// Delete or Backspace on the focused card of the centre: it is cleared and the focus goes to the card that takes
    /// its place.
    ClearFocused(u32),
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
    /// The pointer moved onto this card of the centre; `None` when the pointer is over no card. A revealed card shows
    /// its close and action buttons.
    Reveal(Option<u32>),
    /// The keyboard came to this card or into it, its focus ring showing (`true`), or left it (`false`). The card the
    /// keyboard is on shows its buttons too, whatever the pointer does, so Tab reaches them.
    Focused(u32, bool),
    /// Expand (`true`) or collapse the group with this key.
    Expand(String, bool),
    /// Clear every notification of the group with this key.
    ClearGroup(String),
    /// Clear these notifications as their cards leave (they blur, shrink and fade, then the cards below rise into the
    /// `gap` they leave); at once with Reduce Motion.
    Leave(Vec<u32>, Gap),
    /// Delete or Backspace on the focused card, as [`Input::ClearFocused`], with the card leaving.
    LeaveFocused(u32, Gap),
    /// Clear every notification of the group with this key, as [`Input::ClearGroup`], with the cards leaving.
    LeaveGroup(String, Gap),
    /// The pointer or the keyboard is on the close circle of this collapsed group's card (or `None`: it is not), which
    /// grows into the capsule that clears the group.
    CloseWide(Option<u32>),
}

/// What the windows show beyond the model: whether the centre and its Clear menu are open, where its list is
/// scrolled to, which of its cards are unfolded, which one is revealed, and which groups are expanded or collapsing.
#[derive(Clone, Debug, PartialEq)]
pub struct Ui {
    /// The centre is open and takes input. It stays on screen while it slides out after it is closed
    /// ([`Ui::centre_shown`]).
    pub centre_open: bool,
    /// Where the centre's list is between its place and out of the output.
    pub slide: Slide,
    /// Cards that were cleared and are leaving, then removed from the model.
    pub removals: Vec<Removal>,
    /// Gaps cleared cards left, which the cards below rise into.
    pub rises: Vec<Rise>,
    /// The collapsed group whose close circle has grown into its capsule, by its front card.
    pub close_wide: Option<u32>,
    pub menu_open: bool,
    pub scroll: ScrollState,
    pub unfolded: BTreeSet<u32>,
    pub revealed: Option<u32>,
    /// The card the keyboard is on (or in), its focus ring showing: it shows its buttons as a revealed card does.
    pub focused: Option<u32>,
    /// Expanded groups by key, with when they began to expand.
    pub expanded: BTreeMap<String, f64>,
    /// Groups collapsing by key, with when they began to.
    pub collapsing: BTreeMap<String, f64>,
}

/// The spring a group expands and collapses with.
fn expand_spring() -> Spring {
    Spring::new(SpringParams::from_perceptual(EXPAND_DURATION, 0.0))
}

impl Ui {
    /// How far the group `key` is expanded at `now`: 0 collapsed, 1 expanded.
    pub fn expansion(&self, key: &str, now: f64) -> f64 {
        let spring = expand_spring();
        let progress = |start: f64| {
            let t = now - start;
            if t >= spring.settling_duration() {
                1.0
            } else {
                spring.value(t).clamp(0.0, 1.0)
            }
        };
        match (self.expanded.get(key), self.collapsing.get(key)) {
            (Some(&start), _) => progress(start),
            (None, Some(&start)) => 1.0 - progress(start),
            (None, None) => 0.0,
        }
    }

    /// Whether something of the centre moves at `now` (a group expanding or collapsing, the list sliding, a card
    /// leaving or the cards below it rising), so the centre needs frames.
    pub fn is_animating(&self, now: f64) -> bool {
        let settle = expand_spring().settling_duration();
        self.expanded
            .values()
            .chain(self.collapsing.values())
            .any(|&start| now - start < settle)
            || !self.slide.at_rest(now)
            || !self.removals.is_empty()
            || !self.rises.is_empty()
    }

    /// Whether the centre's window exists at `now`: while it is open and while it slides out.
    pub fn centre_shown(&self, now: f64) -> bool {
        self.centre_open || !self.slide.is_out(now)
    }

    /// Whether the centre is on its way out at `now`: it takes no input then.
    pub fn centre_closing(&self, now: f64) -> bool {
        !self.centre_open && !self.slide.is_out(now)
    }

    /// Open the centre; the list slides in from where it is now (at once with `reduce_motion`). A centre that was
    /// out of the output starts with a fresh list.
    pub fn open_centre(&mut self, now: f64, reduce_motion: bool) {
        if self.centre_open {
            return;
        }
        if self.slide.is_out(now) {
            *self = Ui {
                slide: self.slide,
                ..Ui::default()
            };
        }
        self.centre_open = true;
        self.slide.go_to(0.0, now, reduce_motion);
    }

    /// Close the centre: the list slides out from where it is now (at once with `reduce_motion`) and takes no input
    /// from here on.
    pub fn close_centre(&mut self, now: f64, reduce_motion: bool) {
        if !self.centre_open {
            return;
        }
        self.centre_open = false;
        self.menu_open = false;
        self.revealed = None;
        self.focused = None;
        self.close_wide = None;
        self.slide.go_to(SLIDE_DISTANCE, now, reduce_motion);
    }

    /// How far card `id` has left at `now`, 0 to 1; `None` while it is not leaving.
    pub fn leaving(&self, id: u32, now: f64) -> Option<f64> {
        self.removals
            .iter()
            .find(|r| r.ids.contains(&id))
            .map(|r| motion::leave_progress(r.start, now))
    }

    /// How far below its place a card lying from `y` down is at `now`, in points, while the cards rise into the gap
    /// of a card cleared above it.
    pub fn rise(&self, y: f64, now: f64) -> f64 {
        self.rises.iter().map(|r| r.shift(y, now)).sum()
    }

    /// Remove from the model the cards whose leaving has ended at `now`, and let the cards below rise into their gap.
    pub fn finish_removals(&mut self, center: &mut NotificationCenter, now: f64) {
        self.rises.retain(|r| !r.done(now));
        let (done, going): (Vec<Removal>, Vec<Removal>) = std::mem::take(&mut self.removals)
            .into_iter()
            .partition(|r| now - r.start >= motion::LEAVE_DURATION);
        self.removals = going;
        if done.is_empty() {
            return;
        }
        for removal in done {
            for id in removal.ids {
                center.clear(id, now);
                self.unfolded.remove(&id);
            }
            self.rises.push(Rise {
                start: now,
                gap: removal.gap,
            });
        }
        self.forget_gone_groups(center);
    }

    /// Drop what is kept of groups that are no longer in the list (cleared whole).
    fn forget_gone_groups(&mut self, center: &NotificationCenter) {
        let keys: BTreeSet<String> = center.groups().iter().map(|g| g.key.clone()).collect();
        self.expanded.retain(|key, _| keys.contains(key));
        self.collapsing.retain(|key, _| keys.contains(key));
    }
}

impl Default for Ui {
    fn default() -> Self {
        Ui {
            centre_open: false,
            slide: Slide::OUT,
            removals: Vec::new(),
            rises: Vec::new(),
            close_wide: None,
            menu_open: false,
            scroll: ScrollState {
                offset: Point::ZERO,
                content: Size::ZERO,
                viewport: Size::ZERO,
            },
            unfolded: BTreeSet::new(),
            revealed: None,
            focused: None,
            expanded: BTreeMap::new(),
            collapsing: BTreeMap::new(),
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
    /// Give the keyboard focus to the node with this key once the view is rebuilt
    /// ([`Command::Focus`](cupertino_kit::core::Command::Focus)).
    Focus(Key),
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
    /// The action buttons' keys and labels, the labels cut so the row fits the card: a banner's actions, and the
    /// actions after the first of an unfolded centre card.
    pub actions: Vec<(String, String)>,
    /// The first action of a centre card, which its pill shows over the card's corner while it is revealed.
    pub pill: Option<(String, String)>,
    /// The card has more to show unfolded: a body that is cut, or actions beyond the first.
    pub expandable: bool,
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
        width - LEADING - ICON_SIZE - ICON_GAP - TRAILING
    }

    /// The side of a disclosure button, which Kit takes from the size table at the theme's control size.
    pub fn disclosure_side(&self) -> f64 {
        self.theme
            .metrics
            .geometry(cupertino_kit::theme::ControlFamily::Disclosure)
            .height
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

    /// What `n`'s card shows `width` points wide in `fold`, and its height: a banner shows its action buttons, a centre
    /// card only while it is revealed ([`CardMetrics::card_with`]).
    pub fn card(&self, n: &Notification, width: f64, fold: Fold) -> CardLayout {
        self.card_with(n, width, fold, fold == Fold::Banner)
    }

    /// What `n`'s card shows `width` points wide in `fold`, and its height. A banner shows its action buttons in a
    /// row when `actions`; a centre card shows its first action as a pill over its corner and, unfolded and when
    /// `actions`, the others in a row.
    pub fn card_with(&self, n: &Notification, width: f64, fold: Fold, actions: bool) -> CardLayout {
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
            column - AGE_WIDTH - side - BUTTON_GAP
        } else {
            column - AGE_WIDTH
        };
        let summary = self.fit(
            &plain(&cap(&n.summary, MAX_SUMMARY_CHARS), FontWeight::SEMIBOLD),
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
        let all = n
            .button_actions()
            .take(MAX_BUTTONS)
            .map(|a| (a.key.clone(), a.label.clone()))
            .collect::<Vec<_>>();
        let banner = fold == Fold::Banner;
        let pill = if banner { None } else { all.first().cloned() };
        let expandable = foldable || (!banner && all.len() > 1);
        let labels = match (banner, fold == Fold::Unfolded) {
            (true, _) if actions => all,
            (false, true) if actions => all.into_iter().skip(1).collect(),
            _ => Vec::new(),
        };
        if !labels.is_empty() {
            text += BUTTON_TOP_GAP + BUTTON_ROW;
        }
        CardLayout {
            summary: summary.content,
            summary_width,
            body,
            foldable,
            pill,
            expandable,
            actions: self.fit_labels(labels, column),
            height: (2.0 * PADDING + text.max(ICON_SIZE))
                .ceil()
                .max(MIN_CARD_HEIGHT),
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

/// Where the close button of a card at `card` lies, in the frame `card` is given in: centred on the card's corner curve.
pub fn close_rect(card: Rect) -> Rect {
    Rect::new(
        card.min_x() - CLOSE_REACH_X,
        card.min_y() - CLOSE_REACH_Y,
        CLOSE_DIAMETER,
        CLOSE_DIAMETER,
    )
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

/// The key of the card of notification `id`, a banner's or the front card of its group in the centre.
pub fn card_key(id: u32) -> Key {
    Key::from(format!("card-{id}"))
}

pub fn centre_key() -> WindowKey {
    WindowKey(Key::from("centre"))
}

/// The windows for the model at `now`: the banners on screen, or the open centre instead of them (they stay in the
/// model and show again when it closes). None while the screen is locked: the lock screen hides them, so assistive
/// technology has nothing of them to read, and a banner still on screen at the unlock appears then, not announced
/// again ([`NotificationCenter::is_quiet`]).
pub fn windows<M: Clone + 'static>(
    center: &mut NotificationCenter,
    ui: &Ui,
    metrics: &CardMetrics,
    now: f64,
    map: fn(Input) -> M,
) -> Vec<Window<M>> {
    if center.screen_locked() {
        return Vec::new();
    }
    let width = strip_width(&center.config.layout, &center.config.swipe);
    if ui.centre_shown(now) {
        return vec![centre_window(center, ui, metrics, width, now, map)];
    }
    // the room above a banner's card is the shadow's reach, or what the output has above it
    let room = center.config.layout.top_inset.min(SHADOW_MARGIN);
    let placements = center.placements(Size::new(width, 0.0), &mut metrics.clone(), now);
    placements
        .into_iter()
        .filter(|p| p.opacity > MIN_OPACITY)
        .filter_map(|p| {
            let n = center.shown(center.get(p.id)?).into_owned();
            let quiet = center.is_quiet(p.id);
            let layout = metrics.card(&n, p.rect.width(), Fold::Banner);
            let card = Rect::new(p.rect.x(), room, p.rect.width(), p.rect.height());
            // the resting card and its close button, which reaches past its corner: the swipe keeps the pointer with an
            // implicit grab, so the rest of the strip passes the pointer through to the windows under it
            let rest = &center.config.layout;
            let region = Rect::new(
                width - rest.right_margin - rest.width,
                room,
                rest.width,
                card.height(),
            );
            Some(banner_window(
                &n,
                &layout,
                card,
                vec![region, close_rect(region)],
                p.opacity,
                p.hovered,
                p.leaving || quiet,
                width,
                &age(n.timestamp, now),
                map,
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
    // a centre on its way out takes no input
    if ui.centre_closing(now) {
        return None;
    }
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
            if ui.revealed == Some(id) {
                ui.revealed = None;
            }
            if ui.focused == Some(id) {
                ui.focused = None;
            }
        }
        Input::ClearFocused(id) => {
            let at = fronts(center, ui).iter().position(|&front| front == id);
            center.clear(id, now);
            ui.unfolded.remove(&id);
            return focus_after(center, ui, at);
        }
        Input::LeaveFocused(id, gap) => {
            let at = fronts(center, ui).iter().position(|&front| front == id);
            leave(center, ui, vec![id], gap, now);
            return focus_after(center, ui, at);
        }
        Input::Leave(ids, gap) => leave(center, ui, ids, gap, now),
        Input::LeaveGroup(key, gap) => {
            let ids = group_ids(center, &key);
            leave(center, ui, ids, gap, now);
        }
        Input::CloseWide(id) => ui.close_wide = id,
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
        Input::CloseCentre => ui.close_centre(now, center.config.motion.reduce_motion),
        Input::Reveal(id) => {
            if ui.revealed != id {
                ui.close_wide = None;
            }
            ui.revealed = id
        }
        Input::Focused(id, true) => ui.focused = Some(id),
        Input::Focused(id, false) => {
            if ui.focused == Some(id) {
                ui.focused = None;
            }
        }
        Input::Expand(key, expand) => {
            // with Reduce Motion a group is at once where it goes
            let start = if center.config.motion.reduce_motion {
                f64::NEG_INFINITY
            } else {
                now
            };
            if expand {
                ui.collapsing.remove(&key);
                ui.expanded.insert(key, start);
            } else if ui.expanded.remove(&key).is_some() {
                ui.collapsing.insert(key, start);
            }
            ui.revealed = None;
        }
        Input::ClearGroup(key) => {
            for id in group_ids(center, &key) {
                center.clear(id, now);
                ui.unfolded.remove(&id);
            }
            ui.expanded.remove(&key);
            ui.collapsing.remove(&key);
            ui.revealed = None;
        }
    }
    None
}

/// The ids of the notifications of the group with this key.
fn group_ids(center: &NotificationCenter, key: &str) -> Vec<u32> {
    center
        .groups()
        .iter()
        .filter(|g| g.key == key)
        .flat_map(|g| g.notifications.iter().map(|n| n.id))
        .collect()
}

/// The front card of each group of the list that is not leaving.
fn fronts(center: &NotificationCenter, ui: &Ui) -> Vec<u32> {
    center
        .groups()
        .iter()
        .filter_map(|g| {
            g.notifications
                .iter()
                .map(|n| n.id)
                .find(|&id| !ui.removals.iter().any(|r| r.ids.contains(&id)))
        })
        .collect()
}

/// The card now in the place of the front card that was `at` in the list: its group's next one, the next group's, or
/// the last one when it was last.
fn focus_after(center: &NotificationCenter, ui: &Ui, at: Option<usize>) -> Option<HostAction> {
    let after = fronts(center, ui);
    let next = at.and_then(|at| {
        after
            .get(at)
            .or(at.checked_sub(1).and_then(|i| after.get(i)))
    });
    next.map(|&front| HostAction::Focus(card_key(front)))
}

/// Clear `ids`: they leave first, then the cards below rise into `gap`; with Reduce Motion they are gone at once.
fn leave(center: &mut NotificationCenter, ui: &mut Ui, ids: Vec<u32>, gap: Gap, now: f64) {
    let ids: Vec<u32> = ids
        .into_iter()
        .filter(|&id| ui.leaving(id, now).is_none())
        .collect();
    if ids.is_empty() {
        return;
    }
    if ui.revealed.is_some_and(|id| ids.contains(&id)) {
        ui.revealed = None;
    }
    if ui.focused.is_some_and(|id| ids.contains(&id)) {
        ui.focused = None;
    }
    ui.close_wide = None;
    if center.config.motion.reduce_motion {
        for id in ids {
            center.clear(id, now);
            ui.unfolded.remove(&id);
        }
        ui.forget_gone_groups(center);
    } else {
        ui.removals.push(Removal {
            ids,
            start: now,
            gap,
        });
    }
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
    let (app, summary) = (
        cap(&n.app_name, MAX_APP_NAME_CHARS),
        cap(&n.summary, MAX_SUMMARY_CHARS),
    );
    match (app.trim(), summary.trim()) {
        ("", summary) => summary.to_owned(),
        (app, "") => app.to_owned(),
        (app, summary) => format!("{app}: {summary}"),
    }
}

/// What a banner is called, and so what a screen reader says as it arrives: its application, its summary and its
/// body as the card shows them, cut where the card cuts them, each run of spaces and line breaks one space ("Mail:
/// Lunch, Are you free at noon?"). Its buttons are its children, with names of their own.
pub fn banner_speech(n: &Notification, layout: &CardLayout) -> String {
    let flat = |text: &str| text.split_whitespace().collect::<Vec<_>>().join(" ");
    let head = match (
        flat(&cap(&n.app_name, MAX_APP_NAME_CHARS)),
        flat(&layout.summary.text),
    ) {
        (app, summary) if app.is_empty() => summary,
        (app, summary) if summary.is_empty() => app,
        (app, summary) => format!("{app}: {summary}"),
    };
    let body = layout
        .body
        .as_ref()
        .map(|body| flat(&body.content.text))
        .unwrap_or_default();
    match (head.is_empty(), body.is_empty()) {
        (_, true) => head,
        (true, false) => body,
        (false, false) => format!("{head}, {body}"),
    }
}

#[allow(clippy::too_many_arguments)]
fn banner_window<M: Clone + 'static>(
    n: &Notification,
    layout: &CardLayout,
    card: Rect,
    region: Vec<Rect>,
    opacity: f64,
    hovered: bool,
    unannounced: bool,
    width: f64,
    age: &str,
    map: fn(Input) -> M,
) -> Window<M> {
    let size = Size::new(width, card.min_y() + card.height() + SHADOW_MARGIN);
    // a banner going away is no longer live: one whose window appears only as it leaves (at the unlock, or as the
    // centre closes) is not said again; nor is one that comes back at the unlock, said already as it arrived
    let live = match (unannounced, n.urgency) {
        (true, _) => None,
        (false, Urgency::Critical) => Some(Live::Assertive),
        (false, _) => Some(Live::Polite),
    };
    let content = Element::new(Strip {
        id: n.id,
        card,
        map,
        children: vec![Element::new(Card {
            opacity,
            // a bridge announces a live node by its name alone, as it appears or is renamed: the name is everything
            // the card shows in words, so a new banner and a banner whose text changes in place are said whole
            name: banner_speech(n, layout),
            live,
            children: card_content(
                n,
                layout,
                card.size,
                age,
                Controls {
                    close: hovered.then(|| (Input::Dismiss(n.id), None)),
                    ..Controls::default()
                },
                None,
                map,
            ),
            ..Card::plain(n.id, card, true, map)
        })
        .key(card_key(n.id))],
    });
    window(
        banner_key(n, layout.height),
        spec(accessible_name(n), size, Some(region)),
        content,
    )
}

#[allow(clippy::too_many_arguments)]
fn centre_window<M: Clone + 'static>(
    center: &NotificationCenter,
    ui: &Ui,
    metrics: &CardMetrics,
    width: f64,
    now: f64,
    map: fn(Input) -> M,
) -> Window<M> {
    let layout = &center.config.layout;
    let stack = GroupStack::default();
    let x = width - layout.right_margin - layout.width;
    let groups = center.groups();
    let scopes = center.clear_scopes();

    // the title over the cards' leading edge, the clear button at their trailing edge; no panel behind them
    let mut header = vec![Element::new(Label {
        text: CENTRE_TITLE.to_owned(),
        size: TITLE_SIZE,
        weight: FontWeight::BOLD.0,
        secondary: false,
        align: TextAlign::Start,
        rect: Some(Rect::new(
            x + TITLE_INSET,
            0.0,
            layout.width - CLEAR_WIDTH - TITLE_INSET,
            HEADER_HEIGHT,
        )),
        hidden: false,
    })
    .key("title")];
    if let Some(&only) = scopes.first() {
        let message = if scopes.len() > 1 {
            Input::ToggleClearMenu
        } else {
            Input::ClearScope(only)
        };
        header.push(
            Element::new(ClearButton {
                rect: Rect::new(
                    x + layout.width - CLEAR_WIDTH,
                    ((HEADER_HEIGHT - CLOSE_DIAMETER) / 2.0).round(),
                    CLEAR_WIDTH,
                    CLOSE_DIAMETER,
                ),
                label: "Clear All".into(),
                message: map(message),
            })
            .key("clear"),
        );
    }
    let header = Element::new(Layer {
        size: Size::new(width, HEADER_HEIGHT),
        role: None,
        count: 0,
        children: header,
    })
    .key("header-row");

    // the list: the first card a gap below the header, room for its close button, each group as tall as its stack, or
    // as its cards one below the other while it is expanded
    let mut y = HEADER_GAP.max(CLOSE_REACH_Y);
    let mut cards: Vec<Piece<M>> = Vec::new();
    let mut index = 0usize;
    for (number, group) in groups.iter().enumerate() {
        let shown: Vec<Notification> = group
            .notifications
            .iter()
            .map(|n| center.shown(n).into_owned())
            .collect();
        let count = shown.len();
        let container = number as u32 + 1;
        let expansion = if count > 1 {
            ui.expansion(&group.key, now)
        } else {
            0.0
        };
        let fold = |n: &Notification| {
            if ui.unfolded.contains(&n.id) {
                Fold::Unfolded
            } else {
                Fold::Folded
            }
        };
        let revealed = |n: &Notification| {
            ui.leaving(n.id, now).is_none()
                && (ui.revealed == Some(n.id) || ui.focused == Some(n.id))
        };
        let front = &shown[0];
        let front_layout = metrics.card_with(front, layout.width, fold(front), true);
        let open = count > 1 && (ui.expanded.contains_key(&group.key) || expansion > 0.0);
        // each card of an expanded group, and the room the group takes: its stack, or its header and cards one below
        // the other, or between while it moves
        let layouts: Vec<CardLayout> = if open {
            shown
                .iter()
                .enumerate()
                .map(|(i, n)| match i {
                    0 => front_layout.clone(),
                    _ => metrics.card_with(n, layout.width, fold(n), true),
                })
                .collect()
        } else {
            Vec::new()
        };
        let stacked = stack.height(front_layout.height, count);
        let spread = GROUP_HEADER
            + layouts.iter().map(|l| l.height).sum::<f64>()
            + CARD_GAP * count.saturating_sub(1) as f64
            + stack.bottom_padding;
        let group_height = stacked + (spread - stacked) * expansion;
        let first = index;
        let mut tops = Vec::with_capacity(layouts.len());
        let mut at = y + GROUP_HEADER;
        for l in &layouts {
            tops.push(at);
            at += l.height + CARD_GAP;
        }
        // the gap a cleared group leaves
        let group_gap = Gap {
            top: y,
            height: group_height,
        };
        let shift = ui.rise(y, now);
        let collapsed = Rect::new(x, y + shift, layout.width, front_layout.height);
        let sheets = stack.cards(collapsed, count);
        let card = |n: &Notification, layout_of: &CardLayout, rect: Rect, item: usize| {
            let disclosure = layout_of
                .foldable
                .then_some((n.id, fold(n) == Fold::Unfolded));
            let unfolded = fold(n) == Fold::Unfolded;
            // a card alone in its group leaves with the group's room; one of several leaves with its own
            let gap = if count == 1 {
                group_gap
            } else if open {
                Gap {
                    top: tops[item - first],
                    height: layout_of.height + CARD_GAP,
                }
            } else {
                Gap {
                    top: y,
                    height: stacked - stack.height(layout_of.height, count - 1),
                }
            };
            let chevron = layout_of
                .expandable
                .then_some((Input::Unfold(n.id, !unfolded), unfolded));
            let controls = if revealed(n) {
                Controls {
                    close: Some((Input::Leave(vec![n.id], gap), None)),
                    chevron,
                    pill: true,
                }
            } else {
                Controls::default()
            };
            Card {
                gap,
                name: accessible_name(n),
                description: n.body_text(),
                role: Role::ListItem,
                index: Some(item),
                first: item == 0,
                clickable: ui.leaving(n.id, now).is_none(),
                container,
                revealed: ui.leaving(n.id, now).is_none().then(|| revealed(n)),
                children: card_content(
                    n,
                    layout_of,
                    rect.size,
                    &age(n.timestamp, now),
                    controls,
                    disclosure,
                    map,
                ),
                shadow: false,
                ..Card::plain(n.id, rect, true, map)
            }
        };
        if !open {
            // collapsed: the sheets behind first, the farthest first, then the front card, all one glass shape
            for behind in sheets.iter().rev() {
                let cover = Rect::new(
                    collapsed.x() - behind.rect.x(),
                    collapsed.y() - behind.rect.y(),
                    collapsed.width(),
                    collapsed.height(),
                );
                let mut sheet = Card {
                    brightness: behind.brightness,
                    container,
                    cover: Some(cover),
                    shadow: false,
                    ..Card::plain(front.id, behind.rect, true, map)
                };
                if let Some(p) = ui.leaving(front.id, now) {
                    leaving_card(&mut sheet, p);
                }
                cards.push(Piece::Sheet(sheet));
            }
            let mut front_card = card(front, &front_layout, collapsed, index);
            if count > 1 {
                front_card.description = format!(
                    "{}{}{} more from {}",
                    front_card.description,
                    if front_card.description.is_empty() {
                        ""
                    } else {
                        "; "
                    },
                    count - 1,
                    group.app_name
                );
                front_card.press = Some(Input::Expand(group.key.clone(), true));
                front_card.expanded = Some(false);
                let controls = if revealed(front) {
                    Controls {
                        close: Some((
                            Input::LeaveGroup(group.key.clone(), group_gap),
                            Some(GroupClear {
                                app: group.app_name.to_owned(),
                                wide: ui.close_wide == Some(front.id),
                            }),
                        )),
                        chevron: Some((Input::Expand(group.key.clone(), true), false)),
                        pill: true,
                    }
                } else {
                    Controls::default()
                };
                front_card.children = card_content(
                    front,
                    &front_layout,
                    collapsed.size,
                    &age(front.timestamp, now),
                    controls,
                    front_layout
                        .foldable
                        .then_some((front.id, fold(front) == Fold::Unfolded)),
                    map,
                );
            }
            if let Some(p) = ui.leaving(front.id, now) {
                leaving_card(&mut front_card, p);
            }
            cards.push(Piece::Card(front_card));
            index += 1;
            y += group_height;
            continue;
        }
        // expanded, or on the way: the header row fades in, each card moves from its place in the stack to its own
        let mut header = vec![
            Element::new(Label {
                text: cap(group.app_name, MAX_APP_NAME_CHARS).into_owned(),
                size: 15.0,
                weight: FontWeight::SEMIBOLD.0,
                secondary: false,
                align: TextAlign::Start,
                rect: None,
                hidden: false,
            })
            .fill(),
            Element::new(Button {
                label: "Show Less".into(),
                role: ButtonRole::Normal,
                style: ButtonStyle::Pill,
                enabled: true,
                busy: false,
                on_press: Some(map(Input::Expand(group.key.clone(), false))),
            })
            .key("show-less"),
        ];
        header.push(
            Element::new(ClearButton {
                rect: Rect::new(0.0, 0.0, CLOSE_DIAMETER, CLOSE_DIAMETER),
                label: "Clear".into(),
                message: map(Input::LeaveGroup(group.key.clone(), group_gap)),
            })
            .key("clear-group"),
        );
        let header_rect = Rect::new(x, y + shift, layout.width, GROUP_HEADER);
        cards.push(Piece::Other(
            Element::new(Fade {
                rect: header_rect,
                opacity: expansion,
                children: vec![Element::new(Place {
                    rect: Rect::from_origin_size(Point::ZERO, header_rect.size),
                    axis: Axis::Horizontal,
                    align: Align::Center,
                    padding: Insets {
                        left: TITLE_INSET,
                        right: 0.0,
                        top: 0.0,
                        bottom: 0.0,
                    },
                    spacing: BUTTON_GAP,
                    children: header,
                })],
            })
            .key(format!("group-{}", group.key)),
        ));
        let lerp = |a: f64, b: f64| a + (b - a) * expansion;
        let mut placed = Vec::new();
        for (i, n) in shown.iter().enumerate() {
            let layout_of = layouts[i].clone();
            let to = Rect::new(
                x,
                tops[i] + ui.rise(tops[i], now),
                layout.width,
                layout_of.height,
            );
            let from = match i {
                0 => collapsed,
                _ => sheets
                    .get(i.min(sheets.len()) - 1)
                    .map_or(collapsed, |s| s.rect),
            };
            let rect = Rect::new(
                lerp(from.x(), to.x()),
                lerp(from.y(), to.y()),
                lerp(from.width(), to.width()),
                lerp(from.height(), to.height()),
            );
            placed.push((n, layout_of, rect, to));
        }
        // the deepest first, so the front card covers the others while they still overlap
        for (i, (n, layout_of, rect, to)) in placed.into_iter().enumerate().rev() {
            let mut element = card(n, &layout_of, rect, first + i);
            if i > 0 {
                // the text of a card coming out of the stack shows once it is nearly in place
                let shown = ((expansion - 0.5) * 2.0).clamp(0.0, 1.0);
                element.children = if shown > 0.0 {
                    vec![Element::new(Fade {
                        rect: Rect::from_origin_size(Point::ZERO, to.size),
                        opacity: shown,
                        children: std::mem::take(&mut element.children),
                    })]
                } else {
                    Vec::new()
                };
            }
            if let Some(p) = ui.leaving(n.id, now) {
                leaving_card(&mut element, p);
            }
            cards.push(Piece::Card(element));
        }
        index += count;
        y += group_height;
    }
    let content_height = y.ceil();
    // what of the list is in view, with a card's height to spare for a scroll the state has not caught up with; all
    // of it before the list was first laid out
    let scroll = ui.scroll;
    let in_view = move |rect: Rect| {
        scroll.viewport.height <= 0.0
            || (rect.max_y() > scroll.offset.y - MIN_CARD_HEIGHT
                && rect.min_y() < scroll.offset.y + scroll.viewport.height + MIN_CARD_HEIGHT)
    };
    // the halos of the cards in view, under the list: fully strong while a card is in view, fading as it goes under the
    // header or past the bottom edge
    let list_top = layout.top_inset + HEADER_HEIGHT;
    let slide = ui.slide.offset(now);
    let halos: Vec<Element<M>> = cards
        .iter()
        .filter_map(|piece| match piece {
            Piece::Card(c) if c.opacity > 0.0 => Some(c),
            _ => None,
        })
        .filter_map(|c| {
            let top = list_top + c.rect.y() - scroll.offset.y;
            let bottom = top + c.rect.height();
            let viewport = scroll.viewport.height;
            let above = ((bottom - list_top) / HALO_FADE).clamp(0.0, 1.0);
            let below = if viewport > 0.0 {
                ((list_top + viewport - top) / HALO_FADE).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let visible = above.min(below) * c.opacity;
            (visible > 0.0).then(|| {
                Element::new(Halo {
                    rect: Rect::new(c.rect.x() + slide, top, c.rect.width(), c.rect.height()),
                    opacity: visible,
                })
                .key(format!("halo-{}", c.id))
            })
        })
        .collect();
    let cards = glass_within_budget(cards, in_view);
    let list = Element::from(ScrollView {
        content: Element::new(Layer {
            size: Size::new(width, content_height),
            role: Some(Role::List),
            count: index,
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
    // the whole column slides as one piece: it starts this far from the window's left edge, and what is past the
    // right edge is off the output
    let mut children = vec![
        Element::new(Layer {
            size: Size::new(width + slide, HALO_LAYER_HEIGHT),
            role: None,
            count: 0,
            children: halos,
        })
        .key("halos"),
        Element::new(Column {
            insets: Insets {
                top: layout.top_inset,
                bottom: BOTTOM_MARGIN,
                left: slide,
                right: 0.0,
            },
            children: vec![header, list],
        }),
    ];

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
        let rect = Rect::new(
            x + slide + layout.width - MENU_WIDTH,
            top,
            MENU_WIDTH,
            height,
        );
        children.push(
            Element::new(Layer {
                size: Size::new(width + slide, top + height),
                role: None,
                count: 0,
                children: vec![Element::new(Card {
                    // over the cards, whose glass lies under all of it: the menu paints its fill
                    name: "Clear".into(),
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
                    ..Card::plain(0, rect, false, map)
                })
                .key("clear-menu")],
            })
            .key("menu-layer"),
        );
    }
    let content = Element::new(CentreRoot {
        map,
        menu_open: ui.menu_open,
        revealed: ui.revealed.is_some(),
        children,
    });
    // the layer surface spans the output's right edge from top to bottom; this height is what the list asks for
    let natural = layout.top_inset + HEADER_HEIGHT + content_height + BOTTOM_MARGIN;
    // the surface holds the shadow's reach beside the cards, which takes no input; a list on its way out takes none at
    // all
    let region = if ui.centre_open {
        let edge = layout.width + layout.right_margin + CENTRE_EDGE;
        vec![Rect::new(width - edge, 0.0, edge, CENTRE_REGION_HEIGHT)]
    } else {
        Vec::new()
    };
    window(
        centre_key(),
        spec(
            CENTRE_TITLE.to_owned(),
            Size::new(width, natural.ceil()),
            Some(region),
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
/// [`MAX_BODY_BYTES`] of it, a longer one cut and ending in an ellipsis.
fn body_text(n: &Notification) -> Option<BodyText> {
    let mut text = String::new();
    let mut runs = Vec::new();
    let mut links = Vec::new();
    for span in &n.spans {
        let mut piece = span.text.replace("\r\n", "\n").replace('\r', "\n");
        let room = MAX_BODY_BYTES.saturating_sub(text.len());
        let cut = piece.len() > room;
        if cut {
            let mut end = room.saturating_sub(ELLIPSIS.len_utf8());
            while !piece.is_char_boundary(end) {
                end -= 1;
            }
            piece.truncate(end);
            if !piece.is_empty() {
                piece.push(ELLIPSIS);
            }
        }
        if piece.is_empty() {
            // nothing of the body follows a cut
            if cut {
                break;
            }
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
        if cut {
            break;
        }
    }
    (!text.trim().is_empty()).then_some(BodyText {
        content: AttributedText { text, runs },
        links,
    })
}

/// The icon, the summary, the body and the action buttons of a card `size` large, its close button when `close` is
/// given, and the disclosure button of a foldable centre card (its id and whether it is unfolded).
/// How long ago `then` was at `now` (seconds of one clock), as the card's corner says it.
pub fn age(then: f64, now: f64) -> String {
    let seconds = (now - then).max(0.0);
    match seconds {
        s if s < 60.0 => "now".to_owned(),
        s if s < 3600.0 => format!("{}m ago", (s / 60.0) as u64),
        s if s < 86_400.0 => format!("{}h ago", (s / 3600.0) as u64),
        s => format!("{}d ago", (s / 86_400.0) as u64),
    }
}

/// A collapsed group's close circle, which grows into the capsule that clears the group.
#[derive(Clone, Debug, Default, PartialEq)]
struct GroupClear {
    /// The group's application, which names the capsule.
    app: String,
    /// The capsule is grown.
    wide: bool,
}

/// What a card shows over its content while it is revealed (or, for a banner, hovered).
#[derive(Clone, Debug, Default, PartialEq)]
struct Controls {
    /// The close circle and what it sends; a collapsed group's grows into a capsule.
    close: Option<(Input, Option<GroupClear>)>,
    /// The chevron that takes the age's place, what it sends and whether the card is unfolded.
    chevron: Option<(Input, bool)>,
    /// The first action's pill.
    pill: bool,
}

#[allow(clippy::too_many_arguments)]
fn card_content<M: Clone + 'static>(
    n: &Notification,
    layout: &CardLayout,
    size: Size,
    age: &str,
    controls: Controls,
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
            // the body gives way to the pill over its corner
            secondary: controls.pill && layout.pill.is_some(),
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
            left: LEADING,
            bottom: PADDING,
            right: TRAILING,
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
    if !age.is_empty() && controls.chevron.is_none() {
        content.push(
            Element::new(Label {
                text: age.to_owned(),
                size: AGE_SIZE,
                weight: FontWeight::REGULAR.0,
                secondary: true,
                align: TextAlign::End,
                rect: Some(Rect::new(
                    size.width - TRAILING - AGE_WIDTH,
                    PADDING + 1.0,
                    AGE_WIDTH,
                    AGE_SIZE * 1.4,
                )),
                hidden: false,
            })
            .key("age"),
        );
    }
    if let Some((message, group)) = controls.close {
        let kind = Kind::Close {
            label: group.as_ref().map(|_| "Clear All".to_owned()),
            wide: group.as_ref().is_some_and(|g| g.wide),
        };
        let name = match &group {
            Some(g) => format!("Clear all from {}", g.app),
            None => "Close".to_owned(),
        };
        content.push(
            CardControl {
                id: n.id,
                kind,
                name,
                message,
                map,
            }
            .element(size),
        );
    }
    if let Some((message, open)) = controls.chevron {
        content.push(
            CardControl {
                id: n.id,
                kind: Kind::Chevron { open },
                name: if open { "Show Less" } else { "Show More" }.to_owned(),
                message,
                map,
            }
            .element(size),
        );
    }
    if let (true, Some((key, label))) = (controls.pill, &layout.pill) {
        content.push(
            CardControl {
                id: n.id,
                kind: Kind::Pill {
                    label: label.clone(),
                },
                name: label.clone(),
                message: Input::Invoke(n.id, key.clone()),
                map,
            }
            .element(size),
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

    /// Whether `position` is over the card or its close button.
    fn over(&self, position: Point) -> bool {
        self.card.contains(position) || close_rect(self.card).contains(position)
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
                self.hover(state, self.over(position), cx);
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
                self.hover(state, self.over(position), cx);
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
    /// A card is revealed: the pointer moving over no card, or leaving the window, hides its buttons again.
    revealed: bool,
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
        if self.menu_open != old.menu_open || self.revealed != old.revealed {
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
            // a card stops the motions over it: this one is over none
            Event::Pointer(PointerEvent::Motion { .. } | PointerEvent::Leave, _)
                if self.revealed && cx.phase() != Phase::Capture =>
            {
                cx.emit((self.map)(Input::Reveal(None)));
                Propagation::Continue
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
    /// The glass container the card joins (a collapsed group's front card and the sheets behind it); 0 alone.
    container: u32,
    /// For a sheet behind a group's front card: the front card's rect in the sheet's frame, which the sheet's own
    /// paint (its shadow, darkening and outline) stays out of.
    cover: Option<Rect>,
    /// What a click, Return, Space or assistive technology's click sends; `None` activates the notification.
    press: Option<Input>,
    /// A collapsed group's front card (`false`): pressing it expands the group.
    expanded: Option<bool>,
    /// A centre card: whether it shows its close and action buttons, which hover and keyboard focus reveal.
    revealed: Option<bool>,
    /// The room the card leaves in the list when it is cleared with the keyboard.
    gap: Gap,
    /// How blurred the card is while it leaves, as a blur radius.
    blur: f64,
    /// The card paints its own halo. The centre's cards do not: their halos are drawn under the whole list, where the
    /// scroll view's edges do not cut them ([`Halo`]).
    shadow: bool,
    map: fn(Input) -> M,
    children: Vec<Element<M>>,
}

impl<M> Card<M> {
    /// A card at `rect` that is decoration only, painted as `material` says; the other fields are set by the caller.
    fn plain(id: u32, rect: Rect, material: bool, map: fn(Input) -> M) -> Self {
        Card {
            id,
            rect,
            opacity: 1.0,
            brightness: 0.0,
            material,
            name: String::new(),
            description: String::new(),
            role: Role::Group,
            index: None,
            first: false,
            live: None,
            clickable: false,
            container: 0,
            cover: None,
            press: None,
            expanded: None,
            revealed: None,
            gap: Gap {
                top: 0.0,
                height: 0.0,
            },
            blur: 0.0,
            shadow: true,
            map,
            children: Vec::new(),
        }
    }

    fn pressed(&self) -> Input {
        self.press.clone().unwrap_or(Input::Activate(self.id))
    }
}

/// `card` as it leaves, `progress` of the way from 0 to 1: it shrinks a little, blurs and fades, and takes no input.
/// Its content stays where it was while the platter shrinks under it.
fn leaving_card<M: 'static>(card: &mut Card<M>, progress: f64) {
    let (dx, dy) = (
        card.rect.width() * motion::LEAVE_SHRINK * progress / 2.0,
        card.rect.height() * motion::LEAVE_SHRINK * progress / 2.0,
    );
    let old = card.rect;
    card.rect = Rect::new(
        old.x() + dx,
        old.y() + dy,
        old.width() - 2.0 * dx,
        old.height() - 2.0 * dy,
    );
    card.opacity *= 1.0 - progress;
    card.blur = motion::LEAVE_BLUR * progress;
    card.clickable = false;
    card.revealed = None;
    if !card.children.is_empty() {
        card.children = vec![Element::new(Fade {
            rect: Rect::new(-dx, -dy, old.width(), old.height()),
            opacity: 1.0,
            children: std::mem::take(&mut card.children),
        })];
    }
}

/// A piece of the centre's list before its glass is counted.
enum Piece<M> {
    /// A notification's card, keyed by its id.
    Card(Card<M>),
    /// A sheet peeking under a collapsed group's front card.
    Sheet(Card<M>),
    Other(Element<M>),
}

/// The list's pieces with their glass kept within what one window shows ([`MAX_MATERIALS`]), counting what is in view
/// (`in_view` of a piece's rect): every card keeps its glass, and the sheets of collapsed groups keep theirs a whole group
/// at a time down the list while there is room. The sheets past it are drawn as opaque sheets, so no card further down
/// loses its glass to a sheet above it and no group mixes glass and opaque sheets.
fn glass_within_budget<M: 'static>(
    pieces: Vec<Piece<M>>,
    in_view: impl Fn(Rect) -> bool,
) -> Vec<Element<M>> {
    let cards = pieces
        .iter()
        .filter(|p| matches!(p, Piece::Card(c) if c.material && in_view(c.rect)))
        .count();
    // the sheets in view of each group, by container, which numbers the groups down the list
    let mut sheets = BTreeMap::<u32, usize>::new();
    for piece in &pieces {
        if let Piece::Sheet(sheet) = piece {
            if sheet.material && in_view(sheet.rect) {
                *sheets.entry(sheet.container).or_default() += 1;
            }
        }
    }
    let mut room = MAX_MATERIALS.saturating_sub(cards);
    let mut glass = BTreeSet::new();
    for (container, count) in sheets {
        if count > room {
            break;
        }
        room -= count;
        glass.insert(container);
    }
    pieces
        .into_iter()
        .map(|piece| match piece {
            Piece::Card(card) => {
                let key = card_key(card.id);
                Element::new(card).key(key)
            }
            Piece::Sheet(sheet) => Element::new(Card {
                material: sheet.material && glass.contains(&sheet.container),
                ..sheet
            }),
            Piece::Other(element) => element,
        })
        .collect()
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct CardState {
    armed: bool,
}

/// A card's halo, drawn only outside `rect` (rounded by `radius`): from an opaque copy of its shape that the clip hides.
fn paint_halo(cx: &mut PaintCx<'_>, rect: Rect, radius: f64, fill: Color, shadow: Effect) {
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
        shape: Shape::RoundedRect { rect, radius },
        paint: Paint::Solid(fill),
    });
    cx.push(PaintOp::PopLayer);
    cx.push(PaintOp::PopClip);
}

/// The halo of a card of the centre, in the layer under the whole list: the scroll view clips what it holds at its
/// edges, which would cut a halo at the list's top and bottom, while this layer is cut only by the surface.
struct Halo {
    rect: Rect,
    opacity: f64,
}

impl<M: 'static> Widget<M> for Halo {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            overflow: Overflow::Visible,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.opacity != old.opacity {
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
        let theme = cx.env().theme.clone();
        let Some(fill) = theme.surface(Surface::Popover).fill else {
            return;
        };
        let rect = Rect::new(0.0, 0.0, self.rect.width(), self.rect.height());
        cx.set_group(self.opacity.clamp(0.0, 1.0) as f32, Vec::new());
        paint_halo(
            cx,
            rect,
            CORNER_RADIUS.min(rect.height() / 2.0),
            fill,
            Effect::DropShadow {
                offset: Point::ZERO,
                blur: SHADOW_BLUR,
                color: Color::new(0.0, 0.0, 0.0, SHADOW_ALPHA),
            },
        );
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
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
            // the close button reaches past the corner; the text is exactly as tall as the card
            overflow: Overflow::Visible,
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
            || self.container != old.container
            || self.cover != old.cover
            || self.press != old.press
            || self.expanded != old.expanded
            || self.revealed != old.revealed
            || self.gap != old.gap
            || self.blur != old.blur
            || self.shadow != old.shadow
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
                    cx.emit((self.map)(self.pressed()));
                }
                Propagation::Stop
            }
            // hover reveals a centre card's buttons; the motion goes no further, so the centre knows the pointer is
            // over a card
            Event::Pointer(PointerEvent::Enter { .. } | PointerEvent::Motion { .. }, _)
                if self.revealed.is_some() =>
            {
                if self.revealed == Some(false) {
                    cx.emit((self.map)(Input::Reveal(Some(self.id))));
                }
                Propagation::Stop
            }
            // the keyboard on the card or in it reveals its buttons, as long as its focus ring shows; a focus moving
            // between the card and its buttons is lost and gained again in that order, so it stays revealed
            Event::Focus { gained, visible } | Event::FocusWithin { gained, visible }
                if self.revealed.is_some() =>
            {
                cx.emit((self.map)(Input::Focused(self.id, gained && visible)));
                Propagation::Continue
            }
            Event::Key(key, _) if active && key.pressed && cx.node() == cx.target() => {
                match key.keysym {
                    RETURN | KEYPAD_ENTER | SPACE => cx.emit((self.map)(self.pressed())),
                    DELETE | BACKSPACE => {
                        cx.emit((self.map)(Input::LeaveFocused(self.id, self.gap)))
                    }
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
                cx.emit((self.map)(self.pressed()));
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
            offset: Point::ZERO,
            blur: SHADOW_BLUR,
            color: Color::new(0.0, 0.0, 0.0, SHADOW_ALPHA),
        };
        let shape = Shape::RoundedRect { rect, radius };
        let theme = cx.env().theme.clone();
        let Some(fill) = theme.surface(Surface::Popover).fill else {
            return;
        };
        // a sheet's own paint stays off the front card in front of it, which is glass of the same shape
        if let Some(cover) = self.cover {
            let mut ring = rounded_path(rect.outset(SHADOW_REACH), 0.0);
            ring.extend(rounded_path(cover, CORNER_RADIUS.min(cover.height() / 2.0)));
            cx.push(PaintOp::PushClip(Shape::Path {
                verbs: ring,
                fill_rule: FillRule::EvenOdd,
            }));
        }
        // a card that is leaving blurs before anything else of it is composited
        let blur: Vec<Effect> = if self.blur > 0.0 {
            vec![Effect::Blur { radius: self.blur }]
        } else {
            Vec::new()
        };
        let glass = self.material
            && cx.glass(
                Glass::regular()
                    .radius(radius)
                    .container(self.container, 0.0)
                    .alpha(opacity),
            );
        if glass {
            // the compositor draws the glass under the card: the shadow is drawn only outside the card, from an
            // opaque copy of its shape the clip hides, and the text casts none
            cx.set_group(opacity as f32, blur);
            if self.shadow {
                paint_halo(cx, rect, radius, fill, shadow);
            }
            // a sheet deeper in its group is a little darker
            if self.brightness < 0.0 {
                cx.push(PaintOp::Fill {
                    shape: shape.clone(),
                    paint: Paint::Solid(Color::new(0.0, 0.0, 0.0, (-self.brightness * 2.0) as f32)),
                });
            }
        } else {
            let mut effects = blur;
            if self.shadow {
                effects.push(shadow);
            }
            cx.set_group(opacity as f32, effects);
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
        if self.cover.is_some() {
            cx.push(PaintOp::PopClip);
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
        if self.expanded.is_some() {
            cx.states(AccessibleStates {
                expanded: self.expanded,
                ..AccessibleStates::default()
            });
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

/// Text at a size and weight of its own, in the label or secondary label colour: the centre's title, a card's age and
/// an expanded group's name, which the text roles' sizes do not cover. With a rect it is placed there, its text
/// centred vertically in it; without one it takes the width its parent gives it.
#[derive(Clone, Debug, PartialEq)]
struct Label {
    text: String,
    size: f64,
    weight: u16,
    secondary: bool,
    align: TextAlign,
    rect: Option<Rect>,
    hidden: bool,
}

impl Label {
    fn laid_out(&self, env: &Environment, width: f64) -> Option<ParagraphLayout> {
        if self.text.is_empty() {
            return None;
        }
        let style = TextStyle {
            weight: FontWeight(self.weight),
            ..TextStyle::new(self.size as f32)
        };
        let paragraph = ParagraphStyle {
            align: self.align,
            wrap: Wrap::None,
            max_lines: Some(1),
            ellipsis: true,
            ..ParagraphStyle::default()
        };
        let text = env
            .text
            .paragraph(&AttributedText::plain(&self.text, style), &paragraph)
            .ok()?;
        Some(text.layout(width.is_finite().then_some(width.max(0.0) as f32)))
    }
}

impl<M: 'static> Widget<M> for Label {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        match self.rect {
            Some(rect) => LayoutSpec {
                width: SizeSpec::Fixed(rect.width()),
                height: SizeSpec::Fixed(rect.height()),
                position: rect.origin,
                flex: Flex::NONE,
                ..LayoutSpec::new(ContainerKind::Leaf)
            },
            None => LayoutSpec::new(ContainerKind::Leaf),
        }
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if (&self.text, self.size, self.weight, self.rect)
            != (&old.text, old.size, old.weight, old.rect)
        {
            Change::Layout
        } else if self != old {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        if let Some(rect) = self.rect {
            return Measured {
                size: rect.size,
                baseline: None,
            };
        }
        match self.laid_out(cx.env(), constraints.width.max()) {
            Some(layout) => Measured {
                size: layout.size(),
                baseline: layout.lines().first().map(|line| line.baseline),
            },
            None => no_measure(),
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let Some(layout) = self.laid_out(cx.env(), size.width) else {
            return;
        };
        let role = if self.secondary {
            ColorRole::SecondaryText
        } else {
            ColorRole::Text
        };
        let Some(color) = color(cx, role) else {
            return;
        };
        let top = ((size.height - layout.size().height) / 2.0)
            .max(0.0)
            .round();
        for line in layout.lines() {
            for run in &line.runs {
                cx.push(PaintOp::Glyphs {
                    origin: Point::new(0.0, top) + run.origin,
                    run: run.run.clone(),
                    color,
                });
            }
        }
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.hidden {
            cx.hidden();
            return;
        }
        cx.role(Role::StaticText);
        cx.name(self.text.clone());
    }
}

/// A round button with a cross that grows into a capsule naming itself, against its rect's trailing edge, while the
/// pointer is over it or it has the keyboard: the centre's Clear All, and an expanded group's clear button.
struct ClearButton<M> {
    rect: Rect,
    label: String,
    message: M,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct ClearState {
    armed: bool,
    hovered: bool,
}

impl<M: Clone + 'static> ClearButton<M> {
    /// The button's shape now, in its own frame: the circle, or the capsule wide enough for its label.
    fn shape(
        &self,
        state: &ClearState,
        open: bool,
        env: &Environment,
    ) -> (Rect, Option<ParagraphLayout>) {
        let size = self.rect.size;
        let height = size.height;
        if !(state.hovered || open) {
            return (Rect::new(size.width - height, 0.0, height, height), None);
        }
        let label = Label {
            text: self.label.clone(),
            size: 12.0,
            weight: FontWeight::SEMIBOLD.0,
            secondary: false,
            align: TextAlign::Start,
            rect: None,
            hidden: true,
        };
        let layout = label.laid_out(env, f64::INFINITY);
        let text = layout.as_ref().map_or(0.0, |l| l.size().width);
        let width = (text + height).clamp(height, size.width);
        (Rect::new(size.width - width, 0.0, width, height), layout)
    }
}

impl<M: Clone + 'static> Widget<M> for ClearButton<M> {
    type State = ClearState;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            flex: Flex::NONE,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) -> ClearState {
        ClearState::default()
    }

    fn update(&self, old: &Self, _: &mut ClearState) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &ClearState, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, state: &mut ClearState, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        if cx.phase() == Phase::Capture {
            return Propagation::Continue;
        }
        match *event {
            Event::Pointer(PointerEvent::Enter { .. } | PointerEvent::Motion { .. }, _) => {
                if !state.hovered {
                    state.hovered = true;
                    cx.request_paint();
                }
                Propagation::Continue
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                state.hovered = false;
                state.armed = false;
                cx.request_paint();
                Propagation::Continue
            }
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
            Event::Key(key, _) if key.pressed && cx.node() == cx.target() => match key.keysym {
                RETURN | KEYPAD_ENTER | SPACE => {
                    cx.emit(self.message.clone());
                    Propagation::Stop
                }
                _ => Propagation::Continue,
            },
            Event::Focus { .. } => {
                cx.request_paint();
                Propagation::Continue
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

    fn paint(&self, state: &ClearState, cx: &mut PaintCx<'_>) {
        let open = cx.is_focused() && cx.focus_visible();
        let env = cx.env().clone();
        let (rect, label) = self.shape(state, open, &env);
        let (Some(fill), Some(mark)) = (
            color(cx, ColorRole::ControlFill),
            color(cx, ColorRole::SecondaryText),
        ) else {
            return;
        };
        let radius = rect.height() / 2.0;
        let shape = Shape::RoundedRect { rect, radius };
        cx.push(PaintOp::Fill {
            shape: shape.clone(),
            paint: Paint::Solid(fill),
        });
        if let Some(edge) = color(cx, ColorRole::Separator) {
            cx.push(PaintOp::Stroke {
                shape: shape.clone(),
                width: OUTLINE_WIDTH,
                paint: Paint::Solid(edge),
            });
        }
        match label {
            Some(layout) => {
                let Some(text) = color(cx, ColorRole::Text) else {
                    return;
                };
                let origin = Point::new(
                    rect.x() + ((rect.width() - layout.size().width) / 2.0).round(),
                    ((rect.height() - layout.size().height) / 2.0).round(),
                );
                for line in layout.lines() {
                    for run in &line.runs {
                        cx.push(PaintOp::Glyphs {
                            origin: origin + run.origin,
                            run: run.run.clone(),
                            color: text,
                        });
                    }
                }
            }
            None => {
                let d = rect.height();
                let (a, b) = (d * 0.32, d * 0.68);
                let x = rect.x();
                cx.push(PaintOp::Stroke {
                    shape: Shape::Path {
                        verbs: vec![
                            PathVerb::MoveTo(Point::new(x + a, a)),
                            PathVerb::LineTo(Point::new(x + b, b)),
                            PathVerb::MoveTo(Point::new(x + b, a)),
                            PathVerb::LineTo(Point::new(x + a, b)),
                        ],
                        fill_rule: Default::default(),
                    },
                    width: 1.5,
                    paint: Paint::Solid(mark),
                });
            }
        }
        if open {
            let theme = cx.env().theme.clone();
            if let Some(ring) = theme
                .colors
                .get(ColorRole::KeyboardFocusIndicator, StateVariant::NORMAL)
            {
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: rect.outset(theme.focus.width / 2.0 + 1.0),
                        radius: radius + theme.focus.width / 2.0 + 1.0,
                    },
                    width: theme.focus.width,
                    paint: Paint::Solid(ring),
                });
            }
        }
    }

    fn semantics(&self, _: &ClearState, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Button);
        cx.name(self.label.clone());
        cx.focusable();
        cx.action(Action::Click);
    }
}

/// Its children at `rect`, drawn at `opacity`: an expanded group's header row and the text of a card coming out of
/// the stack, as they fade in.
struct Fade<M> {
    rect: Rect,
    opacity: f64,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for Fade<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            overflow: Overflow::Visible,
            ..LayoutSpec::new(ContainerKind::Absolute)
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect {
            Change::Layout
        } else if self.opacity != old.opacity {
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
        if self.opacity < 1.0 {
            cx.set_group(self.opacity.clamp(0.0, 1.0) as f32, Vec::new());
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

mod controls;
mod motion;

#[cfg(test)]
mod tests;
