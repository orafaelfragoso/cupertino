//! The window a shell panel puts under itself to hear a press outside it: it fills the surface it is given, draws
//! nothing and sends one message for a left or right press anywhere on it. The press stops there, so what lies
//! under the window never hears it. Other buttons, motion and scrolling say nothing. Assistive technology cannot
//! reach it: the panel's own dismissal (Escape) is the keyboard's way.

use crate::PressCatcher;
use cupertino_geometry::Size;
use cupertino_ui_core::layout::{Constraints, ContainerKind, LayoutSpec, Measured, SizeSpec};
use cupertino_ui_core::platform::{DecorationMode, MouseButton, PointerEvent, WindowSpec};
use cupertino_ui_core::{
    Change, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx, Propagation, SemanticsCx,
    Widget, Window, WindowBackground,
};

impl<M: Clone + 'static> From<PressCatcher<M>> for Window<M> {
    fn from(catcher: PressCatcher<M>) -> Self {
        Window {
            key: catcher.key,
            spec: WindowSpec {
                title: catcher.title,
                app_id: catcher.app_id,
                // the surface's role gives it its size, a whole output's as a rule
                size: Size::ZERO,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
                input_region: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: catcher.on_close,
            // shown without a frame of the surface's size: it draws nothing
            background: WindowBackground::INPUT_ONLY,
            content: Element::new(Catcher {
                on_press: catcher.on_press,
            }),
            popup: None,
            menus: None,
        }
    }
}

/// Takes a left or right press anywhere on it and draws nothing.
struct Catcher<M> {
    on_press: M,
}

impl<M: Clone + 'static> Widget<M> for Catcher<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            width: SizeSpec::Fill,
            height: SizeSpec::Fill,
            ..LayoutSpec::new(ContainerKind::Leaf)
        }
    }

    fn state(&self) {}

    fn update(&self, _: &Self, _: &mut ()) -> Change {
        Change::None
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Pointer(
                PointerEvent::Button {
                    button: MouseButton::Left | MouseButton::Right,
                    pressed: true,
                    ..
                },
                _,
            ) => {
                cx.emit(self.on_press.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), _: &mut PaintCx<'_>) {}

    fn semantics(&self, _: &(), _: &mut SemanticsCx<'_>) {}
}
