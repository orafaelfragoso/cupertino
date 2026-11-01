use crate::look;
use crate::Tooltip;
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    Change, CommandRegistry, Element, Environment, Event, EventCx, Key, MeasureCx, PaintCx, Phase,
    Popup, PopupAnchor, Propagation, Role, SemanticsCx, Widget, Window, WindowBackground,
    WindowKey,
};
use cupertino_ui_layout::{Constraints, ContainerKind, LayoutSpec, Measured};
use cupertino_ui_platform::{DecorationMode, Keysym, PointerEvent, Timestamp, WindowSpec};
use cupertino_ui_render::{PaintOp, Shape};
use cupertino_ui_text::{AttributedText, ParagraphLayout, ParagraphStyle, TextStyle};
use cupertino_ui_theme::{ColorRole, StateVariant};

const ESCAPE: Keysym = Keysym(0xff1b);
/// How long the pointer rests on the anchor before the tip is asked for.
const DELAY: u64 = 600_000_000;
const HEIGHT: f64 = 24.0;
const PADDING: f64 = 10.0;
const TEXT_SIZE: f32 = 11.0;
const GAP: f64 = 4.0;
/// The widest the text runs; longer text is cut there.
const TEXT_WIDTH: f64 = 300.0;
const RADIUS: f64 = 5.0;
const EDGE: f64 = 1.0;

impl<M: Clone + 'static> Tooltip<M> {
    /// The popup window that shows the tip below the anchor keyed `anchor` in `parent`; declare it while `shown`. It
    /// never takes the keyboard.
    pub fn window(&self, key: WindowKey, parent: WindowKey, anchor: Key) -> Window<M> {
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
            }),
            popup: Some(Popup {
                parent,
                anchor: PopupAnchor::Below {
                    key: anchor,
                    gap: GAP,
                },
                grab: false,
                on_dismiss: self.on_hide.clone(),
            }),
        }
    }
}

impl<M: Clone + 'static> From<Tooltip<M>> for Element<M> {
    fn from(tooltip: Tooltip<M>) -> Self {
        Element::new(Anchor {
            text: tooltip.text,
            shown: tooltip.shown,
            on_show: tooltip.on_show,
            on_hide: tooltip.on_hide,
            children: vec![tooltip.anchor],
        })
    }
}

/// Whether the pointer is on the anchor or the keyboard focus is in it, and whether the delay is being waited out.
#[derive(Default)]
struct Rest {
    waiting: bool,
    hovered: bool,
    focused: bool,
}

/// Around the anchor: it times the pointer's rest or the keyboard focus, and hears the presses and keys that dismiss
/// the tip.
struct Anchor<M> {
    text: String,
    shown: bool,
    on_show: M,
    on_hide: M,
    children: Vec<Element<M>>,
}

impl<M: Clone> Anchor<M> {
    fn hide(&self, cx: &mut EventCx<'_, M>) {
        if self.shown {
            cx.emit(self.on_hide.clone());
        }
    }

    fn wait(rest: &mut Rest, cx: &mut EventCx<'_, M>) {
        rest.waiting = true;
        cx.wake_at(Timestamp(cx.now().0 + DELAY));
    }

    /// Hides the tip once neither the pointer nor the keyboard focus is on the anchor.
    fn release(&self, rest: &mut Rest, cx: &mut EventCx<'_, M>) {
        if !rest.hovered && !rest.focused {
            rest.waiting = false;
            self.hide(cx);
        }
    }
}

impl<M: Clone + 'static> Widget<M> for Anchor<M> {
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

    /// Presses and Escape are heard on their way down, so the anchor's own control still gets them.
    fn event(&self, rest: &mut Rest, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let own = cx.node() == cx.target();
        match *event {
            Event::Pointer(PointerEvent::Enter { .. }, _) if own => {
                rest.hovered = true;
                Self::wait(rest, cx);
            }
            Event::Pointer(PointerEvent::Leave, _) if own => {
                rest.hovered = false;
                self.release(rest, cx);
            }
            // a focus from the pointer shows no ring and no tip
            Event::FocusWithin {
                gained: true,
                visible: true,
            } => {
                rest.focused = true;
                Self::wait(rest, cx);
            }
            Event::FocusWithin { gained: false, .. } => {
                rest.focused = false;
                self.release(rest, cx);
            }
            Event::Wake(_) if own => {
                if rest.waiting && !self.shown {
                    cx.emit(self.on_show.clone());
                }
                rest.waiting = false;
            }
            Event::Pointer(PointerEvent::Button { pressed: true, .. }, _)
                if cx.phase() == Phase::Capture =>
            {
                rest.waiting = false;
                self.hide(cx);
            }
            Event::Key(key, _)
                if cx.phase() == Phase::Capture
                    && key.pressed
                    && key.keysym == ESCAPE
                    && key.mods.is_empty()
                    && self.shown =>
            {
                cx.emit(self.on_hide.clone());
                return Propagation::Stop;
            }
            _ => {}
        }
        Propagation::Continue
    }

    fn paint(&self, _: &Rest, _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &Rest, cx: &mut SemanticsCx<'_>) {
        cx.describe_content(self.text.clone());
    }
}

/// The tip's text on one line in the tooltip's type.
fn layout(env: &Environment, text: &str) -> Option<ParagraphLayout> {
    let paragraph = env
        .text
        .paragraph(
            &AttributedText::plain(text, TextStyle::new(TEXT_SIZE)),
            &ParagraphStyle::default(),
        )
        .ok()?;
    Some(paragraph.layout(None))
}

/// The tip: its text in a rounded bubble.
struct Bubble {
    text: String,
}

impl<M: 'static> Widget<M> for Bubble {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.text != old.text {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let width = layout(cx.env(), &self.text).map_or(0.0, |l| l.size().width);
        Measured {
            size: Size::new(width.min(TEXT_WIDTH) + 2.0 * PADDING, HEIGHT),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
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
        let area = rect.inset(Insets {
            top: 0.0,
            left: PADDING,
            bottom: 0.0,
            right: PADDING,
        });
        let top = (size.height - text.size().height) / 2.0;
        cx.push(PaintOp::PushClip(Shape::Rect(area)));
        look::glyphs(
            cx,
            &text,
            Point::new(area.min_x(), top),
            color(ColorRole::ToolTipText),
        );
        cx.push(PaintOp::PopClip);
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Tooltip);
        cx.name(self.text.clone());
    }
}
