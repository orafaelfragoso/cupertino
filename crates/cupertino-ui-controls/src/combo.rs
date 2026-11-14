//! A combo box: an editable field in a recessed well, with a button at the well's end that opens a menu of choices.
//! The field takes any text; a choice from the menu replaces it.

use crate::look::{self, paint_focus_ring};
use crate::press::{Keys, Outcome, Press};
use crate::{BareField, ComboBox, Handler, Menu, MenuAnchor, MenuItem};
use cupertino_geometry::{Insets, Point, Rect, Size};
use cupertino_ui_core::{
    AccessibleStates, Action, Change, ControlState, Element, Event, EventCx, Key, MeasureCx,
    PaintCx, Phase, Propagation, Role, SemanticsCx, Widget, WindowKey,
};
use cupertino_ui_layout::{Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured};
use cupertino_ui_render::{Paint, PaintOp};
use cupertino_ui_theme::{ColorRole, ControlFamily};

/// The disclosure button: its side, its distance from the well's end, and its corner radius.
const BUTTON: f64 = 18.0;
const BUTTON_INSET: f64 = 3.0;
const BUTTON_RADIUS: f64 = 4.0;
/// Its chevron's width and stroke.
const CHEVRON: f64 = 7.0;
const CHEVRON_STROKE: f64 = 1.25;

impl<M: Clone + 'static> ComboBox<M> {
    /// The menu of the choices below the box, which has the key `anchor` in `parent`: the one the field holds is
    /// marked; choosing one sends `on_select` with its index, and the menu closes with `on_close`.
    pub fn menu(
        &self,
        parent: WindowKey,
        anchor: Key,
        on_select: Handler<usize, M>,
        on_close: M,
    ) -> Menu<M> {
        let current = self.options.iter().position(|o| *o == self.field.value);
        let items = self
            .options
            .iter()
            .enumerate()
            .map(|(index, label)| MenuItem::Item {
                label: label.clone(),
                enabled: true,
                checked: Some(index) == current,
                on_select: on_select(index),
            })
            .collect();
        let close = on_close.clone();
        Menu {
            parent,
            anchor: MenuAnchor::Below(anchor),
            label: self.field.label.clone(),
            items,
            current,
            open: Vec::new(),
            on_open: Box::new(move |_| close.clone()),
            on_close,
        }
    }
}

impl<M: Clone + 'static> From<ComboBox<M>> for Element<M> {
    fn from(combo: ComboBox<M>) -> Self {
        let enabled = combo.field.enabled;
        let label = combo.field.label.clone();
        let size = combo.text_size;
        Element::new(ComboWell {
            enabled,
            children: vec![
                Element::new(BareField {
                    field: combo.field,
                    size,
                    hold_caret: false,
                })
                .fill(),
                Element::new(Disclosure {
                    label,
                    open: combo.open,
                    enabled,
                    on_open: combo.on_open,
                }),
            ],
        })
    }
}

/// The well the field and the button share.
struct ComboWell<M> {
    enabled: bool,
    children: Vec<Element<M>>,
}

impl<M: 'static> Widget<M> for ComboWell<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::new(0.0, 6.0, 0.0, BUTTON_INSET),
            ..LayoutSpec::new(ContainerKind::Stack {
                axis: Axis::Horizontal,
                spacing: BUTTON_INSET,
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
        if self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), constraints: Constraints, cx: &mut MeasureCx<'_>) -> Measured {
        let height = look::geometry(&cx.env().theme, ControlFamily::ComboBox).height;
        Measured {
            size: Size::new(constraints.width.ideal(), height),
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), _: &Event, _: &mut EventCx<'_, M>) -> Propagation {
        Propagation::Continue
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: self.enabled,
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let height = look::geometry(&theme, ControlFamily::ComboBox).height;
        let rect = Rect::new(0.0, (size.height - height) / 2.0, size.width, height);
        let radius = theme.metrics.field_radius;
        let (fill, edge) = crate::field::well(&theme, &state);
        look::fill(cx, rect, radius, fill);
        look::stroke(
            cx,
            rect.inset(Insets::uniform(0.5)),
            radius - 0.5,
            1.0,
            edge,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Group);
    }
}

/// The button at the well's end: a small quiet bezel with a downward chevron. The menu opens on its press.
struct Disclosure<M> {
    label: String,
    open: bool,
    enabled: bool,
    on_open: Option<M>,
}

impl<M: Clone + 'static> Widget<M> for Disclosure<M> {
    type State = Press;

    fn layout(&self) -> LayoutSpec {
        LayoutSpec::new(ContainerKind::Leaf)
    }

    fn state(&self) -> Press {
        Press::default()
    }

    fn update(&self, old: &Self, _: &mut Press) -> Change {
        if self.open != old.open || self.enabled != old.enabled {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &Press, _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::new(BUTTON, BUTTON),
            baseline: None,
        }
    }

    fn event(&self, press: &mut Press, event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        let Some(on_open) = self.on_open.as_ref().filter(|_| self.enabled) else {
            return Propagation::Continue;
        };
        let pressed = cx.phase() != Phase::Capture && crate::menu::opens_on_press(event, press);
        if pressed {
            cx.emit(on_open.clone());
        }
        match press.event(event, true, Keys::SpaceAndReturn, cx) {
            Outcome::Ignored if pressed => Propagation::Stop,
            Outcome::Ignored => Propagation::Continue,
            Outcome::Changed
            | Outcome::Activated(
                cupertino_ui_core::ActivationSource::Pointer
                | cupertino_ui_core::ActivationSource::Touch,
            ) => {
                cx.request_paint();
                Propagation::Stop
            }
            Outcome::Activated(_) => {
                cx.request_paint();
                cx.emit(on_open.clone());
                Propagation::Stop
            }
        }
    }

    fn paint(&self, press: &Press, cx: &mut PaintCx<'_>) {
        let theme = cx.env().theme.clone();
        let state = ControlState {
            enabled: self.enabled,
            hovered: press.hovered(),
            pressed: press.pressed() || self.open,
            focused: cx.is_focused(),
            focus_visible: cx.focus_visible(),
            window_active: cx.window_active(),
            ..ControlState::default()
        };
        let size = cx.size();
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        look::fill(
            cx,
            rect,
            BUTTON_RADIUS,
            Some(look::quiet(&theme, look::BEZEL, &state)),
        );
        if let Some(color) = look::color(&theme, ColorRole::ControlText, &state) {
            cx.push(PaintOp::Stroke {
                shape: look::chevron(
                    Point::new(size.width / 2.0, size.height / 2.0),
                    CHEVRON,
                    false,
                ),
                width: CHEVRON_STROKE,
                paint: Paint::Solid(color),
            });
        }
        paint_focus_ring(cx, rect, BUTTON_RADIUS);
    }

    fn semantics(&self, _: &Press, cx: &mut SemanticsCx<'_>) {
        cx.role(Role::PopUpButton);
        cx.name(self.label.clone());
        cx.states(AccessibleStates {
            disabled: !self.enabled,
            expanded: Some(self.open),
            ..AccessibleStates::default()
        });
        if self.enabled {
            cx.focusable();
            if self.on_open.is_some() {
                cx.action(Action::Click);
            }
        }
    }
}
