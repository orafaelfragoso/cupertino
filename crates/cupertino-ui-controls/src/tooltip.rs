use crate::look;
use crate::{Handler, Tooltip};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    Change, CommandRegistry, Element, Environment, Event, EventCx, Key, MeasureCx, PaintCx, Phase,
    Popup, PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{DecorationMode, Keysym, PointerEvent, Timestamp, WindowSpec};
use cupertino_ui_text::{AttributedText, ParagraphLayout, ParagraphStyle, TextStyle, Wrap};
use cupertino_ui_theme::{ColorRole, StateVariant};

const ESCAPE: Keysym = Keysym(0xff1b);
/// How long the pointer rests on the anchor before the tip is asked for.
const DELAY: u64 = 600_000_000;
/// How long the pointer may be off both anchor and tip, crossing the gap between them, before the tip hides.
const GRACE: u64 = 300_000_000;
const HEIGHT: f64 = 24.0;
const PADDING: f64 = 10.0;
const TEXT_SIZE: f32 = 11.0;
const GAP: f64 = 4.0;
/// The widest the text runs; longer text is cut there.
const TEXT_WIDTH: f64 = 300.0;
const RADIUS: f64 = 5.0;
const EDGE: f64 = 1.0;

/// Where the pointer and the keyboard focus are, and whether the tip shows. The model keeps it and applies each
/// [`TooltipChange`] the tooltip sends; the anchor and the tip, in separate windows, both read it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TooltipState {
    shown: bool,
    anchor_hovered: bool,
    focused: bool,
    tip_hovered: bool,
    /// The pointer came onto the tip since it was last on the anchor; leaving the tip then times the grace.
    tip_visited: bool,
}

impl TooltipState {
    pub fn shown(&self) -> bool {
        self.shown
    }

    pub fn apply(&mut self, change: TooltipChange) {
        match change.0 {
            Step::Show => self.shown = true,
            Step::Hide => {
                self.shown = false;
                self.tip_hovered = false;
                self.tip_visited = false;
            }
            Step::Anchor(hovered) => {
                self.anchor_hovered = hovered;
                self.tip_visited &= !hovered;
            }
            Step::Focus(focused) => self.focused = focused,
            Step::Tip(hovered) => {
                self.tip_hovered = hovered;
                self.tip_visited |= hovered;
            }
        }
    }

    /// Whether nothing holds the tip open any more.
    fn released(&self) -> bool {
        !self.anchor_hovered && !self.tip_hovered && !self.focused
    }
}

/// One change to a [`TooltipState`]. Each says only what changed, so changes sent in one turn from both windows
/// all apply.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TooltipChange(Step);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Show,
    Hide,
    Anchor(bool),
    Focus(bool),
    Tip(bool),
}

impl<M: Clone + 'static> Tooltip<M> {
    /// The popup window that shows the tip below the anchor keyed `anchor` in `parent`; declare it while the state
    /// is shown. It never takes the keyboard.
    pub fn window(&self, key: WindowKey, parent: WindowKey, anchor: Key) -> Window<M> {
        let hide = (self.on_change)(TooltipChange(Step::Hide));
        Window {
            key,
            spec: WindowSpec {
                title: self.text.clone(),
                app_id: String::new(),
                size: Size::new(TEXT_WIDTH + 2.0 * PADDING, HEIGHT),
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            content: Element::new(Bubble {
                text: self.text.clone(),
                state: self.state,
                on_enter: (self.on_change)(TooltipChange(Step::Tip(true))),
                on_leave: (self.on_change)(TooltipChange(Step::Tip(false))),
                on_hide: hide.clone(),
            }),
            popup: Some(Popup {
                parent,
                anchor: PopupAnchor::Below {
                    key: anchor,
                    gap: GAP,
                },
                grab: false,
                on_dismiss: hide,
            }),
        }
    }
}

impl<M: Clone + 'static> From<Tooltip<M>> for Element<M> {
    fn from(tooltip: Tooltip<M>) -> Self {
        Element::new(Anchor {
            text: tooltip.text,
            state: tooltip.state,
            on_change: tooltip.on_change,
            children: vec![tooltip.anchor],
        })
    }
}

/// When the anchor asked to be woken: to show the tip, and to see whether the pointer left both anchor and tip.
#[derive(Default)]
struct Rest {
    show_at: Option<Timestamp>,
    check_at: Option<Timestamp>,
}

/// Around the anchor: it times the pointer's rest or the keyboard focus, and hears the presses and keys that dismiss
/// the tip.
struct Anchor<M> {
    text: String,
    state: TooltipState,
    on_change: Handler<TooltipChange, M>,
    children: Vec<Element<M>>,
}

impl<M> Anchor<M> {
    fn send(&self, step: Step, cx: &mut EventCx<'_, M>) {
        cx.emit((self.on_change)(TooltipChange(step)));
    }

    fn hide(&self, rest: &mut Rest, cx: &mut EventCx<'_, M>) {
        rest.show_at = None;
        if self.state.shown {
            self.send(Step::Hide, cx);
        }
    }

    fn wait(rest: &mut Rest, cx: &mut EventCx<'_, M>) {
        let at = Timestamp(cx.now().0 + DELAY);
        rest.show_at = Some(at);
        cx.wake_at(at);
    }
}

