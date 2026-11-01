use crate::look;
use crate::scroll;
use crate::Popover;
use cupertino_geometry::{Insets, Rect, Size};
use cupertino_ui_core::{
    Change, CommandRegistry, Element, Event, EventCx, MeasureCx, PaintCx, Popup, PopupAnchor,
    Propagation, Role, SemanticsCx, Widget, Window, WindowBackground, WindowKey,
};
use cupertino_ui_layout::{Align, Axis, Constraints, ContainerKind, Justify, LayoutSpec, Measured};
use cupertino_ui_platform::{DecorationMode, Keysym, WindowSpec};
use cupertino_ui_theme::{ColorRole, StateVariant};
use std::rc::Rc;

const ESCAPE: Keysym = Keysym(0xff1b);
const RADIUS: f64 = 8.0;
const PADDING: f64 = 12.0;
const GAP: f64 = 8.0;
const EDGE: f64 = 1.0;
/// The widest the panel grows; the height bound is not in the spec, so it stays well under a laptop screen.
const MAX: Size = Size {
    width: 400.0,
    height: 600.0,
};

impl<M: Clone + 'static> Popover<M> {
    /// The popup window that shows the popover; declare it while it is open.
    pub fn window(self, key: WindowKey) -> Window<M> {
        Window {
            key,
            spec: WindowSpec {
                title: self.label.clone(),
                app_id: String::new(),
                size: MAX,
                min_size: None,
                max_size: None,
                decorations: DecorationMode::None,
                foreign_parent: None,
            },
            shortcuts: CommandRegistry::new(),
            on_close: None,
            background: WindowBackground::None,
            content: Element::new(Panel {
                label: self.label,
                on_close: self.on_close.clone(),
                children: vec![scroll::fitted(
                    self.content,
                    self.scroll,
                    Rc::new(self.on_scroll),
                )],
            }),
            popup: Some(Popup {
                parent: self.parent,
                anchor: PopupAnchor::Below {
                    key: self.anchor,
                    gap: GAP,
                },
                grab: true,
                on_dismiss: self.on_close,
            }),
            menus: None,
        }
    }
}

struct Panel<M> {
    label: String,
    on_close: M,
    children: Vec<Element<M>>,
}

impl<M: Clone + 'static> Widget<M> for Panel<M> {
    type State = ();

    fn layout(&self) -> LayoutSpec {
        LayoutSpec {
            insets: Insets::uniform(PADDING),
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
        if self.label != old.label {
            Change::Paint
        } else {
            Change::None
        }
    }

    fn measure(&self, _: &(), _: Constraints, _: &mut MeasureCx<'_>) -> Measured {
        Measured {
            size: Size::ZERO,
            baseline: None,
        }
    }

    fn event(&self, _: &mut (), event: &Event, cx: &mut EventCx<'_, M>) -> Propagation {
        match event {
            Event::Key(key, _) if key.pressed && key.keysym == ESCAPE && key.mods.is_empty() => {
                cx.emit(self.on_close.clone());
                Propagation::Stop
            }
            _ => Propagation::Continue,
        }
    }

    fn paint(&self, _: &(), cx: &mut PaintCx<'_>) {
        let size = cx.size();
        let theme = &cx.env().theme;
        let fill = theme
            .colors
            .get(ColorRole::WindowBackground, StateVariant::NORMAL);
        let edge = theme.colors.get(ColorRole::Separator, StateVariant::NORMAL);
        let rect = Rect::new(0.0, 0.0, size.width, size.height);
        look::fill(cx, rect, RADIUS, fill);
        look::stroke(
            cx,
            rect.inset(Insets::uniform(EDGE / 2.0)),
            RADIUS - EDGE / 2.0,
            EDGE,
            edge,
        );
    }

    fn semantics(&self, _: &(), cx: &mut SemanticsCx<'_>) {
        cx.role(Role::Dialog);
        cx.name(self.label.clone());
        cx.focus_scope();
    }
}
