//! The widgets the panels are made of besides the standard controls: the keys and the focus of the whole panel, the
//! alert's body on glass and absolute placement.

use cupertino_kit::core::{
    Change, Element, Event, EventCx, Glass, GlassVariant, MeasureCx, PaintCx, Phase, Propagation,
    Role, SemanticsCx, Widget,
};
use cupertino_kit::geometry::Rect;
use cupertino_kit::layout::{
    Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured, SizeSpec,
};
use cupertino_kit::platform::Keysym;
use cupertino_kit::render::{Paint, PaintOp, Shape};
use cupertino_kit::theme::{Color, ColorRole, StateVariant, Surface};

pub(crate) const ESCAPE: Keysym = Keysym(0xff1b);

fn no_measure() -> Measured {
    Measured {
        size: cupertino_kit::geometry::Size::ZERO,
        baseline: None,
    }
}

fn fill(cx: &mut PaintCx<'_>, shape: Shape, color: Color) {
    cx.push(PaintOp::Fill {
        shape,
        paint: Paint::Solid(color),
    });
}

/// A panel's content: the first look at every key, so Escape and the keys `keys` maps go to the component instead of
/// the focused control, and the alert dialog assistive technology sees. When the focus leaves the panel's window (the
/// person turned to a window) it says so once (`on_let_go`).
pub(crate) struct PanelKeys<M> {
    /// What a key the component takes means; `None` passes the key on to the focus.
    pub keys: Box<dyn Fn(Keysym, Option<char>) -> Option<M>>,
    /// What assistive technology calls the panel.
    pub label: String,
    pub on_let_go: M,
    pub children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for PanelKeys<M> {
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

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        no_measure()
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            // on the way to what has the focus, or as the target itself when nothing under it has the focus
            Event::Key(key, _)
                if key.pressed && matches!(cx.phase(), Phase::Capture | Phase::Target) =>
            {
                match (self.keys)(key.keysym, key.text) {
                    Some(message) => {
                        cx.emit(message);
                        Propagation::Stop
                    }
                    None => Propagation::Continue,
                }
            }
            Event::FocusWithin { gained: false, .. } => {
                cx.emit(self.on_let_go.clone());
                Propagation::Continue
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::AlertDialog);
        cx.name(self.label.clone());
    }
}

/// Width of the outline an alert on glass draws round itself with Increase Contrast, in points: a whole device pixel or
/// more on every display.
pub(crate) const ALERT_CONTRAST_RIM: f64 = 1.0;

/// An alert's body on glass, `width` wide and `height` tall or, without one, as tall as its content, which it pads by
/// `padding`: its corners rounded by `radius`, and the popover surface's fill where the window cannot show glass.
pub(crate) struct AlertBody<M> {
    pub width: f64,
    pub height: Option<f64>,
    pub radius: f64,
    pub padding: cupertino_kit::geometry::Insets,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for AlertBody<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.width),
            height: self.height.map_or(SizeSpec::Fit, SizeSpec::Fixed),
            insets: self.padding,
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
        if self.width != old.width || self.height != old.height || self.padding != old.padding {
            Change::Layout
        } else if self.radius != old.radius {
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
        let size = cx.size();
        // the glass of an alert; the compositor gives it the alert's drop shadow in place of the variant's own
        if cx.glass(
            Glass::regular()
                .variant(GlassVariant::GlassWindow)
                .radius(self.radius),
        ) {
            // with Increase Contrast the glass takes the contrast face, which over a window of the same appearance is
            // that window's tone: the alert marks its edge with the outline its buttons have
            let theme = cx.env().theme.clone();
            let rim = theme
                .colors
                .get(ColorRole::ControlBorder, StateVariant::NORMAL);
            if let (true, Some(rim)) = (theme.preferences.appearance.is_high_contrast(), rim) {
                let half = ALERT_CONTRAST_RIM / 2.0;
                cx.push(PaintOp::Stroke {
                    shape: Shape::RoundedRect {
                        rect: Rect::new(
                            half,
                            half,
                            size.width - ALERT_CONTRAST_RIM,
                            size.height - ALERT_CONTRAST_RIM,
                        ),
                        radius: (self.radius - half).max(0.0),
                    },
                    width: ALERT_CONTRAST_RIM,
                    paint: Paint::Solid(rim),
                });
            }
            return;
        }
        if let Some(color) = cx.env().theme.surface(Surface::Popover).fill {
            fill(
                cx,
                Shape::RoundedRect {
                    rect: Rect::new(0.0, 0.0, size.width, size.height),
                    radius: self.radius,
                },
                color,
            );
        }
    }

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}

/// Its children at `rect` in an absolute parent, stacked along `axis` and placed across it by `align`. `initial` gives
/// its first control the focus when the window opens.
pub(crate) struct Place<M> {
    pub rect: Rect,
    pub axis: Axis,
    pub align: Align,
    pub justify: Justify,
    pub initial: bool,
    pub children: Vec<Element<M>>,
}

impl<M: 'static> Place<M> {
    /// Centred both ways.
    pub(crate) fn new(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            rect,
            axis: Axis::Vertical,
            align: Align::Center,
            justify: Justify::Center,
            initial: false,
            children,
        }
    }

    /// From the top, against the leading edge, as a paragraph.
    pub(crate) fn text(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            align: Align::Start,
            justify: Justify::Start,
            ..Place::new(rect, children)
        }
    }

    /// Filling the box: a button at its frame.
    pub(crate) fn filled(rect: Rect, children: Vec<Element<M>>) -> Self {
        Place {
            align: Align::Stretch,
            ..Place::new(rect, children)
        }
    }
}

impl<M: 'static> Widget<M> for Place<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fixed(self.rect.width()),
            height: SizeSpec::Fixed(self.rect.height()),
            position: self.rect.origin,
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: self.axis,
                spacing: 0.0,
                align: self.align,
                justify: self.justify,
            })
        }
    }

    fn take_children(&mut self) -> Vec<Element<M>> {
        std::mem::take(&mut self.children)
    }

    fn state(&self) {}

    fn update(&self, old: &Self, _: &mut ()) -> Change {
        if self.rect != old.rect
            || self.axis != old.axis
            || self.align != old.align
            || self.justify != old.justify
        {
            Change::Layout
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: self.rect.size,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        if self.initial {
            cx.initial_focus();
        }
    }
}