impl<M: 'static> Widget<M> for Anchor<M> {
    type State = Rest;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Overlay)
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) -> Rest {
        Rest::default()
    }

    fn update(&self, _: &Self, _: &mut Rest) -> Change {
        Change::None
    }

    fn measure(&self, _: &Rest, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    /// Presses are heard on their way down, so the anchor's own control still gets them. A shown tip takes Escape
    /// wherever the focus is, as a dismissible node.
    fn event(&self, rest: &mut Rest, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let own = cx.node() == cx.target();
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if own => {
                self.send(Step::Anchor(true), cx);
                rest.check_at = None;
                Self::wait(rest, cx);
            }
            Event::Pointer(PointerEvent::Leave, _) if own => {
                self.send(Step::Anchor(false), cx);
                if !self.state.focused {
                    rest.show_at = None;
                }
                if self.state.shown {
                    let at = Timestamp(cx.now().0 + GRACE);
                    rest.check_at = Some(at);
                    cx.wake_at(at);
                }
            }
            // a focus from the pointer shows no ring and no tip
            Event::FocusWithin {
                gained: true,
                visible: true,
            } => {
                self.send(Step::Focus(true), cx);
                Self::wait(rest, cx);
            }
            Event::FocusWithin { gained: false, .. } if self.state.focused => {
                self.send(Step::Focus(false), cx);
                if !self.state.anchor_hovered && !self.state.tip_hovered {
                    self.hide(rest, cx);
                }
            }
            Event::Wake(now) if own => {
                if rest.show_at.is_some_and(|at| at <= now) {
                    rest.show_at = None;
                    let held = self.state.anchor_hovered || self.state.focused;
                    if held && !self.state.shown {
                        self.send(Step::Show, cx);
                    }
                }
                if rest.check_at.is_some_and(|at| at <= now) {
                    rest.check_at = None;
                    if self.state.released() && !self.state.tip_visited {
                        self.hide(rest, cx);
                    }
                }
            }
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _)
                if cx.phase() == Phase::Capture =>
            {
                self.hide(rest, cx);
            }
            Event::Dismiss if self.state.shown => {
                self.hide(rest, cx);
                return Propagation::Stop;
            }
            Event::Key(key, _)
                if cx.phase() == Phase::Capture
                    && key.pressed
                    && key.keysym == ESCAPE
                    && key.mods.is_empty() =>
            {
                rest.show_at = None;
            }
            _ => {}
        }
        Propagation::Continue
    }

    fn paint(&self, _: &Rest, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Rest, cx: &mut SemanticsCx<'_>) {
        cx.describe_content(self.text.clone());
        if self.state.shown {
            cx.dismissible();
        }
    }
}

/// The tip's text on one line in the tooltip's type, ending in an ellipsis past the text width.
fn layout(env: &Environment, text: &str) -> Option<ParagraphLayout> {
    let style = ParagraphStyle {
        wrap: Wrap::None,
        ellipsis: true,
        ..ParagraphStyle::default()
    };
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, TextStyle::new(TEXT_SIZE)),
            &style,
        )
        .ok()?;
    Some(look::one_line(&paragraph, TEXT_WIDTH))
}

/// The tip: its text in a rounded bubble. Leaving it gives the pointer the same grace to come back as leaving the
/// anchor.
struct Bubble<M> {
    text: String,
    state: TooltipState,
    on_enter: M,
    on_leave: M,
    on_hide: M,
}

impl<M: Clone + 'static> Widget<M> for Bubble<M> {
    type State = Option<Timestamp>;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Option<Timestamp> {
        None
    }

    fn update(&self, old: &Self, _: &mut Option<Timestamp>) -> Change {
        if self.text != old.text {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Option<Timestamp>, _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = layout(cx.env(), &self.text).map_or(0.0, |l| l.size().width);
        Measured {
            size: Size::new(width + 2.0 * PADDING, HEIGHT),
            baseline: None,
        }
    }

    fn event(
        &self,
        check_at: &mut Option<Timestamp>,
        event: &Event,
        cx: &mut EventCx<'_, M>,
    ) -> Propagation {
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) => {
                *check_at = None;
                cx.emit(self.on_enter.clone());
            }
            Event::Pointer(PointerEvent::Leave, _) => {
                cx.emit(self.on_leave.clone());
                let at = Timestamp(cx.now().0 + GRACE);
                *check_at = Some(at);
                cx.wake_at(at);
            }
            Event::Wake(now) if check_at.is_some_and(|at| at <= now) => {
                *check_at = None;
                if self.state.shown && self.state.released() {
                    cx.emit(self.on_hide.clone());
                }
            }
            _ => {}
        }
        Propagation::Continue
    }

    fn paint(&self, _: &Option<Timestamp>, cx: &mut PaintCx<'_>) {
        let env = cx.env().clone();
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        let color = |role| env.theme.colors.get(role, StateVariant::NORMAL);
        look::fill(cx, rect, RADIUS, color(ColorRole::ToolTip));
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            RADIUS - EDGE / 2.0,
            EDGE,
            color(ColorRole::Separator),
        );
        let Some(text) = layout(&env, &self.text) else {
            return;
        };
        let top = (size.height - text.size().height) / 2.0;
        look::glyphs(
            cx,
            &text,
            Point::new(PADDING, top),
            color(ColorRole::ToolTipText),
        );
    }

    fn semantics(&self, _: &Option<Timestamp>, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Tooltip);
        cx.name(self.text.clone());
    }
}
